//! The per-strategy test runner — the Test button on the Settings → DPI
//! tab. For every stored strategy a throwaway ciadpi instance is spawned
//! on a random local port with that strategy's argument line, fronted by a
//! throwaway sing-box instance (a single socks outbound pointing at ciadpi,
//! clash API enabled). Each site of every *dpi*-routed URL category
//! (Settings → Routing) is then fetched through the tunnel via the clash
//! `delay` endpoint — the same mechanism the endpoint availability scan
//! uses — and the per-site outcomes land in `dpi_url_results`, one pass
//! rate per strategy. A process-wide registry backs `dpi_test_status`
//! (adopting a running test after remount) and `dpi_cancel_test` (checked
//! between strategies).

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

use super::registry::{TestGuard, advance_test, begin_test};
use super::storage::store_strategy_results;
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiTestSummary {
    /// strategies actually tested (a cancelled run reports less)
    pub done: usize,
    pub total: usize,
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

        let result = test_strategy(strategy.id, &strategy.args, &site_list).await;

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
    strategy_id: i64,
    args: &str,
    site_list: &[(i64, String)],
) -> StrategyOutcome {
    match run_tunnel_test(args, strategy_id, site_list).await {
        Ok(probes) => StrategyOutcome { probes, error: None },
        Err(error) => StrategyOutcome { probes: Vec::new(), error: Some(error) },
    }
}

/// Spawns ciadpi + its sing-box front, probes every site, tears both down.
async fn run_tunnel_test(
    args: &str,
    strategy_id: i64,
    site_list: &[(i64, String)],
) -> Result<Vec<(i64, bool)>, String> {
    let dpi_port = latency::free_local_port()?;
    let api_port = latency::free_local_port()?;

    // the ciadpi sidecar with the strategy line (validation happened on
    // save; the app owns the listener flags)
    let runner = crate::process::sidecar("byedpi")
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
    let probed = front_and_probe(dpi_port, api_port, strategy_id, site_list).await;
    let _ = child.kill();
    probed
}

async fn front_and_probe(
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
    let runner = latency::sidecar()?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
