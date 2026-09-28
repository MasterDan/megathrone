-- v16: backfills the merge groups onto catalogs seeded before v15 (the
-- seed only ever writes into an empty table, so an existing install kept
-- its rows ungrouped). The seven whitelist-bypass extras — matched by
-- URL — become the "whitelists" group. The profiles earlier runs gave
-- the group's non-first members are redundant: a grouped run stores ONE
-- shared profile (the first member's), so those profiles go away here
-- (endpoints and results via the FK cascades, source links via
-- ON DELETE SET NULL).

UPDATE discovery_sources
SET merge_group = 'whitelists'
WHERE url IN (
    'https://raw.githubusercontent.com/igareck/vpn-configs-for-russia/refs/heads/main/WHITE-CIDR-RU-all.txt',
    'https://raw.githubusercontent.com/igareck/vpn-configs-for-russia/refs/heads/main/WHITE-SNI-RU-all.txt',
    'https://raw.githubusercontent.com/zieng2/wl/refs/heads/main/vless_universal.txt',
    'https://raw.githubusercontent.com/zieng2/wl/main/vless_lite.txt',
    'https://raw.githubusercontent.com/EtoNeYaProject/etoneyaproject.github.io/refs/heads/main/2',
    'https://raw.githubusercontent.com/ByeWhiteLists/ByeWhiteLists2/refs/heads/main/ByeWhiteLists2.txt',
    'https://wlrus.lol/confs/selected.txt'
);

DELETE FROM profiles
WHERE id IN (
    SELECT ds.profile_id
    FROM discovery_sources ds
    WHERE ds.merge_group IS NOT NULL
      AND ds.profile_id IS NOT NULL
      AND ds.position > (
          SELECT MIN(d2.position) FROM discovery_sources d2
          WHERE d2.merge_group = ds.merge_group
      )
);
