//! The discovery run — the background sweep behind the Discover button.
//! Every enabled source is fetched (bounded parallelism), its insecure
//! lines dropped, the links parsed and deduped, and the result stored as
//! ONE profile per source: a diff update on the source's linked profile
//! (`apply_update` keeps every surviving endpoint's latency results), a
//! fresh import otherwise. Sources sharing a `merge_group` (the seeded
//! whitelist-bypass extras) store as ONE shared profile instead: members
//! buffer until the whole group arrived, their endpoints are concatenated
//! and re-deduped (host:port collisions across members drop), the first
//! member by position naming the profile; a failed member URL is recorded
//! on its row and simply skipped. Optionally, after all sources, each
//! touched profile gets a full latency scan, sequentially. One run at a
//! time; the process-wide registry keeps the final status around (a
//! remounted page adopts it via `discovery_run_status`) until the next
//! run replaces it.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, params};
use tauri::{AppHandle, Emitter, Manager};

use super::filter::{dedup_endpoints, filter_lines};
use super::model::{DiscoveryProgress, DiscoveryRunStatus, DISCOVERY_PROGRESS_EVENT};
use super::settings::KEY_LAST_RUN_AT;
use super::storage::{db_err, normalize_discovery_order};
use crate::db::set_setting;
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

/// The rejection `start_run` reports while another run is active; the
/// scheduler treats it as "nothing to do", not a failure.
const ALREADY_RUNNING: &str = "discovery run already in progress";

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
            return Err(ALREADY_RUNNING.to_string());
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

/// The scheduler's leg: starts a run when one is due (the settings'
/// `test_after` decides the follow-up scans) and reports the interval it
/// armed — the run stamps `discovery_last_run_at` asynchronously, so the
/// caller must compute the next wake from the interval, not from the KV.
/// `Ok(None)` — nothing scheduled, not yet due, or a run already in
/// progress; any other start failure counts into the scheduler's
/// backoff.
pub fn run_if_due(app: &AppHandle) -> Result<Option<i64>, String> {
    let (due, settings) = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        (
            super::settings::next_due_seconds(&conn)?,
            super::settings::load(&conn)?,
        )
    };
    let Some(interval) = settings.auto_update_minutes else {
        return Ok(None);
    };
    if !matches!(due, Some(seconds) if seconds <= 0.0) {
        return Ok(None);
    }
    match start_run(app.clone(), settings.test_after) {
        Ok(()) => Ok(Some(interval)),
        Err(error) if error == ALREADY_RUNNING => Ok(None),
        Err(error) => Err(error),
    }
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

struct SourceJob {
    id: i64,
    url: String,
    name: String,
    position: i64,
    merge_group: Option<String>,
    profile_id: Option<i64>,
}

/// One source's fetch outcome after parsing: the deduped endpoints plus
/// whether the payload was a native sing-box config.
type ParsedSource = Result<(Vec<NewEndpoint>, bool), String>;

