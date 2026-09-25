use rusqlite::{Connection, params};

use super::model::DiscoverySource;
use crate::profiles;

const SOURCE_COLUMNS: &str =
    "id, url, name, enabled, position, profile_id, last_run_at, last_error, last_item_count";

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
    conn.execute(
        "INSERT INTO discovery_sources (url, name, position) VALUES (?1, ?2, ?3)",
        params![url, name, next_position],
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
/// any) goes away too — the caller then notifies the UI with its id.
pub(super) fn delete_source(
    conn: &Connection,
    source_id: i64,
    delete_profile: bool,
) -> Result<Option<i64>, String> {
    let existing = source_by_id(conn, source_id)?;
    let deleted_profile = match (delete_profile, existing.profile_id) {
        (true, Some(profile_id)) => {
            profiles::delete_profile(conn, profile_id)?;
            Some(profile_id)
        }
        _ => None,
    };
    conn.execute("DELETE FROM discovery_sources WHERE id = ?1", params![source_id])
        .map_err(db_err)?;
    Ok(deleted_profile)
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
    })
}

pub(super) fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}
