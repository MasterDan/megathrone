// The era-upgrade tests: every historical schema state (v1-era, v7, v8,
// v9, v10, v12) is recreated by hand — deliberately NOT via the migration
// files, otherwise the chain would only ever be tested against itself —
// and must migrate to the current shape on open.

use std::path::PathBuf;

use rusqlite::Connection;

use super::migrations::{latest_version, Migrations};
use super::runner::{open, table_exists};

/// Creates a throwaway database with `schema` as its exact content and
/// `version` stamped into `PRAGMA user_version` (0 = the era predates
/// version tracking), then closes it — the upgrade runs on a fresh open.
fn legacy_db(tag: &str, schema: &str, version: i64) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "megathrone-db-upgrade-{tag}-{}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(schema).unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
    }
    path
}

#[test]
fn upgrades_v1_schema_in_place() {
    let path = legacy_db(
        "legacy",
        "CREATE TABLE profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            source_url TEXT,
            item_count INTEGER NOT NULL DEFAULT 0,
            skipped_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO profiles (name, source_url, item_count) VALUES ('Legacy', 'https://x', 7);",
        0,
    );

    let conn = open(&path, &Migrations::embedded()).unwrap();

    let columns: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(profiles)").unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    for column in ["source_path", "auto_update_minutes", "last_fetched_at"] {
        assert!(columns.iter().any(|name| name == column), "missing {column}");
    }

    let row: (String, i64, String) = conn
        .query_row(
            "SELECT name, item_count, COALESCE(last_fetched_at, '') FROM profiles",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row.0, "Legacy");
    assert_eq!(row.1, 7);
    // existing rows get last_fetched_at backfilled from updated_at
    assert!(!row.2.is_empty(), "last_fetched_at should be backfilled");

    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, latest_version());
}

