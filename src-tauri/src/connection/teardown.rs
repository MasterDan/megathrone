//! Tearing a session down: the disconnect command, the shared teardown
//! path (system proxy restore first, then the children), the unexpected
//! exit watcher and the synchronous shutdown for the exit handler.

use tauri::async_runtime::Receiver;
use tauri::{AppHandle, Emitter};

use crate::auto_select;
use crate::process::CommandEvent;

use super::byedpi::abort_dpi;
use super::recovery::clear_marker;
use super::startup::tail;
use super::state::{
    CONNECTION_CHANGED_EVENT, ConnectionSnapshot, FLOW, Running, current_snapshot, lock_state,
    snapshot_of,
};
use super::system_proxy::{SystemProxyRestore, restore_system_proxy};
use super::traffic::stop_traffic_poller;

/// Stops the proxy and puts the system proxy settings back; idempotent.
#[tauri::command]
pub async fn connection_disconnect(app: AppHandle) -> Result<ConnectionSnapshot, String> {
    let _flow = FLOW.lock().await;

    let taken = lock_state()?.running.take();
    if let Some(run) = &taken {
        // the supervisor has nothing to feed once the session is down
        auto_select::stop(run.profile_id);
        stop_traffic_poller();
    }
    let snapshot = match taken {
        Some(mut running) => {
            let restore_error = teardown(&app, &mut running).await.err();
            let mut state = lock_state()?;
            state.last_error = restore_error;
            snapshot_of(&state)
        }
        None => current_snapshot(),
    };

    let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
    Ok(snapshot)
}

/// Called from the app exit handler: kills the instance and restores the
/// system proxy synchronously (there is no async runtime to lean on).
pub fn shutdown(app: &AppHandle) {
    let Some(mut running) = lock_state().ok().and_then(|mut state| state.running.take()) else {
        return;
    };
    auto_select::stop(running.profile_id);
    stop_traffic_poller();
    let restore = std::mem::take(&mut running.restore);
    let restored = match &restore {
        SystemProxyRestore::Untouched => Ok(()),
        SystemProxyRestore::Applied(backup) => restore_system_proxy(backup),
    };
    // keep the marker when the restore failed so the next start retries it
    if restored.is_ok() {
        clear_marker(app);
    }
    if let Some(child) = running.child.take() {
        let _ = child.kill();
    }
    abort_dpi(running.dpi.take());
    let _ = std::fs::remove_file(&running.config_path);
}

/// Stops everything a `Running` owns: restores the system proxy first (so
/// traffic goes direct before the port disappears), then kills sing-box and
/// removes the temp config. Best effort — returns the first failure. The
/// session marker is removed on success (kept on failure → the next start
/// retries the restore).
async fn teardown(app: &AppHandle, running: &mut Running) -> Result<(), String> {
    // Android: the session is going down — the foreground service must not
    // outlive it (deliberate disconnect, crash cleanup, mid-flight rebuild)
    crate::mobile::set_foreground_service(false);
    let restore = std::mem::take(&mut running.restore);
    let restored = match &restore {
        SystemProxyRestore::Untouched => Ok(()),
        SystemProxyRestore::Applied(backup) => {
            let backup = backup.clone();
            tauri::async_runtime::spawn_blocking(move || restore_system_proxy(&backup))
                .await
                .map_err(|error| format!("system proxy restore failed: {error}"))?
        }
    };
    if let Some(child) = running.child.take() {
        let _ = child.kill();
    }
    abort_dpi(running.dpi.take());
    let _ = std::fs::remove_file(&running.config_path);
    if restored.is_ok() {
        clear_marker(app);
    }
    Ok(())
}

/// Watches the spawned sing-box: keeps a stderr tail and reports an
/// unexpected exit (crash, OOM kill, permission revocation…) as
/// `connection-changed` with `connected: false` + the reason. A deliberate
/// disconnect removes `Running` from the state *before* killing, so the
/// watcher's `Terminated` finds nothing to report.
pub(super) fn spawn_watcher(app: AppHandle, mut events: Receiver<CommandEvent>) {
    tauri::async_runtime::spawn(async move {
        let mut stderr: Vec<u8> = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stderr(chunk) => {
                    stderr.extend_from_slice(&chunk);
                    let overflow = stderr.len().saturating_sub(8 * 1024);
                    stderr.drain(..overflow);
                }
                CommandEvent::Terminated(_) => {
                    report_unexpected_exit(&app, tail(&stderr)).await;
                    break;
                }
                _ => {}
            }
        }
    });
}

async fn report_unexpected_exit(app: &AppHandle, reason: String) {
    let Some(mut running) = lock_state().ok().and_then(|mut state| state.running.take()) else {
        return;
    };
    auto_select::stop(running.profile_id);
    stop_traffic_poller();
    let restore_error = teardown(app, &mut running).await.err();
    let message = match (reason.is_empty(), restore_error) {
        (true, None) => "sing-box exited unexpectedly".to_string(),
        (true, Some(restore)) => {
            format!("sing-box exited unexpectedly (and restoring the system proxy failed: {restore})")
        }
        (false, None) => format!("sing-box exited unexpectedly: {reason}"),
        (false, Some(restore)) => format!(
            "sing-box exited unexpectedly: {reason} (and restoring the system proxy failed: {restore})"
        ),
    };
    let snapshot = match lock_state() {
        Ok(mut state) => {
            state.last_error = Some(message);
            snapshot_of(&state)
        }
        Err(error) => ConnectionSnapshot {
            last_error: Some(error),
            ..ConnectionSnapshot::default()
        },
    };
    let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
}
