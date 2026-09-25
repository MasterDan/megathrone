-- v8 rework: URL categories absorb the old test-URL list and the routing
-- rule table (the routing UI is now per-category actions). Categories
-- switched off before the rework neither joined the DPI test nor the DPI
-- routing — "direct" is their equivalent. Deep-probe results are transient
-- and regenerate on the next scan, so the old test_urls-fk'd table goes and
-- comes back fk'd to the category rules.

ALTER TABLE test_site_categories ADD COLUMN action TEXT NOT NULL DEFAULT 'dpi';

UPDATE test_site_categories SET action = 'direct' WHERE enabled = 0;

-- child first (endpoint_url_results referenced test_urls)
DROP TABLE IF EXISTS endpoint_url_results;
DROP TABLE IF EXISTS test_urls;
DROP TABLE IF EXISTS route_rules;

CREATE TABLE endpoint_url_results (
    endpoint_id INTEGER NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
    url_id      INTEGER NOT NULL REFERENCES test_sites(id) ON DELETE CASCADE,
    available   INTEGER NOT NULL,
    latency_ms  INTEGER,
    tested_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (endpoint_id, url_id)
);
