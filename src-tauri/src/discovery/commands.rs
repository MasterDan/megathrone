use rusqlite::Connection;
use tauri::{AppHandle, State};

use super::model::{DiscoveryRunStatus, DiscoverySource};
use super::run;
use super::storage;
use crate::profiles;
use crate::AppState;

// ---------------------------------------------------------------------------
// Tauri commands (thin wrappers over the storage layer and the run)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn discovery_sources_list(state: State<AppState>) -> Result<Vec<DiscoverySource>, String> {
    let conn = lock_db(&state)?;
    storage::list_sources(&conn)
}

#[tauri::command]
pub fn discovery_source_add(
    _app: AppHandle,
    state: State<AppState>,
    url: String,
    name: Option<String>,
) -> Result<DiscoverySource, String> {
    let conn = lock_db(&state)?;
    storage::add_source(&conn, &url, name.as_deref())
}

#[tauri::command]
pub fn discovery_source_update(
    _app: AppHandle,
    state: State<AppState>,
    id: i64,
    url: Option<String>,
    name: Option<String>,
    enabled: Option<bool>,
) -> Result<DiscoverySource, String> {
    let conn = lock_db(&state)?;
    storage::update_source(&conn, id, url, name, enabled)
}

#[tauri::command]
pub fn discovery_source_delete(
    app: AppHandle,
    state: State<AppState>,
    id: i64,
    delete_profile: bool,
) -> Result<(), String> {
    let deleted_profile = {
        let conn = lock_db(&state)?;
        storage::delete_source(&conn, id, delete_profile)?
    };
    if let Some(profile_id) = deleted_profile {
        profiles::after_mutation(&app, Some(profile_id), profiles::META);
    }
    Ok(())
}

/// Starts a background discovery run; returns immediately. One run at a
/// time — a second attempt while one is active errors out.
#[tauri::command]
pub fn discovery_run(app: AppHandle, test_after: bool) -> Result<(), String> {
    run::start_run(app, test_after)
}

/// Snapshot of the current (or last finished) run; `None` before the first
/// one — a remounted page adopts a running run through it.
#[tauri::command]
pub fn discovery_run_status() -> Result<Option<DiscoveryRunStatus>, String> {
    run::status_snapshot()
}

/// Stops a running run between sources (and its current latency scan);
/// idempotent.
#[tauri::command]
pub fn discovery_run_cancel() -> Result<(), String> {
    run::request_cancel()
}

fn lock_db<'a>(state: &'a State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}
