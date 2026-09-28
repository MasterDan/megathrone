use rusqlite::{Connection, params};

use super::model::DiscoverySource;
use crate::profiles;

const SOURCE_COLUMNS: &str = "id, url, name, enabled, position, profile_id, \
                              last_run_at, last_error, last_item_count, merge_group";

// ---------------------------------------------------------------------------
// Storage layer
// ---------------------------------------------------------------------------

pub(super) fn list_sources(conn: &Connection) -> Result<Vec<DiscoverySource>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SOURCE_COLUMNS} FROM discovery_sources ORDER BY position, id"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], row_to_source)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

pub(super) fn source_by_id(conn: &Connection, source_id: i64) -> Result<DiscoverySource, String> {
    conn.query_row(
        &format!("SELECT {SOURCE_COLUMNS} FROM discovery_sources WHERE id = ?1"),
        params![source_id],
        row_to_source,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            format!("discovery source {source_id} not found")
        }
        other => db_err(other),
    })
}

pub(super) fn add_source(
    conn: &Connection,
    url: &str,
    name: Option<&str>,
    merge_group: Option<&str>,
) -> Result<DiscoverySource, String> {
    let url = validate_url(url)?;
    let next_position: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM discovery_sources",
            [],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    let name = match name {
        Some(name) => validate_source_name(name)?,
        None => format!("Omega-{}", next_position + 1),
    };
    let merge_group = match merge_group {
        Some(group) => Some(validate_group_name(group)?),
        None => None,
    };
    conn.execute(
        "INSERT INTO discovery_sources (url, name, position, merge_group) VALUES (?1, ?2, ?3, ?4)",
        params![url, name, next_position, merge_group],
    )
    .map_err(db_err)?;
    source_by_id(conn, conn.last_insert_rowid())
}

/// Partial update: only the provided fields change, the rest keep their
/// stored values.
pub(super) fn update_source(
    conn: &Connection,
    source_id: i64,
    url: Option<String>,
    name: Option<String>,
    enabled: Option<bool>,
) -> Result<DiscoverySource, String> {
    let url = match url {
        Some(ref url) => Some(validate_url(url)?),
        None => None,
    };
    let name = match name {
        Some(ref name) => Some(validate_source_name(name)?),
        None => None,
    };
    let existing = source_by_id(conn, source_id)?;
    conn.execute(
        "UPDATE discovery_sources
         SET url = ?2, name = ?3, enabled = ?4, updated_at = datetime('now')
         WHERE id = ?1",
        params![
            source_id,
            url.unwrap_or(existing.url),
            name.unwrap_or(existing.name),
            enabled.unwrap_or(existing.enabled) as i64,
        ],
    )
    .map_err(db_err)?;
    source_by_id(conn, source_id)
}

/// Removes the source row; with `delete_profile` the linked profile (if
/// any) goes away too — unless other sources still link it (a merge
/// group's shared profile): it stays until its last member goes. The
/// caller then notifies the UI with the deleted profile's id.
pub(super) fn delete_source(
    conn: &Connection,
    source_id: i64,
    delete_profile: bool,
) -> Result<Option<i64>, String> {
    let existing = source_by_id(conn, source_id)?;
    let profile_shared = match existing.profile_id {
        Some(profile_id) => {
            let others: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM discovery_sources WHERE profile_id = ?1 AND id != ?2",
                    params![profile_id, source_id],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            others > 0
        }
        None => false,
    };
    let deleted_profile = match (delete_profile, existing.profile_id, profile_shared) {
        (true, Some(profile_id), false) => {
            profiles::delete_profile(conn, profile_id)?;
            Some(profile_id)
        }
        _ => None,
    };
    conn.execute("DELETE FROM discovery_sources WHERE id = ?1", params![source_id])
        .map_err(db_err)?;
    Ok(deleted_profile)
}

/// Removes every member of a merge group in one go — the shared linked
/// profile (if any) goes with it. Returns the deleted profile's id for
/// the UI notice. Unknown group names error out.
pub(super) fn delete_group(conn: &Connection, group: &str) -> Result<Option<i64>, String> {
    let group = validate_group_name(group)?;
    let member_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM discovery_sources WHERE merge_group = ?1",
            params![group],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if member_count == 0 {
        return Err(format!("merge group {group} not found"));
    }
    let profile_id: Option<i64> = conn
        .query_row(
            "SELECT profile_id FROM discovery_sources
             WHERE merge_group = ?1 AND profile_id IS NOT NULL LIMIT 1",
            params![group],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(db_err(other)),
        })?;
    if let Some(profile_id) = profile_id {
        profiles::delete_profile(conn, profile_id)?;
    }
    conn.execute(
        "DELETE FROM discovery_sources WHERE merge_group = ?1",
        params![group],
    )
    .map_err(db_err)?;
    Ok(profile_id)
}