/// The v9 rework: URL rows become `url`-typed rules, the FK'd result
/// tables are rebuilt empty, and the YouTube/Discord categories gain a
/// whole-domain suffix rule.
#[test]
fn upgrades_v8_schema_in_place() {
    let path = legacy_db(
        "v8",
        "CREATE TABLE profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            source_url TEXT,
            item_count INTEGER NOT NULL DEFAULT 0,
            skipped_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            source_path TEXT,
            auto_update_minutes INTEGER,
            last_fetched_at TEXT,
            auto_update_failures INTEGER NOT NULL DEFAULT 0,
            selected_endpoint_key TEXT
        );
        CREATE TABLE test_site_categories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            position INTEGER NOT NULL DEFAULT 0,
            action TEXT NOT NULL DEFAULT 'dpi',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE test_sites (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            category_id INTEGER NOT NULL REFERENCES test_site_categories(id) ON DELETE CASCADE,
            url TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (category_id, url)
        );
        INSERT INTO test_site_categories (name, position) VALUES ('YouTube', 0), ('Other', 1);
        INSERT INTO test_sites (category_id, url) VALUES (1, 'https://youtube.com/');
        INSERT INTO test_sites (category_id, url) VALUES (2, 'https://a.example/path');",
        8,
    );

    let conn = open(&path, &Migrations::embedded()).unwrap();

    let rules: Vec<(String, String, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT c.name, s.rule_type, COUNT(*) FROM test_sites s
                 JOIN test_site_categories c ON c.id = s.category_id
                 GROUP BY c.name, s.rule_type ORDER BY c.name",
            )
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    // old URLs keep their id, value and probe target as `url` rules…
    assert_eq!(
        rules,
        vec![
            ("Other".into(), "url".into(), 1),
            ("YouTube".into(), "domain_suffix".into(), 3),
            ("YouTube".into(), "url".into(), 1),
        ]
    );
    let kept_id: i64 = conn
        .query_row("SELECT id FROM test_sites WHERE value = 'https://youtube.com/'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(kept_id, 1, "ids survive the rebuild (FK'd results stay joinable)");
    // the result tables are back, empty
    for table in ["endpoint_url_results", "dpi_url_results"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    // re-opening is a no-op (the user_version gate keeps rows intact)
    drop(conn);
    let conn = open(&path, &Migrations::embedded()).unwrap();
    let suffixes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM test_sites WHERE rule_type = 'domain_suffix'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(suffixes, 3, "the whole-domain rules are not added twice");
}

/// The v8 rework: legacy enabled/disabled categories become dpi/direct
/// actions, the test-URL and routing-rule tables go away, and
/// `endpoint_url_results` is rebuilt on top of category URLs.
#[test]
fn upgrades_v7_schema_in_place() {
    let path = legacy_db(
        "v7",
        "CREATE TABLE test_site_categories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            position INTEGER NOT NULL DEFAULT 0,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE test_sites (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            category_id INTEGER NOT NULL REFERENCES test_site_categories(id) ON DELETE CASCADE,
            url TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (category_id, url)
        );
        CREATE TABLE test_urls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE endpoint_url_results (
            endpoint_id INTEGER NOT NULL,
            url_id INTEGER NOT NULL REFERENCES test_urls(id) ON DELETE CASCADE,
            available INTEGER NOT NULL,
            latency_ms INTEGER,
            tested_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (endpoint_id, url_id)
        );
        CREATE TABLE route_rules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            match_type TEXT NOT NULL,
            pattern TEXT NOT NULL,
            action TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO test_site_categories (name, position, enabled)
            VALUES ('On', 0, 1), ('Off', 1, 0);
        INSERT INTO test_sites (category_id, url) VALUES (1, 'https://a.example/');
        INSERT INTO test_urls (url) VALUES ('https://legacy.example/');",
        7,
    );

    let conn = open(&path, &Migrations::embedded()).unwrap();

    let actions: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT name, action FROM test_site_categories ORDER BY position")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        actions,
        vec![("On".into(), "dpi".into()), ("Off".into(), "direct".into())]
    );

    assert!(!table_exists(&conn, "test_urls").unwrap());
    assert!(!table_exists(&conn, "route_rules").unwrap());
    // rebuilt with the category-URL foreign key and empty
    let results: i64 = conn
        .query_row("SELECT COUNT(*) FROM endpoint_url_results", [], |row| row.get(0))
        .unwrap();
    assert_eq!(results, 0);
    let fk_target: String = conn
        .query_row(
            "SELECT m.name FROM sqlite_master m
             JOIN pragma_foreign_key_list('endpoint_url_results') f ON f.\"table\" = m.name
             WHERE f.\"from\" = 'url_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fk_target, "test_sites");

    // re-opening is a no-op (user_version gate keeps actions intact)
    drop(conn);
    let conn = open(&path, &Migrations::embedded()).unwrap();
    let still: String = conn
        .query_row(
            "SELECT action FROM test_site_categories WHERE name = 'Off'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still, "direct");
}

/// The v10 rework: the checkmark selection gains a strategy column —
/// rows with a checkmark stay manual, everything else becomes
/// automatic (fastest).
#[test]
fn upgrades_v9_schema_in_place() {
    let path = legacy_db(
        "v9",
        "CREATE TABLE profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            source_url TEXT,
            auto_update_minutes INTEGER,
            item_count INTEGER NOT NULL DEFAULT 0,
            skipped_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            selected_endpoint_key TEXT
        );
        INSERT INTO profiles (name, selected_endpoint_key)
            VALUES ('Checked', 'raw-link'), ('Unchecked', NULL);
        CREATE TABLE test_site_categories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            position INTEGER NOT NULL DEFAULT 0,
            action TEXT NOT NULL DEFAULT 'dpi',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE test_sites (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            category_id INTEGER NOT NULL REFERENCES test_site_categories(id) ON DELETE CASCADE,
            rule_type TEXT NOT NULL DEFAULT 'domain_suffix',
            value TEXT NOT NULL,
            test_enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (category_id, rule_type, value)
        );",
        9,
    );

    let conn = open(&path, &Migrations::embedded()).unwrap();

    let modes: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT name, selection_mode FROM profiles ORDER BY name")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        modes,
        vec![
            ("Checked".into(), "manual".into()),
            ("Unchecked".into(), "fastest".into())
        ]
    );

    // re-opening never touches the modes again (the user_version gate)
    drop(conn);
    let conn = open(&path, &Migrations::embedded()).unwrap();
    let still: String = conn
        .query_row(
            "SELECT selection_mode FROM profiles WHERE name = 'Checked'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still, "manual");
}

/// The v11 gate: the default catalog's new whole-domain suffix rules
/// reach existing databases — inserted once per matching category name,
/// skipped when the rule already exists, user categories left alone.
#[test]
fn upgrades_v10_schema_in_place() {
    let path = legacy_db(
        "v10",
        "CREATE TABLE profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            source_url TEXT,
            auto_update_minutes INTEGER,
            item_count INTEGER NOT NULL DEFAULT 0,
            skipped_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            selection_mode TEXT NOT NULL DEFAULT 'fastest',
            selected_endpoint_key TEXT
        );
        CREATE TABLE test_site_categories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            position INTEGER NOT NULL DEFAULT 0,
            action TEXT NOT NULL DEFAULT 'dpi',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE test_sites (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            category_id INTEGER NOT NULL REFERENCES test_site_categories(id) ON DELETE CASCADE,
            rule_type TEXT NOT NULL DEFAULT 'domain_suffix',
            value TEXT NOT NULL,
            test_enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (category_id, rule_type, value)
        );
        INSERT INTO test_site_categories (name, position)
            VALUES ('Google Video', 0), ('YouTube', 1), ('Custom', 2);
        INSERT INTO test_sites (category_id, rule_type, value) VALUES
            (1, 'url', 'https://rr1---sn-4axm-n8vs.googlevideo.com/'),
            (1, 'domain_suffix', 'googlevideo.com'),
            (2, 'url', 'https://i.ytimg.com/');",
        10,
    );

    let conn = open(&path, &Migrations::embedded()).unwrap();

    let suffixes: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT c.name, s.value FROM test_sites s
                 JOIN test_site_categories c ON c.id = s.category_id
                 WHERE s.rule_type = 'domain_suffix'
                 ORDER BY c.name, s.value",
            )
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        suffixes,
        vec![
            // Google Video already had the rule— not duplicated
            ("Google Video".into(), "googlevideo.com".into()),
            ("YouTube".into(), "ggpht.com".into()),
            ("YouTube".into(), "ytimg.com".into()),
        ]
    );

    // re-opening is a no-op (the user_version gate keeps rows intact)
    drop(conn);
    let conn = open(&path, &Migrations::embedded()).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM test_sites WHERE rule_type = 'domain_suffix'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3, "the whole-domain rules are not added twice");
}

