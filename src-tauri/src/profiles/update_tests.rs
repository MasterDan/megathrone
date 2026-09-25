use rusqlite::params;

use super::endpoints::list_items;
use super::profiles_tests::{test_db, SAMPLE};
use super::sources::read_file;
use super::storage::{db_err, import_profile};
use super::update::{apply_update, load_source, Source};

#[test]
fn update_replaces_endpoints_from_file_source() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "File sub".into(), None, None, SAMPLE).expect("import");

    // point the profile at a temp file with different content
    let file = std::env::temp_dir().join(format!("megathrone-sub-{}.txt", std::process::id()));
    let fresh = "trojan://pw@1.1.1.1:443?security=tls#Only\nvmess://garbage";
    std::fs::write(&file, fresh).expect("write temp sub");

    conn.execute(
        "UPDATE profiles SET source_path = ?2 WHERE id = ?1",
        params![summary.id, file.to_str().unwrap()],
    )
    .map_err(db_err)
    .unwrap();

    let source = load_source(&conn, summary.id).expect("source");
    assert!(matches!(source, Source::Path(_)));
    let content = match &source {
        Source::Url(_) => unreachable!(),
        Source::Path(path) => read_file(path).expect("read"),
    };

    let updated = apply_update(&mut conn, summary.id, &content).expect("update");
    assert_eq!(updated.summary.item_count, 1);
    assert_eq!(updated.summary.skipped_count, 1);
    assert!(updated.set_changed, "a wholly different content changes the set");

    // old endpoints are gone, ids are fresh
    let items = list_items(&conn, summary.id, 0, 10).expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].tag, "Only");
    assert_ne!(items[0].id, 0);

    // last_fetched_at advanced
    let fetched: (String,) = conn
        .query_row(
            "SELECT last_fetched_at FROM profiles WHERE id = ?1",
            params![summary.id],
            |row| Ok((row.get(0)?,)),
        )
        .map_err(db_err)
        .unwrap();
    assert!(!fetched.0.is_empty());
}

#[test]
fn update_diff_preserves_surviving_results() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
    let items = list_items(&conn, summary.id, 0, 10).expect("items");
    let (first_id, first_tag) = (items[0].id, items[0].tag.clone());
    let second_id = items[1].id;

    // a test-site rule so deep-probe results can exist
    conn.execute(
        "INSERT INTO test_site_categories (name, position, action) VALUES ('Cat', 0, 'proxy')",
        [],
    )
    .expect("seed category");
    let category_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO test_sites (category_id, rule_type, value)
         VALUES (?1, 'url', 'https://a.example/')",
        params![category_id],
    )
    .expect("seed url");
    let url_id = conn.last_insert_rowid();

    // the "diamond": a latency scan's verdict on the first endpoint…
    conn.execute(
        "UPDATE endpoints SET available = 1, latency_ms = 123,
            last_tested_at = datetime('now') WHERE id = ?1",
        params![first_id],
    )
    .expect("stamp results");
    // …its deep-probe rows, and one stale row on the endpoint that is
    // about to be removed (must cascade away with it)
    conn.execute(
        "INSERT INTO endpoint_url_results (endpoint_id, url_id, available, latency_ms)
         VALUES (?1, ?2, 1, 50)",
        params![first_id, url_id],
    )
    .expect("seed url result");
    conn.execute(
        "INSERT INTO endpoint_url_results (endpoint_id, url_id, available, latency_ms)
         VALUES (?1, ?2, 1, 60)",
        params![second_id, url_id],
    )
    .expect("seed doomed url result");

    // fresh content: First survives (same raw), Second is gone, a new
    // link appears; the survivor keeps its id and every result
    let fresh = format!(
        "vless://new-uuid@9.9.9.9:443?security=tls#Fresh\n{}",
        SAMPLE.lines().next().unwrap_or_default()
    );
    let outcome = apply_update(&mut conn, summary.id, &fresh).expect("diff update");
    assert!(outcome.set_changed);
    assert_eq!(outcome.summary.item_count, 2);

    let rows = list_items(&conn, summary.id, 0, 10).expect("items");
    assert_eq!(rows.len(), 2);
    // the canonical grid order is availability-first, so check the
    // content order by position, not by grid row
    let survivor = rows.iter().find(|item| item.tag == first_tag).expect("survivor");
    assert_eq!(survivor.id, first_id, "surviving rows keep their id");
    let (fresh_order, survivor_order): (i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT order_index FROM endpoints WHERE tag = 'Fresh'
                  AND profile_id = ?1),
                (SELECT order_index FROM endpoints WHERE id = ?2)",
            params![summary.id, first_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read order");
    assert_eq!((fresh_order, survivor_order), (0, 1), "order follows fresh content");

    let (available, latency_ms, tested, url_ok): (i64, Option<i64>, Option<String>, i64) = conn
        .query_row(
            "SELECT available, latency_ms, last_tested_at,
                    (SELECT COUNT(*) FROM endpoint_url_results r
                     WHERE r.endpoint_id = endpoints.id AND r.available = 1)
             FROM endpoints WHERE id = ?1",
            params![first_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read survivor");
    assert_eq!(available, 1);
    assert_eq!(latency_ms, Some(123));
    assert!(tested.is_some(), "the scan stamp survives the update");
    assert_eq!(url_ok, 1, "deep-probe rows survive on the stable id");

    // the removed endpoint and its url results are gone
    let gone: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM endpoints WHERE id = ?1",
            params![second_id],
            |row| row.get(0),
        )
        .expect("count removed");
    assert_eq!(gone, 0);
    let stale_results: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM endpoint_url_results WHERE endpoint_id = ?1",
            params![second_id],
            |row| row.get(0),
        )
        .expect("count cascaded results");
    assert_eq!(stale_results, 0, "removed rows' url results cascade away");
}

