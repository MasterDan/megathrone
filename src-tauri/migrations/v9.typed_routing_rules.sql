-- v9 rework: category members become typed routing rules. The old `url`
-- rows survive as `url`-typed rules (keeping their ids — FK'd results stay
-- joinable); the FK'd result tables cannot survive the table rebuild and
-- are recreated empty (deep-probe rows regenerate on the next scan,
-- strategy scores on the next strategy test). The YouTube/Discord
-- categories additionally gain one whole-domain suffix rule each.

DROP TABLE IF EXISTS endpoint_url_results;
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
                        AND n.rule_type = 'domain_suffix' AND n.value = 'discord.com');

CREATE TABLE endpoint_url_results (
    endpoint_id INTEGER NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
    url_id      INTEGER NOT NULL REFERENCES test_sites(id) ON DELETE CASCADE,
    available   INTEGER NOT NULL,
    latency_ms  INTEGER,
    tested_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (endpoint_id, url_id)
);

CREATE TABLE dpi_url_results (
    strategy_id INTEGER NOT NULL REFERENCES dpi_strategies(id) ON DELETE CASCADE,
    url_id      INTEGER NOT NULL REFERENCES test_sites(id) ON DELETE CASCADE,
    ok          INTEGER NOT NULL,
    tested_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (strategy_id, url_id)
);
