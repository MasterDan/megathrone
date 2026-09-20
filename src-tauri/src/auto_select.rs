//! The automatic endpoint-selection supervisor (the strategy tabs on the
//! endpoints page). While the proxy runs a profile whose selection mode is
//! one of the automatic strategies (round robin / fastest / most available),
//! a background task keeps the session on a good endpoint on two clocks
//! (Settings → General):
//!
//! - the *re-check* clock re-tests the endpoints the last scan proved
//!   reachable (refreshing latency and availability data; a full re-test
//!   runs when the content changed — a "sticky" pass — or nothing reachable
//!   is known; never-tested rows of a past update ride along until stamped)
//!   and applies the strategy's verdict — fastest /
//!   most_available switch on a clear win, round robin only replaces a
//!   current that is not provably reachable;
//! - the *rotation* clock (round robin only) rotates to the next reachable
//!   endpoint.
//!
//! The scan feeds the supervisor a rescue after every batch: an endpoint
//! the fresh data shows is dead or untested is left mid-scan, the moment a
//! reachable one is known.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

use rusqlite::params;

use crate::AppState;
use crate::connection;
use crate::latency;
use crate::profiles;
use crate::settings;

/// Poll cadence while waiting out a concurrently running scan.
const SCAN_WAIT_POLL: Duration = Duration::from_secs(1);
/// Fastest only switches when the challenger is clearly quicker — plain
/// latency jitter must not restart the session.
const FASTEST_HYSTERESIS_MS: i64 = 50;

/// A supervisor clock: when its work last ran (`None` — never), carried
/// across re-arms so a restart right after a tick does not immediately
/// re-run it.
type Clock = Arc<Mutex<Option<Instant>>>;

struct Supervisor {
    generation: u64,
    notify: Arc<Notify>,
    /// set by `content_updated`: the next re-check keeps the surviving
    /// current endpoint instead of switching to the strategy's pick
    sticky: Arc<AtomicBool>,
    /// when the last re-check pass finished
    last_scan: Clock,
    /// when round robin last rotated
    last_switch: Clock,
}

static SUPERVISORS: LazyLock<Mutex<HashMap<i64, Supervisor>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static GENERATION: AtomicU64 = AtomicU64::new(0);

fn lock() -> Result<MutexGuard<'static, HashMap<i64, Supervisor>>, String> {
    SUPERVISORS.lock().map_err(|_| "supervisor registry poisoned".to_string())
}

/// Arms (or re-arms) the supervisor for a profile. An already running task
/// for the same profile exits on its generation check; the fresh task
/// re-reads the mode and session state at every loop top, so callers don't
/// need to be precise about the current state.
pub fn ensure(app: &AppHandle, profile_id: i64) {
    let Ok(mut supervisors) = lock() else { return };
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    // freshness carries over: a re-arm caused by a switch-restart must not
    // re-scan data that is seconds old
    let previous = supervisors.get(&profile_id);
    let last_scan = previous
        .map(|supervisor| Arc::clone(&supervisor.last_scan))
        .unwrap_or_else(|| Arc::new(Mutex::new(None)));
    let last_switch = previous
        .map(|supervisor| Arc::clone(&supervisor.last_switch))
        .unwrap_or_else(|| Arc::new(Mutex::new(None)));
    let notify = Arc::new(Notify::new());
    let sticky = Arc::new(AtomicBool::new(false));
    supervisors.insert(
        profile_id,
        Supervisor {
            generation,
            notify: notify.clone(),
            sticky: sticky.clone(),
            last_scan: Arc::clone(&last_scan),
            last_switch: Arc::clone(&last_switch),
        },
    );
    tauri::async_runtime::spawn(supervisor_loop(
        app.clone(),
        profile_id,
        generation,
        notify,
        sticky,
        last_scan,
        last_switch,
    ));
}

/// Disarms the supervisor (disconnect, unexpected exit, switch to manual).
pub fn stop(profile_id: i64) {
    if let Ok(mut supervisors) = lock() {
        supervisors.remove(&profile_id);
    }
}

/// A profile update finished: wake the supervisor into a sticky re-check
/// that re-tests the fresh endpoints and re-applies the strategy, keeping
/// the current endpoint while it survived the update.
pub fn content_updated(profile_id: i64) {
    if let Ok(supervisors) = lock() {
        if let Some(supervisor) = supervisors.get(&profile_id) {
            supervisor.sticky.store(true, Ordering::Relaxed);
            supervisor.notify.notify_one();
        }
    }
}

