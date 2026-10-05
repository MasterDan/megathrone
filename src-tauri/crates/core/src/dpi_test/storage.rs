use rusqlite::{Connection, params};
use serde::Serialize;
use tauri::State;

use crate::AppState;
use crate::sites;

/// One strategy's per-site outcomes, grouped by category (details popover
/// on the Settings → DPI tab); `ok` is null for sites never probed. Only
/// dpi-routed categories are listed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiSiteStatus {
    pub category_id: i64,
    pub category: String,
    pub url_id: i64,
    pub url: String,
    pub ok: Option<bool>,
}

#[tauri::command]
pub fn dpi_strategy_urls(state: State<AppState>, strategy_id: i64) -> Result<Vec<DpiSiteStatus>, String> {
    let conn = lock_db(&state)?;
    strategy_url_statuses(&conn, strategy_id)
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// Persists one strategy's full site sweep in a single lock (upserts — a
/// re-test overwrites the previous sweep). A strategy deleted while its
/// test was running is simply skipped.
pub(super) fn store_strategy_results(conn: &Connection, strategy_id: i64, results: &[(i64, bool)]) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM dpi_strategies WHERE id = ?1)",
            params![strategy_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !exists {
        return Ok(());
    }
    let mut stmt = conn
        .prepare(
            "INSERT INTO dpi_url_results (strategy_id, url_id, ok, tested_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(strategy_id, url_id) DO UPDATE SET
                 ok = excluded.ok,
                 tested_at = excluded.tested_at",
        )
        .map_err(db_err)?;
    for (url_id, ok) in results {
        stmt.execute(params![strategy_id, url_id, *ok as i64]).map_err(db_err)?;
    }
    Ok(())
}

fn strategy_url_statuses(conn: &Connection, strategy_id: i64) -> Result<Vec<DpiSiteStatus>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name, s.id, s.rule_type, s.value, r.ok
             FROM test_site_categories c
             JOIN test_sites s ON s.category_id = c.id
             LEFT JOIN dpi_url_results r
                    ON r.url_id = s.id AND r.strategy_id = ?1
             WHERE c.action = 'dpi'
             ORDER BY c.position, c.id, s.id",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![strategy_id], |row| {
            let rule_type: String = row.get(3)?;
            let value: String = row.get(4)?;
            Ok(DpiSiteStatus {
                category_id: row.get(0)?,
                category: row.get(1)?,
                url_id: row.get(2)?,
                // what the test actually fetched (keyword/regex rules are
                // listed with their raw pattern — they never get probed)
                url: sites::probe_url(&rule_type, &value).unwrap_or(value),
                ok: row.get::<_, Option<i64>>(5)?.map(|value| value != 0),
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

fn lock_db<'a>(state: &'a State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}

fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn test_db() -> Connection {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "megathrone-dpitest-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
    }

    fn seed_sites(conn: &Connection) -> (i64, i64, i64, i64) {
        conn.execute(
            "INSERT INTO test_site_categories (name, position, action)
             VALUES ('Cat A', 0, 'dpi'), ('Cat B', 1, 'dpi'), ('Cat P', 2, 'proxy')",
            [],
        )
        .expect("seed categories");
        conn.execute(
            "INSERT INTO test_sites (category_id, rule_type, value) VALUES
                (1, 'url', 'https://a1.example/'), (1, 'url', 'https://a2.example/'),
                (2, 'url', 'https://b1.example/'), (3, 'url', 'https://p1.example/')",
            [],
        )
        .expect("seed sites");
        conn.execute(
            "INSERT INTO dpi_strategies (name, args) VALUES ('S', '-s2')",
            [],
        )
        .expect("seed strategy");
        let strategy_id = conn.last_insert_rowid();
        let url_ids: Vec<i64> = {
            let mut stmt = conn.prepare("SELECT id FROM test_sites ORDER BY id").unwrap();
            stmt.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
        };
        (strategy_id, url_ids[0], url_ids[2], url_ids[3])
    }

    #[test]
    fn store_results_upserts_and_reports_by_category() {
        let conn = test_db();
        let (strategy_id, first, third, proxy_site) = seed_sites(&conn);

        store_strategy_results(&conn, strategy_id, &[(first, true), (third, false)]).expect("store");
        // a re-test overwrites the same pairs
        store_strategy_results(&conn, strategy_id, &[(first, false), (third, true)]).expect("store");

        let statuses = strategy_url_statuses(&conn, strategy_id).expect("statuses");
        assert_eq!(statuses.len(), 3, "only dpi-routed category sites are listed");
        assert_eq!(statuses[0].category, "Cat A");
        assert_eq!(statuses[2].category, "Cat B");
        assert!(!statuses.iter().any(|status| status.url_id == proxy_site));
        let by_url: HashMap<i64, Option<bool>> =
            statuses.iter().map(|status| (status.url_id, status.ok)).collect();
        assert_eq!(by_url.get(&first), Some(&Some(false)), "latest sweep wins");
        assert_eq!(by_url.get(&third), Some(&Some(true)));
        assert_eq!(by_url[&(first + 1)], None, "unprobed site reports null");

        // strategy deletion cascades the results away
        conn.execute("DELETE FROM dpi_strategies WHERE id = ?1", params![strategy_id]).unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM dpi_url_results", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
