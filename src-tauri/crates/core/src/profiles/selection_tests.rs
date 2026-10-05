use super::model::EndpointCandidate;
use super::selection::{
    meets_availability_floor, pick_fastest, pick_most_available, pick_round_robin,
};

fn candidate(tag: &str, available: Option<bool>, latency_ms: Option<i64>) -> EndpointCandidate {
    EndpointCandidate {
        id: 0,
        raw: format!("raw-{tag}"),
        tag: tag.to_string(),
        available,
        latency_ms,
        url_ok: 0,
        url_total: 0,
        order_index: 0,
    }
}

fn fixture() -> Vec<EndpointCandidate> {
    vec![
        candidate("A", Some(true), Some(120)),
        candidate("B", Some(true), Some(80)),
        candidate("C", None, None),
        candidate("D", Some(false), None),
        candidate("E", Some(true), Some(200)),
    ]
}

#[test]
fn pick_fastest_prefers_reachable_lowest_latency() {
    let items = fixture();
    // B is reachable and fastest
    assert_eq!(pick_fastest(&items).expect("pick").tag, "B");

    // B dies → next reachable by latency (A), never the dead D
    let mut dead_b = items.clone();
    dead_b[1].available = Some(false);
    dead_b[1].latency_ms = None;
    assert_eq!(pick_fastest(&dead_b).expect("pick").tag, "A");

    // nothing tested: the untested C outranks the dead D
    let fresh = vec![candidate("C", None, None), candidate("D", Some(false), None)];
    assert_eq!(pick_fastest(&fresh).expect("pick").tag, "C");

    assert!(pick_fastest(&[]).is_none());
}

#[test]
fn pick_most_available_ranks_by_url_share() {
    let mut items = fixture();
    // A loads 1/2 URLs, E loads 2/2 — E wins despite being slower
    items[0].url_ok = 1;
    items[0].url_total = 2;
    items[4].url_ok = 2;
    items[4].url_total = 2;
    assert_eq!(pick_most_available(&items).expect("pick").tag, "E");

    // a probed zero share ranks below an unprobed reachable endpoint
    let mut zero = fixture();
    zero[1].url_ok = 0;
    zero[1].url_total = 2;
    assert_eq!(pick_most_available(&zero).expect("pick").tag, "A");

    // dead endpoints never win
    let dead = vec![candidate("D", Some(false), None)];
    assert_eq!(pick_most_available(&dead).expect("pick").tag, "D");
}

#[test]
fn pick_round_robin_rotates_and_wraps() {
    let items = fixture();

    // from A → the next reachable is B; wrapping E → A
    assert_eq!(pick_round_robin(&items, Some("raw-A")).expect("pick").tag, "B");
    assert_eq!(pick_round_robin(&items, Some("raw-E")).expect("pick").tag, "A");

    // skips the dead D and the untested C
    assert_eq!(pick_round_robin(&items, Some("raw-B")).expect("pick").tag, "E");

    // nothing reachable after the cursor → the current one is kept
    let all_dead = vec![candidate("X", Some(false), None), candidate("Y", Some(false), None)];
    assert_eq!(pick_round_robin(&all_dead, Some("raw-X")).expect("pick").tag, "X");

    // no cursor: the first reachable endpoint
    assert_eq!(pick_round_robin(&items, None).expect("pick").tag, "A");

    // no cursor, nothing reachable, scan data exists → nothing to
    // rotate to (never an untested or known-dead endpoint)
    let scanned = vec![candidate("X", Some(false), None), candidate("Y", None, None)];
    assert!(pick_round_robin(&scanned, None).is_none());

    // no cursor, never scanned → the first listed endpoint
    let fresh = vec![candidate("X", None, None), candidate("Y", None, None)];
    assert_eq!(pick_round_robin(&fresh, None).expect("pick").tag, "X");

    assert!(pick_round_robin(&[], Some("raw-A")).is_none());
}

#[test]
fn meets_availability_floor_boundaries() {
    // unprobed endpoints and a disabled floor accept everything
    assert!(meets_availability_floor(0, 0, 55));
    assert!(meets_availability_floor(0, 5, 0));
    assert!(meets_availability_floor(0, 5, -1));
    // exactly at the floor passes, one under it fails
    assert!(meets_availability_floor(55, 100, 55));
    assert!(!meets_availability_floor(54, 100, 55));
    assert!(meets_availability_floor(11, 20, 55));
    assert!(!meets_availability_floor(1, 2, 55));
    // a 100% floor demands every probed URL
    assert!(meets_availability_floor(2, 2, 100));
    assert!(!meets_availability_floor(1, 2, 100));
}