fn is_current(profile_id: i64, generation: u64) -> bool {
    lock()
        .ok()
        .and_then(|supervisors| supervisors.get(&profile_id).map(|s| s.generation == generation))
        .unwrap_or(false)
}

/// Marks both clocks as just-fired on the armed supervisor: its fresh task
/// waits out the intervals instead of immediately re-applying its own
/// verdict on top of a pick that was just applied by the user (a mode
/// switch).
pub fn mark_pass_now(profile_id: i64) {
    if let Ok(supervisors) = lock() {
        if let Some(supervisor) = supervisors.get(&profile_id) {
            set_clock(&supervisor.last_scan);
            set_clock(&supervisor.last_switch);
        }
    }
}

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

async fn supervisor_loop(
    app: AppHandle,
    profile_id: i64,
    generation: u64,
    notify: Arc<Notify>,
    sticky: Arc<AtomicBool>,
    last_scan: Clock,
    last_switch: Clock,
) {
    loop {
        if !is_current(profile_id, generation) {
            return;
        }
        // exit when the strategy or the session no longer needs us; a
        // missing profile (deleted mid-run) also ends the task
        let mode = {
            let Some(state) = app.try_state::<AppState>() else { return };
            let Ok(conn) = state.db.lock() else { return };
            match profiles::selection_mode(&conn, profile_id) {
                Ok(mode) => mode,
                Err(_) => return,
            }
        };
        if mode == profiles::SELECT_MANUAL || !connection::runs_profile(profile_id) {
            return;
        }

        // a mode switch may have left the profile without a selection —
        // apply the strategy's pick right away, then keep it fed
        ensure_selection(&app, profile_id, &mode).await;
        if !is_current(profile_id, generation) {
            return;
        }

        // the cadences are settings — re-read them every iteration so live
        // edits apply without a reconnect
        let (rotation_every, recheck_every) = load_intervals(&app);
        let scan_elapsed = clock_elapsed(&last_scan);
        let scan_due = scan_elapsed.is_none_or(|elapsed| elapsed >= recheck_every);
        let sticky_pending = sticky.load(Ordering::Relaxed);
        let rotate_due = mode == profiles::SELECT_ROUND_ROBIN
            && clock_elapsed(&last_switch).is_none_or(|elapsed| elapsed >= rotation_every);

        // a just-finished tick (this supervisor is a re-arm caused by its
        // own switch-restart) waits out the rest of its interval — unless
        // a profile update is already pending, which needs a sticky pass
        // right now
        if !scan_due && !rotate_due && !sticky_pending {
            let mut wait = recheck_every.saturating_sub(scan_elapsed.unwrap_or_default());
            if mode == profiles::SELECT_ROUND_ROBIN {
                let switch_elapsed = clock_elapsed(&last_switch).unwrap_or_default();
                wait = wait.min(rotation_every.saturating_sub(switch_elapsed));
            }
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = notify.notified() => {}
            }
            continue;
        }

        // re-check pass: fresh data + the strategy's verdict (rotation
        // keeps its own clock — a scan verdict only replaces a current
        // that is not provably reachable)
        let mut scanned = false;
        if scan_due || sticky_pending {
            let sticky_pass = sticky.swap(false, Ordering::Relaxed);
            run_pass_scan(&app, profile_id, sticky_pass).await;
            if !is_current(profile_id, generation) {
                return;
            }
            apply_pass(&app, profile_id, generation, &mode, sticky_pass, false).await;
            set_clock(&last_scan);
            scanned = true;
        }

        // rotation tick (round robin): rotate to the next reachable
        // endpoint; with no reachable set known at all (never scanned,
        // everything died since), find one first
        if mode == profiles::SELECT_ROUND_ROBIN && rotate_due {
            if !scanned && !has_reachable_endpoint(&app, profile_id) {
                run_pass_scan(&app, profile_id, true).await;
                if !is_current(profile_id, generation) {
                    return;
                }
                set_clock(&last_scan);
            }
            apply_pass(&app, profile_id, generation, &mode, false, true).await;
            set_clock(&last_switch);
        }
    }
}

/// The supervisor cadences (Settings → General): `(rotation, re-check)`.
/// Falls back to the defaults on any read error — the supervisor must not
/// die over a transient database hiccup.
fn load_intervals(app: &AppHandle) -> (Duration, Duration) {
    let defaults = || {
        (
            Duration::from_secs(settings::DEFAULT_ROTATION_MINUTES.max(1) as u64 * 60),
            Duration::from_secs(settings::DEFAULT_RECHECK_MINUTES.max(1) as u64 * 60),
        )
    };
    let Some(state) = app.try_state::<AppState>() else { return defaults() };
    let Ok(conn) = state.db.lock() else { return defaults() };
    match settings::load_general(&conn) {
        Ok(settings) => (
            Duration::from_secs(settings.rotation_minutes.max(1) as u64 * 60),
            Duration::from_secs(settings.recheck_minutes.max(1) as u64 * 60),
        ),
        Err(_) => defaults(),
    }
}

