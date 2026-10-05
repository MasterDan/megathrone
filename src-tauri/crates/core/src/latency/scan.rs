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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use super::clash::{TEST_TIMEOUT, delay_test_chunk, url_test_chunk};
use super::instance::{
    STARTUP_TIMEOUT, SingBoxGuard, build_test_config, free_local_port, isolate_bad_outbounds,
    run_sing_box_check, sidecar, tag_for, wait_for_api, write_config_file,
};
use super::registry::{ScanGuard, advance_scan, begin_scan};
use super::storage::{
    LatencyResult, UrlProbe, load_outbounds, load_reachable_ids, load_untested_ids, store_batch,
};
use crate::AppState;
use crate::profiles;
use crate::routing;

/// How many endpoints are tested in parallel per batch.
const CONCURRENCY: usize = 32;

pub const LATENCY_PROGRESS_EVENT: &str = "latency-progress";

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
pub enum ScanScope {
    All,
    ReachableOnly,
    UntestedOnly,
    ReachableAndUntested,
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
pub async fn run_scan(
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
