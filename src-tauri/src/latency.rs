//! Endpoint availability / latency probing — a URL test through sing-box.
//!
//! Every endpoint is embedded as an outbound into a throwaway sing-box config
//! with the clash API enabled, and latency is a real HTTP request through
//! each proxy (the clash `delay` endpoint, the same mechanism Clash GUIs call
//! a URL test). Endpoints that pass this base probe are additionally
//! deep-probed against the URLs of every *proxy*-routed URL category
//! (Settings → Routing) — every endpoint that passes the base probe is
//! deep-probed. Per-URL outcomes land in
//! `endpoint_url_results`. The instance is killed and the temp config
//! removed when the scan ends. Base results land in the `endpoints` table;
//! the UI hears about all of it via `profiles-changed` (kind `latency`) plus
//! per-batch `latency-progress` events while the scan is running.

use std::collections::{HashMap, HashSet};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;
use crate::parser;
use crate::profiles;
use crate::routing;

/// The URL every proxy has to fetch during the test (Clash convention).
const TEST_URL: &str = "http://www.gstatic.com/generate_204";
/// Per-endpoint budget for the whole proxy roundtrip (connect + TLS + HTTP).
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the throwaway sing-box instance may take to expose its API.
pub(crate) const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
/// How many endpoints are tested in parallel per batch.
const CONCURRENCY: usize = 32;

/// Internal tags in the generated config: ASCII-only, unique, no encoding
/// issues in clash API paths.
const TAG_PREFIX: &str = "mt-";
const DIRECT_TAG: &str = "mt-direct";

pub const LATENCY_PROGRESS_EVENT: &str = "latency-progress";

// ---------------------------------------------------------------------------
// Scan registry
// ---------------------------------------------------------------------------

/// In-flight scans keyed by profile id: live progress (so a re-mounted page
/// can adopt a running scan via `profile_latency_status`) plus the cancel flag.
struct ScanState {
    done: usize,
    total: usize,
    cancel: Arc<AtomicBool>,
}

static SCANS: LazyLock<Mutex<HashMap<i64, ScanState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_scans() -> Result<MutexGuard<'static, HashMap<i64, ScanState>>, String> {
    SCANS.lock().map_err(|_| "scan registry poisoned".to_string())
}

/// Removes the profile's registry entry when the scan ends for any reason,
/// including early returns and panics.
struct ScanGuard(i64);

impl Drop for ScanGuard {
    fn drop(&mut self) {
        if let Ok(mut scans) = SCANS.lock() {
            scans.remove(&self.0);
        }
    }
}

