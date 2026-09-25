//! The discovery run — the background sweep behind the Discover button.
//! Every enabled source is fetched (bounded parallelism), its insecure
//! lines dropped, the links parsed and deduped, and the result stored as
//! ONE profile per source: a diff update on the source's linked profile
//! (`apply_update` keeps every surviving endpoint's latency results), a
//! fresh import otherwise. Optionally, after all sources, each touched
//! profile gets a full latency scan, sequentially. One run at a time; the
//! process-wide registry keeps the final status around (a remounted page
//! adopts it via `discovery_run_status`) until the next run replaces it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, params};
use tauri::{AppHandle, Emitter, Manager};

use super::filter::{dedup_endpoints, filter_lines};
use super::model::{DiscoveryProgress, DiscoveryRunStatus, DISCOVERY_PROGRESS_EVENT};
use super::storage::db_err;
use crate::latency::{self, ScanScope};
use crate::parser::{self, NewEndpoint};
use crate::profiles;
use crate::AppState;

/// Concurrent source fetches per run.
const FETCH_CONCURRENCY: usize = 8;
/// Per-attempt budget of one source fetch — a discovery run sweeps dozens
/// of sources and must not stall on one (the profile updater allows 60s).
const SOURCE_TIMEOUT: Duration = Duration::from_secs(15);
const SOURCE_ATTEMPTS: usize = 2;
/// The same body cap the profile updater enforces.
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// How long the update path waits for a cancelled scan to exit before
/// rewriting the endpoint rows anyway (same bound as `update_profile_flow`).
const SCAN_CANCEL_TIMEOUT: Duration = Duration::from_secs(60);

const PHASE_FETCH: &str = "fetch";
const PHASE_TESTING: &str = "testing";
const PHASE_DONE: &str = "done";

// ---------------------------------------------------------------------------
// Registry (one run per app; the final status stays until the next run)
// ---------------------------------------------------------------------------

struct RunState {
    status: DiscoveryRunStatus,
    cancel: Arc<AtomicBool>,
    active: bool,
}

static RUN: LazyLock<Mutex<Option<RunState>>> = LazyLock::new(|| Mutex::new(None));

fn lock_run() -> Result<MutexGuard<'static, Option<RunState>>, String> {
    RUN.lock().map_err(|_| "discovery run registry poisoned".to_string())
}

fn update_run(change: impl FnOnce(&mut RunState)) {
    if let Ok(mut run) = RUN.lock() {
        if let Some(state) = run.as_mut() {
            change(state);
        }
    }
}

pub(super) fn status_snapshot() -> Result<Option<DiscoveryRunStatus>, String> {
    let run = lock_run()?;
    Ok(run.as_ref().map(|state| state.status.clone()))
}

/// Stops a run between sources (and the latency scan the testing phase is
/// running, via the per-profile cancel API); idempotent.
pub(super) fn request_cancel() -> Result<(), String> {
    let run = lock_run()?;
    let Some(state) = run.as_ref() else { return Ok(()) };
    state.cancel.store(true, Ordering::Relaxed);
    if state.active && state.status.phase == PHASE_TESTING {
        if let Some(profile_id) = state.status.test_profile_id {
            let _ = latency::profile_cancel_latency(profile_id);
        }
    }
    Ok(())
}

