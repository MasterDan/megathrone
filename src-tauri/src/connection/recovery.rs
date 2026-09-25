//! Crash recovery: the on-disk session marker written for every connect
//! and removed on a clean teardown, the startup `recover` pass behind it,
//! and the signal handlers that route Ctrl+C/SIGTERM through the regular
//! exit path.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::process::CommandChild;

use super::state::Running;
use super::system_proxy::{
    MacServiceBackup, SystemProxyBackup, SystemProxyRestore, restore_system_proxy,
};

/// Written to the app data dir for every connect, removed on a clean
/// teardown. Its presence at startup means the previous session died
/// without unwinding (debugger stop, SIGKILL, crash) — the system proxy may
/// still point at the dead port.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMarker {
    /// the app process that wrote the marker (another live instance owns it)
    owner_pid: u32,
    /// the sing-box sidecar; 0 when unknown
    child_pid: u32,
    mixed_port: u16,
    config_path: String,
    /// the system proxy backup to restore after an unclean death (`None`
    /// when the system proxy was never touched)
    #[serde(default)]
    proxy_backup: Option<SystemProxyBackup>,
    /// legacy field of the macOS-only era — read (and restored) so a marker
    /// written by an older build still cleans up after itself; never written
    /// non-empty anymore
    #[serde(default)]
    macos_backups: Vec<MacServiceBackup>,
    /// the byedpi sidecar; 0 when the DPI tunnel was not running
    #[serde(default)]
    dpi_pid: u32,
}

fn marker_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("proxy-session.json"))
}

pub(super) fn write_marker(app: &AppHandle, running: &Running) {
    let Some(path) = marker_path(app) else { return };
    let marker = SessionMarker {
        owner_pid: std::process::id(),
        child_pid: running.child.as_ref().map_or(0, CommandChild::pid),
        mixed_port: running.mixed_port,
        config_path: running.config_path.to_string_lossy().to_string(),
        proxy_backup: match &running.restore {
            SystemProxyRestore::Applied(backup) => Some(backup.clone()),
            SystemProxyRestore::Untouched => None,
        },
        macos_backups: Vec::new(),
        dpi_pid: running.dpi.as_ref().map_or(0, |dpi| dpi.child.pid()),
    };
    if let Ok(json) = serde_json::to_string_pretty(&marker) {
        let _ = std::fs::write(&path, json);
    }
}

pub(super) fn clear_marker(app: &AppHandle) {
    if let Some(path) = marker_path(app) {
        let _ = std::fs::remove_file(&path);
    }
}

/// Cleans up after a previous session that died without unwinding: restores
/// the system proxy, kills an orphaned sing-box, removes its temp config and
/// the marker itself. A no-op after a clean exit (no marker there).
pub fn recover(app: &AppHandle) {
    let Some(path) = marker_path(app) else { return };
    let Ok(raw) = std::fs::read_to_string(&path) else { return };
    let Ok(marker) = serde_json::from_str::<SessionMarker>(&raw) else {
        let _ = std::fs::remove_file(&path);
        return;
    };

    // another app instance still owns that session — hands off
    if marker.owner_pid != 0 && process_comm_is(marker.owner_pid, "megathrone") {
        return;
    }

    // the proxy backup of the crashed session — or, from a pre-cross-platform
    // build, its macOS-only legacy form
    let backup = marker.proxy_backup.clone().or_else(|| {
        (!marker.macos_backups.is_empty())
            .then(|| SystemProxyBackup::MacOs(marker.macos_backups.clone()))
    });
    if let Some(backup) = backup {
        let _ = restore_system_proxy(&backup);
    }
    if marker.child_pid != 0 {
        kill_if_process(marker.child_pid, "sing-box");
    }
    if marker.dpi_pid != 0 {
        // the needle must match the on-disk binary name (the comm is the
        // executable path) — the sidecar ships as `byedpi`, not `ciadpi`
        kill_if_process(marker.dpi_pid, "byedpi");
    }
    if !marker.config_path.is_empty() {
        let _ = std::fs::remove_file(&marker.config_path);
    }
    let _ = std::fs::remove_file(&path);
}