fn begin_scan(profile_id: i64, total: usize) -> Result<Arc<AtomicBool>, String> {
    let mut scans = lock_scans()?;
    if scans.contains_key(&profile_id) {
        return Err(format!("a latency scan is already running for profile {profile_id}"));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    scans.insert(profile_id, ScanState { done: 0, total, cancel: cancel.clone() });
    Ok(cancel)
}

fn advance_scan(profile_id: i64, done: usize) {
    if let Ok(mut scans) = SCANS.lock() {
        if let Some(state) = scans.get_mut(&profile_id) {
            state.done = done;
        }
    }
}

fn request_cancel(profile_id: i64) -> Result<bool, String> {
    let scans = lock_scans()?;
    match scans.get(&profile_id) {
        Some(state) => {
            state.cancel.store(true, Ordering::Relaxed);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Stops a running scan for the profile and (blocking) waits until it has
/// fully exited — the profile update flow calls this before rewriting the
/// endpoint rows under a scan's feet. The cancel is cooperative (checked
/// between batches), so the wait is bounded by one batch. Returns false
/// when the scan outlived the timeout; the caller proceeds anyway and the
/// FK-tolerant writes cover the residual race.
pub(crate) fn cancel_and_wait(profile_id: i64, timeout: Duration) -> bool {
    if !request_cancel(profile_id).unwrap_or(false) {
        return true; // nothing to wait for
    }
    let deadline = std::time::Instant::now() + timeout;
    while scan_running(profile_id) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

// ---------------------------------------------------------------------------
// Payloads & commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LatencyResult {
    id: i64,
    available: bool,
    latency_ms: Option<i64>,
    /// category URLs reachable through this endpoint / probed total (0/0 —
    /// the endpoint failed the base probe, or nothing is marked for testing)
    url_ok: usize,
    url_total: usize,
}

impl LatencyResult {
    fn failed(id: i64) -> Self {
        Self { id, available: false, latency_ms: None, url_ok: 0, url_total: 0 }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencySummary {
    pub total: usize,
    pub reachable: usize,
}

/// Emitted once per test batch (not per endpoint) so the client and the DB
/// see a steady, non-spammy stream of updates: `results` carries everything
/// tested since the previous event.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LatencyProgress {
    profile_id: i64,
    done: usize,
    total: usize,
    results: Vec<LatencyResult>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LatencyStatus {
    pub done: usize,
    pub total: usize,
}

/// Snapshot of a profile's scan; `None` means no scan is running.
#[tauri::command]
pub fn profile_latency_status(profile_id: i64) -> Result<Option<LatencyStatus>, String> {
    let scans = lock_scans()?;
    Ok(scans
        .get(&profile_id)
        .map(|state| LatencyStatus { done: state.done, total: state.total }))
}

/// Whether a scan for this profile is in flight right now (the supervisor
/// waits these out and reuses their data).
pub(crate) fn scan_running(profile_id: i64) -> bool {
    lock_scans().map(|scans| scans.contains_key(&profile_id)).unwrap_or(false)
}

/// Stops a running scan after the current batch; idempotent.
#[tauri::command]
pub fn profile_cancel_latency(profile_id: i64) -> Result<bool, String> {
    request_cancel(profile_id)
}

/// Which endpoints a scan tests: everything (the manual button, passes
/// with fresh content or nothing reachable known) or just the endpoints
/// the last scan proved reachable (the supervisor's cheap re-check passes
/// — on a large mostly-dead profile re-testing all of it every pass would
/// burn the whole interval) or only the endpoints no scan has ever
/// stamped (the post-update pass: a diff update keeps survivors' results,
/// so only fresh rows and the untested tail of an interrupted scan need
/// work) or the reachable ones plus those never-stamped rows (a regular
/// pass on a profile that still has first-test debt — e.g. connected right
/// after an update — refreshes the live data *and* picks the fresh rows
/// up in one run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanScope {
    All,
    ReachableOnly,
    UntestedOnly,
    ReachableAndUntested,
}

/// Outcome of the on-demand single-endpoint deep probe.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointUrlSummary {
    pub url_ok: usize,
    pub url_total: usize,
}

/// Deep-probes one endpoint on demand (the endpoint card's test button): a
/// throwaway sing-box with just this endpoint, every test-marked probeable
/// URL of the *proxy*-routed categories fetched through it. Rows replace any
/// previous deep-probe results of the endpoint, exactly like a scan's deep
/// probe would store them.
#[tauri::command]
pub async fn profile_test_endpoint_urls(
    app: AppHandle,
    item_id: i64,
) -> Result<EndpointUrlSummary, String> {
    // snapshot: the endpoint's outbound, its profile, and the proxy test URLs
    let (profile_id, outbound, test_urls) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
        let (profile_id, raw) = conn
            .query_row(
                "SELECT profile_id, outbound_json FROM endpoints WHERE id = ?1",
                params![item_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => format!("endpoint {item_id} not found"),
                other => db_err(other),
            })?;
        let outbound = serde_json::from_str::<Value>(&raw)
            .ok()
            .filter(|outbound| {
                outbound
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| !kind.is_empty())
            })
            .ok_or_else(|| format!("endpoint {item_id} has no usable outbound config"))?;
        (profile_id, outbound, crate::sites::flat_test_rules(&conn, routing::ACTION_PROXY)?)
    };
    if test_urls.is_empty() {
        return Err(
            "no probeable URLs in the proxy-routed categories (Settings → Routing)".to_string(),
        );
    }

    let api_port = free_local_port()?;

    // validate up front so a broken outbound fails fast with a clear error
    // instead of a startup timeout
        let values = vec![outbound.clone()];
    let valid = tauri::async_runtime::spawn_blocking(move || {
        run_sing_box_check(api_port, &values)
    })
    .await
    .map_err(|error| format!("config validation failed: {error}"))??;
    if !valid {
        return Err("sing-box refused this endpoint's outbound (legacy or broken config)".to_string());
    }

    let mut sing_box =
        SingBoxGuard { child: None, config_path: PathBuf::new(), keep_config: false };
    let config_path = write_config_file(profile_id, api_port, &build_test_config(&[outbound], api_port))?;
    sing_box.config_path = config_path.clone();
    let config_arg = config_path.to_string_lossy().to_string();

    let runner = sidecar()?;
    let (_events, child) = runner
        .args(["run", "-c", &config_arg])
        .spawn()
        .map_err(|error| format!("failed to start sing-box: {error}"))?;
    sing_box.child = Some(child);

    let ready = tauri::async_runtime::spawn_blocking(move || wait_for_api(api_port))
        .await
        .map_err(|error| format!("startup probe failed: {error}"))?;
    if !ready {
        sing_box.keep_config = true;
        return Err(format!(
            "sing-box test instance did not become ready within {STARTUP_TIMEOUT:?} \
             (config kept at {})",
            config_path.display()
        ));
    }

    let tag = tag_for(0);
    let targets: Vec<(i64, String, i64, String)> = test_urls
        .iter()
        .map(|(url_id, url)| (item_id, tag.clone(), *url_id, url.clone()))
        .collect();
    let base_url = format!("http://127.0.0.1:{api_port}");
    let probes = tauri::async_runtime::spawn_blocking(move || {
        url_test_chunk(&base_url, &targets, TEST_TIMEOUT)
    })
    .await
    .map_err(|error| format!("url-test worker failed: {error}"))?;

    // kills the instance, removes the config
    drop(sing_box);

    let url_total = probes.len();
    let url_ok = probes.iter().filter(|(_, _, delay)| delay.is_some()).count();
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
        // full replace: rows for URLs no longer probed must not linger
        conn.execute(
            "DELETE FROM endpoint_url_results WHERE endpoint_id = ?1",
            params![item_id],
        )
        .map_err(db_err)?;
        apply_url_probes(&conn, &probes)?;
    }
    profiles::after_mutation(&app, Some(profile_id), profiles::LATENCY);
    Ok(EndpointUrlSummary { url_ok, url_total })
}

#[tauri::command]
pub async fn profile_check_latency(
    app: AppHandle,
    profile_id: i64,
) -> Result<LatencySummary, String> {
    run_scan(&app, profile_id, ScanScope::All).await
}

/// One whole-availability scan of a profile's endpoints. Shared by the
/// Check Latency button (`profile_check_latency`) and the automatic-selection
/// supervisor — the events and DB writes are identical, the UI simply shows
/// whatever pass is currently running. `scope` narrows the run to the
/// endpoints the last scan proved reachable (the supervisor's re-check
/// passes). Returns an error when another scan for the profile is already in
/// flight (`begin_scan` guard).
pub(crate) async fn run_scan(
    app: &AppHandle,
    profile_id: i64,
    scope: ScanScope,
) -> Result<LatencySummary, String> {
    // everything below is a start-of-scan snapshot: edits made while the
    // scan runs (categories, rules) do not affect it
    let ((mut outbounds, mut untestable), test_urls) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
        (
            load_outbounds(&conn, profile_id)?,
            crate::sites::flat_test_rules(&conn, routing::ACTION_PROXY)?,
        )
    };
    if matches!(scope, ScanScope::ReachableOnly) {
        let reachable = {
            let state = app.state::<AppState>();
            let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
            load_reachable_ids(&conn, profile_id)?
        };
        outbounds.retain(|(id, _)| reachable.contains(id));
        // rows that cannot even be parsed were marked dead by the earlier
        // full scans — re-marking them would just inflate this small pass
        untestable.clear();
    }
    if matches!(scope, ScanScope::UntestedOnly) {
        let untested = {
            let state = app.state::<AppState>();
            let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
            load_untested_ids(&conn, profile_id)?
        };
        outbounds.retain(|(id, _)| untested.contains(id));
        // untestable rows already marked dead by an earlier pass stay put;
        // never-touched ones are new junk and get their dead verdict
        untestable.retain(|id| untested.contains(id));
    }
    if matches!(scope, ScanScope::ReachableAndUntested) {
        let (reachable, untested) = {
            let state = app.state::<AppState>();
            let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
            (
                load_reachable_ids(&conn, profile_id)?,
                load_untested_ids(&conn, profile_id)?,
            )
        };
        outbounds.retain(|(id, _)| reachable.contains(id) || untested.contains(id));
        // reachable rows always had a usable outbound; among the never-touched
        // ones the unparsable are new junk and get their dead verdict, while
        // already-marked-dead rows (tested long ago) keep their verdict
        untestable.retain(|id| untested.contains(id));
    }
    let url_total = test_urls.len();

    let total = outbounds.len() + untestable.len();
    let cancel = begin_scan(profile_id, total)?;
    let _guard = ScanGuard(profile_id);

    let mut done = 0usize;
    let mut reachable = 0usize;
    let mut base_url = String::new();

    // spin up a throwaway sing-box instance with every endpoint as an outbound;
    // only needed when there is something to test
    let mut sing_box = SingBoxGuard { child: None, config_path: PathBuf::new(), keep_config: false };
    if !outbounds.is_empty() {
        let api_port = free_local_port()?;

        // validate up front and isolate rows the current sing-box refuses to
        // load, so one broken outbound cannot kill the whole scan
                let rows = outbounds;
        let validated = tauri::async_runtime::spawn_blocking(move || {
            let check = |values: &[Value]| run_sing_box_check(api_port, values);
            isolate_bad_outbounds(&rows, &check)
        })
        .await
        .map_err(|error| format!("config validation failed: {error}"))?;
        let (good, mut broken) = validated?;
        untestable.append(&mut broken);
        outbounds = good;

        if !outbounds.is_empty() {
            let values: Vec<Value> =
                outbounds.iter().map(|(_, outbound)| outbound.clone()).collect();
            let config = build_test_config(&values, api_port);
            let config_path = write_config_file(profile_id, api_port, &config)?;
            sing_box.config_path = config_path.clone();
            let config_arg = config_path.to_string_lossy().to_string();

            let runner = sidecar()?;
            let (_events, child) = runner
                .args(["run", "-c", &config_arg])
                .spawn()
                .map_err(|error| format!("failed to start sing-box: {error}"))?;
            sing_box.child = Some(child);

            let ready = tauri::async_runtime::spawn_blocking(move || wait_for_api(api_port))
                .await
                .map_err(|error| format!("startup probe failed: {error}"))?;
            if !ready {
                sing_box.keep_config = true;
                return Err(format!(
                    "sing-box test instance did not become ready within {STARTUP_TIMEOUT:?} \
                     (config kept at {})",
                    config_path.display()
                ));
            }
            base_url = format!("http://127.0.0.1:{api_port}");
        }
    }

    // endpoints without a usable outbound can never connect — mark them dead
    if !untestable.is_empty() {
        let failed: Vec<LatencyResult> =
            untestable.iter().map(|&id| LatencyResult::failed(id)).collect();
        store_batch(app, &failed, &[])?;
        done += failed.len();
        advance_scan(profile_id, done);
        emit_progress(app, profile_id, done, total, &failed);
    }

    let tagged: Vec<(i64, String)> = outbounds
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (*id, tag_for(index)))
        .collect();
    for chunk in tagged.chunks(CONCURRENCY) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let batch = chunk.to_vec();
        let base = base_url.clone();
        let results = tauri::async_runtime::spawn_blocking(move || {
            delay_test_chunk(&base, &batch, TEST_TIMEOUT)
        })
        .await
        .map_err(|error| format!("url-test worker failed: {error}"))?;

        // base probe passed → deep-probe the proxy-category URLs; everyone
        // else (and the failures) get their stale URL rows cleared
        let mut results = results;
        let mut url_probes: Vec<UrlProbe> = Vec::new();
        if !test_urls.is_empty() {
            let survivors: Vec<(&(i64, String), &LatencyResult)> = chunk
                .iter()
                .zip(results.iter())
                .filter(|(_, result)| result.available)
                .collect();
            let targets: Vec<(i64, String, i64, String)> = survivors
                .iter()
                .flat_map(|((id, tag), _)| {
                    test_urls
                        .iter()
                        .map(move |(url_id, url)| (*id, tag.clone(), *url_id, url.clone()))
                })
                .collect();
            if !targets.is_empty() {
                let base = base_url.clone();
                url_probes = tauri::async_runtime::spawn_blocking(move || {
                    url_test_chunk(&base, &targets, TEST_TIMEOUT)
                })
                .await
                .map_err(|error| format!("url-test worker failed: {error}"))?;
            }
            let mut ok_counts: HashMap<i64, usize> = HashMap::new();
            for (endpoint_id, _url_id, delay) in &url_probes {
                if delay.is_some() {
                    *ok_counts.entry(*endpoint_id).or_default() += 1;
                }
            }
            // end the immutable borrow of `results` before the update below
            let probed: Vec<(i64, usize)> = survivors
                .iter()
                .map(|((id, _), _)| (*id, ok_counts.get(id).copied().unwrap_or(0)))
                .collect();
            for result in &mut results {
                if let Some((_, url_ok)) = probed.iter().find(|(id, _)| *id == result.id) {
                    result.url_total = url_total;
                    result.url_ok = *url_ok;
                }
            }
        }

        store_batch(app, &results, &url_probes)?;
        reachable += results.iter().filter(|result| result.available).count();
        done += results.len();
        advance_scan(profile_id, done);
        emit_progress(app, profile_id, done, total, &results);
        // fresh availability data just landed — let an automatic strategy
        // running the live session leave a dead or untested endpoint now
        // instead of waiting out the rest of the scan
        crate::auto_select::rescue(app, profile_id).await;
    }

    // kills the instance, removes the config; a cancelled scan reports what
    // it actually got through
    drop(sing_box);
    profiles::after_mutation(app, Some(profile_id), profiles::LATENCY);
    Ok(LatencySummary { total: done, reachable })
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// An endpoint's stored outbound, ready to embed into the test config.
pub(crate) type OutboundRow = (i64, Value);

