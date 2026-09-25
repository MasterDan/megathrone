// The Tauri commands of Settings → General. Each `settings_set_*`
// command owns one card of the tab and persists a merged update (see
// `settings_set`).

use rusqlite::Connection;
use tauri::{AppHandle, State};

use super::general::{load_general, store_general, validate_general, GeneralSettings};
use crate::AppState;
use crate::connection;

#[tauri::command]
pub fn settings_get_general(state: State<AppState>) -> Result<GeneralSettings, String> {
    let conn = lock_db(&state)?;
    load_general(&conn)
}

/// Persists a merged update: loads the stored settings, overlays `patch`,
/// validates the whole struct and writes every key back. `settings_set_*`
/// commands each own one card of the General tab, so saving one card never
/// commits (or rejects on) the other card's pending edits.
fn settings_set(
    state: State<AppState>,
    patch: impl FnOnce(&mut GeneralSettings),
) -> Result<GeneralSettings, String> {
    let conn = lock_db(&state)?;
    let mut settings = load_general(&conn)?;
    patch(&mut settings);
    validate_general(&settings)?;
    store_general(&conn, &settings)?;
    Ok(settings)
}

#[tauri::command]
pub fn settings_set_cadences(
    state: State<AppState>,
    rotation_minutes: i64,
    recheck_minutes: i64,
    min_availability_percent: i64,
) -> Result<GeneralSettings, String> {
    settings_set(state, |settings| {
        settings.rotation_minutes = rotation_minutes;
        settings.recheck_minutes = recheck_minutes;
        settings.min_availability_percent = min_availability_percent;
    })
}

#[tauri::command]
pub fn settings_set_close_to_tray(
    state: State<AppState>,
    close_to_tray: bool,
) -> Result<GeneralSettings, String> {
    settings_set(state, |settings| {
        settings.close_to_tray = close_to_tray;
    })
}

/// Saves the raw local proxy card (toggle + port). While a session with a
/// dialed endpoint runs, a change that alters the generated config (toggle
/// flip, port change) reconnects it — the outcome lands in the global
/// toast; a no-op save never restarts.
#[tauri::command]
pub async fn settings_set_raw_proxy(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
    port: i64,
) -> Result<GeneralSettings, String> {
    let before = {
        let conn = lock_db(&state)?;
        load_general(&conn)?
    };
    let settings = {
        let conn = lock_db(&state)?;
        let mut settings = load_general(&conn)?;
        settings.raw_proxy_enabled = enabled;
        settings.raw_proxy_port = port;
        validate_general(&settings)?;
        store_general(&conn, &settings)?;
        settings
    };
    apply_raw_proxy_change_live(
        &app,
        (before.raw_proxy_enabled, before.raw_proxy_port),
        (settings.raw_proxy_enabled, settings.raw_proxy_port),
    )
    .await;
    Ok(settings)
}

/// A raw-proxy setting just changed: reconnect the live session iff the
/// inbound this change would bake differs from what the config holds now —
/// the toggle flipped, or the port changed while on. Nothing running, a
/// DPI-only session (no endpoint → no raw inbound in its config) and
/// no-op saves leave the session alone. Reports via `restart-result`.
async fn apply_raw_proxy_change_live(
    app: &AppHandle,
    before: (bool, i64),
    after: (bool, i64),
) {
    let baked = |(enabled, port): (bool, i64)| enabled.then_some(port);
    if baked(before) == baked(after) {
        return;
    }
    let Some(profile_id) = connection::running_profile() else {
        return;
    };
    if connection::snapshot().endpoint_tag.is_none() {
        return;
    }
    match connection::restart_if_running(app.clone(), profile_id).await {
        Ok(Some(_)) | Ok(None) => {
            let message = match connection::snapshot().raw_proxy_port {
                Some(port) => format!("reconnected — local proxy on 127.0.0.1:{port}"),
                None => "reconnected — local proxy off".to_string(),
            };
            connection::emit_restart_result(app, true, message);
        }
        Err(error) => connection::emit_restart_result(app, false, error),
    }
}

fn lock_db<'a>(state: &'a State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}
