use rusqlite::{params, Connection};

use super::endpoints::{
    endpoint_url_statuses, get_selected_endpoint, item_index, list_items, set_selected_endpoint,
};
use super::profiles_tests::{test_db, SAMPLE};
use super::selection::{
    load_candidates, load_reachable_candidates, pick_fastest, pick_most_available,
    pick_round_robin, selected_raw_key,
};
use super::storage::{db_err, import_profile};

#[test]
fn selected_endpoint_survives_updates() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
    let items = list_items(&conn, summary.id, 0, 10).expect("items");

    // pick and read back
    let changed = set_selected_endpoint(&conn, summary.id, Some(items[0].id)).expect("select");
    assert!(changed, "the first pick is a change");
    assert!(
        !set_selected_endpoint(&conn, summary.id, Some(items[0].id)).expect("re-pick"),
        "re-picking the same endpoint is not a change"
    );
    let selected = get_selected_endpoint(&conn, summary.id).expect("get selected");
    assert_eq!(selected.map(|item| item.tag), Some(items[0].tag.clone()));

    // a content update keeps surviving endpoints on their ids (same
    // raw links) — the selection resolves either way
    super::update::apply_update(&mut conn, summary.id, SAMPLE).expect("update");
    let still = get_selected_endpoint(&conn, summary.id)
        .expect("get selected")
        .expect("selection survives reimport");
    assert_eq!(still.tag, items[0].tag);
    let fresh = list_items(&conn, summary.id, 0, 10).expect("items");
    let fresh_id = fresh.iter().find(|item| item.tag == still.tag).map(|item| item.id);
    assert_eq!(fresh_id, Some(still.id), "resolved to the new row id");

    // content without the picked endpoint drops the selection
    super::update::apply_update(&mut conn, summary.id, "trojan://pw@1.1.1.1:443?security=tls#Only")
        .expect("update");
    assert!(get_selected_endpoint(&conn, summary.id).expect("get selected").is_none());

    // clearing works, foreign endpoints and unknown profiles are rejected
    set_selected_endpoint(&conn, summary.id, None).expect("clear");
    assert!(get_selected_endpoint(&conn, summary.id).expect("get selected").is_none());

    let other = import_profile(&mut conn, "Other".into(), None, None, SAMPLE).expect("import");
    let other_items = list_items(&conn, other.id, 0, 10).expect("items");
    let error = set_selected_endpoint(&conn, summary.id, Some(other_items[0].id))
        .expect_err("endpoint belongs to another profile");
    assert!(error.contains("not found in profile"));
    assert!(set_selected_endpoint(&conn, summary.id + 100, Some(1)).is_err());
}

/// A proxy-routed category with two test rules; returns their ids.
fn seed_proxy_urls(conn: &Connection) -> Vec<i64> {
    conn.execute(
        "INSERT INTO test_site_categories (name, position, action) VALUES ('Cat', 0, 'proxy')",
        [],
    )
    .unwrap();
    let category_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO test_sites (category_id, rule_type, value)
         VALUES (?1, 'url', 'https://a.example/'), (?1, 'url', 'https://b.example/')",
        params![category_id],
    )
    .unwrap();
    let mut stmt = conn
        .prepare("SELECT id FROM test_sites WHERE category_id = ?1 ORDER BY id")
        .unwrap();
    stmt.query_map(params![category_id], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn items_carry_url_scores_and_endpoint_urls_join() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");

    // nothing probed: every item reports a zero score
    let items = list_items(&conn, summary.id, 0, 10).expect("items");
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|item| item.url_ok == 0 && item.url_total == 0));

    let url_ids = seed_proxy_urls(&conn);

    // first endpoint passes both URLs, second passes one, third is unprobed
    conn.execute(
        "INSERT INTO endpoint_url_results (endpoint_id, url_id, available, latency_ms) VALUES
         (?1, ?2, 1, 30), (?1, ?3, 1, 60), (?4, ?2, 1, 45), (?4, ?3, 0, NULL)",
        params![items[0].id, url_ids[0], url_ids[1], items[1].id],
    )
    .unwrap();

    let scored = list_items(&conn, summary.id, 0, 10).expect("items");
    assert_eq!((scored[0].url_ok, scored[0].url_total), (2, 2));
    assert_eq!((scored[1].url_ok, scored[1].url_total), (1, 2));
    assert_eq!((scored[2].url_ok, scored[2].url_total), (0, 0), "never probed, no score");

    // the selected-endpoint query carries the same score columns
    set_selected_endpoint(&conn, summary.id, Some(items[1].id)).expect("select");
    let selected = get_selected_endpoint(&conn, summary.id)
        .expect("get selected")
        .expect("selected");
    assert_eq!((selected.url_ok, selected.url_total), (1, 2));

    // lazy details: only the probed URLs are listed, under their category
    assert!(endpoint_url_statuses(&conn, items[2].id).expect("statuses").is_empty());
    let first = endpoint_url_statuses(&conn, items[0].id).expect("statuses");
    assert_eq!(first.len(), 2);
    assert!(first.iter().all(|status| status.category == "Cat"));
    assert_eq!(first[0].available, Some(true));
    assert_eq!(first[0].latency_ms, Some(30));
    assert_eq!(first[1].available, Some(true));

    // a dpi-routed category's URL neither scores nor shows up here
    conn.execute(
        "INSERT INTO test_site_categories (name, position, action) VALUES ('DpiCat', 1, 'dpi')",
        [],
    )
    .unwrap();
    let dpi_category = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO test_sites (category_id, rule_type, value) VALUES (?1, 'url', 'https://dpi.example/')",
        params![dpi_category],
    )
    .unwrap();
    let dpi_url = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO endpoint_url_results (endpoint_id, url_id, available) VALUES (?1, ?2, 1)",
        params![items[0].id, dpi_url],
    )
    .unwrap();
    let rescored = list_items(&conn, summary.id, 0, 10).expect("items");
    assert_eq!((rescored[0].url_ok, rescored[0].url_total), (2, 2), "dpi URLs do not count");
    assert_eq!(endpoint_url_statuses(&conn, items[0].id).expect("statuses").len(), 2);
}

