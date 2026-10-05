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

use super::passes::{apply_pass, ensure_selection, has_reachable_endpoint, run_pass_scan};
use crate::AppState;
use crate::connection;
use crate::profiles;
use crate::settings;

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

pub(super) fn is_current(profile_id: i64, generation: u64) -> bool {
    lock()
        .ok()
        .and_then(|supervisors| supervisors.get(&profile_id).map(|s| s.generation == generation))
        .unwrap_or(false)
}

/// Marks both clocks as just-fired on the armed supervisor: its fresh task
/// waits out the intervals instead of immediately re-applying its own
/// verdict on top of a pick that was just applied by the user (a mode
/// switch).
pub(super) fn mark_pass_now(profile_id: i64) {
    if let Ok(supervisors) = lock() {
        if let Some(supervisor) = supervisors.get(&profile_id) {
            set_clock(&supervisor.last_scan);
            set_clock(&supervisor.last_switch);
        }
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
