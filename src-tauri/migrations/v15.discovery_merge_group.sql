-- v15: merged Discovery sources — rows sharing a merge_group fetch as one
-- unit and land in ONE shared profile (named by the group's first row in
-- position order; every member row links it). The seeded whitelist-bypass
-- extras become a single aggregate profile instead of one per URL.

ALTER TABLE discovery_sources ADD COLUMN merge_group TEXT;
