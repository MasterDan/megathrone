use std::collections::HashSet;

use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LatencyResult {
    pub(super) id: i64,
    pub(super) available: bool,
    pub(super) latency_ms: Option<i64>,
    /// category URLs reachable through this endpoint / probed total (0/0 —
    /// the endpoint failed the base probe, or nothing is marked for testing)
    pub(super) url_ok: usize,
    pub(super) url_total: usize,
}

impl LatencyResult {
    pub(super) fn failed(id: i64) -> Self {
        Self { id, available: false, latency_ms: None, url_ok: 0, url_total: 0 }
    }
}

/// An endpoint's stored outbound, ready to embed into the test config.
pub type OutboundRow = (i64, Value);

/// Splits a profile's endpoints into testable outbounds (valid JSON with a
/// type) and ids whose stored outbound is unusable (reported as unavailable).
pub fn load_outbounds(
    conn: &Connection,
    profile_id: i64,
) -> Result<(Vec<OutboundRow>, Vec<i64>), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM profiles WHERE id = ?1)",
            params![profile_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !exists {
        return Err(format!("profile {profile_id} not found"));
    }

    let mut stmt = conn
        .prepare("SELECT id, outbound_json FROM endpoints WHERE profile_id = ?1 ORDER BY order_index")
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![profile_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let mut outbounds = Vec::new();
    let mut untestable = Vec::new();
    for (id, raw) in rows {
        let parsed = serde_json::from_str::<Value>(&raw).ok().filter(|outbound| {
            outbound
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| !kind.is_empty())
        });
        match parsed {
            Some(outbound) => outbounds.push((id, outbound)),
            None => untestable.push(id),
        }
    }
    Ok((outbounds, untestable))
}

/// Ids of the endpoints the last scan proved reachable — the re-check
/// scope's filter.
pub(super) fn load_reachable_ids(conn: &Connection, profile_id: i64) -> Result<HashSet<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM endpoints WHERE profile_id = ?1 AND available = 1")
        .map_err(db_err)?;
    let ids = stmt
        .query_map(params![profile_id], |row| row.get::<_, i64>(0))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(ids.into_iter().collect())
}

/// Ids of the endpoints no scan has ever stamped (`last_tested_at` unset)
/// — the post-update scope's filter.
pub(super) fn load_untested_ids(conn: &Connection, profile_id: i64) -> Result<HashSet<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM endpoints WHERE profile_id = ?1 AND last_tested_at IS NULL")
        .map_err(db_err)?;
    let ids = stmt
        .query_map(params![profile_id], |row| row.get::<_, i64>(0))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(ids.into_iter().collect())
}

/// Persists one tested batch in a single DB lock: base results, deep-probe
/// upserts, and stale URL rows cleanup for endpoints that failed the base
/// probe (their previous scores are meaningless now).
pub(super) fn store_batch(
    app: &AppHandle,
    results: &[LatencyResult],
    url_probes: &[UrlProbe],
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
    apply_results(&conn, results)?;
    apply_url_probes(&conn, url_probes)?;
    for result in results.iter().filter(|result| !result.available || result.url_total == 0) {
        conn.execute(
            "DELETE FROM endpoint_url_results WHERE endpoint_id = ?1",
            params![result.id],
        )
        .map_err(db_err)?;
    }
    Ok(())
}

fn apply_results(conn: &Connection, results: &[LatencyResult]) -> Result<(), String> {
    for result in results {
        // endpoints deleted by a concurrent profile update are simply gone;
        // zero affected rows is fine here
        conn.execute(
            "UPDATE endpoints
             SET available = ?2, latency_ms = ?3, last_tested_at = datetime('now')
             WHERE id = ?1",
            params![result.id, result.available, result.latency_ms],
        )
        .map_err(db_err)?;
    }
    Ok(())
}

/// (endpoint id, test url id, measured delay through the endpoint;
/// `None` = the URL is unreachable through it)
pub(crate) type UrlProbe = (i64, i64, Option<i64>);

pub(super) fn apply_url_probes(conn: &Connection, probes: &[UrlProbe]) -> Result<(), String> {
    // a profile update or delete can race the scan and remove endpoint
    // rows mid-flight — skip probes whose endpoint no longer exists
    // instead of failing the whole batch on the FK violation
    let mut exists = conn
        .prepare("SELECT EXISTS(SELECT 1 FROM endpoints WHERE id = ?1)")
        .map_err(db_err)?;
    let mut stmt = conn
        .prepare(
            "INSERT INTO endpoint_url_results
                (endpoint_id, url_id, available, latency_ms, tested_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(endpoint_id, url_id) DO UPDATE SET
                 available = excluded.available,
                 latency_ms = excluded.latency_ms,
                 tested_at = excluded.tested_at",
        )
        .map_err(db_err)?;
    for (endpoint_id, url_id, delay) in probes {
        let live: bool = exists
            .query_row(params![endpoint_id], |row| row.get(0))
            .map_err(db_err)?;
        if !live {
            continue;
        }
        stmt.execute(params![endpoint_id, url_id, delay.is_some() as i64, delay])
            .map_err(db_err)?;
    }
    Ok(())
}

pub(super) fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}

#[cfg(test)]
mod tests {
    use crate::latency::latency_tests::test_db;

    use super::*;

