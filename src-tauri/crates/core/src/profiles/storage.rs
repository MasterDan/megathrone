use rusqlite::{params, Connection};

use crate::parser::{self, ParsedProfile};

use super::model::ProfileSummary;

const PROFILE_COLUMNS: &str = "id, name, source_url, source_path, source_discovery, \
     auto_update_minutes, last_fetched_at, item_count, skipped_count, created_at, updated_at, \
     sidebar_visible, sidebar_position";

// ---------------------------------------------------------------------------
// Storage layer
// ---------------------------------------------------------------------------

pub(super) fn list_profiles(conn: &Connection) -> Result<Vec<ProfileSummary>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {PROFILE_COLUMNS} FROM profiles
             ORDER BY sidebar_position ASC, name COLLATE NOCASE ASC, id ASC"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], row_to_summary)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

pub(crate) fn import_profile(
    conn: &mut Connection,
    name: String,
    source_url: Option<String>,
    source_path: Option<String>,
    content: &str,
) -> Result<ProfileSummary, String> {
    let parsed: ParsedProfile = parser::parse_subscription(content);
    if parsed.endpoints.is_empty() {
        return Err("no supported endpoints found in the provided content".into());
    }

    let tx = conn.transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO profiles
         (name, source_url, source_path, item_count, skipped_count, last_fetched_at,
          sidebar_position)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'),
                 (SELECT COALESCE(MAX(sidebar_position) + 1, 0) FROM profiles))",
        params![name, source_url, source_path, parsed.endpoints.len() as i64, parsed.skipped as i64],
    )
    .map_err(db_err)?;
    let profile_id = tx.last_insert_rowid();

    insert_endpoints(&tx, profile_id, &parsed)?;
    tx.commit().map_err(db_err)?;
    summary_by_id(conn, profile_id)
}

fn insert_endpoints(conn: &Connection, profile_id: i64, parsed: &ParsedProfile) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "INSERT INTO endpoints
             (profile_id, tag, protocol, server, server_port, raw, outbound_json, order_index)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(db_err)?;
    for (index, endpoint) in parsed.endpoints.iter().enumerate() {
        stmt.execute(params![
            profile_id,
            endpoint.tag,
            endpoint.protocol,
            endpoint.server,
            endpoint.server_port as i64,
            endpoint.raw,
            endpoint.outbound_json,
            index as i64,
        ])
        .map_err(db_err)?;
    }
    Ok(())
}

/// The Discovery run's import: the same shape as `import_profile` but a
/// `source_discovery` profile — no self-update source, the Discovery
/// catalog owns its content.
pub(crate) fn import_discovery_profile(
    conn: &mut Connection,
    name: String,
    content: &str,
) -> Result<ProfileSummary, String> {
    let parsed: ParsedProfile = parser::parse_subscription(content);
    if parsed.endpoints.is_empty() {
        return Err("no supported endpoints found in the provided content".into());
    }

    let tx = conn.transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO profiles
         (name, source_discovery, item_count, skipped_count, last_fetched_at,
          sidebar_position)
         VALUES (?1, 1, ?2, ?3, datetime('now'),
                 (SELECT COALESCE(MAX(sidebar_position) + 1, 0) FROM profiles))",
        params![name, parsed.endpoints.len() as i64, parsed.skipped as i64],
    )
    .map_err(db_err)?;
    let profile_id = tx.last_insert_rowid();

    insert_endpoints(&tx, profile_id, &parsed)?;
    tx.commit().map_err(db_err)?;
    summary_by_id(conn, profile_id)
}

pub(super) fn rename_profile(conn: &Connection, profile_id: i64, new_name: &str) -> Result<(), String> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err("profile name must not be empty".into());
    }
    let affected = conn
        .execute(
            "UPDATE profiles SET name = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![profile_id, new_name],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(format!("profile {profile_id} not found"));
    }
    Ok(())
}

pub(crate) fn delete_profile(conn: &Connection, profile_id: i64) -> Result<(), String> {
    // endpoints are removed by the FK cascade
    conn.execute("DELETE FROM profiles WHERE id = ?1", params![profile_id])
        .map_err(db_err)?;
    Ok(())
}

