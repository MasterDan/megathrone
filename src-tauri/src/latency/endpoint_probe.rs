use std::path::PathBuf;

use rusqlite::params;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use super::clash::{TEST_TIMEOUT, url_test_chunk};
use super::instance::{
    STARTUP_TIMEOUT, SingBoxGuard, build_test_config, free_local_port, run_sing_box_check,
    sidecar, tag_for, wait_for_api, write_config_file,
};
use super::storage::{apply_url_probes, db_err};
use crate::AppState;
use crate::profiles;
use crate::routing;

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