pub(super) fn start_run(app: AppHandle, test_after: bool) -> Result<(), String> {
    let cancel = {
        let mut run = lock_run()?;
        if run.as_ref().is_some_and(|state| state.active) {
            return Err("discovery run already in progress".to_string());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *run = Some(RunState {
            status: DiscoveryRunStatus {
                started_at: String::new(),
                total: 0,
                done: 0,
                failed: 0,
                created: 0,
                updated: 0,
                phase: PHASE_FETCH,
                test_profile_id: None,
                test_profile_name: None,
                cancelled: false,
            },
            cancel: cancel.clone(),
            active: true,
        });
        cancel
    };
    tauri::async_runtime::spawn(async move {
        execute_run(app, test_after, cancel).await;
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

struct SourceJob {
    id: i64,
    url: String,
    name: String,
    position: i64,
    profile_id: Option<i64>,
}

#[derive(Default)]
struct FetchOutcome {
    created: usize,
    updated: usize,
    failed: usize,
    /// (position, profile_id, name) of every touched profile — the testing
    /// phase visits them in position order
    touched: Vec<(i64, i64, String)>,
}

async fn execute_run(app: AppHandle, test_after: bool, cancel: Arc<AtomicBool>) {
    let sources = match read_sources(&app) {
        Ok(sources) => sources,
        Err(error) => {
            eprintln!("[discovery] failed to read the enabled sources: {error}");
            Vec::new()
        }
    };
    let started_at = sqlite_now(&app);
    update_run(|state| {
        state.status.started_at = started_at;
        state.status.total = sources.len();
    });

    let mut outcome = FetchOutcome::default();
    if !sources.is_empty() {
        outcome = fetch_phase(&app, sources, &cancel).await;
        if test_after && !cancel.load(Ordering::Relaxed) {
            update_run(|state| state.status.phase = PHASE_TESTING);
            testing_phase(&app, std::mem::take(&mut outcome.touched), &cancel).await;
        }
    }

    update_run(|state| {
        state.active = false;
        state.status.phase = PHASE_DONE;
        state.status.cancelled = cancel.load(Ordering::Relaxed);
        state.status.test_profile_id = None;
        state.status.test_profile_name = None;
    });
    let _ = app.emit(
        DISCOVERY_PROGRESS_EVENT,
        DiscoveryProgress::Finished {
            created: outcome.created,
            updated: outcome.updated,
            failed: outcome.failed,
        },
    );
}

/// Fetches every source with bounded parallelism; results are processed as
/// they arrive (completion order).
async fn fetch_phase(
    app: &AppHandle,
    sources: Vec<SourceJob>,
    cancel: &Arc<AtomicBool>,
) -> FetchOutcome {
    let total = sources.len();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(FETCH_CONCURRENCY));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    for source in sources {
        let semaphore = semaphore.clone();
        let worker_app = app.clone();
        let worker_cancel = cancel.clone();
        let worker_tx = tx.clone();
        tauri::async_runtime::spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("the discovery semaphore is never closed");
            let result = if worker_cancel.load(Ordering::Relaxed) {
                Err("cancelled".to_string())
            } else {
                let _ = worker_app.emit(
                    DISCOVERY_PROGRESS_EVENT,
                    DiscoveryProgress::SourceStarted {
                        source_id: source.id,
                        name: source.name.clone(),
                    },
                );
                let url = source.url.clone();
                match tauri::async_runtime::spawn_blocking(move || fetch_source(&url)).await {
                    Ok(result) => result,
                    Err(error) => Err(format!("fetch task failed: {error}")),
                }
            };
            let _ = worker_tx.send((source, result));
        });
    }
    drop(tx);

    let mut outcome = FetchOutcome::default();
    let mut received = 0usize;
    while received < total {
        let Some((source, result)) = rx.recv().await else { break };
        received += 1;
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        process_source(app, &mut outcome, source, result).await;
    }
    outcome
}

/// Fetches one source: a dedicated agent (shorter timeout than the profile
/// updater's, a retry for flaky hosts) with the shared body cap.
fn fetch_source(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(SOURCE_TIMEOUT))
        .build()
        .into();
    let mut error = String::new();
    for _ in 0..SOURCE_ATTEMPTS {
        match agent.get(url).call() {
            Ok(mut response) => {
                return response
                    .body_mut()
                    .with_config()
                    .limit(MAX_DOWNLOAD_BYTES)
                    .read_to_string()
                    .map_err(|e| format!("failed to read response body: {e}"));
            }
            Err(e) => error = format!("request failed: {e}"),
        }
    }
    Err(error)
}

enum StoredSource {
    Created(i64),
    /// (profile_id, set_changed)
    Updated(i64, bool),
}

