use rusqlite::Connection;

use super::settings::{
    DiscoverySettings, KEY_AUTO_UPDATE_MINUTES, KEY_LAST_RUN_AT, KEY_TEST_AFTER, load,
    next_due_seconds, save,
};
use crate::db::set_setting;

fn test_db() -> Connection {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "megathrone-discovery-settings-{}-{id}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
}

fn set_key(conn: &Connection, key: &str, value: &str) {
    set_setting(conn, key, value).unwrap();
}

#[test]
fn no_interval_means_no_schedule() {
    let conn = test_db();
    assert_eq!(next_due_seconds(&conn).unwrap(), None);
    // even a long-stale last-run stamp schedules nothing without an
    // interval
    set_key(&conn, KEY_LAST_RUN_AT, "2020-01-01 00:00:00");
    assert_eq!(next_due_seconds(&conn).unwrap(), None);
}

#[test]
fn a_missing_invalid_or_stale_last_run_is_due_now() {
    let conn = test_db();
    set_key(&conn, KEY_AUTO_UPDATE_MINUTES, "60");
    assert_eq!(next_due_seconds(&conn).unwrap(), Some(0.0));
    set_key(&conn, KEY_LAST_RUN_AT, "not a timestamp");
    assert_eq!(next_due_seconds(&conn).unwrap(), Some(0.0));
    set_key(&conn, KEY_LAST_RUN_AT, "2020-01-01 00:00:00");
    assert_eq!(next_due_seconds(&conn).unwrap(), Some(0.0));
}

#[test]
fn a_fresh_last_run_leaves_the_positive_remainder() {
    let conn = test_db();
    set_key(&conn, KEY_AUTO_UPDATE_MINUTES, "60");
    let ten_minutes_ago: String = conn
        .query_row("SELECT datetime('now', '-10 minutes')", [], |row| row.get(0))
        .unwrap();
    set_key(&conn, KEY_LAST_RUN_AT, &ten_minutes_ago);
    let due = next_due_seconds(&conn).unwrap().expect("scheduled");
    assert!(
        (due - 3000.0).abs() < 10.0,
        "a 60-minute interval with 10 minutes elapsed leaves ~3000s, got {due}"
    );
}

#[test]
fn settings_default_to_manual_runs_and_survive_junk() {
    let conn = test_db();
    let defaults = DiscoverySettings { auto_update_minutes: None, test_after: false };
    assert_eq!(load(&conn).unwrap(), defaults);
    // junk values fall back to the same defaults
    set_key(&conn, KEY_AUTO_UPDATE_MINUTES, "soon");
    set_key(&conn, KEY_TEST_AFTER, "yes");
    assert_eq!(load(&conn).unwrap(), defaults);
    // an out-of-window interval is junk too
    set_key(&conn, KEY_AUTO_UPDATE_MINUTES, "0");
    assert_eq!(load(&conn).unwrap().auto_update_minutes, None);
}

#[test]
fn settings_roundtrip_and_off_drops_the_interval_key() {
    let conn = test_db();
    save(&conn, &DiscoverySettings { auto_update_minutes: Some(30), test_after: true }).unwrap();
    assert_eq!(
        load(&conn).unwrap(),
        DiscoverySettings { auto_update_minutes: Some(30), test_after: true }
    );
    save(&conn, &DiscoverySettings { auto_update_minutes: None, test_after: true }).unwrap();
    assert_eq!(
        load(&conn).unwrap(),
        DiscoverySettings { auto_update_minutes: None, test_after: true }
    );
    let stored = crate::db::get_setting(&conn, KEY_AUTO_UPDATE_MINUTES).unwrap();
    assert_eq!(stored, None, "Off removes the interval key entirely");
}

#[test]
fn save_rejects_non_positive_intervals() {
    let conn = test_db();
    for minutes in [0, -1, -60] {
        assert!(
            save(
                &conn,
                &DiscoverySettings { auto_update_minutes: Some(minutes), test_after: false }
            )
            .is_err(),
            "interval {minutes} must be rejected"
        );
    }
    assert!(
        save(&conn, &DiscoverySettings { auto_update_minutes: None, test_after: false }).is_ok()
    );
}
