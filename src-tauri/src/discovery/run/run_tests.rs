// The discovery run's tests: group assembly, the rebuild round-trips, and
// the store path's diff semantics — a Discovery update must merge like a
// regular profile update (survivors keep their id and every test result,
// removals cascade, additions land fresh).

use super::*;

fn test_db() -> Connection {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "megathrone-discovery-run-{}-{id}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
}

fn job(id: i64, position: i64, merge_group: Option<&str>) -> SourceJob {
    SourceJob {
        id,
        url: format!("https://example.com/{id}"),
        name: format!("Omega-{}", position + 1),
        position,
        merge_group: merge_group.map(str::to_string),
        profile_id: None,
    }
}

fn parse(content: &str) -> ParsedSource {
    parse_source(content)
}

const LINK_TEMPLATE: &str = "vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@{host}:443?security=tls&sni=example.com#{tag}";

fn link(host: &str, tag: &str) -> String {
    LINK_TEMPLATE.replace("{host}", host).replace("{tag}", tag)
}

fn endpoint_ids(conn: &Connection, profile_id: i64) -> Vec<i64> {
    let mut stmt = conn
        .prepare("SELECT id FROM endpoints WHERE profile_id = ?1 ORDER BY order_index")
        .expect("prepare");
    stmt.query_map(params![profile_id], |row| row.get(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect")
}

#[test]
fn assemble_group_sorts_by_position_and_re_dedups_across_members() {
    let later = job(2, 1, Some("whitelists"));
    let owner = job(1, 0, Some("whitelists"));
    // the later member reuses the owner's host:port — its duplicate drops
    let batch = vec![
        (
            later,
            parse(&format!("{}\n{}", link("1.2.3.4", "dup"), link("9.9.9.9", "keep"))),
        ),
        (owner, parse(&link("1.2.3.4", "owner"))),
    ];

    let (members, endpoints, native_config) = assemble_group(batch).expect("assembled");

    assert_eq!(members[0].0.id, 1, "the lowest position owns the group");
    assert_eq!(members[0].1, 1, "the owner's own link count");
    assert_eq!(members[1].1, 2, "member counts stay per-URL, pre-cross-dedup");
    assert!(!native_config);
    let raws: Vec<&str> = endpoints.iter().map(|e| e.raw.as_str()).collect();
    assert_eq!(raws.len(), 2, "the cross-member host:port duplicate drops");
    assert!(raws.contains(&link("1.2.3.4", "owner").as_str()));
    assert!(raws.contains(&link("9.9.9.9", "keep").as_str()));
}

#[test]
fn assemble_group_skips_failed_members_and_fails_on_all_failed() {
    let ok = job(1, 0, Some("whitelists"));
    let failed = job(2, 1, Some("whitelists"));
    let (members, endpoints, _) = assemble_group(vec![
        (failed, Err("request failed".to_string())),
        (ok, parse(&link("9.9.9.9", "keep"))),
    ])
    .expect("survivors assemble");
    assert_eq!(members.len(), 1);
    assert_eq!(endpoints.len(), 1);

    let all_failed = assemble_group(vec![
        (job(1, 0, Some("whitelists")), Err("request failed".to_string())),
        (job(2, 1, Some("whitelists")), Err("timeout".to_string())),
    ]);
    assert!(all_failed.is_none());
}

#[test]
fn assemble_group_rebuilds_as_native_when_any_member_needs_it() {
    let native = r#"{"outbounds":[
            {"type":"socks","tag":"a","server":"a.example","server_port":1080}
        ]}"#;
    let batch = vec![
        (job(1, 0, Some("whitelists")), parse(native)),
        (job(2, 1, Some("whitelists")), parse(&link("9.9.9.9", "link"))),
    ];
    let (_, _, native_config) = assemble_group(batch).expect("assembled");
    assert!(native_config, "a mixed group needs the lossless rebuild");
}

#[test]
fn parse_source_rejects_payloads_without_endpoints() {
    assert!(parse_source("not a subscription at all").is_err());
}