/// Splits a profile's endpoints into testable outbounds (valid JSON with a
/// type) and ids whose stored outbound is unusable (reported as unavailable).
pub(crate) fn load_outbounds(
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
fn load_reachable_ids(conn: &Connection, profile_id: i64) -> Result<HashSet<i64>, String> {
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
fn load_untested_ids(conn: &Connection, profile_id: i64) -> Result<HashSet<i64>, String> {
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
fn store_batch(
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

fn apply_url_probes(conn: &Connection, probes: &[UrlProbe]) -> Result<(), String> {
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

// ---------------------------------------------------------------------------
// The throwaway sing-box instance
// ---------------------------------------------------------------------------

pub(crate) fn sidecar() -> Result<crate::process::Command, String> {
    crate::process::sidecar("sing-box")
        .map_err(|error| format!("failed to resolve sing-box sidecar: {error}"))
}

/// Kills the throwaway sing-box instance and removes its config when the scan
/// ends for any reason (including early returns and panics).
pub(crate) struct SingBoxGuard {
    pub(crate) child: Option<crate::process::CommandChild>,
    pub(crate) config_path: PathBuf,
    /// set when the instance failed to start — keep the config for debugging
    pub(crate) keep_config: bool,
}

impl Drop for SingBoxGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            let _ = child.kill();
        }
        if !self.keep_config && !self.config_path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.config_path);
        }
    }
}

