use rusqlite::Connection;
use tauri::{AppHandle, Manager, State};

use crate::AppState;
use crate::connection;
use crate::db::set_setting;
use crate::routing;

use super::settings::{SETTING_DPI_ENABLED, dpi_enabled, load_dpi_port};
use super::strategies::{
    DpiStrategy, add_strategy, db_err, delete_strategy, list_strategies, load_active_strategy,
    select_strategy, update_strategy,
};
use super::{DpiSettings, parse_strategy_args, validate_name};

// ---------------------------------------------------------------------------
// Tauri commands (thin wrappers over the storage layer)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn dpi_list_strategies(state: State<AppState>) -> Result<Vec<DpiStrategy>, String> {
    let conn = lock_db(&state)?;
    list_strategies(&conn)
}

#[tauri::command]
pub fn dpi_add_strategy(
    state: State<AppState>,
    name: String,
    args: String,
) -> Result<DpiStrategy, String> {
    let name = validate_name(&name)?;
    let args = parse_strategy_args(&args)?;
    let conn = lock_db(&state)?;
    add_strategy(&conn, &name, &args)
}

/// Editing a strategy's argument line only touches the live session when
/// it is the active one.
#[tauri::command]
pub async fn dpi_update_strategy(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    name: String,
    args: String,
) -> Result<DpiStrategy, String> {
    let name = validate_name(&name)?;
    let args = parse_strategy_args(&args)?;
    let (updated, was_active) = {
        let conn = lock_db(&state)?;
        let updated = update_strategy(&conn, id, &name, &args)?;
        let was_active = updated.is_active;
        (updated, was_active)
    };
    if was_active {
        apply_dpi_change_live(&app).await;
    }
    Ok(updated)
}

/// Deleting the active strategy takes the tunnel down with it (the session
/// restarts if it ran the tunnel).
#[tauri::command]
pub async fn dpi_delete_strategy(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    let was_active = {
        let conn = lock_db(&state)?;
        let was_active = list_strategies(&conn)?
            .into_iter()
            .find(|strategy| strategy.id == id)
            .is_some_and(|strategy| strategy.is_active);
        delete_strategy(&conn, id)?;
        was_active
    };
    if was_active {
        apply_dpi_change_live(&app).await;
    }
    Ok(())
}

/// Marks exactly one strategy as the one to use (idempotent). A running
/// session restarts with the new tunnel when its DPI involvement changes —
/// the outcome lands in the global toast.
#[tauri::command]
pub async fn dpi_select_strategy(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    {
        let conn = lock_db(&state)?;
        select_strategy(&conn, id)?;
    }
    apply_dpi_change_live(&app).await;
    Ok(())
}

#[tauri::command]
pub fn dpi_get_settings(state: State<AppState>) -> Result<DpiSettings, String> {
    let conn = lock_db(&state)?;
    Ok(DpiSettings {
        port: load_dpi_port(&conn)?,
        enabled: dpi_enabled(&conn)?,
    })
}

#[tauri::command]
pub async fn dpi_set_port(
    app: AppHandle,
    state: State<'_, AppState>,
    port: u16,
) -> Result<DpiSettings, String> {
    if port == 0 {
        return Err("port must be between 1 and 65535".into());
    }
    {
        let conn = lock_db(&state)?;
        set_setting(&conn, "dpi_port", &port.to_string()).map_err(db_err)?;
    }
    apply_dpi_change_live(&app).await;
    Ok(DpiSettings {
        port,
        enabled: {
            let conn = lock_db(&state)?;
            dpi_enabled(&conn)?
        },
    })
}

/// Flips the DPI master toggle: off — the tunnel never starts and the main
/// connect button yields proxy-only sessions. Idempotent. A running session
/// restarts with or without the tunnel accordingly — the outcome lands in
/// the global toast.
#[tauri::command]
pub async fn dpi_set_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<DpiSettings, String> {
    {
        let conn = lock_db(&state)?;
        set_setting(&conn, SETTING_DPI_ENABLED, &(enabled as i64).to_string()).map_err(db_err)?;
    }
    apply_dpi_change_live(&app).await;
    Ok(DpiSettings {
        port: {
            let conn = lock_db(&state)?;
            load_dpi_port(&conn)?
        },
        enabled,
    })
}

/// A DPI-affecting setting just changed: restart the live session iff the
/// tunnel's runtime involvement actually changes — it runs now but would
/// not anymore (toggled off / strategy lost), or it does not run but would
/// now (toggled on / strategy picked / routing uses DPI), or it keeps
/// running with different parameters (strategy or port swapped). Everything
/// else (e.g. DPI toggled while nothing uses it) leaves the session alone.
/// Reports via `restart-result`.
async fn apply_dpi_change_live(app: &AppHandle) {
    let Some(profile_id) = connection::running_profile() else {
        return;
    };
    let runs_now = connection::dpi_tunnel_running();
    let would_run = {
        let Some(state) = app.try_state::<AppState>() else { return };
        let Ok(conn) = state.db.lock() else { return };
        let enabled = dpi_enabled(&conn).unwrap_or(false);
        let strategy = load_active_strategy(&conn).ok().flatten();
        let uses_dpi = routing::load_routing(&conn)
            .map(|config| routing::dpi_used(&config))
            .unwrap_or(false);
        enabled && strategy.is_some() && uses_dpi
    };
    if !runs_now && !would_run {
        return;
    }
    match connection::restart_if_running(app.clone(), profile_id).await {
        Ok(Some(_)) | Ok(None) => {
            let snapshot = connection::snapshot();
            let message = match &snapshot.dpi {
                Some(dpi) => format!("reconnected — DPI tunnel: {}", dpi.strategy),
                None => "reconnected — DPI tunnel off".to_string(),
            };
            connection::emit_restart_result(app, true, message);
        }
        Err(error) => connection::emit_restart_result(app, false, error),
    }
}

fn lock_db<'a>(state: &'a State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}
