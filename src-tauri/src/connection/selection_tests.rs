//! Tests of the endpoint-selection storage layer: the stored checkmark
//! resolution, the dead-selection check, the strategy auto-pick, the
//! live-switch target resolver and the check-failure repick verdict.

use std::collections::HashSet;

use rusqlite::{Connection, params};

use crate::profiles;

use super::selection::{
    SelectedEndpoint, auto_pick_endpoint, decide_repick, load_selected_endpoint,
    selected_endpoint_brief, selection_known_dead, Repick,
};

fn test_db() -> Connection {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "megathrone-connection-{}-{id}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
}

#[test]
fn load_selected_endpoint_treats_missing_choice_as_none() {
    let conn = test_db();
    conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 2)", [])
        .expect("seed profile");
    let profile_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
         VALUES (?1, 'First', 'vless', 'raw-first', ?2, 0)",
        params![profile_id, r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#],
    )
    .expect("seed endpoint");

    // no selection → None (the connect flow decides whether that is fatal)
    let (name, endpoint) = load_selected_endpoint(&conn, profile_id).expect("load");
    assert_eq!(name, "Sub");
    assert!(endpoint.is_none(), "no checkmark means no endpoint");

    // selected → loads the stored endpoint
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = 'raw-first' WHERE id = ?1",
        params![profile_id],
    )
    .expect("select");
    let (_, endpoint) = load_selected_endpoint(&conn, profile_id).expect("selected endpoint");
    let endpoint = endpoint.expect("the checkmark resolves");
    assert_eq!(endpoint.id, 1);
    assert_eq!(endpoint.tag, "First");

    // dangling selection (endpoint vanished in an update) → None
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = 'gone' WHERE id = ?1",
        params![profile_id],
    )
    .expect("dangle");
    let (_, endpoint) = load_selected_endpoint(&conn, profile_id).expect("dangling key");
    assert!(endpoint.is_none(), "a dangling key is no selection");

    // unknown profile
    assert!(load_selected_endpoint(&conn, 999).is_err());

    // stored outbound without a type is unusable — treated as no
    // selection so the auto-pick can replace it
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
         VALUES (?1, 'Broken', 'vless', 'raw-broken', '{}', 1)",
        params![profile_id],
    )
    .expect("seed broken");
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = 'raw-broken' WHERE id = ?1",
        params![profile_id],
    )
    .expect("select broken");
    let (_, endpoint) = load_selected_endpoint(&conn, profile_id).expect("broken outbound");
    assert!(endpoint.is_none(), "a broken selection reads as no selection");
}

/// Without a usable selection the connect flow picks one itself: the
/// strategy's pick is persisted and loaded, broken rows are skipped.
#[test]
fn auto_pick_endpoint_fills_missing_selections() {
    let conn = test_db();
    conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 2)", [])
        .expect("seed profile");
    let profile_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available, latency_ms)
         VALUES (?1, 'Slow', 'vless', 'raw-slow', ?2, 0, 1, 500),
                (?1, 'Quick', 'trojan', 'raw-quick', ?3, 1, 1, 30),
                (?1, 'Broken', 'vless', 'raw-broken', '{}', 2, NULL, NULL)",
        params![
            profile_id,
            r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#,
            r#"{"type":"trojan","server":"5.6.7.8","server_port":443}"#
        ],
    )
    .expect("seed endpoints");

    // fastest (also the manual fallback): Quick wins, and it is persisted
    let endpoint = auto_pick_endpoint(&conn, profile_id, profiles::SELECT_FASTEST)
        .expect("auto pick")
        .expect("a pick exists");
    assert_eq!(endpoint.tag, "Quick");
    assert_eq!(endpoint.id, 2);
    assert_eq!(
        profiles::selected_raw_key(&conn, profile_id).expect("raw key").as_deref(),
        Some("raw-quick")
    );

    // round robin from nothing: the first reachable endpoint
    let endpoint = auto_pick_endpoint(&conn, profile_id, profiles::SELECT_ROUND_ROBIN)
        .expect("auto pick")
        .expect("a pick exists");
    assert_eq!(endpoint.tag, "Slow");

    // nothing pickable at all → None (the connect flow decides the rest)
    conn.execute("DELETE FROM endpoints WHERE profile_id = ?1", params![profile_id])
        .expect("wipe");
    assert!(auto_pick_endpoint(&conn, profile_id, profiles::SELECT_FASTEST)
        .expect("auto pick")
        .is_none());
}