    #[test]
    fn apply_url_probes_upsert_results() {
        let conn = test_db();
        conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 1)", [])
            .expect("seed profile");
        let profile_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
             VALUES (?1, 'A', 'vless', 'raw', '{}', 0)",
            params![profile_id],
        )
        .expect("seed endpoint");
        let endpoint_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO test_site_categories (name, position, action)
             VALUES ('Cat', 0, 'proxy')",
            [],
        )
        .expect("seed category");
        let category_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO test_sites (category_id, rule_type, value) VALUES (?1, 'url', 'https://a.example/')",
            params![category_id],
        )
        .expect("seed url");
        let url_id = conn.last_insert_rowid();

        // insert, then overwrite the same pair
        apply_url_probes(&conn, &[(endpoint_id, url_id, Some(42))]).expect("apply");
        apply_url_probes(&conn, &[(endpoint_id, url_id, None)]).expect("apply again");

        let row: (i64, Option<i64>) = conn
            .query_row(
                "SELECT available, latency_ms FROM endpoint_url_results
                 WHERE endpoint_id = ?1 AND url_id = ?2",
                params![endpoint_id, url_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read back");
        assert_eq!(row, (0, None), "the upsert overwrites the previous probe");
    }

    #[test]
    fn load_outbounds_and_apply_results_roundtrip() {
        let conn = test_db();
        conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 2)", [])
            .expect("seed profile");
        let profile_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO endpoints
             (profile_id, tag, protocol, server, server_port, raw, outbound_json, order_index)
             VALUES (?1, 'A', 'vless', '1.2.3.4', 443, 'raw', ?2, 0),
                    (?1, 'B', 'vmess', NULL, NULL, 'raw', '{}', 1)",
            params![profile_id, r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#],
        )
        .expect("seed endpoints");
        let id_a: i64 = conn
            .query_row(
                "SELECT id FROM endpoints WHERE profile_id = ?1 AND tag = 'A'",
                params![profile_id],
                |row| row.get(0),
            )
            .expect("id of A");
        let id_b: i64 = conn
            .query_row(
                "SELECT id FROM endpoints WHERE profile_id = ?1 AND tag = 'B'",
                params![profile_id],
                |row| row.get(0),
            )
            .expect("id of B");

        let (outbounds, untestable) = load_outbounds(&conn, profile_id).expect("load outbounds");
        assert_eq!(outbounds.len(), 1);
        assert_eq!(outbounds[0].0, id_a);
        assert_eq!(outbounds[0].1["type"], "vless");
        assert_eq!(untestable, vec![id_b], "outbound without a type is untestable");

        apply_results(
            &conn,
            &[
                LatencyResult { id: id_a, available: true, latency_ms: Some(42), url_ok: 0, url_total: 0 },
                LatencyResult::failed(id_b),
            ],
        )
        .expect("apply results");

        let live: (Option<i64>, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT available, latency_ms, last_tested_at FROM endpoints WHERE id = ?1",
                params![id_a],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read live endpoint");
        let (available, latency_ms, tested_at) = live;
        assert_eq!(available, Some(1));
        assert_eq!(latency_ms, Some(42));
        assert!(tested_at.is_some(), "last_tested_at must be stamped");

        let dead: (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT available, latency_ms FROM endpoints WHERE id = ?1",
                params![id_b],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read dead endpoint");
        assert_eq!(dead, (Some(0), None));
    }

    #[test]
    fn apply_url_probes_skips_removed_endpoints() {
        let conn = test_db();
        conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 1)", [])
            .expect("seed profile");
        let profile_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
             VALUES (?1, 'A', 'vless', 'raw', '{}', 0)",
            params![profile_id],
        )
        .expect("seed endpoint");
        let endpoint_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO test_site_categories (name, position, action) VALUES ('Cat', 0, 'proxy')",
            [],
        )
        .expect("seed category");
        let category_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO test_sites (category_id, rule_type, value)
             VALUES (?1, 'url', 'https://a.example/')",
            params![category_id],
        )
        .expect("seed url");
        let url_id = conn.last_insert_rowid();

        // one probe targets a live endpoint, one an endpoint a concurrent
        // profile update already removed — the batch must not fail
        let probes = vec![
            (endpoint_id, url_id, Some(42)),
            (endpoint_id + 4242, url_id, Some(7)),
        ];
        apply_url_probes(&conn, &probes).expect("dangling ids are skipped, not fatal");

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM endpoint_url_results", [], |row| row.get(0))
            .expect("count");
        assert_eq!(rows, 1, "only the live endpoint's probe landed");
    }

    #[test]
    fn load_untested_ids_selects_never_scanned_endpoints() {
        let conn = test_db();
        conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 2)", [])
            .expect("seed profile");
        let profile_id = conn.last_insert_rowid();
        for tag in ["A", "B"] {
            conn.execute(
                "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
                 VALUES (?1, ?2, 'vless', ?2, '{}', 0)",
                params![profile_id, tag],
            )
            .expect("seed endpoint");
        }
        let stale_id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE endpoints SET last_tested_at = datetime('now') WHERE id = ?1",
            params![stale_id],
        )
        .expect("stamp one endpoint");

        let untested = load_untested_ids(&conn, profile_id).expect("untested ids");
        assert_eq!(untested.len(), 1);
        assert!(!untested.contains(&stale_id), "stamped rows are excluded");
    }
}
