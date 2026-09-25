use std::path::Path;

use rusqlite::Connection;

use super::migrations::Migrations;

pub fn open(path: &Path, migrations: &Migrations) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn, migrations)?;
    Ok(conn)
}

fn migrate(conn: &Connection, migrations: &Migrations) -> rusqlite::Result<()> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    // Databases from before the file-based migrator (anything under v8,
    // including version-0 schemas that never tracked a user_version) are
    // brought to the v1 baseline shape by the tolerant heal first — every
    // file from v8 on can then assume an exact starting schema. A fresh
    // database is completely empty and replays the files from v1.
    if version < 8 && legacy_tables_exist(conn)? {
        heal_pre_file(conn, migrations)?;
        version = 1;
    }
    // The loop: find the next file past the current version, apply it in a
    // transaction (schema + the user_version bump commit atomically — a
    // failed migration rolls back and retries on the next open), repeat.
    while let Some(migration) = migrations.after(version) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(&migration.sql)?;
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;
        version = migration.version;
    }
    Ok(())
}

/// Any table from the app's history present means the database predates
/// the file migrator (fresh databases contain nothing at all).
fn legacy_tables_exist(conn: &Connection) -> rusqlite::Result<bool> {
    for table in [
        "profiles",
        "endpoints",
        "dpi_strategies",
        "app_settings",
        "test_site_categories",
        "test_sites",
        "test_urls",
        "route_rules",
    ] {
        if table_exists(conn, table)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Brings any pre-file database to exactly the v1 baseline shape: the
/// baseline's idempotent creates, the profiles columns SQLite has no
/// "ADD COLUMN IF NOT EXISTS" for, and the legacy last_fetched_at
/// backfill. Safe to re-run (a crash mid-heal simply heals again).
fn heal_pre_file(conn: &Connection, migrations: &Migrations) -> rusqlite::Result<()> {
    let baseline = migrations
        .find(1)
        .map(|migration| migration.sql.as_str())
        .ok_or_else(|| {
            rusqlite::Error::InvalidParameterName("the v1 baseline migration is missing".into())
        })?;
    conn.execute_batch(baseline)?;
    add_column_if_missing(conn, "profiles", "source_path", "TEXT")?;
    add_column_if_missing(conn, "profiles", "auto_update_minutes", "INTEGER")?;
    add_column_if_missing(conn, "profiles", "last_fetched_at", "TEXT")?;
    add_column_if_missing(
        conn,
        "profiles",
        "auto_update_failures",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(conn, "profiles", "selected_endpoint_key", "TEXT")?;
    conn.execute_batch(
        "UPDATE profiles SET last_fetched_at = COALESCE(last_fetched_at, updated_at, datetime('now'));",
    )?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> rusqlite::Result<()> {
    if !column_exists(conn, table, column)? {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {declaration};"))?;
    }
    Ok(())
}

pub(super) fn table_exists(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    let exists = stmt
        .query_map([table], |_| Ok(()))?
        .next()
        .transpose()?
        .is_some();
    Ok(exists)
}

pub(super) fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    Ok(exists)
}