fn tag_for(index: usize) -> String {
    format!("{TAG_PREFIX}{index}")
}

/// Drops uTLS fingerprints sing-box does not know — rows stored before the
/// parser sanitized them (e.g. `fp=unsafe`) would otherwise make sing-box
/// reject the whole config. uTLS stays enabled with its default.
pub(crate) fn sanitize_outbound(outbound: &mut Value) {
    let Some(tls) = outbound.get_mut("tls").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(utls) = tls.get_mut("utls").and_then(Value::as_object_mut) else {
        return;
    };
    let unknown = utls
        .get("fingerprint")
        .and_then(Value::as_str)
        .is_some_and(|fingerprint| {
            !parser::UTLS_FINGERPRINTS.contains(&fingerprint.to_ascii_lowercase().as_str())
        });
    if unknown {
        utls.remove("fingerprint");
    }
}

/// Builds the test config: every endpoint as an outbound with a synthetic
/// tag (uniqueness and ASCII guaranteed regardless of the source tags), a
/// direct outbound as the route default, and the clash API on localhost.
pub(crate) fn build_test_config(outbounds: &[Value], api_port: u16) -> Value {
    let mut tagged: Vec<Value> = outbounds
        .iter()
        .enumerate()
        .map(|(index, outbound)| {
            let mut outbound = outbound.clone();
            sanitize_outbound(&mut outbound);
            outbound["tag"] = Value::String(tag_for(index));
            outbound
        })
        .collect();
    tagged.push(json!({ "type": "direct", "tag": DIRECT_TAG }));

    json!({
        "log": { "disabled": true },
        "outbounds": tagged,
        "route": { "final": DIRECT_TAG },
        "experimental": {
            "clash_api": { "external_controller": format!("127.0.0.1:{api_port}") }
        }
    })
}

