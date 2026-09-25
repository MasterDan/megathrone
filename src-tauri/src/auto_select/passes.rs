use std::time::Duration;

use tauri::{AppHandle, Manager};

use rusqlite::params;

use super::supervisor::{is_current, mark_pass_now};
use super::verdicts::should_switch_quality;
use crate::AppState;
use crate::connection;
use crate::latency;
use crate::profiles;

/// Poll cadence while waiting out a concurrently running scan.
const SCAN_WAIT_POLL: Duration = Duration::from_secs(1);

/// Applies a strategy pick right now (a mode switch while connected),
/// without waiting for a scan pass: round robin rotates to the next
/// endpoint, the others take the current data's best. Returns the tag the
/// session runs; `Err` when the pick could not be applied. Call after
/// `ensure` (the supervisor must exist so the pass bookkeeping lands).
pub async fn apply_now(app: &AppHandle, profile_id: i64, mode: &str) -> Result<String, String> {
    let picked = {
        let Some(state) = app.try_state::<AppState>() else {
            return Err("the app state is unavailable".to_string());
        };
        let Ok(conn) = state.db.lock() else {
            return Err("database lock poisoned".to_string());
        };
        let current = profiles::selected_raw_key(&conn, profile_id).ok().flatten();
        let candidates = profiles::load_candidates(&conn, profile_id)?;
        match profiles::pick_by_strategy(mode, &candidates, current.as_deref()) {
            Some(pick) => (pick.id, pick.raw.clone(), pick.tag.clone(), current),
            None => return Err("the profile has no endpoints to choose from".to_string()),
        }
    };
    let (pick_id, pick_raw, pick_tag, current_raw) = picked;
    // the supervisor (armed by the caller) must not immediately re-decide
    // on top of this pick
    mark_pass_now(profile_id);
    if current_raw.as_deref() == Some(pick_raw.as_str()) {
        return Ok(pick_tag); // the strategy already runs this endpoint
    }
    let switched = {
        let Some(state) = app.try_state::<AppState>() else {
            return Err("the app state is unavailable".to_string());
        };
        let Ok(conn) = state.db.lock() else {
            return Err("database lock poisoned".to_string());
        };
        profiles::set_selected_endpoint(&conn, profile_id, Some(pick_id)).unwrap_or(false)
    };
    if !switched {
        return Ok(pick_tag);
    }
    profiles::after_mutation(app, Some(profile_id), profiles::SELECTION);
    match connection::switch_endpoint_if_running(app.clone(), profile_id).await {
        Ok(Some(tag)) => Ok(tag),
        Ok(None) => Ok(pick_tag),
        Err(error) => Err(error),
    }
}

/// Selects the strategy's pick when the profile has no selection at all
/// (fresh profile, a mode switch, a selection dropped by an update).
pub(super) async fn ensure_selection(app: &AppHandle, profile_id: i64, mode: &str) {
    let selected = {
        let Some(state) = app.try_state::<AppState>() else { return };
        let Ok(conn) = state.db.lock() else { return };
        if profiles::selected_raw_key(&conn, profile_id).ok().flatten().is_some() {
            return;
        }
        let candidates = match profiles::load_candidates(&conn, profile_id) {
            Ok(candidates) => candidates,
            Err(_) => return,
        };
        let mut picked_id = None;
        if let Some(pick) = profiles::pick_by_strategy(mode, &candidates, None) {
            if profiles::set_selected_endpoint(&conn, profile_id, Some(pick.id)).is_ok() {
                picked_id = Some(pick.id);
            }
        }
        picked_id
    };
    if selected.is_some() {
        profiles::after_mutation(app, Some(profile_id), profiles::SELECTION);
        let _ = connection::switch_endpoint_if_running(app.clone(), profile_id).await;
    }
}

/// One scan pass: waits out a concurrently running scan (the button or a
/// previous pass), then runs its own. A lost race (another scan started in
/// the gap) is fine — the pass logic simply works with whatever data is
/// there and the next pass re-tests.
pub(super) async fn run_pass_scan(app: &AppHandle, profile_id: i64, sticky_pass: bool) {
    while latency::scan_running(profile_id) {
        tokio::time::sleep(SCAN_WAIT_POLL).await;
    }
    let scope = pass_scope(
        sticky_pass,
        has_reachable_endpoint(app, profile_id),
        has_untested_endpoint(app, profile_id),
    );
    let _ = latency::run_scan(app, profile_id, scope).await;
}