/// Bulk delete (Settings → Profiles): every listed profile in one
/// transaction; endpoints and their results go with the FK cascades.
/// Returns how many rows were actually removed — unknown ids are skipped
/// so a concurrent delete cannot fail the whole batch.
pub(super) fn delete_profiles(conn: &mut Connection, profile_ids: &[i64]) -> Result<usize, String> {
    if profile_ids.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction().map_err(db_err)?;
    let mut deleted = 0usize;
    for id in profile_ids {
        deleted += tx
            .execute("DELETE FROM profiles WHERE id = ?1", params![id])
            .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(deleted)
}

/// Hides/shows a profile in the desktop sidebar (Settings → Profiles).
pub(super) fn set_sidebar_visible(
    conn: &Connection,
    profile_id: i64,
    visible: bool,
) -> Result<(), String> {
    let affected = conn
        .execute(
            "UPDATE profiles SET sidebar_visible = ?2, updated_at = datetime('now')
             WHERE id = ?1",
            params![profile_id, visible as i64],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(format!("profile {profile_id} not found"));
    }
    Ok(())
}

/// Persists an explicit sidebar order: the ids are written as positions
/// 0..N in the order given (the drag-and-drop list's exact order; ids the
/// caller leaves out keep their old positions). Ids that no longer exist
/// are ignored — a concurrent delete must not fail the drag.
pub(super) fn reorder_profiles(conn: &Connection, ordered_ids: &[i64]) -> Result<(), String> {
    let mut stmt = conn
        .prepare("UPDATE profiles SET sidebar_position = ?2 WHERE id = ?1")
        .map_err(db_err)?;
    for (position, id) in ordered_ids.iter().enumerate() {
        stmt.execute(params![id, position as i64]).map_err(db_err)?;
    }
    Ok(())
}

const AUTO_UPDATE_CHOICES: [i64; 5] = [30, 60, 360, 720, 1440];

pub(super) fn set_auto_update(conn: &Connection, profile_id: i64, minutes: Option<i64>) -> Result<(), String> {
    if let Some(minutes) = minutes {
        if !AUTO_UPDATE_CHOICES.contains(&minutes) {
            return Err(format!("unsupported auto-update interval: {minutes} minutes"));
        }
    }
    if minutes.is_some() {
        let (is_discovery, has_source): (bool, bool) = conn
            .query_row(
                "SELECT source_discovery != 0,
                        (source_url IS NOT NULL AND source_url != '')
                     OR (source_path IS NOT NULL AND source_path != '')
                 FROM profiles WHERE id = ?1",
                params![profile_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(db_err)?;
        if is_discovery {
            return Err("a Discovery profile is refreshed by the Discovery run".into());
        }
        if !has_source {
            return Err("auto-update requires a profile with a URL or file source".into());
        }
    }
    let affected = conn
        .execute(
            "UPDATE profiles SET auto_update_minutes = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![profile_id, minutes],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(format!("profile {profile_id} not found"));
    }
    Ok(())
}

/// Counts one more failed scheduled update and returns the new streak.
/// `due_profiles`/`next_wake_seconds` ignore the profile once the streak
/// reaches `MAX_AUTO_UPDATE_FAILURES`; `apply_update` clears it.
pub(super) fn bump_auto_update_failures(conn: &Connection, profile_id: i64) -> Result<i64, String> {
    conn.query_row(
        "UPDATE profiles SET auto_update_failures = auto_update_failures + 1
         WHERE id = ?1 RETURNING auto_update_failures",
        params![profile_id],
        |row| row.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
        other => db_err(other),
    })
}

/// After this many consecutive failed *scheduled* updates the scheduler
/// ignores the profile's auto-update schedule; any successful update
/// (the manual Update button included) resets the streak.
pub const MAX_AUTO_UPDATE_FAILURES: i64 = 3;

/// Profiles whose auto-update interval has elapsed since the last fetch.
/// Profiles paused by too many consecutive failed scheduled updates are
/// skipped — a successful update (the manual button included) unpauses.
pub fn due_profiles(conn: &Connection) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT id FROM profiles
             WHERE auto_update_minutes IS NOT NULL
               AND source_discovery = 0
               AND last_fetched_at IS NOT NULL
               AND auto_update_failures < {MAX_AUTO_UPDATE_FAILURES}
               AND (julianday('now') - julianday(last_fetched_at)) * 1440.0 >= auto_update_minutes
             ORDER BY id"
        ))
        .map_err(db_err)?;
    let ids = stmt
        .query_map([], |row| row.get(0))
        .map_err(db_err)?
        .collect::<Result<Vec<i64>, _>>()
        .map_err(db_err)?;
    Ok(ids)
}

/// Seconds until the next scheduled auto-update (can be negative when due;
/// the scheduler clamps). None when nothing is scheduled. Paused profiles
/// must not hold the wake target at "overdue" forever, so they are excluded
/// here too.
pub fn next_wake_seconds(conn: &Connection) -> Result<Option<f64>, String> {
    conn.query_row(
        &format!(
             "SELECT MIN((julianday(last_fetched_at, '+' || auto_update_minutes || ' minutes')
                          - julianday('now')) * 86400.0)
             FROM profiles
             WHERE auto_update_minutes IS NOT NULL
               AND source_discovery = 0
               AND last_fetched_at IS NOT NULL
               AND auto_update_failures < {MAX_AUTO_UPDATE_FAILURES}
               AND ((source_url IS NOT NULL AND source_url != '')
                    OR (source_path IS NOT NULL AND source_path != ''))"
        ),
        [],
        |row| row.get::<_, Option<f64>>(0),
    )
    .map_err(db_err)
}

pub(super) fn summary_by_id(conn: &Connection, profile_id: i64) -> Result<ProfileSummary, String> {
    conn.query_row(
        &format!("SELECT {PROFILE_COLUMNS} FROM profiles WHERE id = ?1"),
        params![profile_id],
        row_to_summary,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
        other => db_err(other),
    })
}

fn row_to_summary(row: &rusqlite::Row) -> rusqlite::Result<ProfileSummary> {
    Ok(ProfileSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        source_url: row.get(2)?,
        source_path: row.get(3)?,
        source_discovery: row.get::<_, i64>(4)? != 0,
        auto_update_minutes: row.get(5)?,
        last_fetched_at: row.get(6)?,
        item_count: row.get(7)?,
        skipped_count: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        sidebar_visible: row.get::<_, i64>(11)? != 0,
        sidebar_position: row.get(12)?,
    })
}

pub(super) fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}