#[test]
fn storage_units_count_each_merge_group_once() {
    let mut sources: Vec<SourceJob> = (0..25).map(|i| job(i, i, None)).collect();
    sources.extend((0..7).map(|i| job(100 + i, 25 + i, Some("whitelists"))));
    sources.push(job(200, 32, Some("other")));
    // 25 singles + the seeded group + one foreign group
    assert_eq!(storage_units(&sources), 27);
}

#[test]
fn link_lists_round_trip_through_rebuilt_content() {
    let content = "vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@104.18.47.113:80?security=none&type=ws&path=%2F%3Fed%3D2560&host=example.dev#One\n\
                   vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@104.18.47.114:443?security=tls#Two";
    let parsed = parser::parse_subscription(content);
    assert_eq!(parsed.endpoints.len(), 2);
    let deduped = dedup_endpoints(parsed.endpoints);
    let rebuilt = rebuild_content(&deduped, false);
    let reparsed = parser::parse_subscription(&rebuilt);
    let raws: Vec<&str> = reparsed.endpoints.iter().map(|e| e.raw.as_str()).collect();
    let expected: Vec<&str> = deduped.iter().map(|e| e.raw.as_str()).collect();
    assert_eq!(raws, expected);
}

#[test]
fn native_configs_round_trip_as_an_outbound_array() {
    let config = r#"{"outbounds":[
            {"type":"socks","tag":"a","server":"a.example","server_port":1080},
            {"type":"socks","tag":"b","server":"b.example","server_port":1081}
        ]}"#;
    let parsed = parser::parse_subscription(config);
    assert_eq!(parsed.endpoints.len(), 2);
    let rebuilt = rebuild_content(&parsed.endpoints, true);
    let reparsed = parser::parse_subscription(&rebuilt);
    assert_eq!(reparsed.endpoints.len(), 2);
    let raws: Vec<&str> = reparsed.endpoints.iter().map(|e| e.raw.as_str()).collect();
    let expected: Vec<&str> = parsed.endpoints.iter().map(|e| e.raw.as_str()).collect();
    assert_eq!(raws, expected);
    assert_eq!(reparsed.endpoints[0].server, "a.example");
    assert_eq!(reparsed.endpoints[0].server_port, 1080);
}