pub(crate) fn write_config_file(profile_id: i64, api_port: u16, config: &Value) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("megathrone-urltest-{profile_id}-{api_port}.json"));
    let content =
        serde_json::to_string_pretty(config).map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(path)
}

pub(crate) fn free_local_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("cannot pick a local port: {e}"))?
        .local_addr()
        .map_err(|e| format!("cannot pick a local port: {e}"))
        .map(|address| address.port())
}

/// `sing-box check` on a throwaway config built from `values`; must run on a
/// blocking thread (it drives an async sidecar call via `block_on`).
fn run_sing_box_check(api_port: u16, values: &[Value]) -> Result<bool, String> {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("megathrone-urltest-check-{unique}.json"));
    let content = serde_json::to_string_pretty(&build_test_config(values, api_port))
        .map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))?;

    let check = sidecar()?;
    let output = tauri::async_runtime::block_on(check.args(["check", "-c", &path.to_string_lossy()]).output())
        .map_err(|error| format!("failed to run sing-box check: {error}"))?;
    let _ = std::fs::remove_file(&path);
    Ok(output.status.success())
}

/// Isolates outbounds the current sing-box refuses to load (legacy rows,
/// foreign generators) by bisection: `check` validates a whole config
/// cheaply, so a failing half is split again until the individual offenders
/// are found. They end up in the "broken" list and are reported as
/// unavailable instead of killing the whole scan.
fn isolate_bad_outbounds(
    rows: &[OutboundRow],
    check: &impl Fn(&[Value]) -> Result<bool, String>,
) -> Result<(Vec<OutboundRow>, Vec<i64>), String> {
    let values: Vec<Value> = rows.iter().map(|(_, outbound)| outbound.clone()).collect();
    if check(&values)? {
        return Ok((rows.to_vec(), Vec::new()));
    }
    if rows.len() == 1 {
        return Ok((Vec::new(), vec![rows[0].0]));
    }
    let (left, right) = rows.split_at(rows.len() / 2);
    let (mut good, mut broken) = isolate_bad_outbounds(left, check)?;
    let (good_right, broken_right) = isolate_bad_outbounds(right, check)?;
    good.extend(good_right);
    broken.extend(broken_right);
    Ok((good, broken))
}

