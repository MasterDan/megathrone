use std::sync::Arc;

use rusqlite::Connection;

use super::model::{DiscoveryRunStatus, DiscoverySource};
use super::run;
use super::settings::DiscoverySettings;
use super::storage;
use crate::AppState;
use crate::env;
use crate::profiles;

// ---------------------------------------------------------------------------
// Tauri commands (thin wrappers over the storage layer and the run)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn discovery_sources_list() -> Result<Vec<DiscoverySource>, String> {
    let state = env::state();
    let conn = lock_db(&state)?;
    storage::list_sources(&conn)
}

#[tauri::command]
pub fn discovery_source_add(
    url: String,
    name: Option<String>,
    merge_group: Option<String>,
) -> Result<DiscoverySource, String> {
    let state = env::state();
    let conn = lock_db(&state)?;
    storage::add_source(&conn, &url, name.as_deref(), merge_group.as_deref())
}

#[tauri::command]
pub fn discovery_source_update(
    id: i64,
    url: Option<String>,
    name: Option<String>,
    enabled: Option<bool>,
) -> Result<DiscoverySource, String> {
    let state = env::state();
    let conn = lock_db(&state)?;
    storage::update_source(&conn, id, url, name, enabled)
}

#[tauri::command]
pub fn discovery_source_delete(id: i64, delete_profile: bool) -> Result<(), String> {
    let deleted_profile = {
        let state = env::state();
        let conn = lock_db(&state)?;
        storage::delete_source(&conn, id, delete_profile)?
    };
    if let Some(profile_id) = deleted_profile {
        profiles::after_mutation(Some(profile_id), profiles::META);
    }
    Ok(())
}

/// Removes a whole merge group (all its member sources) with the shared
/// profile.
#[tauri::command]
pub fn discovery_source_delete_group(group: String) -> Result<(), String> {
    let deleted_profile = {
        let state = env::state();
        let conn = lock_db(&state)?;
        storage::delete_group(&conn, &group)?
    };
    if let Some(profile_id) = deleted_profile {
        profiles::after_mutation(Some(profile_id), profiles::META);
    }
    Ok(())
}

/// Starts a background discovery run; returns immediately. One run at a
/// time — a second attempt while one is active errors out.
#[tauri::command]
pub fn discovery_run(test_after: bool) -> Result<(), String> {
    run::start_run(test_after)
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

#[tauri::command]
pub fn discovery_get_settings() -> Result<DiscoverySettings, String> {
    let state = env::state();
    let conn = lock_db(&state)?;
    super::settings::load(&conn)
}

/// Persists the Run card's options (the "test after" default and the
/// auto-update interval) and re-arms the scheduler — a freshly set
/// interval may already be due.
#[tauri::command]
pub fn discovery_set_settings(settings: DiscoverySettings) -> Result<DiscoverySettings, String> {
    {
        let state = env::state();
        let conn = lock_db(&state)?;
        super::settings::save(&conn, &settings)?;
    }
    env::state().notify_auto_update();
    let state = env::state();
    let conn = lock_db(&state)?;
    super::settings::load(&conn)
}

fn lock_db(state: &Arc<AppState>) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}