#[test]
fn discovery_group_update_diffs_like_a_regular_update() {
    let mut conn = test_db();

    // the first run: a two-member merge group stores ONE shared profile
    let (members, endpoints, native_config) = assemble_group(vec![
        (job(1, 0, Some("whitelists")), parse(&link("1.2.3.4", "A"))),
        (job(2, 1, Some("whitelists")), parse(&link("5.6.7.8", "B"))),
    ])
    .expect("assemble first run");
    assert_eq!(members.len(), 2);
    let content = rebuild_content(&endpoints, native_config);
    let summary =
        crate::profiles::import_discovery_profile(&mut conn, "Omega-1".into(), &content)
            .expect("import");
    let ids = endpoint_ids(&conn, summary.id);
    assert_eq!(ids.len(), 2);
    let (a_id, b_id) = (ids[0], ids[1]);

    // a scan's verdict on A plus deep-probe rows on both endpoints
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
    conn.execute(
        "UPDATE endpoints SET available = 1, latency_ms = 123,
            last_tested_at = datetime('now') WHERE id = ?1",
        params![a_id],
    )
    .expect("stamp results");
    for (endpoint_id, latency) in [(a_id, 50), (b_id, 60)] {
        conn.execute(
            "INSERT INTO endpoint_url_results (endpoint_id, url_id, available, latency_ms)
             VALUES (?1, ?2, 1, ?3)",
            params![endpoint_id, url_id, latency],
        )
        .expect("seed url result");
    }

    // the next run: member 1 serves A's link plus a fresh C, member 2
    // failed — the group's rebuilt payload must diff, not wipe
    let (_, endpoints, native_config) = assemble_group(vec![
        (
            job(1, 0, Some("whitelists")),
            parse(&format!("{}\n{}", link("1.2.3.4", "A"), link("9.9.9.9", "C"))),
        ),
        (job(2, 1, Some("whitelists")), Err("request failed".to_string())),
    ])
    .expect("assemble second run");
    let fresh = rebuild_content(&endpoints, native_config);
    let outcome =
        crate::profiles::apply_update(&mut conn, summary.id, &fresh).expect("diff update");
    assert!(outcome.set_changed);
    assert_eq!((outcome.added, outcome.removed), (1, 1));
    assert_eq!(outcome.summary.item_count, 2);

    // the survivor keeps its row id and every result verbatim
    let (available, latency_ms, tested): (i64, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT available, latency_ms, last_tested_at FROM endpoints WHERE id = ?1",
            params![a_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read survivor");
    assert_eq!(available, 1);
    assert_eq!(latency_ms, Some(123));
    assert!(tested.is_some(), "the scan stamp survives the update");
    let a_url_ok: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM endpoint_url_results
             WHERE endpoint_id = ?1 AND available = 1",
            params![a_id],
            |row| row.get(0),
        )
        .expect("count survivor url results");
    assert_eq!(a_url_ok, 1, "deep-probe rows survive on the stable id");

    // the removed endpoint and its url results are gone (cascade)
    let b_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM endpoints WHERE id = ?1",
            params![b_id],
            |row| row.get(0),
        )
        .expect("count removed");
    assert_eq!(b_rows, 0);
    let b_results: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM endpoint_url_results WHERE endpoint_id = ?1",
            params![b_id],
            |row| row.get(0),
        )
        .expect("count cascaded results");
    assert_eq!(b_results, 0, "the removed row's url results cascade away");

    // the new link is inserted fresh, without any results
    let (available, latency_ms, tested, url_results): (
        Option<i64>,
        Option<i64>,
        Option<String>,
        i64,
    ) = conn
        .query_row(
            "SELECT available, latency_ms, last_tested_at,
                    (SELECT COUNT(*) FROM endpoint_url_results r
                     WHERE r.endpoint_id = endpoints.id)
             FROM endpoints WHERE profile_id = ?1 AND server = '9.9.9.9'",
            params![summary.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read fresh endpoint");
    assert_eq!((available, latency_ms, tested), (None, None, None));
    assert_eq!(url_results, 0);
}

#[test]
fn native_group_rebuild_diffs_cleanly() {
    let mut conn = test_db();
    let config = r#"{"outbounds":[
        {"type":"socks","tag":"a","server":"a.example","server_port":1080},
        {"type":"socks","tag":"b","server":"b.example","server_port":1081}
    ]}"#;
    let summary =
        crate::profiles::import_discovery_profile(&mut conn, "Omega-2".into(), config)
            .expect("import");
    let a_id = endpoint_ids(&conn, summary.id)[0];
    conn.execute(
        "UPDATE endpoints SET available = 1, latency_ms = 88,
            last_tested_at = datetime('now') WHERE id = ?1",
        params![a_id],
    )
    .expect("stamp results");

    // the next run's group: the same outbound `a` plus one new — the
    // native rebuild's per-outbound JSON raw must match the stored one
    let fresh_config = r#"{"outbounds":[
        {"type":"socks","tag":"a","server":"a.example","server_port":1080},
        {"type":"socks","tag":"c","server":"c.example","server_port":1082}
    ]}"#;
    let (endpoints, _) = parse(fresh_config).expect("parse");
    let rebuilt = rebuild_content(&endpoints, true);
    let outcome =
        crate::profiles::apply_update(&mut conn, summary.id, &rebuilt).expect("diff update");
    assert!(outcome.set_changed);
    assert_eq!((outcome.added, outcome.removed), (1, 1));

    let (server, available, latency_ms): (String, i64, Option<i64>) = conn
        .query_row(
            "SELECT server, available, latency_ms FROM endpoints WHERE id = ?1",
            params![a_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read survivor");
    assert_eq!(server, "a.example");
    assert_eq!(available, 1);
    assert_eq!(latency_ms, Some(88), "the survivor keeps its scan verdict");

    let b_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM endpoints WHERE profile_id = ?1 AND server = 'b.example'",
            params![summary.id],
            |row| row.get(0),
        )
        .expect("count removed");
    assert_eq!(b_rows, 0);
    assert_eq!(endpoint_ids(&conn, summary.id).len(), 2);
}
