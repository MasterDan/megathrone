-- v12: the Discovery catalog — public proxy-subscription URLs a background
-- discovery run sweeps into one profile per source (Omega-N). Seeded from
-- app setup only into an empty table; `profile_id` links a source to the
-- profile the run created (a profile deleted elsewhere unlinks, the next
-- run imports a fresh one).

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
);
