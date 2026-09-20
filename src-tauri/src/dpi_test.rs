//! DPI strategy testing — the Test button on the Settings → DPI tab.
//!
//! For every stored strategy a throwaway ciadpi instance is spawned on a
//! random local port with that strategy's argument line, fronted by a
//! throwaway sing-box instance (a single socks outbound pointing at ciadpi,
//! clash API enabled). Each site of every *dpi*-routed URL category
//! (Settings → Routing) is then fetched through the tunnel via the clash
//! `delay` endpoint — the same mechanism the endpoint availability scan
//! uses — and the per-site outcomes land in `dpi_url_results`, one pass
//! rate per strategy. A process-wide registry backs `dpi_test_status`
//! (adopting a running test after remount) and `dpi_cancel_test` (checked
//! between strategies).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shell::ShellExt;

use crate::AppState;
use crate::connection;
use crate::dpi;
use crate::latency;
use crate::routing;
use crate::sites;

/// Per-site budget for one request through the DPI tunnel.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

pub const DPI_TEST_PROGRESS_EVENT: &str = "dpi-test-progress";
/// Emitted once when the whole test ends (finished, cancelled or failed) —
/// a page that adopted a running test learns to release its button.
pub const DPI_TEST_FINISHED_EVENT: &str = "dpi-test-finished";

/// The outbound tag `build_test_config` assigns to the first (only)
/// outbound — the socks tunnel to ciadpi.
const TUNNEL_TAG: &str = "mt-0";

// ---------------------------------------------------------------------------
// Test registry (one running test per app)
// ---------------------------------------------------------------------------

struct TestRun {
    done: usize,
    total: usize,
    cancel: Arc<AtomicBool>,
}

static TEST: LazyLock<Mutex<Option<TestRun>>> = LazyLock::new(|| Mutex::new(None));

fn lock_test() -> Result<MutexGuard<'static, Option<TestRun>>, String> {
    TEST.lock().map_err(|_| "test registry poisoned".to_string())
}

/// Removes the registry entry when the test ends for any reason, including
/// early returns and panics.
struct TestGuard;

impl Drop for TestGuard {
    fn drop(&mut self) {
        if let Ok(mut test) = TEST.lock() {
            *test = None;
        }
    }
}