#[test]
fn update_reorder_only_keeps_ids_without_set_change() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
    let before = list_items(&conn, summary.id, 0, 10).expect("items");

    // same three links, reverse order — a pure reorder changes nothing
    // the live session's baked config depends on
    let reordered = SAMPLE.lines().rev().collect::<Vec<_>>().join("\n");
    let outcome = apply_update(&mut conn, summary.id, &reordered).expect("reorder update");
    assert!(!outcome.set_changed, "a reorder is not a set change");

    let rows: Vec<(i64, String, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, tag, order_index FROM endpoints
                 WHERE profile_id = ?1 ORDER BY order_index",
            )
            .expect("prepare");
        let rows = stmt
            .query_map(params![summary.id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        rows
    };
    assert_eq!(rows.len(), before.len());
    for (index, (id, tag, order_index)) in rows.iter().enumerate() {
        let original = before.iter().find(|row| &row.tag == tag).expect("same tags");
        assert_eq!(&original.id, id, "ids stay stable on a reorder");
        assert_eq!(index, *order_index as usize, "order follows fresh content");
    }
}

#[test]
fn update_matches_duplicate_links_as_a_multiset() {
    let mut conn = test_db();
    // two identical links parse to two endpoints sharing one raw value
    let link = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#Twin";
    let twin = format!("{link}\n{link}");
    let summary = import_profile(&mut conn, "Twins".into(), None, None, &twin).expect("import");
    let twins = list_items(&conn, summary.id, 0, 10).expect("items");
    assert_eq!(twins.len(), 2, "duplicated links are both stored");
    let raws: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT raw FROM endpoints WHERE profile_id = ?1")
            .expect("prepare");
        let raws = stmt
            .query_map(params![summary.id], |row| row.get::<_, String>(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        raws
    };
    assert_eq!(raws[0], raws[1]);

    // a fresh copy of the same two links matches both stored rows —
    // nothing is inserted or removed
    let outcome = apply_update(&mut conn, summary.id, &twin).expect("twin update");
    assert!(!outcome.set_changed);

    // while a single copy matches only one of them
    let single = link.to_string();
    let outcome = apply_update(&mut conn, summary.id, &single).expect("shrink update");
    assert!(outcome.set_changed);
    assert_eq!(list_items(&conn, summary.id, 0, 10).expect("items").len(), 1);
}

#[test]
fn load_source_requires_a_source() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Text only".into(), None, None, SAMPLE).expect("import");
    let error = load_source(&conn, summary.id).expect_err("no source");
    assert!(error.contains("no source"));
}
