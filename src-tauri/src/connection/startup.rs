//! The sing-box startup probe (clash API polling with the stderr-tail
//! failure reason) and the TUN privilege checks and hints.

use std::time::{Duration, Instant};

use tauri::async_runtime::Receiver;

use crate::latency;
use crate::process::CommandEvent;

use super::state::MODE_TUN;

/// How much of sing-box's stderr is kept for error reporting.
const STDERR_TAIL_CHARS: usize = 1200;

#[derive(Debug)]
pub(super) enum Startup {
    Ready,
    Failed(String),
}

/// Polls the clash API until the instance is up. A fast exit (permissions,
/// bad config) is detected through the event channel; the buffered stderr
/// becomes the failure reason. The `Terminated` event can overtake the last
/// pipe chunks, so after a death the probe keeps draining for a short grace
/// window — otherwise the "why" is lost. Blocking (ureq + sleeps) — run
/// inside `spawn_blocking`. Hands the receiver back on success so the
/// watcher can keep reading it.
pub(super) fn probe_startup(
    mut events: Receiver<CommandEvent>,
    api_port: u16,
) -> (Option<Receiver<CommandEvent>>, Startup) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .into();
    let mut stderr = Vec::new();
    let deadline = Instant::now() + latency::STARTUP_TIMEOUT;
    let died = loop {
        let up = matches!(
            agent.get(&format!("http://127.0.0.1:{api_port}/version")).call(),
            Ok(response) if response.status().as_u16() == 200
        );
        if up {
            return (Some(events), Startup::Ready);
        }
        match events.try_recv() {
            Ok(CommandEvent::Stderr(chunk)) => stderr.extend_from_slice(&chunk),
            Ok(CommandEvent::Terminated(_)) | Ok(CommandEvent::Error(_)) => break true,
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break true,
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    if !died {
        return (
            None,
            Startup::Failed(format!(
                "sing-box did not become ready within {:?}",
                latency::STARTUP_TIMEOUT
            )),
        );
    }

    // the process is gone, but its last stderr lines may still be in flight
    let flush_deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < flush_deadline {
        match events.try_recv() {
            Ok(CommandEvent::Stderr(chunk)) => stderr.extend_from_slice(&chunk),
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    let reason = match tail(&stderr).is_empty() {
        true => "sing-box failed to start (no error output)".to_string(),
        false => format!("sing-box failed to start: {}", tail(&stderr)),
    };
    (None, Startup::Failed(reason))
}

/// TUN failures caused by missing privileges get an actionable hint appended
/// (covers privileged environments the up-front check cannot see).
pub(super) fn permission_hint(mode: &str, reason: &str) -> String {
    let denied =
        reason.contains("not permitted") || reason.contains("permission denied");
    if mode == MODE_TUN && denied {
        format!(
            "{reason}\n\nTUN mode needs administrator privileges to create the \
             network interface. {}",
            elevation_hint()
        )
    } else {
        reason.to_string()
    }
}

/// Per-OS advice for launching the app elevated (TUN refusals).
#[cfg(windows)]
pub(super) fn elevation_hint() -> &'static str {
    "Restart the app with \"Run as administrator\" — or use System Proxy mode instead."
}

#[cfg(not(windows))]
pub(super) fn elevation_hint() -> &'static str {
    "For development launch the app elevated (e.g. `sudo pnpm tauri dev`, or build \
     once and run the binary with sudo) — or use System Proxy mode instead."
}

#[cfg(all(unix, not(target_os = "android")))]
pub(super) fn running_as_root() -> bool {
    // SAFETY: geteuid is a side-effect-free system call
    unsafe { libc::geteuid() == 0 }
}

/// Whether the process token is elevated (TUN/wintun needs a real
/// administrator token, not just a group membership).
#[cfg(target_os = "windows")]
pub(super) fn running_as_root() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: plain query calls on our own process token; every handle
    // handed out is closed before returning
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        ) != 0;
        CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

pub(super) fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() <= STDERR_TAIL_CHARS {
        return trimmed.to_string();
    }
    let skip = trimmed.chars().count() - STDERR_TAIL_CHARS;
    trimmed.chars().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::state::MODE_SYSTEM_PROXY;

    #[test]
    fn tail_keeps_the_last_chars_only() {
        assert_eq!(tail(b"  short message \n"), "short message");
        let long = "x".repeat(STDERR_TAIL_CHARS + 500);
        assert_eq!(tail(long.as_bytes()).chars().count(), STDERR_TAIL_CHARS);
        assert_eq!(tail(&[]), "");
    }

    #[test]
    fn permission_hint_only_extends_tun_denials() {
        let tun_denied = permission_hint(
            MODE_TUN,
            "sing-box failed to start: FATAL create tun: operation not permitted",
        );
        assert!(tun_denied.contains("administrator privileges"));

        // same sing-box error in another mode: no hint
        let proxy_denied = permission_hint(
            MODE_SYSTEM_PROXY,
            "sing-box failed to start: operation not permitted",
        );
        assert_eq!(proxy_denied, "sing-box failed to start: operation not permitted");

        // tun dying for another reason: untouched
        let tun_other = permission_hint(MODE_TUN, "sing-box failed to start: bad config");
        assert_eq!(tun_other, "sing-box failed to start: bad config");
    }

    /// A dying "sing-box": stderr chunks, then Terminated — and one more
    /// chunk arriving *after* Terminated (the overtake race the grace window
    /// exists for). The probe must report the whole reason.
    #[test]
    fn probe_startup_reports_the_death_reason() {
        let (tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);
        let api_port = latency::free_local_port().expect("free port");
        tx.blocking_send(CommandEvent::Stderr(
            b"FATAL[0000] start service: create tun interface: ".to_vec(),
        ))
        .expect("send chunk 1");
        tx.blocking_send(CommandEvent::Terminated(crate::process::TerminatedPayload {
            code: Some(1),
            signal: None,
        }))
        .expect("send terminated");
        tx.blocking_send(CommandEvent::Stderr(b"operation not permitted\n".to_vec()))
        .expect("send chunk 2 (late)");

        let (receiver, outcome) = probe_startup(rx, api_port);

        assert!(receiver.is_none(), "a failed probe must not hand the receiver back");
        let Startup::Failed(reason) = outcome else {
            panic!("a dead process must fail the probe, got {outcome:?}");
        };
        assert!(reason.contains("sing-box failed to start"), "got: {reason}");
        assert!(reason.contains("create tun interface"), "got: {reason}");
        assert!(reason.contains("operation not permitted"), "got: {reason}");
    }

    /// No events, no API: the probe runs into its deadline and reports a
    /// timeout (without stderr).
    #[test]
    fn probe_startup_times_out_when_nothing_answers() {
        let (_tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);
        let api_port = latency::free_local_port().expect("free port");

        let started = Instant::now();
        let (receiver, outcome) = probe_startup(rx, api_port);

        assert!(receiver.is_none());
        let Startup::Failed(reason) = outcome else {
            panic!("expected a failure, got {outcome:?}");
        };
        assert!(reason.contains("did not become ready"), "got: {reason}");
        assert!(
            started.elapsed() >= latency::STARTUP_TIMEOUT,
            "the probe must wait out its deadline, took {:?}",
            started.elapsed()
        );
    }
}