fn begin_test(total: usize) -> Result<Arc<AtomicBool>, String> {
    let mut test = lock_test()?;
    if test.is_some() {
        return Err("a DPI strategy test is already running".to_string());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *test = Some(TestRun { done: 0, total, cancel: cancel.clone() });
    Ok(cancel)
}

fn advance_test(done: usize) {
    if let Ok(mut test) = TEST.lock() {
        if let Some(run) = test.as_mut() {
            run.done = done;
        }
    }
}

fn request_cancel() -> Result<bool, String> {
    let test = lock_test()?;
    match test.as_ref() {
        Some(run) => {
            run.cancel.store(true, Ordering::Relaxed);
            Ok(true)
        }
        None => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// Payloads & commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiTestResult {
    pub strategy_id: i64,
    /// sites reachable through the strategy / configured total
    pub url_ok: usize,
    pub url_total: usize,
    /// set when the strategy never got a tunnel up (bad flag combination,
    /// sidecar failure) — no rows are stored in that case
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiTestProgress {
    pub done: usize,
    pub total: usize,
    pub results: Vec<DpiTestResult>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DpiTestStatus {
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiTestSummary {
    /// strategies actually tested (a cancelled run reports less)
    pub done: usize,
    pub total: usize,
}

/// Snapshot of the running test; `None` means no test is running.
#[tauri::command]
pub fn dpi_test_status() -> Result<Option<DpiTestStatus>, String> {
    let test = lock_test()?;
    Ok(test
        .as_ref()
        .map(|run| DpiTestStatus { done: run.done, total: run.total }))
}

/// Stops a running test after the current strategy; idempotent.
#[tauri::command]
pub fn dpi_cancel_test() -> Result<bool, String> {
    request_cancel()
}

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

/// Runs every stored strategy against every configured site, one ciadpi +
/// sing-box pair at a time. Per-strategy results are stored and announced
/// via `dpi-test-progress` as they arrive.
#[tauri::command]
pub async fn dpi_test_strategies(app: AppHandle) -> Result<DpiTestSummary, String> {
    let (strategies, site_list) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
        (dpi::list_strategies(&conn)?, sites::flat_test_rules(&conn, routing::ACTION_DPI)?)
    };
    if strategies.is_empty() {
        return Err("no strategies to test — add one first".to_string());
    }
    if site_list.is_empty() {
        return Err(
            "no DPI-routed rules to test — add test-marked rules to a DPI-routed category in \
             Settings → Routing"
                .to_string(),
        );
    }
    if connection::tun_active() {
        return Err(
            "disconnect the proxy first — while TUN mode is running it captures the test \
             traffic, and every strategy would look equally good"
                .to_string(),
        );
    }
    let url_total = site_list.len();

    let total = strategies.len();
    let cancel = begin_test(total)?;
    let _guard = TestGuard;

    let mut done = 0usize;
    let mut failure: Option<String> = None;
    for strategy in &strategies {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let result = test_strategy(&app, strategy.id, &strategy.args, &site_list).await;

        // a completed sweep is stored and announced even when the user asked
        // to stop mid-run; the top-of-loop check then ends the test
        if result.error.is_none() {
            let ok_ids: Vec<(i64, bool)> =
                result.probes.iter().map(|(url_id, ok)| (*url_id, *ok)).collect();
            let stored = app
                .state::<AppState>()
                .db
                .lock()
                .map_err(|_| "database lock poisoned".to_string())
                .and_then(|conn| store_strategy_results(&conn, strategy.id, &ok_ids));
            if let Err(error) = stored {
                failure = Some(error);
                break;
            }
        }

        let announced = DpiTestResult {
            strategy_id: strategy.id,
            url_ok: result.probes.iter().filter(|(_, ok)| *ok).count(),
            url_total,
            error: result.error.clone(),
        };
        done += 1;
        advance_test(done);
        let _ = app.emit(
            DPI_TEST_PROGRESS_EVENT,
            DpiTestProgress { done, total, results: vec![announced] },
        );
    }

    let summary = DpiTestSummary { done, total };
    let _ = app.emit(DPI_TEST_FINISHED_EVENT, &summary);
    match failure {
        Some(error) => Err(error),
        None => Ok(summary),
    }
}

// ---------------------------------------------------------------------------
// One strategy
// ---------------------------------------------------------------------------

/// Everything `test_strategy` learned: either per-site outcomes or the
/// reason no tunnel came up (`probes` empty, `error` set).
struct StrategyOutcome {
    probes: Vec<(i64, bool)>,
    error: Option<String>,
}

async fn test_strategy(
    app: &AppHandle,
    strategy_id: i64,
    args: &str,
    site_list: &[(i64, String)],
) -> StrategyOutcome {
    match run_tunnel_test(app, args, strategy_id, site_list).await {
        Ok(probes) => StrategyOutcome { probes, error: None },
        Err(error) => StrategyOutcome { probes: Vec::new(), error: Some(error) },
    }
}

/// Spawns ciadpi + its sing-box front, probes every site, tears both down.
async fn run_tunnel_test(
    app: &AppHandle,
    args: &str,
    strategy_id: i64,
    site_list: &[(i64, String)],
) -> Result<Vec<(i64, bool)>, String> {
    let dpi_port = latency::free_local_port()?;
    let api_port = latency::free_local_port()?;

    // the ciadpi sidecar with the strategy line (validation happened on
    // save; the app owns the listener flags)
    let runner = app
        .shell()
        .sidecar("byedpi")
        .map_err(|error| format!("failed to resolve the byedpi sidecar: {error}"))?;
    let runner = runner.args(["-i", "127.0.0.1", "-p", &dpi_port.to_string()]);
    let runner = if args.trim().is_empty() {
        runner
    } else {
        runner.args(args.split_whitespace().collect::<Vec<_>>())
    };
    let (events, child) = runner
        .spawn()
        .map_err(|error| format!("failed to start byedpi: {error}"))?;

    // ready once the SOCKS port accepts connections; a fast exit (bad flag
    // combination) surfaces with its stderr tail
    let (receiver, outcome) = tauri::async_runtime::spawn_blocking(move || {
        connection::probe_dpi_ready(events, dpi_port)
    })
    .await
    .map_err(|error| format!("byedpi startup probe failed: {error}"))?;
    drop(receiver); // no watcher for the short-lived test instance
    if let Err(reason) = outcome {
        let _ = child.kill();
        return Err(reason);
    }

    // a sing-box front with the tunnel as its only outbound + the clash API
    let probed = front_and_probe(app, dpi_port, api_port, strategy_id, site_list).await;
    let _ = child.kill();
    probed
}

async fn front_and_probe(
    app: &AppHandle,
    dpi_port: u16,
    api_port: u16,
    strategy_id: i64,
    site_list: &[(i64, String)],
) -> Result<Vec<(i64, bool)>, String> {
    let config = latency::build_test_config(&[socks_outbound(dpi_port)], api_port);
    let config_path = latency::write_config_file(strategy_id, api_port, &config)?;
    let config_arg = config_path.to_string_lossy().to_string();

    let mut sing_box = latency::SingBoxGuard {
        child: None,
        config_path: config_path.clone(),
        keep_config: false,
    };
    let runner = latency::sidecar(app)?;
    let (_events, child) = runner
        .args(["run", "-c", &config_arg])
        .spawn()
        .map_err(|error| {
            sing_box.keep_config = true;
            format!("failed to start sing-box: {error}")
        })?;
    sing_box.child = Some(child);

    let ready = tauri::async_runtime::spawn_blocking(move || latency::wait_for_api(api_port))
        .await
        .map_err(|error| format!("startup probe failed: {error}"))?;
    if !ready {
        sing_box.keep_config = true;
        return Err(format!(
            "the sing-box test front did not become ready within {:?} (config kept at {})",
            latency::STARTUP_TIMEOUT,
            config_path.display()
        ));
    }

    let targets: Vec<(i64, String, i64, String)> = site_list
        .iter()
        .map(|(url_id, url)| (strategy_id, TUNNEL_TAG.to_string(), *url_id, url.clone()))
        .collect();
    let probes = tauri::async_runtime::spawn_blocking(move || {
        latency::url_test_chunk(&format!("http://127.0.0.1:{api_port}"), &targets, TEST_TIMEOUT)
    })
    .await
    .map_err(|error| format!("url-test worker failed: {error}"))?;

    drop(sing_box); // kills the instance, removes the config
    Ok(probes.into_iter().map(|(_endpoint, url_id, delay)| (url_id, delay.is_some())).collect())
}

/// The only outbound of the test front: a socks5 hop into ciadpi.
fn socks_outbound(dpi_port: u16) -> Value {
    json!({
        "type": "socks",
        "server": "127.0.0.1",
        "server_port": dpi_port,
        "version": "5",
    })
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// Persists one strategy's full site sweep in a single lock (upserts — a
/// re-test overwrites the previous sweep). A strategy deleted while its
/// test was running is simply skipped.
fn store_strategy_results(conn: &Connection, strategy_id: i64, results: &[(i64, bool)]) -> Result<(), String> {
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
        crate::db::open(&path).expect("test db should open")
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
    fn socks_outbound_points_at_ciadpi() {
        let outbound = socks_outbound(41234);
        assert_eq!(outbound["type"], "socks");
        assert_eq!(outbound["server"], "127.0.0.1");
        assert_eq!(outbound["server_port"], 41234);
        assert_eq!(outbound["version"], "5");

        // embedded into a test config it becomes the retagged first outbound
        let config = latency::build_test_config(&[outbound], 45678);
        assert_eq!(config["outbounds"][0]["tag"], TUNNEL_TAG);
        assert_eq!(config["outbounds"][0]["server_port"], 41234);
        assert_eq!(config["outbounds"][1]["type"], "direct");
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

    #[test]
    fn registry_tracks_and_cancels() {
        let cancel = begin_test(10).expect("begin test");
        assert!(begin_test(20).is_err(), "a second test must be rejected");

        advance_test(4);
        assert_eq!(dpi_test_status().expect("status"), Some(DpiTestStatus { done: 4, total: 10 }));

        assert!(!cancel.load(Ordering::Relaxed));
        assert!(request_cancel().expect("cancel"));
        assert!(cancel.load(Ordering::Relaxed));
        assert!(request_cancel().expect("cancel"), "cancel is idempotent");

        drop(TestGuard);
        assert_eq!(dpi_test_status().expect("status"), None);
        assert!(!request_cancel().expect("cancel"), "no test, nothing to cancel");

        // the slot is free again after a finished test
        begin_test(1).expect("begin again");
        drop(TestGuard);
        assert_eq!(dpi_test_status().expect("status"), None);
    }
}