/// The v13 rework: profiles gain the explicit sidebar order and the
/// visibility flag. Existing rows are backfilled in the order the UI
/// showed until then (name, then id) so nothing jumps on upgrade.
#[test]
fn upgrades_v12_schema_in_place() {
    let path = legacy_db(
        "v12",
        "CREATE TABLE profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            source_url TEXT,
            auto_update_minutes INTEGER,
            item_count INTEGER NOT NULL DEFAULT 0,
            skipped_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            selection_mode TEXT NOT NULL DEFAULT 'fastest',
            selected_endpoint_key TEXT
        );
        INSERT INTO profiles (name) VALUES ('Zulu'), ('alpha'), ('alpha');

        CREATE TABLE discovery_sources (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL,
            name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            position INTEGER NOT NULL,
            profile_id INTEGER REFERENCES profiles(id) ON DELETE SET NULL,
            last_run_at TEXT,
            last_error TEXT,
            last_item_count INTEGER,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
        12,
    );

    let conn = open(&path, &Migrations::embedded()).unwrap();

    let rows: Vec<(String, i64, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT name, sidebar_visible, sidebar_position FROM profiles
                 ORDER BY sidebar_position, id",
            )
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    // every profile starts visible, in the name order the old UI showed
    assert_eq!(
        rows,
        vec![
            ("alpha".into(), 1, 0),
            ("alpha".into(), 1, 1),
            ("Zulu".into(), 1, 2),
        ]
    );

    // re-opening never reorders again (the user_version gate)
    drop(conn);
    let conn = open(&path, &Migrations::embedded()).unwrap();
    let zulu: i64 = conn
        .query_row(
            "SELECT sidebar_position FROM profiles WHERE name = 'Zulu'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(zulu, 2);
}

#[test]
fn upgrades_v13_schema_in_place() {
    // a v13-era database: discovery runs already linked their profiles,
    // which carried the source URL and could auto-update
    let path = legacy_db(
        "v13",
        "CREATE TABLE profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            source_url TEXT,
            auto_update_minutes INTEGER
        );
        INSERT INTO profiles (name, source_url, auto_update_minutes) VALUES
            ('Mine', 'https://example.com/my-sub', 60),
            ('Omega-1', 'https://example.com/omega', 30);

        CREATE TABLE discovery_sources (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL,
            name TEXT NOT NULL,
            position INTEGER NOT NULL DEFAULT 0,
            profile_id INTEGER
        );
        INSERT INTO discovery_sources (url, name, position, profile_id)
            VALUES ('https://example.com/omega', 'Omega-1', 0, 2);",
        13,
    );

    let conn = open(&path, &Migrations::embedded()).unwrap();

    // the discovery-linked profile became a Discovery source: flagged,
    // its self-update URL and interval gone (the URL lives on in the
    // discovery catalog)
    let rows: Vec<(String, i64, Option<String>, Option<i64>)> = {
        let mut stmt = conn
            .prepare(
                "SELECT name, source_discovery, source_url, auto_update_minutes
                 FROM profiles ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    };
    assert_eq!(
        rows,
        vec![
            ("Mine".into(), 0, Some("https://example.com/my-sub".into()), Some(60)),
            ("Omega-1".into(), 1, None, None),
        ]
    );

    // re-opening never re-marks (the user_version gate)
    drop(conn);
    let conn = open(&path, &Migrations::embedded()).unwrap();
    let mine_url: Option<String> = conn
        .query_row(
            "SELECT source_url FROM profiles WHERE name = 'Mine'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(mine_url.as_deref(), Some("https://example.com/my-sub"));
}

/// A database an interim build crippled: its pre-file heal forgot the
/// `auto_update_failures` re-add and still stamped the schema past v8, so
/// the file chain sees it as current while the update flow fails with
/// "no such column: auto_update_failures". The post-chain repair brings
/// the column back in place.
#[test]
fn repairs_past_v8_db_missing_auto_update_failures() {
    let path = legacy_db(
        "repair",
        "CREATE TABLE profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            source_url TEXT,
            item_count INTEGER NOT NULL DEFAULT 0,
            skipped_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            source_path TEXT,
            auto_update_minutes INTEGER,
            last_fetched_at TEXT,
            selected_endpoint_key TEXT,
            selection_mode TEXT NOT NULL DEFAULT 'fastest',
            source_discovery INTEGER NOT NULL DEFAULT 0,
            sidebar_visible INTEGER NOT NULL DEFAULT 1,
            sidebar_position INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO profiles (name, source_url) VALUES ('Omega-1', 'https://example.com/omega');
        CREATE TABLE discovery_sources (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL,
            name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            position INTEGER NOT NULL,
            profile_id INTEGER REFERENCES profiles(id) ON DELETE SET NULL,
            merge_group TEXT,
            last_run_at TEXT,
            last_error TEXT,
            last_item_count INTEGER,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO discovery_sources (url, name, position, profile_id)
            VALUES ('https://example.com/omega', 'Omega-1', 0, 1);",
        16,
    );

    let conn = open(&path, &Migrations::embedded()).unwrap();

    // the column is back and serves the update flow's statements
    let cleared: i64 = conn
        .query_row(
            "UPDATE profiles SET auto_update_failures = 0
             WHERE id = 1 RETURNING auto_update_failures",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cleared, 0);
    let due: Vec<i64> = conn
        .prepare("SELECT id FROM profiles WHERE auto_update_failures < 3")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(due, vec![1]);

    // re-opening is a no-op (the column already there is kept, not re-added)
    drop(conn);
    let conn = open(&path, &Migrations::embedded()).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, latest_version());
}

/// The v16 backfill: catalogs seeded before v15 carry the seven
/// whitelist-bypass extras ungrouped, each with the profile an earlier
/// run gave it. The upgrade groups them by URL and drops the redundant
/// profiles (the group stores into the first member's ONE shared
/// profile) — endpoints cascade, the stale links null out.
#[test]
fn upgrades_v16_schema_in_place() {
    let path = legacy_db(
        "v16",
        "CREATE TABLE profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL
        );
        INSERT INTO profiles (name) VALUES
            ('Omega-1'),
            ('Omega-26'),
            ('Omega-27'),
            ('Omega-28');

        CREATE TABLE endpoints (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            profile_id INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE
        );
        INSERT INTO endpoints (profile_id) VALUES (1), (2), (3), (4);

        CREATE TABLE discovery_sources (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL,
            name TEXT NOT NULL,
            position INTEGER NOT NULL,
            profile_id INTEGER REFERENCES profiles(id) ON DELETE SET NULL,
            merge_group TEXT
        );
        INSERT INTO discovery_sources (url, name, position, profile_id) VALUES
            ('https://example.com/plain', 'Omega-1', 0, 1),
            ('https://raw.githubusercontent.com/igareck/vpn-configs-for-russia/refs/heads/main/WHITE-CIDR-RU-all.txt', 'Omega-26', 1, 2),
            ('https://raw.githubusercontent.com/zieng2/wl/main/vless_lite.txt', 'Omega-27', 2, 3),
            ('https://wlrus.lol/confs/selected.txt', 'Omega-28', 3, 4);",
        15,
    );

    let conn = open(&path, &Migrations::embedded()).unwrap();

    // the extras are grouped, the plain source is not
    let groups: Vec<(String, Option<String>)> = {
        let mut stmt = conn
            .prepare("SELECT name, merge_group FROM discovery_sources ORDER BY position")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        groups,
        vec![
            ("Omega-1".into(), None),
            ("Omega-26".into(), Some("whitelists".into())),
            ("Omega-27".into(), Some("whitelists".into())),
            ("Omega-28".into(), Some("whitelists".into())),
        ]
    );

    // the group's non-first profiles (and their endpoints) are gone, the
    // first member's stays linked
    let profiles: Vec<String> = {
        let mut stmt = conn.prepare("SELECT name FROM profiles ORDER BY id").unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(profiles, vec!["Omega-1".to_string(), "Omega-26".to_string()]);
    let (owner_link, endpoint_count): (Option<i64>, i64) = conn
        .query_row(
            "SELECT (SELECT profile_id FROM discovery_sources WHERE position = 1),
                    (SELECT COUNT(*) FROM endpoints)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(owner_link, Some(2), "the first member keeps its profile");
    assert_eq!(endpoint_count, 2, "the redundant profiles' endpoints cascade");
    let stale_link: Option<i64> = conn
        .query_row(
            "SELECT profile_id FROM discovery_sources WHERE position = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale_link, None, "the deleted members' links null out");
}