/// `isolate_bad_outbounds` driven by the real sidecar — the scan's and the
/// connect-fallback's shared "which rows can this sing-box even load"
/// check. Returns (loadable rows, ids sing-box refuses).
pub(crate) async fn isolate_unloadable_outbounds(
    app: &AppHandle,
    rows: Vec<OutboundRow>,
) -> Result<(Vec<OutboundRow>, Vec<i64>), String> {
    let api_port = free_local_port()?;
    let _ = app;
    tauri::async_runtime::spawn_blocking(move || {
        let check = |values: &[Value]| run_sing_box_check(api_port, values);
        isolate_bad_outbounds(&rows, &check)
    })
    .await
    .map_err(|error| format!("config validation failed: {error}"))?
}

/// Polls the clash API until it answers or the deadline passes.
pub(crate) fn wait_for_api(port: u16) -> bool {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .into();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        let up = matches!(
            agent.get(&format!("http://127.0.0.1:{port}/version")).call(),
            Ok(response) if response.status().as_u16() == 200
        );
        if up {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------
// URL testing via the clash API
// ---------------------------------------------------------------------------

/// Tests a batch in parallel on scoped threads (the caller guarantees the
/// batch is small enough).
fn delay_test_chunk(base_url: &str, targets: &[(i64, String)], timeout: Duration) -> Vec<LatencyResult> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout + Duration::from_secs(2)))
        .build()
        .into();
    std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(id, tag)| {
                let agent = &agent;
                scope.spawn(move || delay_test_one(agent, base_url, tag, timeout, *id))
            })
            .collect();
        handles
            .into_iter()
            .zip(targets)
            .map(|(handle, (id, _))| {
                handle.join().unwrap_or_else(|_| LatencyResult::failed(*id))
            })
            .collect()
    })
}

/// The base availability probe: the built-in URL through one proxy.
fn delay_test_one(
    agent: &ureq::Agent,
    base_url: &str,
    tag: &str,
    timeout: Duration,
    id: i64,
) -> LatencyResult {
    let Some(delay) = proxy_url_delay(agent, base_url, tag, TEST_URL, timeout) else {
        return LatencyResult::failed(id);
    };
    LatencyResult { id, available: true, latency_ms: Some(delay), url_ok: 0, url_total: 0 }
}

/// One URL test through a proxy: `GET /proxies/{tag}/delay?url=…` makes
/// sing-box fetch `url` through that outbound and answer with the measured
/// delay in ms. `None` = not reachable. The URL is percent-encoded for the
/// query string (user URLs carry `&`, `?`, spaces, non-ASCII…).
fn proxy_url_delay(
    agent: &ureq::Agent,
    base_url: &str,
    tag: &str,
    url: &str,
    timeout: Duration,
) -> Option<i64> {
    let encoded = utf8_percent_encode(url, NON_ALPHANUMERIC);
    let request =
        format!("{base_url}/proxies/{tag}/delay?url={encoded}&timeout={}", timeout.as_millis());

    let Ok(mut response) = agent.get(&request).call() else {
        return None;
    };
    if response.status().as_u16() != 200 {
        return None;
    }
    let Ok(body) = response.body_mut().with_config().limit(4096).read_to_string() else {
        return None;
    };
    let Ok(value) = serde_json::from_str::<Value>(&body) else {
        return None;
    };
    match value.get("delay").and_then(Value::as_i64) {
        Some(delay) if delay >= 0 => Some(delay),
        _ => None,
    }
}

/// Tests (endpoint, custom URL) pairs in parallel on scoped threads.
pub(crate) fn url_test_chunk(
    base_url: &str,
    targets: &[(i64, String, i64, String)],
    timeout: Duration,
) -> Vec<UrlProbe> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout + Duration::from_secs(2)))
        .build()
        .into();
    std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(endpoint_id, tag, url_id, url)| {
                let agent = &agent;
                scope.spawn(move || {
                    (*endpoint_id, *url_id, proxy_url_delay(agent, base_url, tag, url, timeout))
                })
            })
            .collect();
        // a panicked worker yields an impossible id 0 sentinel, dropped by
        // the caller's ok-count lookup
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or((0, 0, None)))
            .collect()
    })
}

