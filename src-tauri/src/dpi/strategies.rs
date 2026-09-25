//! DPI-bypass strategies for the byedpi (ciadpi) sidecar.
//!
//! A strategy is literally the argument line handed to the `ciadpi` binary
//! (e.g. `-s2 -d2`); the app manages the listener itself (`-i 127.0.0.1
//! -p <port>`), so the strategy line must not fight over those. Exactly one
//! strategy may be active — it is the one used whenever the proxy session
//! needs the DPI tunnel.

use rusqlite::{Connection, params};
use serde::Serialize;

/// Room for the bundled preset catalog (see `PRESET_STRATEGIES`) plus a
/// healthy number of the user's own lines.
pub(super) const MAX_STRATEGIES: usize = 96;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DpiStrategy {
    pub id: i64,
    pub name: String,
    /// raw argument line, as typed by the user
    pub args: String,
    pub is_active: bool,
    /// test sites reachable through this strategy / configured total
    /// (filled from the last strategy test; `tested == 0` — never tested)
    pub url_ok: usize,
    pub url_total: usize,
    pub tested: usize,
}

// ---------------------------------------------------------------------------
// Storage layer
// ---------------------------------------------------------------------------

fn row_to_strategy(row: &rusqlite::Row<'_>) -> rusqlite::Result<DpiStrategy> {
    Ok(DpiStrategy {
        id: row.get(0)?,
        name: row.get(1)?,
        args: row.get(2)?,
        is_active: row.get::<_, i64>(3)? != 0,
        url_ok: row.get::<_, i64>(4)? as usize,
        url_total: row.get::<_, i64>(5)? as usize,
        tested: row.get::<_, i64>(6)? as usize,
    })
}

/// Columns every strategy SELECT must produce, in `row_to_strategy` order:
/// identity + the test-site counters (last test's reachable rules, the
/// configured total and how many rules were probed at all). Only *dpi*
/// routed categories count — and only rules marked for testing that can be
/// probed at all (`url`, `domain_suffix`, `domain`); the others neither
/// get probed nor dilute the rate.
const STRATEGY_COLUMNS: &str = "s.id, s.name, s.args, s.is_active, \
     (SELECT COUNT(*) FROM dpi_url_results r \
        JOIN test_sites u ON u.id = r.url_id \
        JOIN test_site_categories c ON c.id = u.category_id \
        WHERE r.strategy_id = s.id AND r.ok = 1 AND c.action = 'dpi'), \
     (SELECT COUNT(*) FROM test_sites u \
        JOIN test_site_categories c ON c.id = u.category_id \
        WHERE c.action = 'dpi' AND u.test_enabled = 1 \
          AND u.rule_type IN ('url', 'domain_suffix', 'domain')), \
     (SELECT COUNT(*) FROM dpi_url_results r WHERE r.strategy_id = s.id)";

pub fn list_strategies(conn: &Connection) -> Result<Vec<DpiStrategy>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {STRATEGY_COLUMNS} FROM dpi_strategies s ORDER BY s.id"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], row_to_strategy)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

/// The one strategy marked active, if any.
pub fn load_active_strategy(conn: &Connection) -> Result<Option<DpiStrategy>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {STRATEGY_COLUMNS} FROM dpi_strategies s WHERE s.is_active = 1"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], row_to_strategy)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows.into_iter().next())
}

pub(super) fn add_strategy(conn: &Connection, name: &str, args: &[String]) -> Result<DpiStrategy, String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM dpi_strategies", [], |row| row.get(0))
        .map_err(db_err)?;
    if count as usize >= MAX_STRATEGIES {
        return Err(format!("at most {MAX_STRATEGIES} strategies can be stored"));
    }
    conn.execute(
        "INSERT INTO dpi_strategies (name, args) VALUES (?1, ?2)",
        params![name, args.join(" ")],
    )
    .map_err(db_err)?;
    Ok(DpiStrategy {
        id: conn.last_insert_rowid(),
        name: name.to_string(),
        args: args.join(" "),
        is_active: false,
        url_ok: 0,
        url_total: 0,
        tested: 0,
    })
}

pub(super) fn update_strategy(conn: &Connection, id: i64, name: &str, args: &[String]) -> Result<DpiStrategy, String> {
    let updated = conn
        .execute(
            "UPDATE dpi_strategies SET name = ?2, args = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![id, name, args.join(" ")],
        )
        .map_err(db_err)?;
    if updated == 0 {
        return Err(format!("strategy {id} not found"));
    }
    conn.query_row(
        &format!("SELECT {STRATEGY_COLUMNS} FROM dpi_strategies s WHERE s.id = ?1"),
        params![id],
        row_to_strategy,
    )
    .map_err(db_err)
}

pub(super) fn delete_strategy(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM dpi_strategies WHERE id = ?1", params![id])
        .map_err(db_err)?;
    Ok(())
}

pub(super) fn select_strategy(conn: &Connection, id: i64) -> Result<(), String> {
    // the flag-flipping UPDATE touches every row (by design), so existence
    // is checked separately to keep "unknown id" an error
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dpi_strategies WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if exists == 0 {
        return Err(format!("strategy {id} not found"));
    }
    conn.execute(
        "UPDATE dpi_strategies SET is_active = CASE WHEN id = ?1 THEN 1 ELSE 0 END,
         updated_at = datetime('now')",
        params![id],
    )
    .map_err(db_err)?;
    Ok(())
}

pub(super) fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}