#[test]
fn items_are_sorted_by_outcome_speed_and_url_share() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
    let mut items = list_items(&conn, summary.id, 0, 10).expect("items");
    // profile order before any scan: First, Second, Third
    assert_eq!(
        items.iter().map(|item| item.tag.as_str()).collect::<Vec<_>>(),
        vec!["First", "Second", "Third"]
    );

    // a dead fourth endpoint joins the party
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available, latency_ms)
         VALUES (?1, 'Dead', 'trojan', 'raw', '{}', 3, 0, NULL)",
        params![summary.id],
    )
    .map_err(db_err)
    .unwrap();
    let url_ids = seed_proxy_urls(&conn);

    // Second: fastest (50ms) but no URL passes; First: 100ms, 1/2 URLs;
    // a 100ms newcomer with 2/2 URLs must outrank First (speed tie → share desc);
    // Third stays untested; Dead sinks to the bottom
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available, latency_ms)
         VALUES (?1, 'Full', 'vless', 'raw', '{}', 4, 1, 100)",
        params![summary.id],
    )
    .map_err(db_err)
    .unwrap();
    let full_id = conn.last_insert_rowid();
    conn.execute(
        "UPDATE endpoints SET available = 1, latency_ms = 100 WHERE tag = 'First'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE endpoints SET available = 1, latency_ms = 50 WHERE tag = 'Second'",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO endpoint_url_results (endpoint_id, url_id, available)
         VALUES ((SELECT id FROM endpoints WHERE tag = 'First'), ?1, 1),
                ((SELECT id FROM endpoints WHERE tag = 'First'), ?2, 0),
                (?3, ?1, 1),
                (?3, ?2, 1)",
        params![url_ids[0], url_ids[1], full_id],
    )
    .unwrap();

    items = list_items(&conn, summary.id, 0, 10).expect("items");
    assert_eq!(
        items.iter().map(|item| item.tag.as_str()).collect::<Vec<_>>(),
        vec!["Second", "Full", "First", "Third", "Dead"],
        "reachable by speed, then URL share desc; untested middle; dead last"
    );

    // pagination follows the same global order (page 1 of 2)
    let page = list_items(&conn, summary.id, 0, 2).expect("page");
    assert_eq!(
        page.iter().map(|item| item.tag.as_str()).collect::<Vec<_>>(),
        vec!["Second", "Full"]
    );
}

#[test]
fn load_candidates_keeps_list_order_and_scores() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
    let url_ids = seed_proxy_urls(&conn);
    let items = list_items(&conn, summary.id, 0, 10).expect("items");

    // deep-probe the first endpoint 1/2
    conn.execute(
        "INSERT INTO endpoint_url_results (endpoint_id, url_id, available)
         VALUES (?1, ?2, 1), (?1, ?3, 0)",
        params![items[0].id, url_ids[0], url_ids[1]],
    )
    .unwrap();

    let candidates = load_candidates(&conn, summary.id).expect("candidates");
    assert_eq!(
        candidates.iter().map(|c| c.tag.as_str()).collect::<Vec<_>>(),
        vec!["First", "Second", "Third"],
        "list order, not the availability sort"
    );
    assert_eq!((candidates[0].url_ok, candidates[0].url_total), (1, 2));
    assert_eq!(candidates[0].tag, items[0].tag);
    assert!(selected_raw_key(&conn, summary.id).expect("raw key").is_none());

    // broken outbounds never become candidates
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
         VALUES (?1, 'Broken', 'vless', 'raw-broken', '{}', 9)",
        params![summary.id],
    )
    .unwrap();
    let candidates = load_candidates(&conn, summary.id).expect("candidates");
    assert!(!candidates.iter().any(|c| c.tag == "Broken"));
    assert!(load_candidates(&conn, 999).is_err(), "unknown profile");
}

