use rusqlite::Connection;
use tauri::{AppHandle, Manager, State};

use crate::connection;
use crate::AppState;

use super::endpoints::{
    endpoint_url_statuses, get_selected_endpoint, item_detail, item_index, list_items,
    set_selected_endpoint,
};
use super::model::{EndpointItem, EndpointUrlStatus, ItemDetail, ProfileSummary, SELECT_MANUAL};
use super::selection::{selection_mode, selection_mode_label, set_selection_mode};
use super::sources::{
    after_mutation, default_name_from_path, default_name_from_url, fetch_url, read_file, CONTENT,
    META, SELECTION,
};
use super::storage::{
    delete_profile, import_profile, list_profiles, rename_profile, set_auto_update, summary_by_id,
};
use super::update::{
    lock_running_updates, rebuild_session_after_update, update_profile_flow, UpdateTrigger,
};

// ---------------------------------------------------------------------------
// Tauri commands (thin wrappers over the storage layer)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn profiles_list(state: State<AppState>) -> Result<Vec<ProfileSummary>, String> {
    let conn = lock_db(&state)?;
    list_profiles(&conn)
}

#[tauri::command]
pub fn profile_get(state: State<AppState>, profile_id: i64) -> Result<ProfileSummary, String> {
    let conn = lock_db(&state)?;
    summary_by_id(&conn, profile_id)
}

#[tauri::command]
pub fn profile_import_from_url(
    app: AppHandle,
    state: State<AppState>,
    url: String,
    name: Option<String>,
) -> Result<ProfileSummary, String> {
    let content = fetch_url(&url)?;
    let name = name.unwrap_or_else(|| default_name_from_url(&url));
    let url = Some(url);
    let mut conn = lock_db(&state)?;
    import_profile(&mut conn, name, url, None, &content).inspect(|summary| {
        after_mutation(&app, Some(summary.id), CONTENT);
    })
}

#[tauri::command]
pub fn profile_import_from_file(
    app: AppHandle,
    state: State<AppState>,
    path: String,
    name: Option<String>,
) -> Result<ProfileSummary, String> {
    let content = read_file(&path)?;
    let name = name.unwrap_or_else(|| default_name_from_path(&path));
    let mut conn = lock_db(&state)?;
    import_profile(&mut conn, name, None, Some(path), &content).inspect(|summary| {
        after_mutation(&app, Some(summary.id), CONTENT);
    })
}

#[tauri::command]
pub fn profile_import_from_text(
    app: AppHandle,
    state: State<AppState>,
    content: String,
    name: String,
) -> Result<ProfileSummary, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("profile name must not be empty".into());
    }
    let mut conn = lock_db(&state)?;
    import_profile(&mut conn, name, None, None, &content).inspect(|summary| {
        after_mutation(&app, Some(summary.id), CONTENT);
    })
}

/// Manual update (the Update button). The live session, when the endpoint
/// set actually changed, is rebuilt right here and reported through the
/// global restart toast — like every user-initiated reconfiguration.
#[tauri::command]
pub async fn profile_update(app: AppHandle, profile_id: i64) -> Result<ProfileSummary, String> {
    let outcome = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            update_profile_flow(&app, profile_id, UpdateTrigger::Manual)
        })
            .await
            .map_err(|error| format!("update task failed: {error}"))?
    }?;
    rebuild_session_after_update(app, profile_id, outcome.set_changed, true).await;
    Ok(outcome.summary)
}

/// True while this profile is being updated right now — by the Update button
/// or by the background scheduler (both run `update_profile_flow`).
#[tauri::command]
pub fn profile_update_status(profile_id: i64) -> Result<bool, String> {
    Ok(lock_running_updates()?.contains(&profile_id))
}

#[tauri::command]
pub fn profile_rename(
    app: AppHandle,
    state: State<AppState>,
    profile_id: i64,
    new_name: String,
) -> Result<(), String> {
    let conn = lock_db(&state)?;
    rename_profile(&conn, profile_id, &new_name)?;
    after_mutation(&app, Some(profile_id), META);
    Ok(())
}

#[tauri::command]
pub fn profile_delete(app: AppHandle, state: State<AppState>, profile_id: i64) -> Result<(), String> {
    let conn = lock_db(&state)?;
    delete_profile(&conn, profile_id)?;
    after_mutation(&app, Some(profile_id), META);
    Ok(())
}

#[tauri::command]
pub fn profile_set_auto_update(
    app: AppHandle,
    state: State<AppState>,
    profile_id: i64,
    minutes: Option<i64>,
) -> Result<(), String> {
    let conn = lock_db(&state)?;
    set_auto_update(&conn, profile_id, minutes)?;
    after_mutation(&app, Some(profile_id), META);
    Ok(())
}