/// A stored selection the last scan marked dead is replaced by the
/// strategy's pick at connect time (automatic modes only): round robin
/// rotates from the dead cursor onto the next reachable endpoint.
#[test]
fn auto_pick_rotates_off_a_dead_selection() {
    let conn = test_db();
    conn.execute(
        "INSERT INTO profiles (name, item_count, selected_endpoint_key)
         VALUES ('Sub', 3, 'raw-dead')",
        [],
    )
    .expect("seed profile");
    let profile_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available)
         VALUES (?1, 'Dead',     'vless',  'raw-dead',  ?2, 0, 0),
                (?1, 'Alive',    'trojan', 'raw-alive', ?3, 1, 1),
                (?1, 'Untested', 'socks5', 'raw-new',   ?4, 2, NULL)",
        params![
            profile_id,
            r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#,
            r#"{"type":"trojan","server":"5.6.7.8","server_port":443}"#,
            r#"{"type":"socks","server":"9.9.9.9","server_port":1080}"#
        ],
    )
    .expect("seed endpoints");

    // only a known-dead resolution reads as dead
    assert!(selection_known_dead(&conn, profile_id));
    for key in ["raw-alive", "raw-new"] {
        conn.execute(
            "UPDATE profiles SET selected_endpoint_key = ?2 WHERE id = ?1",
            params![profile_id, key],
        )
        .expect("reselect");
        assert!(!selection_known_dead(&conn, profile_id), "{key} is not dead");
    }
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = NULL WHERE id = ?1",
        params![profile_id],
    )
    .expect("clear");
    assert!(!selection_known_dead(&conn, profile_id));

    // round robin from the dead cursor: the next reachable endpoint
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = 'raw-dead' WHERE id = ?1",
        params![profile_id],
    )
    .expect("select dead");
    let endpoint = auto_pick_endpoint(&conn, profile_id, profiles::SELECT_ROUND_ROBIN)
        .expect("auto pick")
        .expect("a pick exists");
    assert_eq!(endpoint.tag, "Alive");
    assert_eq!(
        profiles::selected_raw_key(&conn, profile_id)
            .expect("raw key")
            .as_deref(),
        Some("raw-alive")
    );

    // nothing reachable anywhere: the dead current is kept as the last
    // resort (something must be dialed), the untested one is not chosen
    conn.execute(
        "UPDATE endpoints SET available = 0 WHERE raw = 'raw-alive'",
        [],
    )
    .expect("kill alive");
    let endpoint = auto_pick_endpoint(&conn, profile_id, profiles::SELECT_ROUND_ROBIN)
        .expect("auto pick")
        .expect("kept current");
    assert_eq!(endpoint.tag, "Alive");
}

/// The live-switch target resolver: the stored key maps to the row's id
/// and display tag; a missing or dangling key is no selection.
#[test]
fn selected_endpoint_brief_resolves_the_stored_key() {
    let conn = test_db();
    conn.execute(
        "INSERT INTO profiles (name, item_count, selected_endpoint_key)
         VALUES ('Sub', 2, 'raw-first')",
        [],
    )
    .expect("seed profile");
    let profile_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
         VALUES (?1, 'First', 'vless', 'raw-first', ?2, 0),
                (?1, 'Second', 'trojan', 'raw-second', ?3, 1)",
        params![
            profile_id,
            r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#,
            r#"{"type":"trojan","server":"5.6.7.8","server_port":443}"#
        ],
    )
    .expect("seed endpoints");

    assert_eq!(
        selected_endpoint_brief(&conn, profile_id).expect("brief"),
        Some((1, "First".to_string()))
    );

    // dangling key → None
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = 'gone' WHERE id = ?1",
        params![profile_id],
    )
    .expect("dangle");
    assert_eq!(selected_endpoint_brief(&conn, profile_id).expect("brief"), None);

    // no key → None
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = NULL WHERE id = ?1",
        params![profile_id],
    )
    .expect("clear");
    assert_eq!(selected_endpoint_brief(&conn, profile_id).expect("brief"), None);
}

