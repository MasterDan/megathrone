//! Tests of the endpoint-selection storage layer: the stored checkmark
//! resolution, the dead-selection check, the strategy auto-pick and the
//! live-switch target resolver.

use rusqlite::{Connection, params};

use crate::profiles;

use super::selection::{
    auto_pick_endpoint, load_selected_endpoint, selected_endpoint_brief, selection_known_dead,
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