fn emit_progress(
    app: &AppHandle,
    profile_id: i64,
    done: usize,
    total: usize,
    results: &[LatencyResult],
) {
    let _ = app.emit(
        LATENCY_PROGRESS_EVENT,
        LatencyProgress {
            profile_id,
            done,
            total,
            results: results.to_vec(),
        },
    );
}

fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    fn test_db() -> Connection {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "megathrone-latency-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
    }

    #[test]
    fn build_test_config_retags_and_wires_the_clash_api() {
        let first = json!({
            "type": "vless", "tag": "original #2", "server": "1.2.3.4", "server_port": 443
        });
        let second = json!({ "type": "socks", "server": "5.6.7.8", "server_port": 1080 });

        let config = build_test_config(&[first, second], 45678);

        let outbounds = config["outbounds"].as_array().expect("outbounds");
        assert_eq!(outbounds.len(), 3, "endpoint outbounds + the direct default");
        assert_eq!(outbounds[0]["tag"], "mt-0", "source tags are replaced");
        assert_eq!(outbounds[0]["type"], "vless");
        assert_eq!(outbounds[1]["tag"], "mt-1");
        assert_eq!(outbounds[2]["type"], "direct");
        assert_eq!(outbounds[2]["tag"], "mt-direct");
        assert_eq!(config["route"]["final"], "mt-direct");
        assert_eq!(
            config["experimental"]["clash_api"]["external_controller"],
            "127.0.0.1:45678"
        );
    }

    /// A minimal fake of the clash delay API: one request per connection,
    /// `Connection: close`, canned responses picked by the proxy tag in the
    /// request path.
    #[test]
    fn delay_test_chunk_parses_clash_api_responses() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake api");
        let port = listener.local_addr().expect("addr").port();
        let base_url = format!("http://127.0.0.1:{port}");

        std::thread::scope(|scope| {
            scope.spawn(move || {
                for _ in 0..2 {
                    let (mut stream, _) = listener.accept().expect("accept");
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    loop {
                        let read = stream.read(&mut buffer).expect("read request");
                        if read == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..read]);
                        if request.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let head = String::from_utf8_lossy(&request);
                    let (status, body) = if head.contains("/proxies/mt-good/") {
                        ("200 OK", r#"{"delay":42}"#)
                    } else {
                        ("408 Request Timeout", r#"{"message":"context deadline exceeded"}"#)
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).expect("write response");
                }
            });

            let results = delay_test_chunk(
                &base_url,
                &[(10, "mt-good".into()), (11, "mt-bad".into())],
                Duration::from_secs(2),
            );

            assert_eq!(results.len(), 2);
            let good = results.iter().find(|result| result.id == 10).expect("good result");
            assert!(good.available);
            assert_eq!(good.latency_ms, Some(42));
            assert_eq!((good.url_ok, good.url_total), (0, 0), "counts are filled by the scan loop");
            let bad = results.iter().find(|result| result.id == 11).expect("bad result");
            assert!(!bad.available);
            assert!(bad.latency_ms.is_none());
        });
    }

    /// Same fake API shape, but the canned response is picked by the
    /// *encoded* custom URL in the query string — proving user URLs with
    /// `&`/`?` survive the trip to the clash API — plus the tag.
    #[test]
    fn url_test_chunk_encodes_urls_and_reports_per_url() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake api");
        let port = listener.local_addr().expect("addr").port();
        let base_url = format!("http://127.0.0.1:{port}");

        std::thread::scope(|scope| {
            scope.spawn(move || {
                for _ in 0..2 {
                    let (mut stream, _) = listener.accept().expect("accept");
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    loop {
                        let read = stream.read(&mut buffer).expect("read request");
                        if read == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..read]);
                        if request.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let head = String::from_utf8_lossy(&request);
                    let (status, body) = if head.contains("/proxies/mt-good/")
                        && head.contains("url=http%3A%2F%2Fhost%2Fcheck%3Fq%3D1%26z%3D2")
                    {
                        ("200 OK", r#"{"delay":73}"#)
                    } else {
                        ("408 Request Timeout", r#"{"message":"context deadline exceeded"}"#)
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).expect("write response");
                }
            });

            let nasty = "http://host/check?q=1&z=2";
            let probes = url_test_chunk(
                &base_url,
                &[
                    (7, "mt-good".into(), 10, nasty.into()),
                    (7, "mt-bad".into(), 10, nasty.into()),
                ],
                Duration::from_secs(2),
            );

            assert_eq!(probes.len(), 2);
            assert_eq!(probes[0], (7, 10, Some(73)));
            assert_eq!(probes[1], (7, 10, None));
        });
    }

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
    fn build_test_config_drops_unknown_fingerprints() {
        let poisoned = json!({
            "type": "vless", "server": "1.2.3.4", "server_port": 443,
            "tls": { "enabled": true, "utls": { "enabled": true, "fingerprint": "unsafe" } }
        });

        let config = build_test_config(&[poisoned], 45678);

        let utls = &config["outbounds"][0]["tls"]["utls"];
        assert!(utls.get("fingerprint").is_none(), "junk fingerprint must be dropped");
        assert_eq!(utls["enabled"], true, "uTLS stays enabled with the default");
    }

    #[test]
    fn isolate_bad_outbounds_bisects_until_offenders_are_found() {
        // even indices are poisoned; the check fails iff any poison is present
        let rows: Vec<OutboundRow> = (0..5)
            .map(|index| (index, json!({ "type": "test", "poison": index % 2 == 0 })))
            .collect();
        let check = |values: &[Value]| -> Result<bool, String> {
            Ok(values.iter().all(|value| value["poison"] != json!(true)))
        };

        let (good, broken) = isolate_bad_outbounds(&rows, &check).expect("isolate");

        assert_eq!(good.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(broken, vec![0, 2, 4]);

        // a clean set passes through untouched, a single bad row is isolated
        let clean: Vec<OutboundRow> = vec![(7, json!({ "type": "test" }))];
        let (good, broken) =
            isolate_bad_outbounds(&clean, &check).expect("isolate");
        assert_eq!(good.len(), 1);
        assert!(broken.is_empty());
        let single: Vec<OutboundRow> = vec![(8, json!({ "type": "test", "poison": true }))];
        let (good, broken) =
            isolate_bad_outbounds(&single, &check).expect("isolate");
        assert!(good.is_empty());
        assert_eq!(broken, vec![8]);
    }

    #[test]
    fn load_outbounds_rejects_unknown_profile() {
        let conn = test_db();
        let error = load_outbounds(&conn, 999).expect_err("unknown profile");
        assert!(error.contains("not found"));
    }

    #[test]
    fn scan_registry_tracks_and_cancels() {
        // unique ids: the registry is a process-wide static and tests run in parallel
        let profile_a = 900_001i64;
        let profile_b = 900_002i64;

        let cancel = begin_scan(profile_a, 10).expect("begin scan");
        assert!(begin_scan(profile_a, 20).is_err(), "a second scan must be rejected");
        let cancel_b = begin_scan(profile_b, 5).expect("begin scan b");

        advance_scan(profile_a, 4);
        assert_eq!(
            profile_latency_status(profile_a).expect("status"),
            Some(LatencyStatus { done: 4, total: 10 })
        );
        assert_eq!(
            profile_latency_status(profile_b).expect("status"),
            Some(LatencyStatus { done: 0, total: 5 })
        );

        assert!(!cancel.load(Ordering::Relaxed));
        assert!(request_cancel(profile_a).expect("cancel"), "running scan reported cancelled");
        assert!(cancel.load(Ordering::Relaxed), "the worker must observe the flag");
        assert!(request_cancel(profile_a).expect("cancel"), "cancel is idempotent");
        assert!(!cancel_b.load(Ordering::Relaxed), "profiles are independent");

        drop(ScanGuard(profile_a));
        assert_eq!(profile_latency_status(profile_a).expect("status"), None, "guard cleans the entry");
        assert!(!request_cancel(999_999).expect("cancel"), "no scan, nothing to cancel");

        drop(ScanGuard(profile_b));
        assert_eq!(profile_latency_status(profile_b).expect("status"), None);
    }

    #[test]
    fn cancel_and_wait_blocks_until_the_scan_exits() {
        // unique id: the registry is a process-wide static
        let profile = 900_101i64;

        // no scan — nothing to wait for
        assert!(cancel_and_wait(profile, Duration::from_millis(10)));

        // a scan that exits on its own after a moment: the wait observes it
        let _cancel = begin_scan(profile, 10).expect("begin scan");
        let exiter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            drop(ScanGuard(profile)); // what a real scan's exit path does
        });
        let started = std::time::Instant::now();
        assert!(
            cancel_and_wait(profile, Duration::from_secs(5)),
            "the scan exited within the timeout"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "the wait actually blocked until the exit"
        );
        exiter.join().expect("exiter");

        // a scan that never exits: the timeout fires (proceed-anyway path)
        let _held = begin_scan(profile, 3).expect("begin scan again");
        let started = std::time::Instant::now();
        assert!(
            !cancel_and_wait(profile, Duration::from_millis(80)),
            "a stuck scan times out"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(ScanGuard(profile));
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