fn clock_elapsed(clock: &Clock) -> Option<Duration> {
    clock.lock().ok().and_then(|at| at.map(|at| at.elapsed()))
}

fn set_clock(clock: &Clock) {
    if let Ok(mut at) = clock.lock() {
        *at = Some(Instant::now());
    }
}

/// Selects the strategy's pick when the profile has no selection at all
/// (fresh profile, a mode switch, a selection dropped by an update).
async fn ensure_selection(app: &AppHandle, profile_id: i64, mode: &str) {
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
async fn run_pass_scan(app: &AppHandle, profile_id: i64, sticky_pass: bool) {
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

fn has_reachable_endpoint(app: &AppHandle, profile_id: i64) -> bool {
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
async fn apply_pass(
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

/// Mid-scan rescue, called by the latency scan after every persisted batch:
/// while fresh availability data is landing, a connected profile with an
/// automatic strategy must not keep sitting on an endpoint the new data
/// shows is dead or untested — the moment a reachable pick exists, the
/// selection moves and the session switches onto it live (a selector flip
/// on the running instance, or a reconnect when the target was not baked;
/// round robin rotates to the first found reachable endpoint, the quality
/// strategies take their pick). A reachable current is left alone — the
/// pass's regular verdict applies when the scan finishes; manual mode and
/// disconnected profiles are never touched.
pub async fn rescue(app: &AppHandle, profile_id: i64) {
    let picked = {
        let Some(state) = app.try_state::<AppState>() else { return };
        let Ok(conn) = state.db.lock() else { return };
        let Ok(mode) = profiles::selection_mode(&conn, profile_id) else { return };
        if mode == profiles::SELECT_MANUAL || !connection::runs_profile(profile_id) {
            return;
        }
        let current_raw = profiles::selected_raw_key(&conn, profile_id).ok().flatten();
        // only endpoints proven reachable by the data tested so far — the
        // rescue moves onto a living endpoint or does not move at all
        let Ok(reachable) = profiles::load_reachable_candidates(&conn, profile_id) else {
            return;
        };
        let Some(pick) = rescue_pick(&mode, &reachable, current_raw.as_deref()) else {
            return;
        };
        let pick_id = pick.id;
        let tag = pick.tag.clone();
        profiles::set_selected_endpoint(&conn, profile_id, Some(pick_id))
            .unwrap_or(false)
            .then_some(tag)
    };
    if let Some(tag) = picked {
        eprintln!("[auto-select] profile {profile_id}: rescued onto {tag} mid-scan");
        profiles::after_mutation(app, Some(profile_id), profiles::SELECTION);
        let _ = connection::switch_endpoint_if_running(app.clone(), profile_id).await;
    }
}

/// The rescue verdict over the endpoints tested so far (all reachable):
/// `Some(pick)` when the current selection is not among the proven-reachable
/// (dead, untested or dangling) and the strategy has a pick. A reachable
/// current waits for the pass's regular verdict — no mid-scan churn.
fn rescue_pick<'a>(
    mode: &str,
    reachable: &'a [profiles::EndpointCandidate],
    current_raw: Option<&str>,
) -> Option<&'a profiles::EndpointCandidate> {
    if let Some(raw) = current_raw {
        if reachable.iter().any(|candidate| candidate.raw == raw) {
            return None;
        }
    }
    profiles::pick_by_strategy(mode, reachable, current_raw)
}

/// Whether the challenger is worth switching the live session for:
/// - a sticky pass (just after a content update) keeps the current endpoint
///   unless it is known-dead — untested fresh rows are not a reason to move;
/// - fastest switches only on a clear latency win (hysteresis, so jitter
///   does not flap the session);
/// - most available switches on a strictly better deep-probe share.
fn should_switch_quality(
    mode: &str,
    current: &profiles::EndpointCandidate,
    pick: &profiles::EndpointCandidate,
    sticky_pass: bool,
) -> bool {
    if current.available != Some(true) {
        return true; // dead or unknown — the strategy's pick cannot be worse
    }
    if sticky_pass {
        return false; // survived the update and alive — hold it
    }
    match mode {
        profiles::SELECT_MOST_AVAILABLE => url_share_of(pick) > url_share_of(current),
        _ => match (pick.latency_ms, current.latency_ms) {
            (Some(pick_latency), Some(current_latency)) => {
                pick_latency + FASTEST_HYSTERESIS_MS < current_latency
            }
            // the pick measured, the current one did not — trust data
            (Some(_), None) => true,
            _ => false,
        },
    }
}

fn url_share_of(candidate: &profiles::EndpointCandidate) -> f64 {
    if candidate.url_total > 0 {
        candidate.url_ok as f64 / candidate.url_total as f64
    } else {
        -1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use profiles::EndpointCandidate;

    fn candidate(
        tag: &str,
        available: Option<bool>,
        latency_ms: Option<i64>,
        url_ok: i64,
        url_total: i64,
    ) -> EndpointCandidate {
        EndpointCandidate {
            id: 0,
            raw: format!("raw-{tag}"),
            tag: tag.to_string(),
            available,
            latency_ms,
            url_ok,
            url_total,
            order_index: 0,
        }
    }

    #[test]
    fn quality_switches_only_on_clear_wins() {
        let current = candidate("Current", Some(true), Some(100), 2, 4);
        let same = candidate("Same", Some(true), Some(80), 2, 4);
        let slower = candidate("Slower", Some(true), Some(120), 3, 4);
        let share = candidate("Share", Some(true), Some(50), 3, 4);

        // fastest: 80ms vs 100ms is inside the hysteresis band — no switch
        assert!(!should_switch_quality(profiles::SELECT_FASTEST, &current, &same, false));
        // 49ms vs 100ms is a clear win
        let quick = candidate("Quick", Some(true), Some(49), 0, 0);
        assert!(should_switch_quality(profiles::SELECT_FASTEST, &current, &quick, false));
        // unmeasured current loses to a measured challenger
        let blind = candidate("Blind", Some(true), None, 0, 0);
        assert!(should_switch_quality(profiles::SELECT_FASTEST, &blind, &same, false));
        assert!(!should_switch_quality(profiles::SELECT_FASTEST, &current, &slower, false));

        // most available: a strictly better share switches, latency does not
        assert!(should_switch_quality(profiles::SELECT_MOST_AVAILABLE, &current, &share, false));
        assert!(!should_switch_quality(profiles::SELECT_MOST_AVAILABLE, &current, &same, false));

        // a dead or unknown current always switches
        let dead = candidate("Dead", Some(false), None, 0, 0);
        assert!(should_switch_quality(profiles::SELECT_FASTEST, &dead, &same, false));
        let unknown = candidate("Unknown", None, None, 0, 0);
        assert!(should_switch_quality(profiles::SELECT_FASTEST, &unknown, &same, false));

        // sticky pass: an alive current is kept, even if outclassed
        assert!(!should_switch_quality(profiles::SELECT_FASTEST, &current, &quick, true));
        assert!(!should_switch_quality(
            profiles::SELECT_MOST_AVAILABLE,
            &current,
            &share,
            true
        ));
        // ...but a dead one still switches
        assert!(should_switch_quality(profiles::SELECT_FASTEST, &dead, &same, true));
    }

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

    #[test]
    fn rescue_moves_only_off_a_not_reachable_current() {
        // the list models what load_reachable_candidates returns: only the
        // endpoints the scan data has proven reachable so far
        let reachable = vec![
            candidate("First", Some(true), Some(90), 0, 0),
            candidate("Second", Some(true), Some(120), 0, 0),
        ];

        // a reachable current is never rescued away mid-scan
        assert!(rescue_pick(profiles::SELECT_ROUND_ROBIN, &reachable, Some("raw-Second")).is_none());
        assert!(rescue_pick(profiles::SELECT_FASTEST, &reachable, Some("raw-First")).is_none());

        // an untested / dead / dangling selection moves onto the strategy's
        // reachable pick: round robin takes the first in list order,
        // fastest the lowest latency
        let rr = rescue_pick(profiles::SELECT_ROUND_ROBIN, &reachable, Some("raw-untested"))
            .expect("rescue");
        assert_eq!(rr.tag, "First");
        let dangling =
            rescue_pick(profiles::SELECT_MOST_AVAILABLE, &reachable, Some("raw-gone")).expect("rescue");
        assert_eq!(dangling.tag, "First");

        // nothing reachable known yet → nothing to rescue with
        assert!(rescue_pick(profiles::SELECT_ROUND_ROBIN, &[], Some("raw-untested")).is_none());
        // and with no selection at all the first reachable is the rescue
        let pick = rescue_pick(profiles::SELECT_ROUND_ROBIN, &reachable, None).expect("rescue");
        assert_eq!(pick.tag, "First");
    }
}
