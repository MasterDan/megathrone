use rusqlite::{Connection, params};

use crate::db::set_setting;

use super::presets::{DESPAIR_NAME, PRESET_STRATEGIES};
use super::settings::{DEFAULT_DPI_PORT, SETTING_DPI_ENABLED};
use super::strategies::{
    MAX_STRATEGIES, add_strategy, delete_strategy, select_strategy, update_strategy,
};
use super::validation::{MAX_NAME_CHARS, MAX_STRATEGY_TOKENS};
use super::*;

fn test_db() -> Connection {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "megathrone-dpi-{}-{id}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
}

#[test]
fn strategy_args_allow_real_flags_and_reject_managed_ones() {
    let ok = parse_strategy_args("  -s2  -d2 ").expect("valid");
    assert_eq!(ok, vec!["-s2".to_string(), "-d2".to_string()]);

    assert_eq!(parse_strategy_args("").expect("empty is fine"), Vec::<String>::new());
    assert_eq!(
        parse_strategy_args("--fake -1 --ttl 8").expect("long flags"),
        vec!["--fake", "-1", "--ttl", "8"]
    );
    // case-sensitive getopt: -d is disorder, -D would be the daemon flag
    assert!(parse_strategy_args("-d3 --disorder 3+s").is_ok());
    assert!(parse_strategy_args("--fake-data=:GET / HTTP/1.1").is_ok());

    for bad in [
        "-p 2080",
        "-p2080",
        "--port 2080",
        "--port=2080",
        "-i 127.0.0.1",
        "--ip=0.0.0.0",
        "-D",
        "-w /tmp/pid",
        "--pidfile=/tmp/pid",
        "--daemon",
        "-E",
        "--transparent",
        "--",
        "-",
    ] {
        assert!(parse_strategy_args(bad).is_err(), "{bad:?} must be rejected");
    }

    let too_long = "-s 1 ".repeat(MAX_STRATEGY_TOKENS + 1);
    assert!(parse_strategy_args(&too_long).is_err());
}

#[test]
fn names_are_trimmed_and_bounded() {
    assert_eq!(validate_name("  Fake 1 ").expect("valid"), "Fake 1");
    assert!(validate_name("   ").is_err());
    assert!(validate_name(&"x".repeat(MAX_NAME_CHARS + 1)).is_err());
}

#[test]
fn strategies_crud_and_single_selection() {
    let conn = test_db();

    let first = add_strategy(&conn, "Split", &parse_strategy_args("-s2 -d2").unwrap())
        .expect("add");
    let second =
        add_strategy(&conn, "Fake", &parse_strategy_args("--fake -1 --ttl 8").unwrap())
            .expect("add");
    assert!(!first.is_active && !second.is_active);
    assert!(load_active_strategy(&conn).expect("active").is_none());

    select_strategy(&conn, second.id).expect("select");
    let active = load_active_strategy(&conn).expect("active").expect("some");
    assert_eq!(active.id, second.id);
    // selecting the same one again keeps it the only active strategy
    select_strategy(&conn, second.id).expect("re-select");
    let list = list_strategies(&conn).expect("list");
    assert_eq!(list.iter().filter(|entry| entry.is_active).count(), 1);

    // editing keeps the selection
    let edited = update_strategy(&conn, second.id, "Fake v2", &[]).expect("update");
    assert_eq!(edited.name, "Fake v2");
    assert_eq!(edited.args, "");
    assert!(edited.is_active);

    // deleting the active strategy leaves none selected
    delete_strategy(&conn, second.id).expect("delete");
    assert!(load_active_strategy(&conn).expect("active").is_none());
    assert_eq!(list_strategies(&conn).expect("list").len(), 1);

    assert!(select_strategy(&conn, 999).is_err());
    assert!(update_strategy(&conn, 999, "x", &[]).is_err());
    // deleting an unknown id is fine (idempotent)
    delete_strategy(&conn, 999).expect("idempotent delete");
}