#[tauri::command]
pub fn profile_items(
    state: State<AppState>,
    profile_id: i64,
    offset: i64,
    limit: i64,
) -> Result<Vec<EndpointItem>, String> {
    let conn = lock_db(&state)?;
    list_items(&conn, profile_id, offset, limit)
}

/// 0-based position of one endpoint in the canonical order `profile_items`
/// returns — the endpoints page scrolls its grid to the selected card.
#[tauri::command]
pub fn profile_item_index(
    state: State<AppState>,
    profile_id: i64,
    item_id: i64,
) -> Result<Option<i64>, String> {
    let conn = lock_db(&state)?;
    item_index(&conn, profile_id, item_id)
}

#[tauri::command]
pub fn profile_item_detail(state: State<AppState>, item_id: i64) -> Result<ItemDetail, String> {
    let conn = lock_db(&state)?;
    item_detail(&conn, item_id)
}

/// Latest deep-probe outcomes for one endpoint (proxy-routed category
/// URLs); fetched lazily by the card popover/modal.
#[tauri::command]
pub fn profile_endpoint_urls(state: State<AppState>, item_id: i64) -> Result<Vec<EndpointUrlStatus>, String> {
    let conn = lock_db(&state)?;
    endpoint_url_statuses(&conn, item_id)
}

#[tauri::command]
pub fn profile_selected_endpoint(
    state: State<AppState>,
    profile_id: i64,
) -> Result<Option<EndpointItem>, String> {
    let conn = lock_db(&state)?;
    get_selected_endpoint(&conn, profile_id)
}

/// The profile's selection strategy (tabs on the endpoints page).
#[tauri::command]
pub fn profile_selection_mode(state: State<AppState>, profile_id: i64) -> Result<String, String> {
    let conn = lock_db(&state)?;
    selection_mode(&conn, profile_id)
}

/// Switches the strategy (the tabs). While the profile's session runs, the
/// strategy is applied right away — automatic modes pick per their rules
/// (round robin rotates now) and the session switches when the endpoint
/// changes; the outcome lands in the global toast.
#[tauri::command]
pub async fn profile_set_selection_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: i64,
    mode: String,
) -> Result<(), String> {
    {
        let conn = lock_db(&state)?;
        set_selection_mode(&conn, profile_id, &mode)?;
    }
    after_mutation(&app, Some(profile_id), SELECTION);
    if !connection::runs_profile(profile_id) {
        return Ok(());
    }
    if mode == SELECT_MANUAL {
        crate::auto_select::stop(profile_id);
        // the session keeps running the picked endpoint — nothing to rebuild
        let tag = {
            let state = app.state::<AppState>();
            let conn = lock_db(&state)?;
            get_selected_endpoint(&conn, profile_id)?
                .map(|item| item.tag)
                .unwrap_or_else(|| "no endpoint picked".to_string())
        };
        connection::emit_restart_result(&app, true, format!("Endpoint mode — {tag}"));
    } else {
        crate::auto_select::ensure(&app, profile_id);
        match crate::auto_select::apply_now(&app, profile_id, &mode).await {
            Ok(tag) => connection::emit_restart_result(
                &app,
                true,
                format!("{} — via {tag}", selection_mode_label(&mode)),
            ),
            Err(error) => connection::emit_restart_result(&app, false, error),
        }
    }
    Ok(())
}

/// Picks the endpoint (an explicit user choice always switches the profile
/// to manual — like tapping a server in Hiddify) and applies it live. While
/// connected the outcome lands in the global toast.
#[tauri::command]
pub async fn profile_select_endpoint(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: i64,
    item_id: Option<i64>,
) -> Result<(), String> {
    let changed = {
        let conn = lock_db(&state)?;
        set_selection_mode(&conn, profile_id, SELECT_MANUAL)?;
        set_selected_endpoint(&conn, profile_id, item_id)?
    };
    // the supervisor obeys manual mode; stop it right away instead of
    // waiting for its loop-top check
    crate::auto_select::stop(profile_id);
    after_mutation(&app, Some(profile_id), SELECTION);
    if changed {
        // applies live (a selector flip on the running instance; a cleared
        // selection re-picks on the reconnect); Ok(None) — nothing runs,
        // nothing to report
        match connection::switch_endpoint_if_running(app.clone(), profile_id).await {
            Ok(Some(tag)) => {
                connection::emit_restart_result(&app, true, format!("switched to {tag}"))
            }
            Ok(None) => {}
            Err(error) => connection::emit_restart_result(&app, false, error),
        }
    }
    Ok(())
}

fn lock_db<'a>(state: &'a State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}