/// The check-failure repick verdict (`decide_repick`): a selection that
/// survives the isolation is kept untouched, an unloadable manual pick is
/// reported (never silently replaced), every other lost selection re-picks
/// per the strategy among the survivors.
#[test]
fn decide_repick_resolves_the_selection_against_loadable_survivors() {
    let conn = test_db();
    conn.execute(
        "INSERT INTO profiles (name, item_count) VALUES ('Sub', 3)",
        [],
    )
    .expect("seed profile");
    let profile_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available, latency_ms)
         VALUES (?1, 'Slow',    'vless',  'raw-slow', ?2, 0, 1, 500),
                (?1, 'Quick',   'trojan', 'raw-quick', ?3, 1, 1, 30),
                (?1, 'Poisoned', 'vless', 'raw-junk',  ?4, 2, 1, 80)",
        params![
            profile_id,
            r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#,
            r#"{"type":"trojan","server":"5.6.7.8","server_port":443}"#,
            r#"{"type":"vless","server":"9.9.9.9","server_port":443}"#
        ],
    )
    .expect("seed endpoints");
    let (slow_id, quick_id, junk_id) = (1, 2, 3);
    let loadable: HashSet<i64> = [slow_id, quick_id].into_iter().collect();
    let selected = |id: i64| {
        Some(SelectedEndpoint {
            id,
            tag: format!("tag-{id}"),
        })
    };
    let stored_key = |conn: &Connection| {
        profiles::selected_raw_key(conn, profile_id)
            .expect("raw key")
            .unwrap_or_default()
    };

    // a surviving selection is kept as-is — no re-pick, no DB write
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = 'raw-slow' WHERE id = ?1",
        params![profile_id],
    )
    .expect("select slow");
    let outcome =
        decide_repick(&conn, profile_id, profiles::SELECT_FASTEST, false, selected(slow_id), &loadable)
            .expect("decide");
    assert!(matches!(outcome, Repick::Kept(ref pick) if pick.id == slow_id));
    assert_eq!(stored_key(&conn), "raw-slow");

    // the selection itself is unloadable and manual → Lost (the caller
    // surfaces the check error; the pick stays the user's)
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = 'raw-junk' WHERE id = ?1",
        params![profile_id],
    )
    .expect("select junk");
    let outcome =
        decide_repick(&conn, profile_id, profiles::SELECT_FASTEST, true, selected(junk_id), &loadable)
            .expect("decide");
    assert!(matches!(outcome, Repick::Lost));
    assert_eq!(stored_key(&conn), "raw-junk");

    // the same unloadable selection under an automatic strategy → Switched
    // to the strategy's pick among the survivors (persisted)
    let outcome =
        decide_repick(&conn, profile_id, profiles::SELECT_FASTEST, false, selected(junk_id), &loadable)
            .expect("decide");
    let Repick::Switched(pick) = outcome else {
        panic!("expected Switched, got {outcome:?}");
    };
    assert_eq!(pick.id, quick_id);
    assert_eq!(stored_key(&conn), "raw-quick");

    // manual without any selection (the connect's fastest fallback ran,
    // so manual_pick is false — but a manual mode re-pick must also work)
    // re-picks among the survivors like any other lost pick
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = NULL WHERE id = ?1",
        params![profile_id],
    )
    .expect("clear selection");
    let outcome =
        decide_repick(&conn, profile_id, profiles::SELECT_MANUAL, true, None, &loadable)
            .expect("decide");
    let Repick::Switched(pick) = outcome else {
        panic!("expected Switched, got a different verdict");
    };
    assert_eq!(pick.id, quick_id);
    assert_eq!(stored_key(&conn), "raw-quick");

    // nothing loadable survives at all → Empty, with or without a pick
    let outcome =
        decide_repick(&conn, profile_id, profiles::SELECT_FASTEST, false, selected(junk_id), &HashSet::new())
            .expect("decide");
    assert!(matches!(outcome, Repick::Empty));
    let outcome =
        decide_repick(&conn, profile_id, profiles::SELECT_FASTEST, false, None, &HashSet::new())
            .expect("decide");
    assert!(matches!(outcome, Repick::Empty));

    // round robin refuses when nothing is scan-reachable among the
    // survivors (it never dials an untested or known-dead endpoint) — the
    // selection stays untouched for the caller to report
    for raw in ["raw-slow", "raw-quick", "raw-junk"] {
        conn.execute(
            "UPDATE endpoints SET available = 0, last_tested_at = datetime('now')
             WHERE raw = ?2",
            params![profile_id, raw],
        )
        .expect("kill scan data");
    }
    conn.execute(
        "UPDATE profiles SET selected_endpoint_key = 'raw-junk' WHERE id = ?1",
        params![profile_id],
    )
    .expect("select junk");
    let outcome =
        decide_repick(&conn, profile_id, profiles::SELECT_ROUND_ROBIN, false, selected(junk_id), &loadable)
            .expect("decide");
    assert!(matches!(outcome, Repick::Refused));
    assert_eq!(stored_key(&conn), "raw-junk");

    // never-scanned survivors: round robin still dials the first one
    conn.execute(
        "UPDATE endpoints SET available = NULL, last_tested_at = NULL WHERE profile_id = ?1",
        params![profile_id],
    )
    .expect("wipe scan data");
    let outcome =
        decide_repick(&conn, profile_id, profiles::SELECT_ROUND_ROBIN, false, selected(junk_id), &loadable)
            .expect("decide");
    let Repick::Switched(pick) = outcome else {
        panic!("expected Switched, got a different verdict");
    };
    assert_eq!(pick.id, slow_id);
    assert_eq!(stored_key(&conn), "raw-slow");
}