/// A merge group assembled for storage: the successful members with
/// their own endpoint counts (sorted by position — the first owns the
/// profile), the concatenated re-deduped endpoints, and whether the
/// rebuild must be a native config.
type AssembledGroup = (Vec<(SourceJob, usize)>, Vec<NewEndpoint>, bool);

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
    record_last_run_at(&app, &started_at);
    update_run(|state| {
        state.status.started_at = started_at;
        state.status.total = storage_units(&sources);
    });

    let mut outcome = FetchOutcome::default();
    if !sources.is_empty() {
        outcome = fetch_phase(&app, sources, &cancel).await;
        if outcome.created > 0 {
            normalize_created_order(&app);
        }
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

/// The fetch phase stores results in network completion order, so freshly
/// imported profiles land in the sidebar however the parallel fetches
/// raced. A run that created any re-normalizes: the discovery-linked
/// profiles go to the tail of the sidebar in their source's position
/// order, every other profile keeps its relative order ahead of them.
/// Non-fatal — on failure the sidebar just keeps the completion order.
fn normalize_created_order(app: &AppHandle) {
    let normalized = app.try_state::<AppState>().and_then(|state| {
        let conn = state.db.lock().ok()?;
        normalize_discovery_order(&conn).ok()
    });
    match normalized {
        Some(()) => profiles::after_mutation(app, None, profiles::META),
        None => eprintln!("[discovery] failed to normalize the created profiles' order"),
    }
}

/// Fetches every source with bounded parallelism; results are processed as
/// they arrive (completion order). A merge group's members buffer until
/// the whole group arrived, then store as one shared profile.
async fn fetch_phase(
    app: &AppHandle,
    sources: Vec<SourceJob>,
    cancel: &Arc<AtomicBool>,
) -> FetchOutcome {
    let total = sources.len();
    let mut expected: HashMap<String, usize> = HashMap::new();
    for source in &sources {
        if let Some(group) = source.merge_group.as_deref() {
            *expected.entry(group.to_string()).or_default() += 1;
        }
    }
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
    let mut pending_groups: HashMap<String, Vec<(SourceJob, ParsedSource)>> = HashMap::new();
    while received < total {
        let Some((source, result)) = rx.recv().await else { break };
        received += 1;
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let parsed = match result {
            Ok(content) => parse_source(&content),
            Err(error) => Err(error),
        };
        let Some(group) = source.merge_group.clone() else {
            process_single_source(app, &mut outcome, source, parsed).await;
            continue;
        };
        record_source_outcome(app, &mut outcome, &source, &parsed);
        let batch = pending_groups.entry(group.clone()).or_default();
        batch.push((source, parsed));
        if batch.len() == expected[&group] {
            let batch = pending_groups.remove(&group).expect("the batch was just inserted");
            store_group(app, &mut outcome, group.clone(), batch).await;
        }
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

/// One fetched payload → deduped endpoints: insecure lines dropped, links
/// parsed, per-source dedup, and whether the payload was a native
/// sing-box config.
fn parse_source(content: &str) -> ParsedSource {
    let (filtered, _removed) = filter_lines(content);
    let native_config = filtered.trim_start().starts_with('{');
    let endpoints = dedup_endpoints(parser::parse_subscription(&filtered).endpoints);
    if endpoints.is_empty() {
        Err("no supported endpoints found".to_string())
    } else {
        Ok((endpoints, native_config))
    }
}

enum StoredSource {
    Created(i64),
    /// (profile_id, set_changed)
    Updated(i64, bool),
}

/// The ungrouped path: one source's parsed result straight into its own
/// profile (the single-member case of the shared store).
async fn process_single_source(
    app: &AppHandle,
    outcome: &mut FetchOutcome,
    source: SourceJob,
    parsed: ParsedSource,
) {
    let source_id = source.id;
    let source_name = source.name.clone();
    let source_position = source.position;
    let mut item_count = 0usize;
    let stored = match parsed {
        Ok((endpoints, native_config)) => {
            item_count = endpoints.len();
            let members = vec![(source, item_count)];
            let app = app.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                store_members_result(&app, members, endpoints, native_config)
            })
            .await
            {
                Ok(stored) => stored,
                Err(error) => Err(format!("discovery task failed: {error}")),
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
                    name: source_name.clone(),
                    ok: false,
                    item_count: 0,
                    error: Some(error),
                },
            );
            return;
        }
    };

    finish_stored_source(app, outcome, source_position, source_name.clone(), stored);
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

/// The per-URL bookkeeping of a merge-group member: its run-status
/// failure (a failed URL is skipped, not fatal for the group — the unit's
/// `done` counting happens at the group's completion, not per member) and
/// the SourceDone event. The profile write happens once the whole group
/// arrived (`store_group`).
fn record_source_outcome(
    app: &AppHandle,
    outcome: &mut FetchOutcome,
    source: &SourceJob,
    parsed: &ParsedSource,
) {
    match parsed {
        Ok((endpoints, _)) => {
            let _ = app.emit(
                DISCOVERY_PROGRESS_EVENT,
                DiscoveryProgress::SourceDone {
                    source_id: source.id,
                    name: source.name.clone(),
                    ok: true,
                    item_count: endpoints.len(),
                    error: None,
                },
            );
        }
        Err(error) => {
            outcome.failed += 1;
            update_run(|state| state.status.failed += 1);
            record_source_error(app, source.id, error);
            let _ = app.emit(
                DISCOVERY_PROGRESS_EVENT,
                DiscoveryProgress::SourceDone {
                    source_id: source.id,
                    name: source.name.clone(),
                    ok: false,
                    item_count: 0,
                    error: Some(error.clone()),
                },
            );
        }
    }
}

/// Stores a complete merge group as ONE shared profile and counts it.
/// Every member failed → nothing to store (each row already carries its
/// own error), the unit still completes.
async fn store_group(
    app: &AppHandle,
    outcome: &mut FetchOutcome,
    group: String,
    batch: Vec<(SourceJob, ParsedSource)>,
) {
    // the group's owner is its first member by position — also when every
    // fetch failed, the unit still narrates under its name
    let owner_name = batch
        .iter()
        .min_by_key(|(job, _)| (job.position, job.id))
        .map(|(job, _)| job.name.clone())
        .unwrap_or_default();
    let Some((members, endpoints, native_config)) = assemble_group(batch) else {
        update_run(|state| state.status.done += 1);
        let _ = app.emit(
            DISCOVERY_PROGRESS_EVENT,
            DiscoveryProgress::GroupDone {
                group,
                name: owner_name,
                ok: false,
                item_count: 0,
                error: Some("all group members failed".to_string()),
            },
        );
        return;
    };
    let owner_position = members[0].0.position;
    let item_count = endpoints.len();
    let member_ids: Vec<i64> = members.iter().map(|(job, _)| job.id).collect();
    let result = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            store_members_result(&app, members, endpoints, native_config)
        })
        .await
    };
    match result {
        Ok(stored) => {
            let stored = match stored {
                Ok(stored) => stored,
                Err(error) => {
                    record_store_error(app, &member_ids, &error, outcome);
                    emit_group_done(app, &group, &owner_name, false, 0, Some(&error));
                    return;
                }
            };
            finish_stored_source(app, outcome, owner_position, owner_name.clone(), stored);
            emit_group_done(app, &group, &owner_name, true, item_count, None);
        }
        Err(error) => {
            let error = format!("discovery task failed: {error}");
            record_store_error(app, &member_ids, &error, outcome);
            emit_group_done(app, &group, &owner_name, false, 0, Some(&error));
        }
    }
}