async fn process_source(
    app: &AppHandle,
    outcome: &mut FetchOutcome,
    source: SourceJob,
    result: Result<String, String>,
) {
    let source_id = source.id;
    let source_name = source.name.clone();
    let source_position = source.position;
    let mut item_count = 0usize;
    let stored = match result {
        Ok(content) => {
            let (filtered, _removed) = filter_lines(&content);
            let native_config = filtered.trim_start().starts_with('{');
            let endpoints = dedup_endpoints(parser::parse_subscription(&filtered).endpoints);
            if endpoints.is_empty() {
                Err("no supported endpoints found".to_string())
            } else {
                item_count = endpoints.len();
                let app = app.clone();
                match tauri::async_runtime::spawn_blocking(move || {
                    store_source_result(&app, &source, endpoints, native_config)
                })
                .await
                {
                    Ok(stored) => stored,
                    Err(error) => Err(format!("discovery task failed: {error}")),
                }
            }
        }
        Err(error) => Err(error),
    };

    let stored = match stored {
        Ok(stored) => stored,
        Err(error) => {
            outcome.failed += 1;
            update_run(|state| {
                state.status.done += 1;
                state.status.failed += 1;
            });
            record_source_error(app, source_id, &error);
            let _ = app.emit(
                DISCOVERY_PROGRESS_EVENT,
                DiscoveryProgress::SourceDone {
                    source_id,
                    name: source_name,
                    ok: false,
                    item_count: 0,
                    error: Some(error),
                },
            );
            return;
        }
    };

    let profile_id = source_profile_id(&stored);
    match stored {
        StoredSource::Created(_) => {
            outcome.created += 1;
            update_run(|state| {
                state.status.done += 1;
                state.status.created += 1;
            });
        }
        StoredSource::Updated(_, set_changed) => {
            outcome.updated += 1;
            update_run(|state| {
                state.status.done += 1;
                state.status.updated += 1;
            });
            if set_changed {
                // silent, like the scheduler's rebuilds
                tauri::async_runtime::spawn(profiles::rebuild_session_after_update(
                    app.clone(),
                    profile_id,
                    true,
                    false,
                ));
            }
        }
    }
    outcome.touched.push((source_position, profile_id, source_name.clone()));
    let _ = app.emit(
        DISCOVERY_PROGRESS_EVENT,
        DiscoveryProgress::SourceDone {
            source_id,
            name: source_name,
            ok: true,
            item_count,
            error: None,
        },
    );
}

/// Applies one source's deduped content to the DB (blocking — runs inside
/// `spawn_blocking`): a diff update when the linked profile still exists,
/// a fresh import otherwise, plus the source-row bookkeeping.
fn store_source_result(
    app: &AppHandle,
    source: &SourceJob,
    endpoints: Vec<NewEndpoint>,
    native_config: bool,
) -> Result<StoredSource, String> {
    let item_count = endpoints.len() as i64;
    let content = rebuild_content(&endpoints, native_config);

    // a dangling link (the profile was deleted elsewhere) imports afresh
    let existing = source
        .profile_id
        .filter(|&profile_id| profile_still_exists(app, profile_id));
    if let Some(profile_id) = existing {
        // a running scan works off a snapshot of the endpoint rows; stop it
        // before the diff rewrites them (it exits between batches)
        latency::cancel_and_wait(profile_id, SCAN_CANCEL_TIMEOUT);
    }

    let state = app.state::<AppState>();
    let mut conn = state
        .db
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;
    let stored = match existing {
        Some(profile_id) => {
            let update = profiles::apply_update(&mut conn, profile_id, &content)?;
            StoredSource::Updated(profile_id, update.set_changed)
        }
        None => {
            let summary = profiles::import_profile(
                &mut conn,
                source.name.clone(),
                Some(source.url.clone()),
                None,
                &content,
            )?;
            StoredSource::Created(summary.id)
        }
    };
    conn.execute(
        "UPDATE discovery_sources
         SET profile_id = COALESCE(?2, profile_id),
             last_run_at = datetime('now'), last_error = NULL,
             last_item_count = ?3, updated_at = datetime('now')
         WHERE id = ?1",
        params![source.id, source_profile_id(&stored), item_count],
    )
    .map_err(db_err)?;
    profiles::after_mutation(app, Some(source_profile_id(&stored)), profiles::CONTENT);
    Ok(stored)
}

fn source_profile_id(stored: &StoredSource) -> i64 {
    match stored {
        StoredSource::Created(profile_id) | StoredSource::Updated(profile_id, _) => *profile_id,
    }
}

fn record_source_error(app: &AppHandle, source_id: i64, error: &str) {
    let recorded = app.try_state::<AppState>().and_then(|state| {
        state.db.lock().ok().and_then(|conn| {
            conn.execute(
                "UPDATE discovery_sources
                 SET last_run_at = datetime('now'), last_error = ?2, updated_at = datetime('now')
                 WHERE id = ?1",
                params![source_id, error],
            )
            .ok()
        })
    });
    if recorded.is_none() {
        eprintln!("[discovery] failed to record the source error: {error}");
    }
}