/// A pass re-checks only the endpoints the last scan proved reachable —
/// with most of a large profile dead, re-testing all of it every pass burns
/// the whole interval. Right after a content update (a sticky pass) the
/// diff has already kept every surviving endpoint's results, so the pass
/// only needs the rows no scan has ever stamped (plus, when the reachable
/// set was wiped out, a full scan — the only way to find new lives). A
/// regular pass on a profile that still carries never-tested rows —
/// connected right after an update whose sticky pass nobody ran — picks
/// that debt up alongside the reachable re-check, once, and falls back to
/// the cheap re-check when everything has been stamped.
fn pass_scope(sticky_pass: bool, has_reachable: bool, has_untested: bool) -> latency::ScanScope {
    if sticky_pass && has_reachable {
        latency::ScanScope::UntestedOnly
    } else if sticky_pass || !has_reachable {
        latency::ScanScope::All
    } else if has_untested {
        latency::ScanScope::ReachableAndUntested
    } else {
        latency::ScanScope::ReachableOnly
    }
}

#[cfg(test)]
mod tests {
    use super::pass_scope;
    use crate::latency;

    #[test]
    fn pass_scope_rechecks_only_reachable_endpoints() {
        assert_eq!(pass_scope(false, true, false), latency::ScanScope::ReachableOnly);
        // a sticky pass after a diff update only needs the never-tested
        // rows — survivors keep their results through the update
        assert_eq!(pass_scope(true, true, false), latency::ScanScope::UntestedOnly);
        assert_eq!(pass_scope(false, false, false), latency::ScanScope::All);
        // ...but with no reachable set left the sticky pass goes full
        assert_eq!(pass_scope(true, false, false), latency::ScanScope::All);
        // a regular pass picks up never-tested rows (e.g. connected right
        // after an update whose sticky pass nobody ran) alongside the
        // reachable re-check
        assert_eq!(
            pass_scope(false, true, true),
            latency::ScanScope::ReachableAndUntested
        );
        // nothing reachable known → a full scan covers the untested anyway
        assert_eq!(pass_scope(false, false, true), latency::ScanScope::All);
    }
}

pub(super) fn has_reachable_endpoint(app: &AppHandle, profile_id: i64) -> bool {
    let Some(state) = app.try_state::<AppState>() else { return false };
    let Ok(conn) = state.db.lock() else { return false };
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM endpoints WHERE profile_id = ?1 AND available = 1)",
        params![profile_id],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// Whether the profile still has endpoints no scan has ever stamped —
/// fresh rows of a past update waiting for their first test.
fn has_untested_endpoint(app: &AppHandle, profile_id: i64) -> bool {
    let Some(state) = app.try_state::<AppState>() else { return false };
    let Ok(conn) = state.db.lock() else { return false };
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM endpoints WHERE profile_id = ?1 AND last_tested_at IS NULL)",
        params![profile_id],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// Applies the strategy to the fresh scan data: computes the pick and
/// switches the live session when it differs from the current selection.
/// `rotate` marks the round-robin rotation tick — on it rotation is the
/// whole point; on scan ticks (`rotate == false`) a reachable current is
/// kept and only a dead, unknown or dangling one is replaced.
pub(super) async fn apply_pass(
    app: &AppHandle,
    profile_id: i64,
    generation: u64,
    mode: &str,
    sticky_pass: bool,
    rotate: bool,
) {
    let picked = {
        let Some(state) = app.try_state::<AppState>() else { return };
        let Ok(conn) = state.db.lock() else { return };
        let Some(current_raw) = profiles::selected_raw_key(&conn, profile_id).ok().flatten()
        else {
            return;
        };
        let Ok(candidates) = profiles::load_candidates(&conn, profile_id) else {
            return;
        };
        let pick = match profiles::pick_by_strategy(mode, &candidates, Some(&current_raw)) {
            Some(pick) => pick,
            None => return,
        };
        if pick.raw == current_raw {
            return; // nothing to do
        }
        let current = candidates.iter().find(|candidate| candidate.raw == current_raw);
        let should_switch = match current {
            // the current endpoint is gone or broken — switch
            None => true,
            Some(current) => match mode {
                // rotation is the whole point of round robin — but only on
                // the rotation tick; a scan verdict keeps a reachable
                // current (rescue already moved off the dead ones)
                profiles::SELECT_ROUND_ROBIN => {
                    rotate || current.available != Some(true)
                }
                _ => should_switch_quality(mode, current, pick, sticky_pass),
            },
        };
        should_switch.then(|| (pick.id, pick.tag.clone()))
    };
    if let Some((item_id, tag)) = picked {
        // a manual pick / mode switch / disarm that landed while the pass
        // was deciding must not be overwritten by its stale verdict
        if !is_current(profile_id, generation) {
            return;
        }
        let switched = {
            let Some(state) = app.try_state::<AppState>() else { return };
            let Ok(conn) = state.db.lock() else { return };
            profiles::set_selected_endpoint(&conn, profile_id, Some(item_id)).unwrap_or(false)
        };
        if switched {
            eprintln!("[auto-select] profile {profile_id} ({mode}): now on {tag}");
            profiles::after_mutation(app, Some(profile_id), profiles::SELECTION);
            let _ = connection::switch_endpoint_if_running(app.clone(), profile_id).await;
        }
    }
}
