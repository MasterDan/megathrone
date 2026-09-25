use std::path::Path;

use rusqlite::Connection;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS profiles (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    name                 TEXT    NOT NULL,
    source_url           TEXT,
    item_count           INTEGER NOT NULL DEFAULT 0,
    skipped_count        INTEGER NOT NULL DEFAULT 0,
    created_at           TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at           TEXT    NOT NULL DEFAULT (datetime('now')),
    source_path          TEXT,
    auto_update_minutes  INTEGER,
    last_fetched_at      TEXT,
    -- consecutive failed scheduled updates; at the limit the scheduler
    -- ignores the profile until any update succeeds again
    auto_update_failures INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS endpoints (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id     INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    tag            TEXT    NOT NULL,
    protocol       TEXT    NOT NULL,
    server         TEXT,
    server_port    INTEGER,
    raw            TEXT    NOT NULL,
    outbound_json  TEXT    NOT NULL,
    order_index    INTEGER NOT NULL,

    -- runtime/test placeholders (filled once we can connect & measure)
    available      INTEGER,
    latency_ms     INTEGER,
    up_bytes       INTEGER NOT NULL DEFAULT 0,
    down_bytes     INTEGER NOT NULL DEFAULT 0,
    speed_bps      REAL,
    last_tested_at TEXT,
    updated_at     TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_endpoints_profile ON endpoints(profile_id, order_index);

-- v5: DPI bypass (byedpi/ciadpi sidecar). A strategy is a raw ciadpi argument
-- line ('-s2 -d2'); at most one is active at a time and drives the spawned
-- process together with the managed listen address/port.
CREATE TABLE IF NOT EXISTS dpi_strategies (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL,
    args       TEXT    NOT NULL DEFAULT '',
    is_active  INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- v5: scalar app settings (dpi listen port, route fallback action, …)
CREATE TABLE IF NOT EXISTS app_settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- v6/v8: URL categories (Settings → Routing). Each category carries
-- a routing action (dpi | proxy | direct, switched in the Routing list):
-- dpi — probed by the DPI strategy test and routed into the DPI tunnel;
-- proxy — its rules are deep-probed through endpoints during latency scans
-- and its hosts go through the proxy; direct — matched hosts bypass both.
-- Deleting a category (or a single rule) cascades the matching results away.
CREATE TABLE IF NOT EXISTS test_site_categories (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL UNIQUE,
    position   INTEGER NOT NULL DEFAULT 0,
    action     TEXT    NOT NULL DEFAULT 'dpi',
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- v9: every category member is a full routing rule. `rule_type` is what the
-- value is (url | domain_suffix | domain | domain_keyword | domain_regex);
-- `test_enabled` marks rules that join the DPI strategy test and the
-- endpoint deep probe (keyword/regex rules cannot be probed at all).
CREATE TABLE IF NOT EXISTS test_sites (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    category_id  INTEGER NOT NULL REFERENCES test_site_categories(id) ON DELETE CASCADE,
    rule_type    TEXT    NOT NULL DEFAULT 'domain_suffix',
    value        TEXT    NOT NULL,
    test_enabled INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (category_id, rule_type, value)
);

-- one row per (endpoint, category URL) — written only for endpoints picked
-- for a deep probe during a latency scan; endpoint rows die with their
-- profile, url rows die with their category — cascades keep the table
-- honest on both sides
CREATE TABLE IF NOT EXISTS endpoint_url_results (
    endpoint_id INTEGER NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
    url_id      INTEGER NOT NULL REFERENCES test_sites(id) ON DELETE CASCADE,
    available   INTEGER NOT NULL,
    latency_ms  INTEGER,
    tested_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (endpoint_id, url_id)
);

-- one row per (strategy, site); strategies die with their row, sites die
-- with their category — cascades keep the table honest on both sides
CREATE TABLE IF NOT EXISTS dpi_url_results (
    strategy_id INTEGER NOT NULL REFERENCES dpi_strategies(id) ON DELETE CASCADE,
    url_id      INTEGER NOT NULL REFERENCES test_sites(id) ON DELETE CASCADE,
    ok          INTEGER NOT NULL,
    tested_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (strategy_id, url_id)
);

INSERT OR IGNORE INTO app_settings (key, value) VALUES ('dpi_port', '1080');
INSERT OR IGNORE INTO app_settings (key, value) VALUES ('dpi_enabled', '1');
INSERT OR IGNORE INTO app_settings (key, value) VALUES ('route_fallback', 'proxy');
INSERT OR IGNORE INTO app_settings (key, value) VALUES ('raw_proxy_enabled', '1');
INSERT OR IGNORE INTO app_settings (key, value) VALUES ('raw_proxy_port', '7890');
";

const SCHEMA_VERSION: i64 = 11;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    // v8 rework: URL categories absorb the old test-URL list and the routing
    // rule table (the routing UI is now per-category actions). The gate is
    // read once, before the idempotent schema batch — the v8 tables must be
    // (re)created by it, so the legacy ones have to go first.
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version < 8 {
        if table_exists(conn, "test_site_categories")? {
            add_column_if_missing(
                conn,
                "test_site_categories",
                "action",
                "TEXT NOT NULL DEFAULT 'dpi'",
            )?;
            // categories switched off before the rework neither joined the
            // DPI test nor the DPI routing — "direct" is their equivalent
            if column_exists(conn, "test_site_categories", "enabled")? {
                conn.execute_batch(
                    "UPDATE test_site_categories SET action = 'direct' WHERE enabled = 0;",
                )?;
            }
        }
        // child first (endpoint_url_results referenced test_urls); deep-probe
        // results are transient and regenerate on the next scan
        conn.execute_batch(
            "DROP TABLE IF EXISTS endpoint_url_results;
             DROP TABLE IF EXISTS test_urls;
             DROP TABLE IF EXISTS route_rules;",
        )?;
    }

    // v9 rework: category members become typed routing rules. The old `url`
    // rows survive as `url`-typed rules; the FK'd result tables cannot
    // survive the table rebuild and are recreated empty by the schema batch
    // (deep-probe rows regenerate on the next scan, strategy scores on the
    // next strategy test). The YouTube/Discord catalogs additionally gain
    // one whole-domain suffix rule each.
    if version < 9 && table_exists(conn, "test_sites")? && column_exists(conn, "test_sites", "url")? {
        conn.execute_batch(
            "DROP TABLE IF EXISTS endpoint_url_results;
             DROP TABLE IF EXISTS dpi_url_results;
             CREATE TABLE test_sites_v9 (
                 id           INTEGER PRIMARY KEY AUTOINCREMENT,
                 category_id  INTEGER NOT NULL REFERENCES test_site_categories(id) ON DELETE CASCADE,
                 rule_type    TEXT    NOT NULL DEFAULT 'domain_suffix',
                 value        TEXT    NOT NULL,
                 test_enabled INTEGER NOT NULL DEFAULT 1,
                 created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
                 UNIQUE (category_id, rule_type, value)
             );
             INSERT INTO test_sites_v9 (id, category_id, rule_type, value, test_enabled, created_at)
                 SELECT id, category_id, 'url', url, 1, created_at FROM test_sites;
             DROP TABLE test_sites;
             ALTER TABLE test_sites_v9 RENAME TO test_sites;
             INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
                 SELECT c.id, 'domain_suffix', 'youtube.com', 1 FROM test_site_categories c
                 WHERE c.name = 'YouTube'
                   AND NOT EXISTS (SELECT 1 FROM test_sites n
                                   WHERE n.category_id = c.id
                                     AND n.rule_type = 'domain_suffix' AND n.value = 'youtube.com');
             INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
                 SELECT c.id, 'domain_suffix', 'discord.com', 1 FROM test_site_categories c
                 WHERE c.name = 'Discord'
                   AND NOT EXISTS (SELECT 1 FROM test_sites n
                                   WHERE n.category_id = c.id
                                     AND n.rule_type = 'domain_suffix' AND n.value = 'discord.com');",
        )?;
    }

    conn.execute_batch(SCHEMA)?;

    // Databases created before v2 lack the update-related columns; SQLite has
    // no "ADD COLUMN IF NOT EXISTS", so check table_info first.
    add_column_if_missing(conn, "profiles", "source_path", "TEXT")?;
    add_column_if_missing(conn, "profiles", "auto_update_minutes", "INTEGER")?;
    add_column_if_missing(conn, "profiles", "last_fetched_at", "TEXT")?;
    add_column_if_missing(
        conn,
        "profiles",
        "auto_update_failures",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    // v3: the endpoint picked in the UI, remembered by its raw link so the
    // choice survives content updates (endpoint ids are not stable across them)
    add_column_if_missing(conn, "profiles", "selected_endpoint_key", "TEXT")?;
    // v10: how the picked endpoint is chosen — one of the automatic
    // strategies (round_robin | fastest | most_available) or `manual` (the
    // checkmark). Existing rows with a checkmark stay manual; everything
    // else starts automatic.
    add_column_if_missing(
        conn,
        "profiles",
        "selection_mode",
        "TEXT NOT NULL DEFAULT 'fastest'",
    )?;
    if version < 10 {
        conn.execute_batch(
            "UPDATE profiles SET selection_mode = 'manual'
              WHERE selected_endpoint_key IS NOT NULL AND selected_endpoint_key != '';",
        )?;
    }

    // v11: the default catalog gained whole-domain suffix rules (the CDN
    // domains its URL rules enumerate subdomains of). They reach existing
    // databases the way the v9 gate shipped youtube.com/discord.com —
    // matched by category name, skipped when already present; renamed or
    // deleted categories simply match nothing.
    if version < 11 {
        conn.execute_batch(
            "INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
                SELECT c.id, 'domain_suffix', 'googlevideo.com', 1 FROM test_site_categories c
                WHERE c.name = 'Google Video'
                  AND NOT EXISTS (SELECT 1 FROM test_sites n
                                  WHERE n.category_id = c.id
                                    AND n.rule_type = 'domain_suffix' AND n.value = 'googlevideo.com');
            INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
                SELECT c.id, 'domain_suffix', 'ytimg.com', 1 FROM test_site_categories c
                WHERE c.name = 'YouTube'
                  AND NOT EXISTS (SELECT 1 FROM test_sites n
                                  WHERE n.category_id = c.id
                                    AND n.rule_type = 'domain_suffix' AND n.value = 'ytimg.com');
            INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
                SELECT c.id, 'domain_suffix', 'ggpht.com', 1 FROM test_site_categories c
                WHERE c.name = 'YouTube'
                  AND NOT EXISTS (SELECT 1 FROM test_sites n
                                  WHERE n.category_id = c.id
                                    AND n.rule_type = 'domain_suffix' AND n.value = 'ggpht.com');
            INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
                SELECT c.id, 'domain_suffix', 't.me', 1 FROM test_site_categories c
                WHERE c.name = 'Telegram'
                  AND NOT EXISTS (SELECT 1 FROM test_sites n
                                  WHERE n.category_id = c.id
                                    AND n.rule_type = 'domain_suffix' AND n.value = 't.me');
            INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
                SELECT c.id, 'domain_suffix', 'twimg.com', 1 FROM test_site_categories c
                WHERE c.name = 'Social'
                  AND NOT EXISTS (SELECT 1 FROM test_sites n
                                  WHERE n.category_id = c.id
                                    AND n.rule_type = 'domain_suffix' AND n.value = 'twimg.com');",
        )?;
    }
    conn.execute_batch(
        "UPDATE profiles SET last_fetched_at = COALESCE(last_fetched_at, updated_at, datetime('now'));",
    )?;

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> rusqlite::Result<()> {
    if !column_exists(conn, table, column)? {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {declaration};"))?;
    }
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    let exists = stmt
        .query_map([table], |_| Ok(()))?
        .next()
        .transpose()?
        .is_some();
    Ok(exists)
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    Ok(exists)
}

/// Reads a scalar setting; `None` when the key has never been written.
pub fn get_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .map(Some)
    .or_else(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// Writes a scalar setting (upsert).
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_v1_schema_in_place() {
        let path = std::env::temp_dir().join(format!(
            "megathrone-db-upgrade-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
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
            )
            .unwrap();
        }

        let conn = open(&path).unwrap();

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
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// The v9 rework: URL rows become `url`-typed rules, the FK'd result
    /// tables are rebuilt empty, and the YouTube/Discord categories gain a
    /// whole-domain suffix rule.
    #[test]
    fn upgrades_v8_schema_in_place() {
        let path = std::env::temp_dir().join(format!(
            "megathrone-db-upgrade-v8-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE test_site_categories (
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
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 8).unwrap();
        }

        let conn = open(&path).unwrap();

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
        let conn = open(&path).unwrap();
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
        let path = std::env::temp_dir().join(format!(
            "megathrone-db-upgrade-v7-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
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
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 7).unwrap();
        }

        let conn = open(&path).unwrap();

        let actions: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT name, action FROM test_site_categories ORDER BY position")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(actions, vec![("On".into(), "dpi".into()), ("Off".into(), "direct".into())]);

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
        let conn = open(&path).unwrap();
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
        let path = std::env::temp_dir().join(format!(
            "megathrone-db-upgrade-v9-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE profiles (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    item_count INTEGER NOT NULL DEFAULT 0,
                    skipped_count INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                    selected_endpoint_key TEXT
                );
                INSERT INTO profiles (name, selected_endpoint_key)
                    VALUES ('Checked', 'raw-link'), ('Unchecked', NULL);",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 9).unwrap();
        }

        let conn = open(&path).unwrap();

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
        let conn = open(&path).unwrap();
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
        let path = std::env::temp_dir().join(format!(
            "megathrone-db-upgrade-v10-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE test_site_categories (
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
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 10).unwrap();
        }

        let conn = open(&path).unwrap();

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
                // Google Video already had the rule — not duplicated
                ("Google Video".into(), "googlevideo.com".into()),
                ("YouTube".into(), "ggpht.com".into()),
                ("YouTube".into(), "ytimg.com".into()),
            ]
        );

        // re-opening is a no-op (the user_version gate keeps rows intact)
        drop(conn);
        let conn = open(&path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM test_sites WHERE rule_type = 'domain_suffix'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3, "the whole-domain rules are not added twice");
    }
}
