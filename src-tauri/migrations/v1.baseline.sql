-- v1: consolidated baseline of the whole pre-file schema history (the
-- squashed repository carries no v1..v7 archaeology, so this file is their
-- sum — the exact shape the v8 rework was written to consume: categories
-- still toggle with `enabled`, category members are plain URLs, and the
-- standalone test-URL / route-rule tables still exist). Fresh databases
-- start here; legacy pre-v8 databases are brought to exactly this shape
-- by the tolerant heal in db.rs before the file loop takes over.

CREATE TABLE IF NOT EXISTS profiles (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    name                  TEXT    NOT NULL,
    source_url            TEXT,
    item_count            INTEGER NOT NULL DEFAULT 0,
    skipped_count         INTEGER NOT NULL DEFAULT 0,
    created_at            TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at            TEXT    NOT NULL DEFAULT (datetime('now')),
    source_path           TEXT,
    auto_update_minutes   INTEGER,
    last_fetched_at       TEXT,
    -- consecutive failed scheduled updates; at the limit the scheduler
    -- ignores the profile until any update succeeds again
    auto_update_failures  INTEGER NOT NULL DEFAULT 0,
    -- the endpoint picked in the UI, remembered by its raw link so the
    -- choice survives content updates (endpoint ids are not stable across them)
    selected_endpoint_key TEXT
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

-- DPI bypass (byedpi/ciadpi sidecar). A strategy is a raw ciadpi argument
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

-- scalar app settings (dpi listen port, route fallback action, …)
CREATE TABLE IF NOT EXISTS app_settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- URL categories (Settings → Routing), still toggled with `enabled` (the
-- per-category routing action arrives in v8)
CREATE TABLE IF NOT EXISTS test_site_categories (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL UNIQUE,
    position   INTEGER NOT NULL DEFAULT 0,
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- category members as plain URLs (typed rules arrive in v9)
CREATE TABLE IF NOT EXISTS test_sites (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    category_id  INTEGER NOT NULL REFERENCES test_site_categories(id) ON DELETE CASCADE,
    url          TEXT    NOT NULL,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (category_id, url)
);

-- the standalone test-URL list and the hand-written routing rules — both
-- absorbed into the categories by the v8 rework
CREATE TABLE IF NOT EXISTS test_urls (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    url        TEXT    NOT NULL UNIQUE,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS route_rules (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    match_type TEXT    NOT NULL,
    pattern    TEXT    NOT NULL,
    action     TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- one row per (endpoint, URL) — written only for endpoints picked for a
-- deep probe during a latency scan; endpoint rows die with their profile,
-- url rows die with their URL — cascades keep the table honest on both sides
CREATE TABLE IF NOT EXISTS endpoint_url_results (
    endpoint_id INTEGER NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
    url_id      INTEGER NOT NULL REFERENCES test_urls(id) ON DELETE CASCADE,
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