/// Ctrl+C / SIGTERM: route through the regular exit path so `RunEvent::Exit`
/// performs the same cleanup as a window close. (SIGKILL — e.g. a debugger
/// stop — cannot be caught; the session marker covers that case.)
#[cfg(unix)]
pub fn install_exit_signals(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        let (mut interrupt, mut terminate) = match (interrupt, terminate) {
            (Ok(interrupt), Ok(terminate)) => (interrupt, terminate),
            _ => return,
        };
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
        handle.exit(0);
    });
}

#[cfg(not(unix))]
pub fn install_exit_signals(_app: &AppHandle) {}

#[cfg(unix)]
fn process_comm_is(pid: u32, needle: &str) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .map(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim().contains(needle)
        })
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_comm_is(_pid: u32, _needle: &str) -> bool {
    false
}

#[cfg(unix)]
fn kill_if_process(pid: u32, needle: &str) {
    if process_comm_is(pid, needle) {
        // SAFETY: kill(2) on a pid verified to be the expected process
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_if_process(_pid: u32, _needle: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::system_proxy::ProxyBackup;

    /// The marker is the crash-recovery source of truth — it must survive a
    /// serialize/deserialize roundtrip with the proxy backups intact.
    #[test]
    fn session_marker_roundtrips() {
        let marker = SessionMarker {
            owner_pid: 111,
            child_pid: 222,
            mixed_port: 7897,
            config_path: "/tmp/megathrone-run-1-7897.json".to_string(),
            proxy_backup: Some(SystemProxyBackup::MacOs(vec![MacServiceBackup {
                service: "Wi-Fi".to_string(),
                kinds: [
                    ProxyBackup::Disabled,
                    ProxyBackup::Enabled { host: "10.0.0.2".to_string(), port: 3128 },
                    ProxyBackup::Disabled,
                ],
            }])),
            macos_backups: Vec::new(),
            dpi_pid: 333,
        };

        let json = serde_json::to_string(&marker).expect("serialize");
        let back: SessionMarker = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.owner_pid, 111);
        assert_eq!(back.child_pid, 222);
        assert_eq!(back.dpi_pid, 333);
        assert_eq!(back.mixed_port, 7897);
        assert!(back.macos_backups.is_empty());
        let Some(SystemProxyBackup::MacOs(backups)) = back.proxy_backup else {
            panic!("the macOS backup must survive the roundtrip");
        };
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].service, "Wi-Fi");
        assert_eq!(
            backups[0].kinds[1],
            ProxyBackup::Enabled { host: "10.0.0.2".to_string(), port: 3128 }
        );
    }

    /// Markers written before the DPI integration have no `dpi_pid` — they
    /// must still deserialize (serde default).
    #[test]
    fn session_marker_tolerates_legacy_payloads() {
        let legacy = serde_json::json!({
            "ownerPid": 111,
            "childPid": 222,
            "mixedPort": 7897,
            "configPath": "/tmp/x.json",
            "macosBackups": [],
        });
        let marker: SessionMarker = serde_json::from_value(legacy).expect("legacy marker");
        assert_eq!(marker.dpi_pid, 0);
        assert!(marker.proxy_backup.is_none());

        // markers from the macOS-only era carry `macosBackups` instead of
        // `proxyBackup` — the recover path still restores them
        let legacy = serde_json::json!({
            "ownerPid": 111,
            "childPid": 222,
            "mixedPort": 7897,
            "configPath": "/tmp/x.json",
            "macosBackups": [
                {
                    "service": "Wi-Fi",
                    "kinds": [
                        "Disabled",
                        { "Enabled": { "host": "10.0.0.2", "port": 3128 } },
                        "Disabled",
                    ],
                },
            ],
        });
        let marker: SessionMarker =
            serde_json::from_value(legacy).expect("legacy marker with backups");
        assert!(marker.proxy_backup.is_none());
        assert_eq!(marker.macos_backups.len(), 1);
        assert_eq!(marker.macos_backups[0].service, "Wi-Fi");
    }

    #[test]
    fn process_comm_is_rejects_unknown_pids() {
        assert!(!process_comm_is(u32::MAX, "sing-box"));
    }
}