#[test]
fn add_enforces_the_cap() {
    let conn = test_db();
    for index in 0..MAX_STRATEGIES {
        add_strategy(&conn, &format!("S{index}"), &[]).expect("add below cap");
    }
    let error = add_strategy(&conn, "One too many", &[]).expect_err("cap");
    assert!(error.contains("at most"));
}

#[test]
fn preset_catalog_is_valid_and_seeds_once() {
    for (name, args) in PRESET_STRATEGIES {
        assert!(validate_name(name).is_ok(), "{name} must be a valid name");
        assert!(parse_strategy_args(args).is_ok(), "{args} must pass validation");
    }

    let conn = test_db();
    seed_default_strategies(&conn).expect("seed");
    let seeded = list_strategies(&conn).expect("list");
    assert_eq!(seeded.len(), PRESET_STRATEGIES.len());
    assert_eq!(seeded[0].name, PRESET_STRATEGIES[0].0);
    assert_eq!(seeded[0].args, PRESET_STRATEGIES[0].1);
    assert!(seeded.iter().all(|strategy| !strategy.is_active));
    assert!(
        seeded.iter().any(|strategy| strategy.name == DESPAIR_NAME),
        "the Despair preset ships with the catalog"
    );

    // re-seeding is a no-op, and any existing collection blocks seeding
    seed_default_strategies(&conn).expect("seed again");
    assert_eq!(list_strategies(&conn).expect("list").len(), PRESET_STRATEGIES.len());
}

#[test]
fn list_reports_site_result_counts() {
    let conn = test_db();
    conn.execute(
        "INSERT INTO test_site_categories (name, position, action)
         VALUES ('DpiCat', 0, 'dpi'), ('ProxyCat', 1, 'proxy')",
        [],
    )
    .expect("seed categories");
    for index in 0..3 {
        conn.execute(
            "INSERT INTO test_sites (category_id, rule_type, value) VALUES (1, 'url', ?1)",
            params![format!("https://s{index}.example/")],
        )
        .expect("seed site");
    }
    conn.execute(
        "INSERT INTO test_sites (category_id, rule_type, value)
         VALUES (2, 'url', 'https://proxy.example/')",
        [],
    )
    .expect("seed proxy site");
    let strategy = add_strategy(&conn, "S", &[]).expect("add");
    conn.execute(
        "INSERT INTO dpi_url_results (strategy_id, url_id, ok)
         VALUES (?1, 1, 1), (?1, 2, 0), (?1, 3, 1), (?1, 4, 1)",
        params![strategy.id],
    )
    .expect("seed results");

    // only dpi-routed sites count towards ok/total; every probed row counts as tested
    let list = list_strategies(&conn).expect("list");
    assert_eq!((list[0].url_ok, list[0].url_total, list[0].tested), (2, 3, 4));

    // results cascade away with the strategy
    delete_strategy(&conn, strategy.id).expect("delete");
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM dpi_url_results", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn dpi_port_roundtrip_with_default() {
    let conn = test_db();
    assert_eq!(load_dpi_port(&conn).expect("port"), DEFAULT_DPI_PORT);
    set_setting(&conn, "dpi_port", "2080").expect("set");
    assert_eq!(load_dpi_port(&conn).expect("port"), 2080);
    // junk in the settings table falls back to the default instead of
    // breaking the connect flow
    set_setting(&conn, "dpi_port", "not-a-port").expect("set junk");
    assert_eq!(load_dpi_port(&conn).expect("port"), DEFAULT_DPI_PORT);
}

#[test]
fn dpi_toggle_roundtrip_with_default() {
    let conn = test_db();
    assert!(dpi_enabled(&conn).expect("toggle"), "on by default");

    set_setting(&conn, SETTING_DPI_ENABLED, "0").expect("off");
    assert!(!dpi_enabled(&conn).expect("toggle"));

    set_setting(&conn, SETTING_DPI_ENABLED, "1").expect("on");
    assert!(dpi_enabled(&conn).expect("toggle"));

    // junk in the settings table reads as on instead of breaking the
    // connect flow
    set_setting(&conn, SETTING_DPI_ENABLED, "junk").expect("junk");
    assert!(dpi_enabled(&conn).expect("toggle"));
}
