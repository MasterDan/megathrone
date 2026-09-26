use rusqlite::{params, Connection};

use super::endpoints::{item_detail, list_items};
use super::model::{SELECT_FASTEST, SELECT_MANUAL, SELECTION_MODES};
use super::selection::{selection_mode, set_selection_mode};
use super::storage::{
    bump_auto_update_failures, db_err, delete_profile, due_profiles, import_profile,
    list_profiles, next_wake_seconds, rename_profile, set_auto_update,
};
use super::update::apply_update;

pub(super) fn test_db() -> Connection {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("megathrone-test-{}-{id}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
}

pub(super) const SAMPLE: &str = "\
vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@1.2.3.4:443?security=tls&sni=example.com#First
ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#Second
ssr://not-supported
trojan://pw@9.9.9.9:443?security=tls#Third";

#[test]
fn import_list_rename_delete_flow() {
    let mut conn = test_db();

    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
    assert_eq!(summary.item_count, 3);
    assert_eq!(summary.skipped_count, 1);
    assert!(summary.last_fetched_at.is_some());

    let mut list = list_profiles(&conn).expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Sub");

    // pagination: first page of 2 keeps file order
    let page = list_items(&conn, summary.id, 0, 2).expect("items");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].tag, "First");
    assert_eq!(page[0].protocol, "vless");
    assert_eq!(page[1].tag, "Second");

    let rest = list_items(&conn, summary.id, 2, 2).expect("items");
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].tag, "Third");

    // detail roundtrip: stored JSON parses and keeps the type
    let detail = item_detail(&conn, page[0].id).expect("detail");
    let outbound: serde_json::Value = serde_json::from_str(&detail.outbound_json).expect("valid json");
    assert_eq!(outbound["type"], "vless");
    assert!(detail.raw.starts_with("vless://"));

    // rename
    rename_profile(&conn, summary.id, "Renamed").expect("rename");
    list = list_profiles(&conn).expect("list");
    assert_eq!(list[0].name, "Renamed");
    assert!(rename_profile(&conn, summary.id, "  ").is_err());

    // delete cascades
    delete_profile(&conn, summary.id).expect("delete");
    assert!(list_profiles(&conn).expect("list").is_empty());
    assert!(list_items(&conn, summary.id, 0, 10).expect("items").is_empty());
}

#[test]
fn list_profiles_orders_by_name() {
    let mut conn = test_db();

    import_profile(&mut conn, "Zulu".into(), None, None, SAMPLE).expect("import");
    import_profile(&mut conn, "alpha".into(), None, None, SAMPLE).expect("import");
    import_profile(&mut conn, "Beta".into(), None, None, SAMPLE).expect("import");

    let names: Vec<String> = list_profiles(&conn)
        .expect("list")
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, vec!["alpha", "Beta", "Zulu"]);
}

#[test]
fn import_rejects_garbage() {
    let mut conn = test_db();
    let error = import_profile(&mut conn, "Bad".into(), None, None, "hello world\nnothing useful")
        .expect_err("should reject");
    assert!(error.contains("no supported endpoints"));
    assert!(list_profiles(&conn).expect("list").is_empty());
}

