//! Live reconfiguration of a running session: the restart-free endpoint
//! switch through the Clash API, profile switches and reconnects, and the
//! `restart-result` toast reporting shared by every user-initiated flow.

use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

use super::connect::connection_connect;
use super::selection::selected_endpoint_brief;
use super::state::{
    CONNECTION_CHANGED_EVENT, ConnectionSnapshot, FLOW, PROXY_TAG, current_snapshot, lock_state,
    snapshot_of,
};
use super::teardown::connection_disconnect;

/// One-shot report of a user-initiated live reconfiguration (endpoint pick,
/// strategy switch, DPI change, profile switch): the global toast shows it —
/// successes hide themselves, errors stay until dismissed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartResult {
    pub ok: bool,
    pub message: String,
}

pub const RESTART_RESULT_EVENT: &str = "restart-result";

pub fn emit_restart_result(app: &AppHandle, ok: bool, message: String) {
    let _ = app.emit(RESTART_RESULT_EVENT, RestartResult { ok, message });
}

/// Re-points the live session at another profile (the mode stays; the new
/// profile's strategy applies, its supervisor is armed by the connect
/// flow). Reports via `restart-result`. A no-op when the proxy is off or
/// already runs this profile.
#[tauri::command]
pub async fn connection_switch_profile(
    app: AppHandle,
    profile_id: i64,
) -> Result<ConnectionSnapshot, String> {
    let current = {
        let _flow = FLOW.lock().await;
        lock_state().ok().and_then(|state| {
            state
                .running
                .as_ref()
                .map(|run| (run.profile_id, run.mode.to_string()))
        })
    };
    let Some((running_id, mode)) = current else {
        return Ok(current_snapshot());
    };
    if running_id == profile_id {
        return Ok(current_snapshot());
    }

    let result = async {
        connection_disconnect(app.clone()).await?;
        connection_connect(app.clone(), profile_id, mode).await
    }
    .await;
    match result {
        Ok(fresh) => {
            let message = match (&fresh.profile_name, &fresh.endpoint_tag) {
                (Some(name), Some(tag)) => format!("switched to {name} — via {tag}"),
                (Some(name), None) => format!("switched to {name} — DPI only"),
                _ => "switched profile".to_string(),
            };
            emit_restart_result(&app, true, message);
        }
        Err(error) => emit_restart_result(&app, false, error),
    }
    Ok(current_snapshot())
}

/// Reconnects the running session when it uses this profile — the connect
/// flow re-reads the current settings, so endpoint/strategy/DPI changes
/// apply live. `Ok(Some(tag))` — the session now runs `tag` (`None` for a
/// DPI-only session); `Ok(None)` — nothing was running for this profile;
/// `Err` — the reconnect failed and the session is down (the reason is
/// also surfaced as `last_error` via `connection-changed`; the settings
/// themselves have already been persisted by the caller). Silent by
/// design: user-initiated flows report through `emit_restart_result`
/// themselves, background supervisor switches stay quiet.
pub async fn restart_if_running(
    app: AppHandle,
    profile_id: i64,
) -> Result<Option<String>, String> {
    let mode = {
        let _flow = FLOW.lock().await;
        lock_state().ok().and_then(|state| {
            state
                .running
                .as_ref()
                .filter(|run| run.profile_id == profile_id)
                .map(|run| run.mode.to_string())
        })
    };
    let Some(mode) = mode else { return Ok(None) };

    let result = async {
        connection_disconnect(app.clone()).await?;
        connection_connect(app.clone(), profile_id, mode)
            .await
            .map(|fresh| fresh.endpoint_tag)
    }
    .await;
    match result {
        Ok(tag) => Ok(tag),
        Err(error) => {
            let message = format!("reconnecting after a settings change failed: {error}");
            if let Ok(mut state) = lock_state() {
                state.last_error = Some(message.clone());
                let snapshot = snapshot_of(&state);
                let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
            }
            Err(message)
        }
    }
}

/// Applies a persisted endpoint change (strategy switch, rescue, a manual
/// pick) to the running session *without* a reconnect: the run config bakes
/// every endpoint behind the `mt-proxy` selector, so switching is a single
/// Clash API call — the listener, TUN device and system proxy stay as they
/// are, new connections ride the new endpoint immediately and open ones
/// drain on the old one (`interrupt_exist_connections` stays off). Falls
/// back to a reconnect when nothing runs, the target was not baked (fresh
/// content, DPI-only session) or the API call fails. `Ok(Some(tag))` — the
/// tag the session runs now; `Ok(None)` — nothing was running for this
/// profile; `Err` — the fallback reconnect failed. Like
/// `restart_if_running`, silent by design: callers report user-initiated
/// switches through `emit_restart_result` themselves.
pub async fn switch_endpoint_if_running(
    app: AppHandle,
    profile_id: i64,
) -> Result<Option<String>, String> {
    // (endpoint row id, display tag) of the stored selection; an unreadable
    // or cleared selection falls through to the reconnect path
    let selection: Option<(i64, String)> = app
        .state::<AppState>()
        .db
        .lock()
        .ok()
        .and_then(|conn| selected_endpoint_brief(&conn, profile_id).ok().flatten());
    let Some((id, display)) = selection else {
        return restart_if_running(app, profile_id).await;
    };
    let live = lock_state().ok().and_then(|state| {
        let run = state.running.as_ref().filter(|run| run.profile_id == profile_id)?;
        run.endpoint_tags.get(&id).map(|tag| (run.api_port, tag.clone()))
    });
    // nothing runs for this profile (the reconnect is a no-op), or the
    // endpoint was never baked into the live config
    let Some((api_port, tag)) = live else {
        return restart_if_running(app, profile_id).await;
    };
    let switched = match tauri::async_runtime::spawn_blocking(move || {
        select_on_clash_api(api_port, &tag)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(format!("endpoint switch failed: {error}")),
    };
    match switched {
        Ok(()) => {
            let snapshot = {
                let mut state = lock_state()?;
                if let Some(run) = state.running.as_mut() {
                    run.endpoint_tag = Some(display.clone());
                }
                snapshot_of(&state)
            };
            let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
            Ok(Some(display))
        }
        Err(error) => {
            eprintln!(
                "[connection] live endpoint switch failed ({error}) — reconnecting instead"
            );
            restart_if_running(app, profile_id).await
        }
    }
}

/// Points the running instance's `mt-proxy` selector at `tag` via the
/// Clash API. Blocking (ureq) — run inside `spawn_blocking`.
fn select_on_clash_api(api_port: u16, tag: &str) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .build()
        .into();
    let url = format!("http://127.0.0.1:{api_port}/proxies/{PROXY_TAG}");
    let response = agent
        .put(&url)
        .send_json(json!({ "name": tag }))
        .map_err(|error| format!("clash api select failed: {error}"))?;
    if response.status().as_u16() / 100 == 2 {
        Ok(())
    } else {
        Err(format!(
            "clash api refused the switch: HTTP {}",
            response.status()
        ))
    }
}