/// Puts the discovery-linked profiles at the tail of the sidebar, in
/// their source's position order; every other profile keeps its relative
/// order ahead of them. Called after a run that imported fresh profiles —
/// the fetch phase stores results in network completion order, so the
/// fresh rows would otherwise land however the parallel fetches raced.
pub(super) fn normalize_discovery_order(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "WITH per_source AS (
             -- one row per linked profile: a merge group's members share it,
             -- so aggregate them (the first member's position represents it)
             SELECT profile_id, MIN(position) AS position, MIN(id) AS source_id
             FROM discovery_sources
             WHERE profile_id IS NOT NULL
             GROUP BY profile_id
         ),
         ranked AS (
             SELECT p.id AS profile_id,
                    ROW_NUMBER() OVER (
                        ORDER BY (s.source_id IS NOT NULL) ASC,
                                 COALESCE(s.position, p.sidebar_position) ASC,
                                 p.sidebar_position ASC,
                                 p.id ASC
                    ) - 1 AS pos
             FROM profiles p
             LEFT JOIN per_source s ON s.profile_id = p.id
         )
         UPDATE profiles
         SET sidebar_position = (SELECT pos FROM ranked WHERE ranked.profile_id = profiles.id)",
        [],
    )
    .map_err(db_err)?;
    Ok(())
}

fn validate_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let parsed = url::Url::parse(trimmed).map_err(|_| format!("invalid URL: {trimmed}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("the source URL must be http(s): {trimmed}"));
    }
    Ok(trimmed.to_string())
}

fn validate_source_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("source name must not be empty".into());
    }
    Ok(trimmed.to_string())
}

fn validate_group_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("merge group must not be empty".into());
    }
    Ok(trimmed.to_string())
}

fn row_to_source(row: &rusqlite::Row) -> rusqlite::Result<DiscoverySource> {
    Ok(DiscoverySource {
        id: row.get(0)?,
        url: row.get(1)?,
        name: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        position: row.get(4)?,
        profile_id: row.get(5)?,
        last_run_at: row.get(6)?,
        last_error: row.get(7)?,
        last_item_count: row.get(8)?,
        merge_group: row.get(9)?,
    })
}

