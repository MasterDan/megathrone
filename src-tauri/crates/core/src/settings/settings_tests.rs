use rusqlite::Connection;

use super::general::{
    KEY_CLOSE_TO_TRAY, KEY_MIN_AVAILABILITY, KEY_RAW_PROXY_ENABLED, KEY_RAW_PROXY_PORT,
    KEY_RECHECK_MINUTES, KEY_ROTATION_MINUTES,
};
use super::*;
use crate::db::set_setting;

fn test_db() -> Connection {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "megathrone-settings-{}-{id}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
}

#[test]
fn validate_accepts_http_urls_and_normalizes_them() {
    assert_eq!(
        validate_test_url("  https://Example.com/a?b=1  ").expect("valid"),
        "https://example.com/a?b=1"
    );
    assert_eq!(validate_test_url("http://1.2.3.4:8080/").expect("valid"), "http://1.2.3.4:8080/");

    // bare hosts get https:// assumed, host:port is not mistaken for a scheme
    assert_eq!(validate_test_url("youtube.com").expect("valid"), "https://youtube.com/");
    assert_eq!(
        validate_test_url("example.com:8080/check").expect("valid"),
        "https://example.com:8080/check"
    );

    for bad in [
        "",
        "   ",
        "ftp://example.com",
        "http://",
        "not a url",
    ] {
        assert!(validate_test_url(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn general_settings_roundtrip_with_defaults() {
    let conn = test_db();

    // a fresh database serves the defaults
    assert_eq!(
        load_general(&conn).expect("load"),
        GeneralSettings {
            rotation_minutes: DEFAULT_ROTATION_MINUTES,
            recheck_minutes: DEFAULT_RECHECK_MINUTES,
            close_to_tray: DEFAULT_CLOSE_TO_TRAY,
            min_availability_percent: DEFAULT_MIN_AVAILABILITY_PERCENT,
            raw_proxy_enabled: DEFAULT_RAW_PROXY_ENABLED,
            raw_proxy_port: DEFAULT_RAW_PROXY_PORT,
        }
    );

    // the command layer validates before storing — mirror that here
    set_setting(&conn, KEY_ROTATION_MINUTES, "30").expect("set");
    set_setting(&conn, KEY_RECHECK_MINUTES, "2").expect("set");
    set_setting(&conn, KEY_CLOSE_TO_TRAY, "false").expect("set");
    set_setting(&conn, KEY_MIN_AVAILABILITY, "70").expect("set");
    set_setting(&conn, KEY_RAW_PROXY_ENABLED, "0").expect("set");
    set_setting(&conn, KEY_RAW_PROXY_PORT, "2080").expect("set");
    assert_eq!(
        load_general(&conn).expect("load"),
        GeneralSettings {
            rotation_minutes: 30,
            recheck_minutes: 2,
            close_to_tray: false,
            min_availability_percent: 70,
            raw_proxy_enabled: false,
            raw_proxy_port: 2080,
        }
    );
}

#[test]
fn junk_in_the_settings_table_falls_back_to_defaults() {
    let conn = test_db();
    set_setting(&conn, KEY_ROTATION_MINUTES, "later").expect("junk");
    set_setting(&conn, KEY_RECHECK_MINUTES, "0").expect("out of range");
    set_setting(&conn, KEY_CLOSE_TO_TRAY, "maybe").expect("junk bool");
    set_setting(&conn, KEY_MIN_AVAILABILITY, "soon").expect("junk");
    set_setting(&conn, KEY_RAW_PROXY_ENABLED, "perhaps").expect("junk bool");
    set_setting(&conn, KEY_RAW_PROXY_PORT, "not-a-port").expect("junk");
    assert_eq!(
        load_general(&conn).expect("load"),
        GeneralSettings {
            rotation_minutes: DEFAULT_ROTATION_MINUTES,
            recheck_minutes: DEFAULT_RECHECK_MINUTES,
            close_to_tray: DEFAULT_CLOSE_TO_TRAY,
            min_availability_percent: DEFAULT_MIN_AVAILABILITY_PERCENT,
            raw_proxy_enabled: DEFAULT_RAW_PROXY_ENABLED,
            raw_proxy_port: DEFAULT_RAW_PROXY_PORT,
        }
    );
}

#[test]
fn close_to_tray_accepts_the_common_spellings() {
    let conn = test_db();
    for (stored, expected) in [("true", true), ("1", true), ("false", false), ("0", false)] {
        set_setting(&conn, KEY_CLOSE_TO_TRAY, stored).expect("set");
        assert_eq!(
            load_general(&conn).expect("load").close_to_tray,
            expected,
            "{stored:?} must read as {expected}"
        );
    }
}
