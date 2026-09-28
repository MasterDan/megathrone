-- v14: the third profile source type — Discovery. Profiles a Discovery
-- run created are owned by it: they cannot refresh themselves, so the
-- upgrade marks every discovery-linked profile, drops its self-update
-- source (the URL lives on in discovery_sources) and clears any
-- auto-update interval; the update flow and set_auto_update refuse
-- them from now on.

ALTER TABLE profiles ADD COLUMN source_discovery INTEGER NOT NULL DEFAULT 0;

UPDATE profiles
SET source_discovery = 1, source_url = NULL, auto_update_minutes = NULL
WHERE id IN (SELECT profile_id FROM discovery_sources WHERE profile_id IS NOT NULL);