pub(super) fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "megathrone-discovery-storage-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
    }

    const SAMPLE: &str =
        "vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@1.2.3.4:443?security=tls&sni=example.com#First";

    fn sidebar_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM profiles ORDER BY sidebar_position, name COLLATE NOCASE, id")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn created_profiles_follow_their_source_order_at_the_tail() {
        let mut conn = test_db();

        // two user profiles, then three discovery ones "imported" in
        // network completion order (Omega-3, Omega-1, Omega-2)
        crate::profiles::import_profile(&mut conn, "Mine".into(), None, None, SAMPLE).unwrap();
        crate::profiles::import_profile(&mut conn, "Also mine".into(), None, None, SAMPLE)
            .unwrap();
        let third =
            crate::profiles::import_profile(&mut conn, "Omega-3".into(), None, None, SAMPLE)
                .unwrap();
        let first =
            crate::profiles::import_profile(&mut conn, "Omega-1".into(), None, None, SAMPLE)
                .unwrap();
        let second =
            crate::profiles::import_profile(&mut conn, "Omega-2".into(), None, None, SAMPLE)
                .unwrap();
        for (position, profile) in [(2, &third), (0, &first), (1, &second)] {
            conn.execute(
                "INSERT INTO discovery_sources (url, name, position, profile_id)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    format!("https://example.com/{position}"),
                    format!("Omega-{}", position + 1),
                    position,
                    profile.id
                ],
            )
            .unwrap();
        }

        // until the normalization the sidebar shows completion order
        assert_eq!(
            sidebar_names(&conn),
            vec!["Mine", "Also mine", "Omega-3", "Omega-1", "Omega-2"]
        );

        normalize_discovery_order(&conn).expect("normalize");

        // user profiles keep their relative order ahead; the discovery ones
        // follow their source's position at the tail
        assert_eq!(
            sidebar_names(&conn),
            vec!["Mine", "Also mine", "Omega-1", "Omega-2", "Omega-3"]
        );
    }

    #[test]
    fn merge_group_members_share_one_profile_slot_in_the_sidebar() {
        let mut conn = test_db();

        // one plain discovery profile, then one shared by a 7-member group
        // whose members sit at positions 1..=7
        let plain =
            crate::profiles::import_profile(&mut conn, "Omega-1".into(), None, None, SAMPLE)
                .unwrap();
        let shared =
            crate::profiles::import_profile(&mut conn, "Omega-2".into(), None, None, SAMPLE)
                .unwrap();
        conn.execute(
            "INSERT INTO discovery_sources (url, name, position, profile_id)
             VALUES ('https://example.com/0', 'Omega-1', 0, ?1)",
            params![plain.id],
        )
        .unwrap();
        for position in 1..=7 {
            conn.execute(
                "INSERT INTO discovery_sources (url, name, position, merge_group, profile_id)
                 VALUES (?1, ?2, ?3, 'whitelists', ?4)",
                params![
                    format!("https://example.com/{position}"),
                    format!("Omega-{}", position + 1),
                    position,
                    shared.id
                ],
            )
            .unwrap();
        }

        normalize_discovery_order(&conn).expect("normalize");

        // the shared profile occupies ONE slot — its first member's — and
        // positions stay unique across profiles
        let rows: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare("SELECT id, name FROM profiles ORDER BY sidebar_position")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(rows, vec![(plain.id, "Omega-1".into()), (shared.id, "Omega-2".into())]);
        let positions: Vec<i64> = {
            let mut stmt = conn.prepare("SELECT sidebar_position FROM profiles").unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(positions.len(), 2);
        assert_ne!(positions[0], positions[1]);
    }

    #[test]
    fn delete_keeps_a_merge_groups_shared_profile_until_its_last_member() {
        let mut conn = test_db();
        let profile =
            crate::profiles::import_profile(&mut conn, "Omega-26".into(), None, None, SAMPLE)
                .unwrap();
        let first = add_source(
            &conn,
            "https://example.com/a",
            Some("Omega-26"),
            Some("whitelists"),
        )
        .unwrap();
        let second = add_source(
            &conn,
            "https://example.com/b",
            Some("Omega-27"),
            Some("whitelists"),
        )
        .unwrap();
        conn.execute(
            "UPDATE discovery_sources SET profile_id = ?1 WHERE id IN (?2, ?3)",
            params![profile.id, first.id, second.id],
        )
        .unwrap();

        // deleting a member (even with the checkbox) keeps the shared profile
        assert_eq!(delete_source(&conn, first.id, true).unwrap(), None);
        assert!(conn
            .query_row("SELECT COUNT(*) FROM profiles WHERE id = ?1", params![profile.id], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
            == 1);

        // the last member's delete takes the profile with it
        assert_eq!(delete_source(&conn, second.id, true).unwrap(), Some(profile.id));
        assert!(conn
            .query_row("SELECT COUNT(*) FROM profiles WHERE id = ?1", params![profile.id], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
            == 0);
    }

    #[test]
    fn delete_group_removes_every_member_and_the_shared_profile() {
        let mut conn = test_db();
        let profile =
            crate::profiles::import_profile(&mut conn, "Omega-26".into(), None, None, SAMPLE)
                .unwrap();
        for letter in ["a", "b", "c"] {
            let source = add_source(
                &conn,
                &format!("https://example.com/{letter}"),
                Some("Omega-26"),
                Some("whitelists"),
            )
            .unwrap();
            conn.execute(
                "UPDATE discovery_sources SET profile_id = ?1 WHERE id = ?2",
                params![profile.id, source.id],
            )
            .unwrap();
        }
        add_source(&conn, "https://example.com/plain", None, None).unwrap();

        assert_eq!(delete_group(&conn, "whitelists").unwrap(), Some(profile.id));
        let remaining: Vec<String> = {
            let mut stmt = conn.prepare("SELECT url FROM discovery_sources").unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(remaining, vec!["https://example.com/plain".to_string()]);
        assert!(conn
            .query_row("SELECT COUNT(*) FROM profiles WHERE id = ?1", params![profile.id], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
            == 0);

        // an unknown group errors instead of deleting silently
        assert!(delete_group(&conn, "no-such-group").is_err());
    }
}
