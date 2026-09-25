//! Read-only queries over the session state plus the status command the UI
//! polls.

use super::state::{ConnectionSnapshot, MODE_TUN, current_snapshot, lock_state};

#[tauri::command]
pub fn connection_status() -> Result<ConnectionSnapshot, String> {
    Ok(current_snapshot())
}

/// Read-only access for sibling modules (toast messages etc.).
pub fn snapshot() -> ConnectionSnapshot {
    current_snapshot()
}

/// Whether the long-lived proxy currently runs in TUN mode — its routes
/// would capture a DPI strategy test's outgoing probes and skew the
/// results, so the test refuses to run while it is active.
pub(crate) fn tun_active() -> bool {
    lock_state()
        .map(|state| state.running.as_ref().is_some_and(|run| run.mode == MODE_TUN))
        .unwrap_or(false)
}

/// Whether the long-lived proxy currently runs this profile (the supervisor
/// only lives as long as its session does).
pub(crate) fn runs_profile(profile_id: i64) -> bool {
    lock_state()
        .map(|state| state.running.as_ref().is_some_and(|run| run.profile_id == profile_id))
        .unwrap_or(false)
}

/// The profile the live session runs, if any (settings changes decide
/// whether a restart is needed).
pub fn running_profile() -> Option<i64> {
    lock_state()
        .ok()
        .and_then(|state| state.running.as_ref().map(|run| run.profile_id))
}

/// Whether the byedpi tunnel runs alongside the live session.
pub fn dpi_tunnel_running() -> bool {
    lock_state()
        .map(|state| {
            state
                .running
                .as_ref()
                .is_some_and(|run| run.dpi.is_some())
        })
        .unwrap_or(false)
}
