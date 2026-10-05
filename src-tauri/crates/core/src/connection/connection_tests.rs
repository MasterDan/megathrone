//! Tests of the module's public surface — currently the byedpi readiness
//! probe shared with the DPI strategy test.

use std::time::Duration;

use crate::latency;
use crate::process::CommandEvent;

use super::byedpi::probe_dpi_ready;

/// A stale tunnel from a crashed run keeps the port answering while the
/// fresh child dies with a bind error: the first connect succeeds before
/// the port ever refused one, then Terminated arrives in the grace
/// window — the probe must fail with the stderr reason, not go ready.
#[test]
fn probe_dpi_ready_rejects_a_stale_listener() {
    let stale = std::net::TcpListener::bind("127.0.0.1:0").expect("stale listener");
    let port = stale.local_addr().expect("addr").port();
    let (tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);
    tx.blocking_send(CommandEvent::Stderr(b"bind: Address already in use\n".to_vec()))
        .expect("send stderr");
    tx.blocking_send(CommandEvent::Terminated(crate::process::TerminatedPayload {
        code: Some(1),
        signal: None,
    }))
    .expect("send terminated");

    let (receiver, outcome) = probe_dpi_ready(rx, port);

    assert!(receiver.is_none(), "a dead child must not hand the receiver back");
    let Err(reason) = outcome else {
        panic!("a stale listener must fail the probe, got {outcome:?}");
    };
    assert!(reason.contains("bind: Address already in use"), "got: {reason}");
}

/// Early success with a living child (no termination within the grace)
/// is accepted — the answering listener is ours.
#[test]
fn probe_dpi_ready_accepts_a_live_child_on_an_occupied_port() {
    let ours = std::net::TcpListener::bind("127.0.0.1:0").expect("our listener");
    let port = ours.local_addr().expect("addr").port();
    // an open sender keeps the channel alive — the child is doing fine
    let (_tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);

    let (receiver, outcome) = probe_dpi_ready(rx, port);

    assert!(outcome.is_ok(), "a live child must pass the probe, got {outcome:?}");
    assert!(receiver.is_some(), "a ready probe must hand the receiver back");
}

/// The regular startup: the port refuses first (proving it was free),
/// then the child binds and the probe succeeds — no grace detour.
#[test]
fn probe_dpi_ready_accepts_a_late_listener_on_a_free_port() {
    let port = latency::free_local_port().expect("free port");
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind");
        // hold the port until the probe has connected
        std::thread::sleep(Duration::from_secs(2));
        drop(listener);
    });

    // the sender stays alive for the whole test — the child is healthy
    let (_tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);
    let (receiver, outcome) = probe_dpi_ready(rx, port);

    assert!(outcome.is_ok(), "a late listener on a free port must pass, got {outcome:?}");
    assert!(receiver.is_some(), "a ready probe must hand the receiver back");
}