#[test]
fn auto_update_due_and_next_wake() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Auto".into(), Some("https://example.com/sub".into()), None, SAMPLE)
        .expect("import");

    // no schedule yet
    assert!(due_profiles(&conn).expect("due").is_empty());
    assert!(next_wake_seconds(&conn).expect("next").is_none());

    // validation: bad interval rejected
    assert!(set_auto_update(&conn, summary.id, Some(45)).is_err());

    // enable auto-update, pretend the last fetch was an hour ago
    set_auto_update(&conn, summary.id, Some(30)).expect("enable");
    conn.execute(
        "UPDATE profiles SET last_fetched_at = datetime('now', '-1 hour') WHERE id = ?1",
        params![summary.id],
    )
    .map_err(db_err)
    .unwrap();

    assert_eq!(due_profiles(&conn).expect("due"), vec![summary.id]);

    // after a "fetch" it is no longer due and the next wake is ~30 minutes
    apply_update(&mut conn, summary.id, SAMPLE).expect("update");
    assert!(due_profiles(&conn).expect("due").is_empty());
    let next = next_wake_seconds(&conn).expect("next").expect("scheduled");
    assert!((1790.0..=1810.0).contains(&next), "next wake was {next}");

    // a fresh profile with no last_fetched_at never fires
    let other = import_profile(&mut conn, "NoFetch".into(), Some("https://example.com/2".into()), None, SAMPLE)
        .expect("import");
    conn.execute(
        "UPDATE profiles SET auto_update_minutes = 60, last_fetched_at = NULL WHERE id = ?1",
        params![other.id],
    )
    .map_err(db_err)
    .unwrap();
    assert!(!due_profiles(&conn).expect("due").contains(&other.id));

    // sourceless profiles cannot enable auto-update
    let text_only = import_profile(&mut conn, "Text".into(), None, None, SAMPLE).expect("import");
    assert!(set_auto_update(&conn, text_only.id, Some(60)).is_err());
}

#[test]
fn auto_update_pauses_after_consecutive_failures() {
    let mut conn = test_db();
    let summary = import_profile(
        &mut conn,
        "Auto".into(),
        Some("https://example.com/sub".into()),
        None,
        SAMPLE,
    )
    .expect("import");
    set_auto_update(&conn, summary.id, Some(30)).expect("enable");
    conn.execute(
        "UPDATE profiles SET last_fetched_at = datetime('now', '-1 hour') WHERE id = ?1",
        params![summary.id],
    )
    .map_err(db_err)
    .unwrap();

    // two failed scheduled updates: the profile stays scheduled
    assert_eq!(bump_auto_update_failures(&conn, summary.id).expect("bump"), 1);
    assert_eq!(bump_auto_update_failures(&conn, summary.id).expect("bump"), 2);
    assert_eq!(due_profiles(&conn).expect("due"), vec![summary.id]);
    assert!(next_wake_seconds(&conn).expect("next").is_some());

    // the third consecutive failure pauses the schedule: the scheduler
    // neither runs the profile nor holds its wake target overdue
    assert_eq!(bump_auto_update_failures(&conn, summary.id).expect("bump"), 3);
    assert!(due_profiles(&conn).expect("due").is_empty());
    assert!(next_wake_seconds(&conn).expect("next").is_none());

    // a failed bump of a missing profile is an error, not a pause
    assert!(bump_auto_update_failures(&conn, 999).is_err());

    // a successful update (the manual Update button runs the same
    // apply path) clears the streak and unpauses the schedule
    apply_update(&mut conn, summary.id, SAMPLE).expect("update");
    let failures: (i64,) = conn
        .query_row(
            "SELECT auto_update_failures FROM profiles WHERE id = ?1",
            params![summary.id],
            |row| Ok((row.get(0)?,)),
        )
        .unwrap();
    assert_eq!(failures.0, 0);
    conn.execute(
        "UPDATE profiles SET last_fetched_at = datetime('now', '-1 hour') WHERE id = ?1",
        params![summary.id],
    )
    .map_err(db_err)
    .unwrap();
    assert_eq!(due_profiles(&conn).expect("due"), vec![summary.id]);
}

#[test]
fn selection_mode_roundtrip_and_validation() {
    let mut conn = test_db();
    let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");

    // fresh profiles are automatic (the migration default)
    assert_eq!(selection_mode(&conn, summary.id).expect("mode"), SELECT_FASTEST);

    for mode in SELECTION_MODES {
        set_selection_mode(&conn, summary.id, mode).expect("set mode");
        assert_eq!(selection_mode(&conn, summary.id).expect("mode"), mode);
    }
    assert!(set_selection_mode(&conn, summary.id, "random").is_err());
    assert!(set_selection_mode(&conn, 999, SELECT_MANUAL).is_err());
    assert!(selection_mode(&conn, 999).is_err());
}
