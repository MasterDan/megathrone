-- v13: the sidebar's explicit profile order and visibility. Existing rows
-- are backfilled in the order the UI showed until now (name, then id) so
-- nothing jumps on upgrade; new imports append at the end (see
-- import_profile). `sidebar_visible` hides a profile from the sidebar
-- without deleting it — the settings page still lists every profile.

ALTER TABLE profiles ADD COLUMN sidebar_visible INTEGER NOT NULL DEFAULT 1;
ALTER TABLE profiles ADD COLUMN sidebar_position INTEGER NOT NULL DEFAULT 0;

UPDATE profiles SET sidebar_position = (
    SELECT COUNT(*)
    FROM profiles older
    WHERE (older.name COLLATE NOCASE) < (profiles.name COLLATE NOCASE)
       OR (older.name COLLATE NOCASE) = (profiles.name COLLATE NOCASE)
          AND older.id < profiles.id
);
