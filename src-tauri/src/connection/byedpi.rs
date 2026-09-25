//! The byedpi (ciadpi) sidecar spawned next to sing-box when routing needs
//! the DPI tunnel: spawn, readiness probe (with the stale-listener
//! distrust), watcher and deliberate-kill handling.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::async_runtime::Receiver;
use tauri::{AppHandle, Emitter};

use crate::process::{CommandChild, CommandEvent};

use super::startup::tail;
use super::state::{CONNECTION_CHANGED_EVENT, ConnectionSnapshot, lock_state, snapshot_of};

/// How long ciadpi gets to open its listening port.
const DPI_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a suspicious early connect waits for proof that the answering
/// listener is not our freshly spawned child (see `probe_dpi_ready`).
const DPI_DEATH_GRACE: Duration = Duration::from_millis(300);

/// The byedpi (ciadpi) sidecar spawned next to sing-box when routing needs
/// the DPI tunnel. `deliberate` marks kills initiated by the app itself so
/// the watcher can tell them from a crash.
pub(super) struct DpiRunning {
    pub(super) child: CommandChild,
    pub(super) port: u16,
    pub(super) strategy: String,
    pub(super) deliberate: Arc<AtomicBool>,
}

/// Kills a not-yet-committed byedpi child (connect aborted mid-flight).
pub(super) fn abort_dpi(dpi: Option<DpiRunning>) {
    if let Some(dpi) = dpi {
        dpi.deliberate.store(true, Ordering::Relaxed);
        let _ = dpi.child.kill();
    }
}

/// Spawns the ciadpi sidecar: a local SOCKS5 server with the user's strategy
/// as its argument line, on the configured port bound to loopback (the app
/// owns `-i`/`-p`, the strategy line is validated against them in `dpi.rs`).
/// Ready once the port accepts a TCP connection.
pub(super) async fn start_byedpi(
    app: &AppHandle,
    port: u16,
    args: &str,
    strategy_name: &str,
) -> Result<DpiRunning, String> {
    let command = crate::process::sidecar("byedpi")
        .map_err(|e| format!("failed to resolve the byedpi sidecar: {e}"))?;
    let command = command
        .args(["-i", "127.0.0.1", "-p", &port.to_string()])
        .args(args.split_whitespace());
    let (events, child) = command
        .spawn()
        .map_err(|e| format!("failed to start byedpi: {e}"))?;

    let deliberate = Arc::new(AtomicBool::new(false));
    let (receiver, outcome) = tauri::async_runtime::spawn_blocking(move || {
        probe_dpi_ready(events, port)
    })
    .await
    .map_err(|error| format!("byedpi startup probe failed: {error}"))?;

    match outcome {
        Ok(()) => {
            let events = receiver.expect("a ready probe hands the receiver back");
            spawn_dpi_watcher(app.clone(), events, deliberate.clone(), strategy_name.to_string());
            Ok(DpiRunning {
                child,
                port,
                strategy: strategy_name.to_string(),
                deliberate,
            })
        }
        Err(reason) => {
            let _ = child.kill();
            Err(reason)
        }
    }
}

/// Polls the ciadpi listening port; a fast exit (bad flag combination, port
/// taken) surfaces through the event channel with the stderr tail. Blocking —
/// run inside `spawn_blocking`. Hands the receiver back on success.
///
/// A connect that succeeds before the port ever refused one is not trusted
/// blindly: a stale tunnel from a crashed run keeps answering while our own
/// child dies with "bind: address already in use" — the false-ready that used
/// to surface later as an unexpected-exit toast. When the port is first seen
/// occupied, the probe waits out `DPI_DEATH_GRACE` for our child's
/// termination before calling the listener ours; once the port has refused a
/// connection, any listener appearing afterwards can only be ours.
pub(crate) fn probe_dpi_ready(
    mut events: Receiver<CommandEvent>,
    port: u16,
) -> (Option<Receiver<CommandEvent>>, Result<(), String>) {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stderr = Vec::new();
    let mut port_was_seen_free = false;
    let deadline = Instant::now() + DPI_STARTUP_TIMEOUT;
    loop {
        if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok() {
            let our_child_died =
                !port_was_seen_free && wait_out_death_grace(&mut events, &mut stderr);
            if !our_child_died {
                return (Some(events), Ok(()));
            }
            break; // the answering listener is not ours; the reason is in stderr
        }
        port_was_seen_free = true;
        match events.try_recv() {
            Ok(CommandEvent::Stderr(chunk)) => stderr.extend_from_slice(&chunk),
            Ok(CommandEvent::Terminated(_)) | Ok(CommandEvent::Error(_)) => break,
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // the process is gone, but its last stderr lines may still be in flight
    let flush_deadline = Instant::now() + Duration::from_millis(300);
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
        true => format!(
            "byedpi did not start within {:?} (is port {port} already in use?)",
            DPI_STARTUP_TIMEOUT
        ),
        false => format!("byedpi failed to start: {}", tail(&stderr)),
    };
    (None, Err(reason))
}

/// Drains the event channel for `DPI_DEATH_GRACE`, collecting stderr along
/// the way; true when the child terminated (or the channel died) in that
/// window — a dead child cannot own the port that just answered.
fn wait_out_death_grace(events: &mut Receiver<CommandEvent>, stderr: &mut Vec<u8>) -> bool {
    let deadline = Instant::now() + DPI_DEATH_GRACE;
    loop {
        match events.try_recv() {
            Ok(CommandEvent::Stderr(chunk)) => stderr.extend_from_slice(&chunk),
            Ok(CommandEvent::Terminated(_)) | Ok(CommandEvent::Error(_)) => return true,
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return true,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

/// Watches the byedpi sidecar: an unexpected exit (crash, port stolen
/// mid-run) is reported as `last_error` + `connection-changed`, but does not
/// tear the whole session down — proxy and direct routes keep working.
/// Deliberate kills (disconnect/reconnect) are marked on `deliberate` and
/// stay silent.
fn spawn_dpi_watcher(
    app: AppHandle,
    mut events: Receiver<CommandEvent>,
    deliberate: Arc<AtomicBool>,
    strategy: String,
) {
    tauri::async_runtime::spawn(async move {
        let mut stderr: Vec<u8> = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stderr(chunk) => {
                    stderr.extend_from_slice(&chunk);
                    let overflow = stderr.len().saturating_sub(4 * 1024);
                    stderr.drain(..overflow);
                }
                CommandEvent::Terminated(_) => {
                    if !deliberate.load(Ordering::Relaxed) {
                        report_dpi_exit(&app, &strategy, tail(&stderr));
                    }
                    break;
                }
                _ => {}
            }
        }
    });
}

fn report_dpi_exit(app: &AppHandle, strategy: &str, reason: String) {
    let message = if reason.is_empty() {
        format!(
            "the DPI tunnel (strategy {strategy:?}) exited unexpectedly — DPI routes no \
             longer work; reconnect to restart it"
        )
    } else {
        format!(
            "the DPI tunnel (strategy {strategy:?}) exited unexpectedly: {reason} — DPI \
             routes no longer work; reconnect to restart it"
        )
    };
    let snapshot = match lock_state() {
        Ok(mut state) => {
            state.last_error = Some(message);
            if let Some(run) = state.running.as_mut() {
                run.dpi = None;
            }
            snapshot_of(&state)
        }
        Err(error) => ConnectionSnapshot {
            last_error: Some(error),
            ..ConnectionSnapshot::default()
        },
    };
    let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
}