/// Runs the optional full-scope latency scan of every touched profile, one
/// at a time in position order; a scan error just moves on to the next.
async fn testing_phase(app: &AppHandle, touched: Vec<(i64, i64, String)>, cancel: &AtomicBool) {
    let mut order = touched;
    order.sort_by_key(|(position, _, _)| *position);
    for (_, profile_id, name) in order {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        update_run(|state| {
            state.status.test_profile_id = Some(profile_id);
            state.status.test_profile_name = Some(name.clone());
        });
        let _ = app.emit(
            DISCOVERY_PROGRESS_EVENT,
            DiscoveryProgress::ScanStarted { profile_id, name: name.clone() },
        );
        let _ = latency::run_scan(app, profile_id, ScanScope::All).await;
        update_run(|state| {
            state.status.test_profile_id = None;
            state.status.test_profile_name = None;
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_sources(app: &AppHandle) -> Result<Vec<SourceJob>, String> {
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, url, name, position, profile_id
             FROM discovery_sources WHERE enabled = 1 ORDER BY position, id",
        )
        .map_err(db_err)?;
    let sources = stmt
        .query_map([], |row| {
            Ok(SourceJob {
                id: row.get(0)?,
                url: row.get(1)?,
                name: row.get(2)?,
                position: row.get(3)?,
                profile_id: row.get(4)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(sources)
}

fn sqlite_now(app: &AppHandle) -> String {
    app.try_state::<AppState>()
        .and_then(|state| {
            state
                .db
                .lock()
                .ok()
                .and_then(|conn| {
                    conn.query_row("SELECT datetime('now')", [], |row| row.get::<_, String>(0))
                        .ok()
                })
        })
        .unwrap_or_default()
}

fn profile_still_exists(app: &AppHandle, profile_id: i64) -> bool {
    app.try_state::<AppState>()
        .and_then(|state| {
            state
                .db
                .lock()
                .ok()
                .map(|conn| profile_exists(&conn, profile_id))
        })
        .unwrap_or(false)
}

fn profile_exists(conn: &Connection, profile_id: i64) -> bool {
    conn.query_row("SELECT 1 FROM profiles WHERE id = ?1", params![profile_id], |_| Ok(()))
        .is_ok()
}

/// Rebuilds a subscription payload that parses back to exactly the deduped
/// endpoints: raw links joined by lines, or a whole `{"outbounds":[…]}`
/// config for native sing-box sources (whose raw is per-outbound JSON a
/// link parser cannot re-read).
fn rebuild_content(endpoints: &[NewEndpoint], native_config: bool) -> String {
    if native_config {
        let outbounds = endpoints
            .iter()
            .map(|endpoint| endpoint.outbound_json.as_str())
            .collect::<Vec<_>>()
            .join(",");
        format!("{{\"outbounds\":[{outbounds}]}}")
    } else {
        endpoints
            .iter()
            .map(|endpoint| endpoint.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_lists_round_trip_through_rebuilt_content() {
        let content = "vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@104.18.47.113:80?security=none&type=ws&path=%2F%3Fed%3D2560&host=example.dev#One\n\
                       vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@104.18.47.114:443?security=tls#Two";
        let parsed = parser::parse_subscription(content);
        assert_eq!(parsed.endpoints.len(), 2);
        let deduped = dedup_endpoints(parsed.endpoints);
        let rebuilt = rebuild_content(&deduped, false);
        let reparsed = parser::parse_subscription(&rebuilt);
        let raws: Vec<&str> = reparsed.endpoints.iter().map(|e| e.raw.as_str()).collect();
        let expected: Vec<&str> = deduped.iter().map(|e| e.raw.as_str()).collect();
        assert_eq!(raws, expected);
    }

    #[test]
    fn native_configs_round_trip_as_an_outbound_array() {
        let config = r#"{"outbounds":[
            {"type":"socks","tag":"a","server":"a.example","server_port":1080},
            {"type":"socks","tag":"b","server":"b.example","server_port":1081}
        ]}"#;
        let parsed = parser::parse_subscription(config);
        assert_eq!(parsed.endpoints.len(), 2);
        let rebuilt = rebuild_content(&parsed.endpoints, true);
        let reparsed = parser::parse_subscription(&rebuilt);
        assert_eq!(reparsed.endpoints.len(), 2);
        let raws: Vec<&str> = reparsed.endpoints.iter().map(|e| e.raw.as_str()).collect();
        let expected: Vec<&str> = parsed.endpoints.iter().map(|e| e.raw.as_str()).collect();
        assert_eq!(raws, expected);
        assert_eq!(reparsed.endpoints[0].server, "a.example");
        assert_eq!(reparsed.endpoints[0].server_port, 1080);
    }
}
