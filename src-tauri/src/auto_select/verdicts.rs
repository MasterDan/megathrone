use tauri::{AppHandle, Manager};

use crate::AppState;
use crate::connection;
use crate::profiles;

/// Fastest (and the most-available latency tie-break) only switches when
/// the challenger is clearly quicker — plain latency jitter must not
/// restart the session.
const FASTEST_HYSTERESIS_MS: i64 = 50;

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

#[cfg(test)]
mod rescue_tests {
    use super::rescue_pick;
    use crate::auto_select::auto_select_tests::candidate;
    use crate::profiles;

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

/// Whether the challenger is worth switching the live session for:
/// - a sticky pass (just after a content update) keeps the current endpoint
///   unless it is known-dead — untested fresh rows are not a reason to move;
/// - fastest switches only on a clear latency win (hysteresis, so jitter
///   does not flap the session);
/// - most available switches on a strictly better deep-probe share — or,
///   on an equal share (the pick's own tie-break territory, e.g. 96% vs
///   96% at 596ms vs 70ms), on a clear latency win.
pub(super) fn should_switch_quality(
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
        profiles::SELECT_MOST_AVAILABLE => {
            let (pick_share, current_share) = (url_share_of(pick), url_share_of(current));
            pick_share > current_share
                || (pick_share == current_share && latency_clearly_better(pick, current))
        }
        _ => latency_clearly_better(pick, current),
    }
}

/// The challenger's latency beats the current's beyond the hysteresis
/// band; an unmeasured current loses to a measured pick (trust data), an
/// unmeasured pick never wins.
fn latency_clearly_better(
    pick: &profiles::EndpointCandidate,
    current: &profiles::EndpointCandidate,
) -> bool {
    match (pick.latency_ms, current.latency_ms) {
        (Some(pick_latency), Some(current_latency)) => {
            pick_latency + FASTEST_HYSTERESIS_MS < current_latency
        }
        // the pick measured, the current one did not — trust data
        (Some(_), None) => true,
        _ => false,
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
mod quality_tests {
    use super::should_switch_quality;
    use crate::auto_select::auto_select_tests::candidate;
    use crate::profiles;

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

        // most available on an equal share falls back to the pick's own
        // tie-break — latency: a clear win switches (the mid-scan rescue
        // may have landed on an equal-share-but-slow endpoint), jitter
        // inside the hysteresis band does not
        let slow_share = candidate("SlowShare", Some(true), Some(600), 24, 25);
        let fast_share = candidate("FastShare", Some(true), Some(70), 24, 25);
        assert!(should_switch_quality(
            profiles::SELECT_MOST_AVAILABLE,
            &slow_share,
            &fast_share,
            false
        ));
        let jitter_share = candidate("JitterShare", Some(true), Some(560), 24, 25);
        assert!(!should_switch_quality(
            profiles::SELECT_MOST_AVAILABLE,
            &slow_share,
            &jitter_share,
            false
        ));
        // an unprobed pair (no deep-probe URLs at all) is an equal share
        // too — the strategy degenerates to fastest's hysteresis
        let slow_blind = candidate("SlowBlind", Some(true), Some(600), 0, 0);
        let fast_blind = candidate("FastBlind", Some(true), Some(70), 0, 0);
        assert!(should_switch_quality(
            profiles::SELECT_MOST_AVAILABLE,
            &slow_blind,
            &fast_blind,
            false
        ));

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
}