fn emit_group_done(
    app: &AppHandle,
    group: &str,
    name: &str,
    ok: bool,
    item_count: usize,
    error: Option<&str>,
) {
    let _ = app.emit(
        DISCOVERY_PROGRESS_EVENT,
        DiscoveryProgress::GroupDone {
            group: group.to_string(),
            name: name.to_string(),
            ok,
            item_count,
            error: error.map(str::to_string),
        },
    );
}

/// A shared store that failed after its members already reported ok: the
/// unit completes failed, every member row carries the error so the UI
/// shows it after the refetch.
fn record_store_error(
    app: &AppHandle,
    member_ids: &[i64],
    error: &str,
    outcome: &mut FetchOutcome,
) {
    outcome.failed += 1;
    update_run(|state| {
        state.status.done += 1;
        state.status.failed += 1;
    });
    for id in member_ids {
        record_source_error(app, *id, error);
    }
    eprintln!("[discovery] failed to store a merge group: {error}");
}

/// The common tail of a successful store: the unit's `done`, the
/// created/updated counters and the touched list for the testing phase,
/// plus the silent live-session rebuild on a set change (like the
/// scheduler's). Progress events stay with the per-URL outcomes.
fn finish_stored_source(
    app: &AppHandle,
    outcome: &mut FetchOutcome,
    source_position: i64,
    source_name: String,
    stored: StoredSource,
) {
    update_run(|state| state.status.done += 1);
    let profile_id = source_profile_id(&stored);
    match stored {
        StoredSource::Created(_) => {
            outcome.created += 1;
            update_run(|state| {
                state.status.created += 1;
            });
        }
        StoredSource::Updated(_, set_changed) => {
            outcome.updated += 1;
            update_run(|state| {
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
    outcome.touched.push((source_position, profile_id, source_name));
}

/// Assembles a complete merge group: the successful members in position
/// order (the first is the group's owner — its name and linked profile
/// are the group's), their endpoint lists concatenated and re-deduped
/// (host:port collisions across members drop; the earlier member wins),
/// `native_config` when any member needs it (the lossless rebuild for
/// mixed groups). `None` when no member produced endpoints.
fn assemble_group(batch: Vec<(SourceJob, ParsedSource)>) -> Option<AssembledGroup> {
    let mut ok: Vec<(SourceJob, Vec<NewEndpoint>, bool)> = batch
        .into_iter()
        .filter_map(|(job, parsed)| {
            parsed.ok().map(|(endpoints, native)| (job, endpoints, native))
        })
        .collect();
    if ok.is_empty() {
        return None;
    }
    ok.sort_by_key(|(job, _, _)| (job.position, job.id));
    let native_config = ok.iter().any(|(_, _, native)| *native);
    let mut members = Vec::with_capacity(ok.len());
    let mut concatenated = Vec::new();
    for (job, endpoints, _) in ok {
        members.push((job, endpoints.len()));
        concatenated.extend(endpoints);
    }
    Some((members, dedup_endpoints(concatenated), native_config))
}

/// Applies the deduped content of one profile to the DB (blocking — runs
/// inside `spawn_blocking`): a diff update when the owner's linked
/// profile still exists, a fresh import otherwise, plus every member
/// row's bookkeeping. `members` is sorted by position — the first owns
/// the profile (its name, its link); each member row records its own
/// endpoint count.
fn store_members_result(
    app: &AppHandle,
    members: Vec<(SourceJob, usize)>,
    endpoints: Vec<NewEndpoint>,
    native_config: bool,
) -> Result<StoredSource, String> {
    let content = rebuild_content(&endpoints, native_config);
    let owner = &members[0].0;

    // a dangling link (the profile was deleted elsewhere) imports afresh
    let existing = owner
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
            let summary =
                profiles::import_discovery_profile(&mut conn, owner.name.clone(), &content)?;
            StoredSource::Created(summary.id)
        }
    };
    let profile_id = source_profile_id(&stored);
    for (member, member_count) in &members {
        conn.execute(
            "UPDATE discovery_sources
             SET profile_id = ?2, last_run_at = datetime('now'), last_error = NULL,
                 last_item_count = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![member.id, profile_id, *member_count as i64],
        )
        .map_err(db_err)?;
    }
    profiles::after_mutation(app, Some(profile_id), profiles::CONTENT);
    // an automatic strategy re-picks over the fresh rows, keeping the
    // current endpoint while it survived the update
    crate::auto_select::content_updated(profile_id);
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

/// The run's progress units: one per ungrouped source plus one per merge
/// group (its members store as ONE shared profile). This is what the
/// run's done/total narrate — not the raw URL count.
fn storage_units(sources: &[SourceJob]) -> usize {
    let mut groups: HashSet<&str> = HashSet::new();
    let mut units = 0usize;
    for source in sources {
        match source.merge_group.as_deref() {
            Some(group) => {
                groups.insert(group);
            }
            None => units += 1,
        }
    }
    units + groups.len()
}

fn read_sources(app: &AppHandle) -> Result<Vec<SourceJob>, String> {
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, url, name, position, merge_group, profile_id
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
                merge_group: row.get(4)?,
                profile_id: row.get(5)?,
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

/// Stamps the KV key the scheduler's discovery wake is computed from —
/// every run (manual or scheduled) refreshes it. Non-fatal: a failed
/// stamp only skews the next scheduled run.
fn record_last_run_at(app: &AppHandle, started_at: &str) {
    let stamped = app.try_state::<AppState>().and_then(|state| {
        state
            .db
            .lock()
            .ok()
            .and_then(|conn| set_setting(&conn, KEY_LAST_RUN_AT, started_at).ok())
    });
    if stamped.is_none() {
        eprintln!("[discovery] failed to stamp the last-run timestamp");
    }
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
mod run_tests;