#[test]
fn load_reachable_candidates_keeps_only_proven_endpoints() {
    let conn = test_db();
    conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 3)", [])
        .expect("seed profile");
    let profile_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available)
         VALUES (?1, 'Live',  'vless',  'raw-live',  ?2, 0, 1),
                (?1, 'Dead',  'vless',  'raw-dead',  ?2, 1, 0),
                (?1, 'Fresh', 'vless',  'raw-fresh', ?2, 2, NULL),
                (?1, 'Broken','vless',  'raw-broke', '{}', 3, 1)",
        params![
            profile_id,
            r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#
        ],
    )
    .unwrap();

    let candidates = load_reachable_candidates(&conn, profile_id).expect("candidates");
    assert_eq!(
        candidates.iter().map(|c| c.tag.as_str()).collect::<Vec<_>>(),
        vec!["Live"],
        "only reachable rows with a usable outbound, in list order"
    );
    assert_eq!(candidates[0].available, Some(true));
}

#[test]
fn availability_floor_marks_low_share_endpoints_dead() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
    let url_ids = seed_proxy_urls(&conn);
    let items = list_items(&conn, summary.id, 0, 10).expect("items");

    // all reachable; First passes 1/2 (50%) and is the fastest,
    // Second passes 2/2, Third was never deep-probed
    conn.execute(
        "UPDATE endpoints SET available = 1, latency_ms = 50 WHERE tag = 'First'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE endpoints SET available = 1, latency_ms = 120 WHERE tag = 'Second'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE endpoints SET available = 1, latency_ms = 200 WHERE tag = 'Third'",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO endpoint_url_results (endpoint_id, url_id, available) VALUES
         (?1, ?2, 1), (?1, ?3, 0), (?4, ?2, 1), (?4, ?3, 1)",
        params![items[0].id, url_ids[0], url_ids[1], items[1].id],
    )
    .unwrap();

    // the default floor is 55: First reads as dead to the strategies,
    // its raw score columns are still reported
    let candidates = load_candidates(&conn, summary.id).expect("candidates");
    let first = candidates.iter().find(|c| c.tag == "First").unwrap();
    assert_eq!(first.available, Some(false), "below the floor reads as dead");
    assert_eq!((first.url_ok, first.url_total), (1, 2), "the score itself is kept");
    assert_eq!(
        candidates
            .iter()
            .find(|c| c.tag == "Second")
            .unwrap()
            .available,
        Some(true)
    );
    assert_eq!(
        candidates
            .iter()
            .find(|c| c.tag == "Third")
            .unwrap()
            .available,
        Some(true),
        "an unprobed reachable endpoint stays alive"
    );

    // every strategy avoids the below-floor endpoint
    assert_eq!(pick_fastest(&candidates).expect("pick").tag, "Second");
    assert_eq!(pick_most_available(&candidates).expect("pick").tag, "Second");
    // round robin wrapping past First lands on Second
    let third_raw = candidates
        .iter()
        .find(|c| c.tag == "Third")
        .unwrap()
        .raw
        .clone();
    assert_eq!(
        pick_round_robin(&candidates, Some(&third_raw)).expect("pick").tag,
        "Second"
    );

    // the rescue list drops it outright
    let reachable = load_reachable_candidates(&conn, summary.id).expect("reachable");
    assert_eq!(
        reachable.iter().map(|c| c.tag.as_str()).collect::<Vec<_>>(),
        vec!["Second", "Third"]
    );

    // floor 0 (off) restores First as an ordinary reachable endpoint
    crate::db::set_setting(&conn, "min_endpoint_availability_percent", "0").unwrap();
    let candidates = load_candidates(&conn, summary.id).expect("candidates");
    assert_eq!(
        candidates
            .iter()
            .find(|c| c.tag == "First")
            .unwrap()
            .available,
        Some(true)
    );
    assert_eq!(pick_fastest(&candidates).expect("pick").tag, "First");
}

#[test]
fn item_index_matches_list_order() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
    let items = list_items(&conn, summary.id, 0, 10).expect("items");

    for (expected, item) in items.iter().enumerate() {
        let index = item_index(&conn, summary.id, item.id).expect("index");
        assert_eq!(index, Some(expected as i64));
    }
    assert_eq!(
        item_index(&conn, summary.id, 999),
        Ok(None),
        "unknown endpoint"
    );

    let other = import_profile(
        &mut conn,
        "Other".into(),
        None,
        None,
        "vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@1.2.3.4:443?security=tls&sni=example.com#Only",
    )
    .expect("import");
    assert_eq!(
        item_index(&conn, other.id, items[0].id),
        Ok(None),
        "endpoint of another profile"
    );
}
