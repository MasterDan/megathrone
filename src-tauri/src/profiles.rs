use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::AppState;
use crate::connection;
use crate::latency;
use crate::parser::{self, ParsedProfile};
use crate::settings;

pub const PROFILES_CHANGED_EVENT: &str = "profiles-changed";

/// Global profile-update notifications (`ProfileUpdateNotice` payloads):
/// the update flow announces its start and its outcome (success or error)
/// so the toast can tell the user what a background refresh is doing.
pub const PROFILE_UPDATE_EVENT: &str = "profile-update";

pub const UPDATE_NOTICE_STARTED: &str = "started";
pub const UPDATE_NOTICE_SUCCEEDED: &str = "success";
pub const UPDATE_NOTICE_FAILED: &str = "error";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdateNotice {
    pub profile_id: i64,
    pub profile_name: String,
    /// one of UPDATE_NOTICE_* — the frontend styles and stacks by it
    pub kind: &'static str,
    /// the outcome detail ("2 added, 1 removed" / the error text)
    pub message: Option<String>,
}

/// How the profile's live endpoint is chosen (Settings of the endpoints
/// page): one of the automatic strategies or the manual checkmark.
pub const SELECT_MANUAL: &str = "manual";
pub const SELECT_FASTEST: &str = "fastest";
pub const SELECT_MOST_AVAILABLE: &str = "most_available";
pub const SELECT_ROUND_ROBIN: &str = "round_robin";

pub const SELECTION_MODES: [&str; 4] =
    [SELECT_ROUND_ROBIN, SELECT_FASTEST, SELECT_MOST_AVAILABLE, SELECT_MANUAL];

#[derive(Debug, Clone)]
pub struct EndpointCandidate {
    pub id: i64,
    pub raw: String,
    pub tag: String,
    pub available: Option<bool>,
    pub latency_ms: Option<i64>,
    pub url_ok: i64,
    pub url_total: i64,
    pub order_index: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub id: i64,
    pub name: String,
    pub source_url: Option<String>,
    pub source_path: Option<String>,
    pub auto_update_minutes: Option<i64>,
    pub last_fetched_at: Option<String>,
    pub item_count: i64,
    pub skipped_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointItem {
    pub id: i64,
    pub tag: String,
    pub protocol: String,
    pub server: Option<String>,
    pub server_port: Option<i64>,
    pub available: Option<bool>,
    pub latency_ms: Option<i64>,
    pub up_bytes: i64,
    pub down_bytes: i64,
    pub speed_bps: Option<f64>,
    /// how many of the deep-probed proxy-category URLs this endpoint reaches
    /// / how many were probed (0/0 — not deep-probed in the last scan)
    pub url_ok: i64,
    pub url_total: i64,
}

/// One category URL's latest deep-probe outcome for an endpoint; rows exist
/// only for URLs actually probed through it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointUrlStatus {
    pub url_id: i64,
    pub category: String,
    pub url: String,
    pub available: Option<bool>,
    pub latency_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetail {
    pub raw: String,
    pub outbound_json: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesChangedPayload {
    pub profile_id: Option<i64>,
    /// "content" — endpoints changed, "meta" — only profile settings changed,
    /// "latency" — only availability test results were refreshed,
    /// "selection" — the picked endpoint changed,
    /// "updating" — an update of this profile just started,
    /// "update-done" — the update finished (successfully or not)
    pub kind: &'static str,
}

const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

const PROFILE_COLUMNS: &str = "id, name, source_url, source_path, auto_update_minutes, \
     last_fetched_at, item_count, skipped_count, created_at, updated_at";

/// Where a profile's content comes from; URL wins if both are somehow set.
#[derive(Debug, Clone)]
pub enum Source {
    Url(String),
    Path(String),
}

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

// ---------------------------------------------------------------------------
// Storage layer
// ---------------------------------------------------------------------------

fn list_profiles(conn: &Connection) -> Result<Vec<ProfileSummary>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {PROFILE_COLUMNS} FROM profiles ORDER BY created_at DESC, id DESC"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], row_to_summary)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

fn import_profile(
    conn: &mut Connection,
    name: String,
    source_url: Option<String>,
    source_path: Option<String>,
    content: &str,
) -> Result<ProfileSummary, String> {
    let parsed: ParsedProfile = parser::parse_subscription(content);
    if parsed.endpoints.is_empty() {
        return Err("no supported endpoints found in the provided content".into());
    }

    let tx = conn.transaction().map_err(db_err)?;

    tx.execute(
        "INSERT INTO profiles
         (name, source_url, source_path, item_count, skipped_count, last_fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
        params![name, source_url, source_path, parsed.endpoints.len() as i64, parsed.skipped as i64],
    )
    .map_err(db_err)?;
    let profile_id = tx.last_insert_rowid();

    insert_endpoints(&tx, profile_id, &parsed)?;
    tx.commit().map_err(db_err)?;
    summary_by_id(conn, profile_id)
}

fn insert_endpoints(conn: &Connection, profile_id: i64, parsed: &ParsedProfile) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "INSERT INTO endpoints
             (profile_id, tag, protocol, server, server_port, raw, outbound_json, order_index)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(db_err)?;
    for (index, endpoint) in parsed.endpoints.iter().enumerate() {
        stmt.execute(params![
            profile_id,
            endpoint.tag,
            endpoint.protocol,
            endpoint.server,
            endpoint.server_port as i64,
            endpoint.raw,
            endpoint.outbound_json,
            index as i64,
        ])
        .map_err(db_err)?;
    }
    Ok(())
}

/// What an update did to the profile: the refreshed summary plus the diff
/// shape — whether the endpoint *set* changed (rows added or removed — a
/// mere reorder or tag refresh keeps a running session's baked config
/// valid) and the counts, for the update notification.
pub struct UpdateOutcome {
    pub summary: ProfileSummary,
    pub set_changed: bool,
    pub added: usize,
    pub removed: usize,
}

/// After this many consecutive failed *scheduled* updates the scheduler
/// ignores the profile's auto-update schedule; any successful update
/// (the manual Update button included) resets the streak.
pub const MAX_AUTO_UPDATE_FAILURES: i64 = 3;

/// Who asked for `update_profile_flow` to run: only scheduled attempts
/// grow the consecutive-failure counter, manual attempts never do (but a
/// successful one clears it via `apply_update`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateTrigger {
    Manual,
    Scheduler,
}

/// Full update cycle usable from commands and the background scheduler:
/// reads the source, fetches fresh content (NO db lock held during io),
/// then applies it as a diff in a short transaction. Announces itself via
/// `profiles-changed`: kind `updating` up front and kind `update-done` at the
/// end (successfully or not), so the UI can spin the Update button no matter
/// whether the update was clicked or scheduled; every phase also goes out
/// as a global `profile-update` notice (started / success / error).
pub fn update_profile_flow(
    app: &AppHandle,
    profile_id: i64,
    trigger: UpdateTrigger,
) -> Result<UpdateOutcome, String> {
    if let Ok(mut updates) = lock_running_updates() {
        updates.insert(profile_id);
    }
    let _guard = UpdateGuard(profile_id);
    after_mutation(app, Some(profile_id), UPDATING);
    emit_update_notice(app, profile_id, UPDATE_NOTICE_STARTED, None);
    let result = fetch_and_apply_update(app, profile_id);
    after_mutation(app, Some(profile_id), UPDATE_DONE);
    match &result {
        Ok(outcome) => emit_update_notice(
            app,
            profile_id,
            UPDATE_NOTICE_SUCCEEDED,
            Some(describe_diff(outcome)),
        ),
        Err(error) => {
            let message = if trigger == UpdateTrigger::Scheduler {
                let failures = app
                    .try_state::<AppState>()
                    .and_then(|state| {
                        state.db.lock().ok().and_then(|conn| {
                            bump_auto_update_failures(&conn, profile_id).ok()
                        })
                    })
                    .unwrap_or(0);
                if failures >= MAX_AUTO_UPDATE_FAILURES {
                    format!(
                        "{error} — auto-update paused after {failures} failed attempts; \
                         a successful manual update resumes it"
                    )
                } else {
                    error.clone()
                }
            } else {
                error.clone()
            };
            emit_update_notice(app, profile_id, UPDATE_NOTICE_FAILED, Some(message));
        }
    }
    result
}

/// The success notice text: what the diff actually did to the endpoint set.
fn describe_diff(outcome: &UpdateOutcome) -> String {
    match (outcome.added, outcome.removed) {
        (0, 0) => "no endpoint changes".into(),
        (added, 0) => format!("{added} added"),
        (0, removed) => format!("{removed} removed"),
        (added, removed) => format!("{added} added, {removed} removed"),
    }
}

/// Emits one global update notification; the profile's display name rides
/// along so the toast reads naturally for background updates.
fn emit_update_notice(
    app: &AppHandle,
    profile_id: i64,
    kind: &'static str,
    message: Option<String>,
) {
    let profile_name = app
        .try_state::<AppState>()
        .and_then(|state| {
            state.db.lock().ok().and_then(|conn| {
                conn.query_row(
                    "SELECT name FROM profiles WHERE id = ?1",
                    params![profile_id],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            })
        })
        .unwrap_or_else(|| format!("profile #{profile_id}"));
    let _ = app.emit(
        PROFILE_UPDATE_EVENT,
        ProfileUpdateNotice { profile_id, profile_name, kind, message },
    );
}

/// How long the update flow waits for a cancelled scan to actually exit
/// before rewriting the endpoint rows anyway (the FK-tolerant scan writes
/// cover the residual race).
const SCAN_CANCEL_TIMEOUT: Duration = Duration::from_secs(60);

fn fetch_and_apply_update(app: &AppHandle, profile_id: i64) -> Result<UpdateOutcome, String> {
    // a running scan works off a start-of-scan snapshot of the endpoint
    // rows; stop it before the diff rewrites them (it exits between
    // batches, so the wait is bounded by one batch)
    latency::cancel_and_wait(profile_id, SCAN_CANCEL_TIMEOUT);

    let source = {
        let state = app.state::<AppState>();
        let conn = lock_db_ref(&state)?;
        load_source(&conn, profile_id)?
    };

    let content = match &source {
        Source::Url(url) => fetch_url(url)?,
        Source::Path(path) => read_file(path)?,
    };

    let outcome = {
        let state = app.state::<AppState>();
        let mut conn = lock_db_ref(&state)?;
        apply_update(&mut conn, profile_id, &content)?
    };

    after_mutation(app, Some(profile_id), CONTENT);
    // an automatic strategy re-picks over the fresh rows, keeping the
    // current endpoint while it survived the update
    crate::auto_select::content_updated(profile_id);
    Ok(outcome)
}

/// Applies a finished update to the live session: the running config bakes
/// the endpoint set, so a changed set means the session is stale — rebuild
/// it. `report` distinguishes the user-initiated path (the global restart
/// toast; failures stay visible) from a background scheduler update
/// (silent; failures land in `last_error`).
pub async fn rebuild_session_after_update(
    app: AppHandle,
    profile_id: i64,
    set_changed: bool,
    report: bool,
) {
    if !set_changed || !connection::runs_profile(profile_id) {
        return;
    }
    match connection::restart_if_running(app.clone(), profile_id).await {
        Ok(Some(tag)) if report => {
            connection::emit_restart_result(&app, true, format!("profile updated — via {tag}"))
        }
        Ok(_) => {}
        Err(error) if report => connection::emit_restart_result(&app, false, error),
        Err(error) => {
            eprintln!(
                "[auto-update] session rebuild after profile {profile_id} update failed: {error}"
            );
        }
    }
}

fn load_source(conn: &Connection, profile_id: i64) -> Result<Source, String> {
    conn.query_row(
        "SELECT source_url, source_path FROM profiles WHERE id = ?1",
        params![profile_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
        other => db_err(other),
    })
    .and_then(|(url, path)| match (url, path) {
        (Some(url), _) if !url.is_empty() => Ok(Source::Url(url)),
        (_, Some(path)) if !path.is_empty() => Ok(Source::Path(path)),
        _ => Err("profile has no source to update from".into()),
    })
}

/// Applies fresh subscription content as a **diff**: endpoints whose raw
/// link survives keep their row (a stable id — and with it every test
/// result: availability, latency, deep-probe scores), only removed links'
/// rows are deleted and never-seen links are inserted. Display fields
/// (tag, order) follow the fresh content. Parsing is deterministic, so a
/// surviving link's re-parsed outbound is rewritten as a no-op (it only
/// differs when the parser itself changed between app versions).
fn apply_update(conn: &mut Connection, profile_id: i64, content: &str) -> Result<UpdateOutcome, String> {
    let parsed: ParsedProfile = parser::parse_subscription(content);
    if parsed.endpoints.is_empty() {
        return Err("no supported endpoints found in the updated content".into());
    }

    // match fresh endpoints to stored rows by raw link, as a multiset: the
    // k-th occurrence of a duplicated link maps to the k-th stored one
    let old_rows: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, raw FROM endpoints WHERE profile_id = ?1 ORDER BY order_index")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![profile_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };
    let mut free_by_raw: HashMap<&str, VecDeque<i64>> = HashMap::new();
    for (id, raw) in &old_rows {
        free_by_raw.entry(raw.as_str()).or_default().push_back(*id);
    }
    let matched: Vec<Option<i64>> = parsed
        .endpoints
        .iter()
        .map(|endpoint| {
            free_by_raw
                .get_mut(endpoint.raw.as_str())
                .and_then(|queue| queue.pop_front())
        })
        .collect();

    let kept: HashSet<i64> = matched.iter().filter_map(|id| *id).collect();
    let removed: Vec<i64> = old_rows
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| !kept.contains(id))
        .collect();
    let added = matched.iter().filter(|id| id.is_none()).count();
    let set_changed = !removed.is_empty() || added > 0;

    let tx = conn.transaction().map_err(db_err)?;
    let affected = tx
        .execute(
            "UPDATE profiles
             SET item_count = ?2, skipped_count = ?3,
                 last_fetched_at = datetime('now'), updated_at = datetime('now'),
                 auto_update_failures = 0
             WHERE id = ?1",
            params![profile_id, parsed.endpoints.len() as i64, parsed.skipped as i64],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(format!("profile {profile_id} not found"));
    }
    // removed links' rows die here (their latency/url results cascade away)
    for id in &removed {
        tx.execute("DELETE FROM endpoints WHERE id = ?1", params![id])
            .map_err(db_err)?;
    }
    // survivors: refresh display fields and the re-parsed outbound, keep
    // id and every test-result column untouched
    {
        let mut stmt = tx
            .prepare(
                "UPDATE endpoints
                 SET tag = ?2, protocol = ?3, server = ?4, server_port = ?5,
                     outbound_json = ?6, order_index = ?7, updated_at = datetime('now')
                 WHERE id = ?1",
            )
            .map_err(db_err)?;
        for (index, endpoint) in parsed.endpoints.iter().enumerate() {
            if let Some(id) = matched[index] {
                stmt.execute(params![
                    id,
                    endpoint.tag,
                    endpoint.protocol,
                    endpoint.server,
                    endpoint.server_port as i64,
                    endpoint.outbound_json,
                    index as i64
                ])
                .map_err(db_err)?;
            }
        }
    }
    // never-seen links are inserted at their fresh position
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO endpoints
                 (profile_id, tag, protocol, server, server_port, raw, outbound_json, order_index)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .map_err(db_err)?;
        for (index, endpoint) in parsed.endpoints.iter().enumerate() {
            if matched[index].is_none() {
                stmt.execute(params![
                    profile_id,
                    endpoint.tag,
                    endpoint.protocol,
                    endpoint.server,
                    endpoint.server_port as i64,
                    endpoint.raw,
                    endpoint.outbound_json,
                    index as i64
                ])
                .map_err(db_err)?;
            }
        }
    }
    tx.commit().map_err(db_err)?;
    let summary = summary_by_id(conn, profile_id)?;
    Ok(UpdateOutcome { summary, set_changed, added, removed: removed.len() })
}

fn rename_profile(conn: &Connection, profile_id: i64, new_name: &str) -> Result<(), String> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err("profile name must not be empty".into());
    }
    let affected = conn
        .execute(
            "UPDATE profiles SET name = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![profile_id, new_name],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(format!("profile {profile_id} not found"));
    }
    Ok(())
}

fn delete_profile(conn: &Connection, profile_id: i64) -> Result<(), String> {
    // endpoints are removed by the FK cascade
    conn.execute("DELETE FROM profiles WHERE id = ?1", params![profile_id])
        .map_err(db_err)?;
    Ok(())
}

const AUTO_UPDATE_CHOICES: [i64; 5] = [30, 60, 360, 720, 1440];

fn set_auto_update(conn: &Connection, profile_id: i64, minutes: Option<i64>) -> Result<(), String> {
    if let Some(minutes) = minutes {
        if !AUTO_UPDATE_CHOICES.contains(&minutes) {
            return Err(format!("unsupported auto-update interval: {minutes} minutes"));
        }
    }
    if minutes.is_some() {
        let has_source: bool = conn
            .query_row(
                "SELECT (source_url IS NOT NULL AND source_url != '')
                     OR (source_path IS NOT NULL AND source_path != '')
                 FROM profiles WHERE id = ?1",
                params![profile_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if !has_source {
            return Err("auto-update requires a profile with a URL or file source".into());
        }
    }
    let affected = conn
        .execute(
            "UPDATE profiles SET auto_update_minutes = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![profile_id, minutes],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(format!("profile {profile_id} not found"));
    }
    Ok(())
}

/// Counts one more failed scheduled update and returns the new streak.
/// `due_profiles`/`next_wake_seconds` ignore the profile once the streak
/// reaches `MAX_AUTO_UPDATE_FAILURES`; `apply_update` clears it.
fn bump_auto_update_failures(conn: &Connection, profile_id: i64) -> Result<i64, String> {
    conn.query_row(
        "UPDATE profiles SET auto_update_failures = auto_update_failures + 1
         WHERE id = ?1 RETURNING auto_update_failures",
        params![profile_id],
        |row| row.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
        other => db_err(other),
    })
}

/// Profiles whose auto-update interval has elapsed since the last fetch.
/// Profiles paused by too many consecutive failed scheduled updates are
/// skipped — a successful update (the manual button included) unpauses.
pub fn due_profiles(conn: &Connection) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT id FROM profiles
             WHERE auto_update_minutes IS NOT NULL
               AND last_fetched_at IS NOT NULL
               AND auto_update_failures < {MAX_AUTO_UPDATE_FAILURES}
               AND (julianday('now') - julianday(last_fetched_at)) * 1440.0 >= auto_update_minutes
             ORDER BY id"
        ))
        .map_err(db_err)?;
    let ids = stmt
        .query_map([], |row| row.get(0))
        .map_err(db_err)?
        .collect::<Result<Vec<i64>, _>>()
        .map_err(db_err)?;
    Ok(ids)
}

/// Seconds until the next scheduled auto-update (can be negative when due;
/// the scheduler clamps). None when nothing is scheduled. Paused profiles
/// must not hold the wake target at "overdue" forever, so they are excluded
/// here too.
pub fn next_wake_seconds(conn: &Connection) -> Result<Option<f64>, String> {
    conn.query_row(
        &format!(
            "SELECT MIN((julianday(last_fetched_at, '+' || auto_update_minutes || ' minutes')
                         - julianday('now')) * 86400.0)
             FROM profiles
             WHERE auto_update_minutes IS NOT NULL
               AND last_fetched_at IS NOT NULL
               AND auto_update_failures < {MAX_AUTO_UPDATE_FAILURES}
               AND ((source_url IS NOT NULL AND source_url != '')
                    OR (source_path IS NOT NULL AND source_path != ''))"
        ),
        [],
        |row| row.get::<_, Option<f64>>(0),
    )
    .map_err(db_err)
}

/// Columns shared by every EndpointItem query: base endpoint fields plus the
/// deep-probe score over the *proxy*-routed category rules (`url_ok` of the
/// probed `url_total`; 0/0 — this endpoint was not deep-probed last scan).
/// Only rules marked for testing are ever probed, so opted-out rules count
/// on neither side.
const URL_SCORE_COLUMNS: &str = "(
        SELECT COUNT(*) FROM endpoint_url_results r
        JOIN test_sites s ON s.id = r.url_id
        JOIN test_site_categories c ON c.id = s.category_id
        WHERE r.endpoint_id = e.id AND r.available = 1 AND c.action = 'proxy'
     ) AS url_ok, (
        SELECT COUNT(*) FROM endpoint_url_results r
        JOIN test_sites s ON s.id = r.url_id
        JOIN test_site_categories c ON c.id = s.category_id
        WHERE r.endpoint_id = e.id AND c.action = 'proxy'
     ) AS url_total";

/// Canonical endpoint order of the endpoints grid — `list_items` pages it
/// and `item_index` resolves positions in it, so both must stay in sync.
const CANONICAL_ORDER: &str = "CASE e.available WHEN 1 THEN 0 WHEN 0 THEN 2 ELSE 1 END,
                  e.latency_ms,
                  url_ok DESC,
                  e.order_index";

fn list_items(conn: &Connection, profile_id: i64, offset: i64, limit: i64) -> Result<Vec<EndpointItem>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT e.id, e.tag, e.protocol, e.server, e.server_port, e.available, e.latency_ms,
                    e.up_bytes, e.down_bytes, e.speed_bps,
                    {URL_SCORE_COLUMNS}
             FROM endpoints e
             WHERE e.profile_id = ?1
             ORDER BY {CANONICAL_ORDER}
             LIMIT ?2 OFFSET ?3"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![profile_id, limit.clamp(1, 500), offset], row_to_item)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

/// 0-based index of an endpoint in the grid's canonical order; None when the
/// endpoint is gone or belongs to another profile.
fn item_index(conn: &Connection, profile_id: i64, item_id: i64) -> Result<Option<i64>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT e.id, {URL_SCORE_COLUMNS}
             FROM endpoints e
             WHERE e.profile_id = ?1
             ORDER BY {CANONICAL_ORDER}"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![profile_id], |row| row.get::<_, i64>(0))
        .map_err(db_err)?;
    for (index, row) in rows.enumerate() {
        if row.map_err(db_err)? == item_id {
            return Ok(Some(index as i64));
        }
    }
    Ok(None)
}

/// Latest outcome of every deep-probed proxy-category rule for one
/// endpoint, grouped in category order (shown as the URL it probed).
fn endpoint_url_statuses(conn: &Connection, item_id: i64) -> Result<Vec<EndpointUrlStatus>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT u.id, c.name, u.rule_type, u.value, r.available, r.latency_ms
             FROM endpoint_url_results r
             JOIN test_sites u ON u.id = r.url_id
             JOIN test_site_categories c ON c.id = u.category_id
             WHERE r.endpoint_id = ?1 AND c.action = 'proxy'
             ORDER BY c.position, c.id, u.id",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![item_id], |row| {
            let rule_type: String = row.get(2)?;
            let value: String = row.get(3)?;
            Ok(EndpointUrlStatus {
                url_id: row.get(0)?,
                category: row.get(1)?,
                url: crate::sites::probe_url(&rule_type, &value).unwrap_or(value),
                available: row.get::<_, Option<i64>>(4)?.map(|value| value != 0),
                latency_ms: row.get(5)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

fn item_detail(conn: &Connection, item_id: i64) -> Result<ItemDetail, String> {
    conn.query_row(
        "SELECT raw, outbound_json FROM endpoints WHERE id = ?1",
        params![item_id],
        |row| Ok(ItemDetail { raw: row.get(0)?, outbound_json: row.get(1)? }),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("endpoint {item_id} not found"),
        other => db_err(other),
    })
}

/// The selected endpoint is remembered as its raw link: endpoint ids are not
/// stable across content updates, but the link usually is. Returns None when
/// the stored choice no longer matches anything (the endpoint disappeared).
fn get_selected_endpoint(conn: &Connection, profile_id: i64) -> Result<Option<EndpointItem>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT e.id, e.tag, e.protocol, e.server, e.server_port, e.available, e.latency_ms,
                    e.up_bytes, e.down_bytes, e.speed_bps,
                    {URL_SCORE_COLUMNS}
             FROM profiles p
             JOIN endpoints e ON e.profile_id = p.id AND e.raw = p.selected_endpoint_key
             WHERE p.id = ?1
             ORDER BY e.order_index
             LIMIT 1"
        ))
        .map_err(db_err)?;
    let mut rows = stmt
        .query_map(params![profile_id], row_to_item)
        .map_err(db_err)?;
    match rows.next() {
        Some(row) => Ok(Some(row.map_err(db_err)?)),
        None => Ok(None),
    }
}

/// Stores the selection (endpoint id → its raw link). Returns whether the
/// stored key actually changed — callers use it to skip no-op restarts.
pub(crate) fn set_selected_endpoint(
    conn: &Connection,
    profile_id: i64,
    item_id: Option<i64>,
) -> Result<bool, String> {
    let key = match item_id {
        Some(item_id) => Some(conn
            .query_row(
                "SELECT raw FROM endpoints WHERE id = ?1 AND profile_id = ?2",
                params![item_id, profile_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    format!("endpoint {item_id} not found in profile {profile_id}")
                }
                other => db_err(other),
            })?),
        None => None,
    };
    let previous: Option<String> = conn
        .query_row(
            "SELECT selected_endpoint_key FROM profiles WHERE id = ?1",
            params![profile_id],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
            other => db_err(other),
        })?;
    if previous == key {
        return Ok(false);
    }
    let affected = conn
        .execute(
            "UPDATE profiles SET selected_endpoint_key = ?2, updated_at = datetime('now')
             WHERE id = ?1",
            params![profile_id, key],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(format!("profile {profile_id} not found"));
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Selection strategies
// ---------------------------------------------------------------------------

/// The stored strategy of the profile; errors when the profile is gone.
pub fn selection_mode(conn: &Connection, profile_id: i64) -> Result<String, String> {
    conn.query_row(
        "SELECT selection_mode FROM profiles WHERE id = ?1",
        params![profile_id],
        |row| row.get::<_, String>(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
        other => db_err(other),
    })
}

pub fn set_selection_mode(conn: &Connection, profile_id: i64, mode: &str) -> Result<(), String> {
    if !SELECTION_MODES.contains(&mode) {
        return Err(format!("unknown selection mode: {mode}"));
    }
    let affected = conn
        .execute(
            "UPDATE profiles SET selection_mode = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![profile_id, mode],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(format!("profile {profile_id} not found"));
    }
    Ok(())
}

/// Human name of a strategy (toast copy).
pub fn selection_mode_label(mode: &str) -> &str {
    match mode {
        SELECT_ROUND_ROBIN => "Round Robin",
        SELECT_MOST_AVAILABLE => "Most Available",
        SELECT_MANUAL => "Endpoint",
        _ => "Fastest",
    }
}

/// The raw link of the current selection (the supervisor's round-robin
/// cursor and switch check); `None` when nothing is selected.
pub fn selected_raw_key(conn: &Connection, profile_id: i64) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT selected_endpoint_key FROM profiles WHERE id = ?1",
        params![profile_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
        other => db_err(other),
    })
}

/// All pickable endpoints of the profile in list order. Broken outbounds
/// (unparseable JSON, no type) cannot be dialed, so they never become
/// candidates. A reachable endpoint whose last deep-probe share falls
/// below the availability floor (Settings → General) reads as dead here —
/// every automatic pick (and the connect-time re-pick) then avoids it.
pub fn load_candidates(
    conn: &Connection,
    profile_id: i64,
) -> Result<Vec<EndpointCandidate>, String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM profiles WHERE id = ?1)",
            params![profile_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !exists {
        return Err(format!("profile {profile_id} not found"));
    }
    let mut stmt = conn
        .prepare(&format!(
            "SELECT e.id, e.raw, e.tag, e.available, e.latency_ms, e.order_index,
                    e.outbound_json,
                    {URL_SCORE_COLUMNS}
             FROM endpoints e
             WHERE e.profile_id = ?1
             ORDER BY e.order_index"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![profile_id], candidate_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let floor = availability_floor(conn);
    Ok(rows
        .into_iter()
        .filter(|(_, raw)| candidate_usable(raw))
        .map(|(mut candidate, _)| {
            apply_availability_floor(&mut candidate, floor);
            candidate
        })
        .collect())
}

/// The endpoints the last scan proved reachable, in list order. The rescue
/// path (and any other "pick among the living" query) uses this instead of
/// `load_candidates`: rotation targets must be reachable anyway, and on a
/// large mostly-dead profile this keeps both the query and the per-batch
/// rescue cheap. Endpoints below the availability floor are not "proven
/// reachable" for selection purposes — they are dropped, not marked dead
/// (the list feeds `pick_by_strategy` directly).
pub fn load_reachable_candidates(
    conn: &Connection,
    profile_id: i64,
) -> Result<Vec<EndpointCandidate>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT e.id, e.raw, e.tag, e.available, e.latency_ms, e.order_index,
                    e.outbound_json,
                    {URL_SCORE_COLUMNS}
             FROM endpoints e
             WHERE e.profile_id = ?1 AND e.available = 1
             ORDER BY e.order_index"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![profile_id], candidate_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let floor = availability_floor(conn);
    Ok(rows
        .into_iter()
        .filter(|(_, raw)| candidate_usable(raw))
        .filter(|(candidate, _)| {
            meets_availability_floor(candidate.url_ok, candidate.url_total, floor)
        })
        .map(|(candidate, _)| candidate)
        .collect())
}

/// The configured availability floor in percent (Settings → General);
/// the default when the setting cannot be read.
fn availability_floor(conn: &Connection) -> i64 {
    settings::load_general(conn)
        .map(|general| general.min_availability_percent)
        .unwrap_or(settings::DEFAULT_MIN_AVAILABILITY_PERCENT)
}

/// Whether an endpoint's deep-probe outcome clears the availability floor:
/// endpoints that were never deep-probed (`url_total == 0`) stay neutral
/// (unknown is not bad), probed ones must pass `floor_percent` of their
/// URLs. A floor of 0 (or below) accepts everything.
pub fn meets_availability_floor(url_ok: i64, url_total: i64, floor_percent: i64) -> bool {
    floor_percent <= 0
        || url_total == 0
        || url_ok.saturating_mul(100) >= floor_percent.saturating_mul(url_total)
}

/// Marks a reachable candidate below the availability floor as dead — the
/// strategies then treat it exactly like a failed base probe (never a
/// pick, a switch away when current, a last-resort dial at best).
fn apply_availability_floor(candidate: &mut EndpointCandidate, floor_percent: i64) {
    if candidate.available == Some(true)
        && !meets_availability_floor(candidate.url_ok, candidate.url_total, floor_percent)
    {
        candidate.available = Some(false);
    }
}

/// The shared `(candidate, raw)` row shape of the candidate queries above.
fn candidate_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(EndpointCandidate, String)> {
    Ok((
        EndpointCandidate {
            id: row.get(0)?,
            raw: row.get(1)?,
            tag: row.get(2)?,
            available: row.get::<_, Option<i64>>(3)?.map(|v| v != 0),
            latency_ms: row.get(4)?,
            url_ok: row.get::<_, i64>(7)?,
            url_total: row.get::<_, i64>(8)?,
            order_index: row.get(5)?,
        },
        row.get::<_, String>(6)?,
    ))
}

/// A stored outbound can be dialed when it parses and names a protocol type.
fn candidate_usable(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|outbound| {
            outbound
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|kind| !kind.is_empty())
}

/// Lower sorts first: reachable, then never-tested, then known-dead.
fn availability_rank(available: Option<bool>) -> u8 {
    match available {
        Some(true) => 0,
        None => 1,
        Some(false) => 2,
    }
}

/// The deep-probe success share; unprobed endpoints report below any
/// probed one (a failed probe is knowledge).
fn url_share(candidate: &EndpointCandidate) -> f64 {
    if candidate.url_total > 0 {
        candidate.url_ok as f64 / candidate.url_total as f64
    } else {
        -1.0
    }
}

fn order_by_latency(
    a: &EndpointCandidate,
    b: &EndpointCandidate,
) -> std::cmp::Ordering {
    availability_rank(a.available)
        .cmp(&availability_rank(b.available))
        .then(a.latency_ms.unwrap_or(i64::MAX).cmp(&b.latency_ms.unwrap_or(i64::MAX)))
        .then(a.order_index.cmp(&b.order_index))
}

/// Fastest: the reachable endpoint with the lowest latency (ties by list
/// order). Falls back to the least-disliked endpoint when nothing is known
/// reachable, so a connect always has something to dial.
pub fn pick_fastest(candidates: &[EndpointCandidate]) -> Option<&EndpointCandidate> {
    candidates.iter().min_by(|a, b| order_by_latency(a, b))
}

/// Most available: among reachable endpoints, the best deep-probe outcome —
/// probed-with-passes first (share desc), then unprobed (unknown beats a
/// proven zero), then latency, then list order.
fn most_available_class(candidate: &EndpointCandidate) -> u8 {
    if candidate.url_total == 0 {
        1 // unprobed
    } else if candidate.url_ok > 0 {
        2 // proved some reachability
    } else {
        0 // proved none
    }
}

pub fn pick_most_available(candidates: &[EndpointCandidate]) -> Option<&EndpointCandidate> {
    candidates.iter().min_by(|a, b| {
        availability_rank(a.available)
            .cmp(&availability_rank(b.available))
            .then_with(|| most_available_class(b).cmp(&most_available_class(a)))
            .then_with(|| url_share(b).partial_cmp(&url_share(a)).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.latency_ms.unwrap_or(i64::MAX).cmp(&b.latency_ms.unwrap_or(i64::MAX)))
            .then_with(|| a.order_index.cmp(&b.order_index))
    })
}

/// Round robin: rotates strictly among endpoints the last scan proved
/// reachable — the next one strictly after the current (wrapping around).
/// With nothing reachable the current one is kept (switching to an untested
/// or known-dead endpoint is never a rotation); with no current there is
/// nothing to rotate to once any scan data exists, while a never-scanned
/// profile still dials its first endpoint (a connect needs something to
/// dial, and the first supervisor pass re-tests and rotates from there).
pub fn pick_round_robin<'a>(
    candidates: &'a [EndpointCandidate],
    current_raw: Option<&str>,
) -> Option<&'a EndpointCandidate> {
    let first = candidates.first()?;
    let position = current_raw.and_then(|raw| candidates.iter().position(|c| c.raw == raw));
    let start = position.map_or(0, |p| (p + 1).min(candidates.len()));
    let order = (start..candidates.len()).chain(0..start);
    if let Some(index) = order.into_iter().find(|&i| candidates[i].available == Some(true)) {
        return Some(&candidates[index]);
    }
    match position {
        Some(p) => Some(&candidates[p]),
        None if candidates.iter().any(|c| c.available.is_some()) => None,
        None => Some(first),
    }
}

/// Resolves a strategy to its pick; `manual` has no automatic pick (the
/// caller falls back to `pick_fastest` when a connect needs *some* endpoint).
pub fn pick_by_strategy<'a>(
    mode: &str,
    candidates: &'a [EndpointCandidate],
    current_raw: Option<&str>,
) -> Option<&'a EndpointCandidate> {
    match mode {
        SELECT_ROUND_ROBIN => pick_round_robin(candidates, current_raw),
        SELECT_MOST_AVAILABLE => pick_most_available(candidates),
        _ => pick_fastest(candidates),
    }
}

fn summary_by_id(conn: &Connection, profile_id: i64) -> Result<ProfileSummary, String> {
    conn.query_row(
        &format!("SELECT {PROFILE_COLUMNS} FROM profiles WHERE id = ?1"),
        params![profile_id],
        row_to_summary,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
        other => db_err(other),
    })
}

fn row_to_summary(row: &rusqlite::Row) -> rusqlite::Result<ProfileSummary> {
    Ok(ProfileSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        source_url: row.get(2)?,
        source_path: row.get(3)?,
        auto_update_minutes: row.get(4)?,
        last_fetched_at: row.get(5)?,
        item_count: row.get(6)?,
        skipped_count: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<EndpointItem> {
    Ok(EndpointItem {
        id: row.get(0)?,
        tag: row.get(1)?,
        protocol: row.get(2)?,
        server: row.get(3)?,
        server_port: row.get(4)?,
        available: row.get::<_, Option<i64>>(5)?.map(|v| v != 0),
        latency_ms: row.get(6)?,
        up_bytes: row.get(7)?,
        down_bytes: row.get(8)?,
        speed_bps: row.get(9)?,
        url_ok: row.get(10)?,
        url_total: row.get(11)?,
    })
}

// ---------------------------------------------------------------------------
// Sources & notifications
// ---------------------------------------------------------------------------

fn fetch_url(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|e| format!("request failed: {e}"))?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_string()
        .map_err(|e| format!("failed to read response body: {e}"))
}

fn read_file(path: &str) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if metadata.len() > MAX_DOWNLOAD_BYTES {
        return Err(format!("file is too large: {} bytes", metadata.len()));
    }
    std::fs::read_to_string(Path::new(path)).map_err(|e| format!("cannot read {path}: {e}"))
}

fn default_name_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| "Imported profile".to_string())
}

fn default_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Imported profile")
        .to_string()
}

/// Notify the UI and re-arm the auto-update scheduler after any mutation.
/// Use `CONTENT` when endpoints may have changed, `META` for settings-only
/// changes (rename, auto-update interval), `LATENCY` when only availability
/// test results were refreshed.
pub fn after_mutation(app: &AppHandle, profile_id: Option<i64>, kind: &'static str) {
    let _ = app.emit(PROFILES_CHANGED_EVENT, ProfilesChangedPayload { profile_id, kind });
    app.state::<AppState>().notify_auto_update();
}

pub const CONTENT: &str = "content";
pub const META: &str = "meta";
pub const LATENCY: &str = "latency";
pub const SELECTION: &str = "selection";
pub const UPDATING: &str = "updating";
pub const UPDATE_DONE: &str = "update-done";

// ---------------------------------------------------------------------------
// Update registry
// ---------------------------------------------------------------------------

/// Profile ids with an update currently in flight (button click or the
/// background scheduler), so a re-mounted page can adopt the "updating" state
/// via `profile_update_status`.
static RUNNING_UPDATES: LazyLock<Mutex<HashSet<i64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn lock_running_updates() -> Result<MutexGuard<'static, HashSet<i64>>, String> {
    RUNNING_UPDATES
        .lock()
        .map_err(|_| "update registry poisoned".to_string())
}

/// Removes the profile's registry entry when the update ends for any reason,
/// including early returns and panics.
struct UpdateGuard(i64);

impl Drop for UpdateGuard {
    fn drop(&mut self) {
        if let Ok(mut updates) = RUNNING_UPDATES.lock() {
            updates.remove(&self.0);
        }
    }
}

fn lock_db<'a>(state: &'a State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}

fn lock_db_ref<'a>(
    state: &'a tauri::State<'_, AppState>,
) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}

fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("megathrone-test-{}-{id}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        crate::db::open(&path).expect("test db should open")
    }

    const SAMPLE: &str = "\
vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@1.2.3.4:443?security=tls&sni=example.com#First
ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#Second
ssr://not-supported
trojan://pw@9.9.9.9:443?security=tls#Third";

    #[test]
    fn import_list_rename_delete_flow() {
        let mut conn = test_db();

        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
        assert_eq!(summary.item_count, 3);
        assert_eq!(summary.skipped_count, 1);
        assert!(summary.last_fetched_at.is_some());

        let mut list = list_profiles(&conn).expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Sub");

        // pagination: first page of 2 keeps file order
        let page = list_items(&conn, summary.id, 0, 2).expect("items");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].tag, "First");
        assert_eq!(page[0].protocol, "vless");
        assert_eq!(page[1].tag, "Second");

        let rest = list_items(&conn, summary.id, 2, 2).expect("items");
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].tag, "Third");

        // detail roundtrip: stored JSON parses and keeps the type
        let detail = item_detail(&conn, page[0].id).expect("detail");
        let outbound: serde_json::Value = serde_json::from_str(&detail.outbound_json).expect("valid json");
        assert_eq!(outbound["type"], "vless");
        assert!(detail.raw.starts_with("vless://"));

        // rename
        rename_profile(&conn, summary.id, "Renamed").expect("rename");
        list = list_profiles(&conn).expect("list");
        assert_eq!(list[0].name, "Renamed");
        assert!(rename_profile(&conn, summary.id, "  ").is_err());

        // delete cascades
        delete_profile(&conn, summary.id).expect("delete");
        assert!(list_profiles(&conn).expect("list").is_empty());
        assert!(list_items(&conn, summary.id, 0, 10).expect("items").is_empty());
    }

    #[test]
    fn import_rejects_garbage() {
        let mut conn = test_db();
        let error = import_profile(&mut conn, "Bad".into(), None, None, "hello world\nnothing useful")
            .expect_err("should reject");
        assert!(error.contains("no supported endpoints"));
        assert!(list_profiles(&conn).expect("list").is_empty());
    }

    #[test]
    fn update_replaces_endpoints_from_file_source() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "File sub".into(), None, None, SAMPLE).expect("import");

        // point the profile at a temp file with different content
        let file = std::env::temp_dir().join(format!("megathrone-sub-{}.txt", std::process::id()));
        let fresh = "trojan://pw@1.1.1.1:443?security=tls#Only\nvmess://garbage";
        std::fs::write(&file, fresh).expect("write temp sub");

        conn.execute(
            "UPDATE profiles SET source_path = ?2 WHERE id = ?1",
            params![summary.id, file.to_str().unwrap()],
        )
        .map_err(db_err)
        .unwrap();

        let source = load_source(&conn, summary.id).expect("source");
        assert!(matches!(source, Source::Path(_)));
        let content = match &source {
            Source::Url(_) => unreachable!(),
            Source::Path(path) => read_file(path).expect("read"),
        };

        let updated = apply_update(&mut conn, summary.id, &content).expect("update");
        assert_eq!(updated.summary.item_count, 1);
        assert_eq!(updated.summary.skipped_count, 1);
        assert!(updated.set_changed, "a wholly different content changes the set");

        // old endpoints are gone, ids are fresh
        let items = list_items(&conn, summary.id, 0, 10).expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].tag, "Only");
        assert_ne!(items[0].id, 0);

        // last_fetched_at advanced
        let fetched: (String,) = conn
            .query_row(
                "SELECT last_fetched_at FROM profiles WHERE id = ?1",
                params![summary.id],
                |row| Ok((row.get(0)?,)),
            )
            .map_err(db_err)
            .unwrap();
        assert!(!fetched.0.is_empty());
    }

    #[test]
    fn update_diff_preserves_surviving_results() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
        let items = list_items(&conn, summary.id, 0, 10).expect("items");
        let (first_id, first_tag) = (items[0].id, items[0].tag.clone());
        let second_id = items[1].id;

        // a test-site rule so deep-probe results can exist
        conn.execute(
            "INSERT INTO test_site_categories (name, position, action) VALUES ('Cat', 0, 'proxy')",
            [],
        )
        .expect("seed category");
        let category_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO test_sites (category_id, rule_type, value)
             VALUES (?1, 'url', 'https://a.example/')",
            params![category_id],
        )
        .expect("seed url");
        let url_id = conn.last_insert_rowid();

        // the "diamond": a latency scan's verdict on the first endpoint…
        conn.execute(
            "UPDATE endpoints SET available = 1, latency_ms = 123,
                last_tested_at = datetime('now') WHERE id = ?1",
            params![first_id],
        )
        .expect("stamp results");
        // …its deep-probe rows, and one stale row on the endpoint that is
        // about to be removed (must cascade away with it)
        conn.execute(
            "INSERT INTO endpoint_url_results (endpoint_id, url_id, available, latency_ms)
             VALUES (?1, ?2, 1, 50)",
            params![first_id, url_id],
        )
        .expect("seed url result");
        conn.execute(
            "INSERT INTO endpoint_url_results (endpoint_id, url_id, available, latency_ms)
             VALUES (?1, ?2, 1, 60)",
            params![second_id, url_id],
        )
        .expect("seed doomed url result");

        // fresh content: First survives (same raw), Second is gone, a new
        // link appears; the survivor keeps its id and every result
        let fresh = format!(
            "vless://new-uuid@9.9.9.9:443?security=tls#Fresh\n{}",
            SAMPLE.lines().next().unwrap_or_default()
        );
        let outcome = apply_update(&mut conn, summary.id, &fresh).expect("diff update");
        assert!(outcome.set_changed);
        assert_eq!(outcome.summary.item_count, 2);

        let rows = list_items(&conn, summary.id, 0, 10).expect("items");
        assert_eq!(rows.len(), 2);
        // the canonical grid order is availability-first, so check the
        // content order by position, not by grid row
        let survivor = rows.iter().find(|item| item.tag == first_tag).expect("survivor");
        assert_eq!(survivor.id, first_id, "surviving rows keep their id");
        let (fresh_order, survivor_order): (i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT order_index FROM endpoints WHERE tag = 'Fresh'
                      AND profile_id = ?1),
                    (SELECT order_index FROM endpoints WHERE id = ?2)",
                params![summary.id, first_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read order");
        assert_eq!((fresh_order, survivor_order), (0, 1), "order follows fresh content");

        let (available, latency_ms, tested, url_ok): (i64, Option<i64>, Option<String>, i64) = conn
            .query_row(
                "SELECT available, latency_ms, last_tested_at,
                        (SELECT COUNT(*) FROM endpoint_url_results r
                         WHERE r.endpoint_id = endpoints.id AND r.available = 1)
                 FROM endpoints WHERE id = ?1",
                params![first_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read survivor");
        assert_eq!(available, 1);
        assert_eq!(latency_ms, Some(123));
        assert!(tested.is_some(), "the scan stamp survives the update");
        assert_eq!(url_ok, 1, "deep-probe rows survive on the stable id");

        // the removed endpoint and its url results are gone
        let gone: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM endpoints WHERE id = ?1",
                params![second_id],
                |row| row.get(0),
            )
            .expect("count removed");
        assert_eq!(gone, 0);
        let stale_results: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM endpoint_url_results WHERE endpoint_id = ?1",
                params![second_id],
                |row| row.get(0),
            )
            .expect("count cascaded results");
        assert_eq!(stale_results, 0, "removed rows' url results cascade away");
    }

    #[test]
    fn update_reorder_only_keeps_ids_without_set_change() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
        let before = list_items(&conn, summary.id, 0, 10).expect("items");

        // same three links, reverse order — a pure reorder changes nothing
        // the live session's baked config depends on
        let reordered = SAMPLE.lines().rev().collect::<Vec<_>>().join("\n");
        let outcome = apply_update(&mut conn, summary.id, &reordered).expect("reorder update");
        assert!(!outcome.set_changed, "a reorder is not a set change");

        let rows: Vec<(i64, String, i64)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, tag, order_index FROM endpoints
                     WHERE profile_id = ?1 ORDER BY order_index",
                )
                .expect("prepare");
            let rows = stmt
                .query_map(params![summary.id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect");
            rows
        };
        assert_eq!(rows.len(), before.len());
        for (index, (id, tag, order_index)) in rows.iter().enumerate() {
            let original = before.iter().find(|row| &row.tag == tag).expect("same tags");
            assert_eq!(&original.id, id, "ids stay stable on a reorder");
            assert_eq!(index, *order_index as usize, "order follows fresh content");
        }
    }

    #[test]
    fn update_matches_duplicate_links_as_a_multiset() {
        let mut conn = test_db();
        // two identical links parse to two endpoints sharing one raw value
        let link = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#Twin";
        let twin = format!("{link}\n{link}");
        let summary = import_profile(&mut conn, "Twins".into(), None, None, &twin).expect("import");
        let twins = list_items(&conn, summary.id, 0, 10).expect("items");
        assert_eq!(twins.len(), 2, "duplicated links are both stored");
        let raws: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT raw FROM endpoints WHERE profile_id = ?1")
                .expect("prepare");
            let raws = stmt
                .query_map(params![summary.id], |row| row.get::<_, String>(0))
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect");
            raws
        };
        assert_eq!(raws[0], raws[1]);

        // a fresh copy of the same two links matches both stored rows —
        // nothing is inserted or removed
        let outcome = apply_update(&mut conn, summary.id, &twin).expect("twin update");
        assert!(!outcome.set_changed);

        // while a single copy matches only one of them
        let single = link.to_string();
        let outcome = apply_update(&mut conn, summary.id, &single).expect("shrink update");
        assert!(outcome.set_changed);
        assert_eq!(list_items(&conn, summary.id, 0, 10).expect("items").len(), 1);
    }

    #[test]
    fn load_source_requires_a_source() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Text only".into(), None, None, SAMPLE).expect("import");
        let error = load_source(&conn, summary.id).expect_err("no source");
        assert!(error.contains("no source"));
    }

    #[test]
    fn selected_endpoint_survives_updates() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
        let items = list_items(&conn, summary.id, 0, 10).expect("items");

        // pick and read back
        let changed = set_selected_endpoint(&conn, summary.id, Some(items[0].id)).expect("select");
        assert!(changed, "the first pick is a change");
        assert!(
            !set_selected_endpoint(&conn, summary.id, Some(items[0].id)).expect("re-pick"),
            "re-picking the same endpoint is not a change"
        );
        let selected = get_selected_endpoint(&conn, summary.id).expect("get selected");
        assert_eq!(selected.map(|item| item.tag), Some(items[0].tag.clone()));

        // a content update keeps surviving endpoints on their ids (same
        // raw links) — the selection resolves either way
        apply_update(&mut conn, summary.id, SAMPLE).expect("update");
        let still = get_selected_endpoint(&conn, summary.id)
            .expect("get selected")
            .expect("selection survives reimport");
        assert_eq!(still.tag, items[0].tag);
        let fresh = list_items(&conn, summary.id, 0, 10).expect("items");
        let fresh_id = fresh.iter().find(|item| item.tag == still.tag).map(|item| item.id);
        assert_eq!(fresh_id, Some(still.id), "resolved to the new row id");

        // content without the picked endpoint drops the selection
        apply_update(&mut conn, summary.id, "trojan://pw@1.1.1.1:443?security=tls#Only")
            .expect("update");
        assert!(get_selected_endpoint(&conn, summary.id).expect("get selected").is_none());

        // clearing works, foreign endpoints and unknown profiles are rejected
        set_selected_endpoint(&conn, summary.id, None).expect("clear");
        assert!(get_selected_endpoint(&conn, summary.id).expect("get selected").is_none());

        let other = import_profile(&mut conn, "Other".into(), None, None, SAMPLE).expect("import");
        let other_items = list_items(&conn, other.id, 0, 10).expect("items");
        let error = set_selected_endpoint(&conn, summary.id, Some(other_items[0].id))
            .expect_err("endpoint belongs to another profile");
        assert!(error.contains("not found in profile"));
        assert!(set_selected_endpoint(&conn, summary.id + 100, Some(1)).is_err());
    }

    /// A proxy-routed category with two test rules; returns their ids.
    fn seed_proxy_urls(conn: &Connection) -> Vec<i64> {
        conn.execute(
            "INSERT INTO test_site_categories (name, position, action) VALUES ('Cat', 0, 'proxy')",
            [],
        )
        .unwrap();
        let category_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO test_sites (category_id, rule_type, value)
             VALUES (?1, 'url', 'https://a.example/'), (?1, 'url', 'https://b.example/')",
            params![category_id],
        )
        .unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM test_sites WHERE category_id = ?1 ORDER BY id")
            .unwrap();
        stmt.query_map(params![category_id], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn items_carry_url_scores_and_endpoint_urls_join() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");

        // nothing probed: every item reports a zero score
        let items = list_items(&conn, summary.id, 0, 10).expect("items");
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|item| item.url_ok == 0 && item.url_total == 0));

        let url_ids = seed_proxy_urls(&conn);

        // first endpoint passes both URLs, second passes one, third is unprobed
        conn.execute(
            "INSERT INTO endpoint_url_results (endpoint_id, url_id, available, latency_ms) VALUES
             (?1, ?2, 1, 30), (?1, ?3, 1, 60), (?4, ?2, 1, 45), (?4, ?3, 0, NULL)",
            params![items[0].id, url_ids[0], url_ids[1], items[1].id],
        )
        .unwrap();

        let scored = list_items(&conn, summary.id, 0, 10).expect("items");
        assert_eq!((scored[0].url_ok, scored[0].url_total), (2, 2));
        assert_eq!((scored[1].url_ok, scored[1].url_total), (1, 2));
        assert_eq!((scored[2].url_ok, scored[2].url_total), (0, 0), "never probed, no score");

        // the selected-endpoint query carries the same score columns
        set_selected_endpoint(&conn, summary.id, Some(items[1].id)).expect("select");
        let selected = get_selected_endpoint(&conn, summary.id)
            .expect("get selected")
            .expect("selected");
        assert_eq!((selected.url_ok, selected.url_total), (1, 2));

        // lazy details: only the probed URLs are listed, under their category
        assert!(endpoint_url_statuses(&conn, items[2].id).expect("statuses").is_empty());
        let first = endpoint_url_statuses(&conn, items[0].id).expect("statuses");
        assert_eq!(first.len(), 2);
        assert!(first.iter().all(|status| status.category == "Cat"));
        assert_eq!(first[0].available, Some(true));
        assert_eq!(first[0].latency_ms, Some(30));
        assert_eq!(first[1].available, Some(true));

        // a dpi-routed category's URL neither scores nor shows up here
        conn.execute(
            "INSERT INTO test_site_categories (name, position, action) VALUES ('DpiCat', 1, 'dpi')",
            [],
        )
        .unwrap();
        let dpi_category = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO test_sites (category_id, rule_type, value) VALUES (?1, 'url', 'https://dpi.example/')",
            params![dpi_category],
        )
        .unwrap();
        let dpi_url = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO endpoint_url_results (endpoint_id, url_id, available) VALUES (?1, ?2, 1)",
            params![items[0].id, dpi_url],
        )
        .unwrap();
        let rescored = list_items(&conn, summary.id, 0, 10).expect("items");
        assert_eq!((rescored[0].url_ok, rescored[0].url_total), (2, 2), "dpi URLs do not count");
        assert_eq!(endpoint_url_statuses(&conn, items[0].id).expect("statuses").len(), 2);
    }

    #[test]
    fn items_are_sorted_by_outcome_speed_and_url_share() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
        let mut items = list_items(&conn, summary.id, 0, 10).expect("items");
        // profile order before any scan: First, Second, Third
        assert_eq!(
            items.iter().map(|item| item.tag.as_str()).collect::<Vec<_>>(),
            vec!["First", "Second", "Third"]
        );

        // a dead fourth endpoint joins the party
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available, latency_ms)
             VALUES (?1, 'Dead', 'trojan', 'raw', '{}', 3, 0, NULL)",
            params![summary.id],
        )
        .map_err(db_err)
        .unwrap();
        let url_ids = seed_proxy_urls(&conn);

        // Second: fastest (50ms) but no URL passes; First: 100ms, 1/2 URLs;
        // a 100ms newcomer with 2/2 URLs must outrank First (speed tie → share desc);
        // Third stays untested; Dead sinks to the bottom
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available, latency_ms)
             VALUES (?1, 'Full', 'vless', 'raw', '{}', 4, 1, 100)",
            params![summary.id],
        )
        .map_err(db_err)
        .unwrap();
        let full_id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE endpoints SET available = 1, latency_ms = 100 WHERE tag = 'First'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE endpoints SET available = 1, latency_ms = 50 WHERE tag = 'Second'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO endpoint_url_results (endpoint_id, url_id, available)
             VALUES ((SELECT id FROM endpoints WHERE tag = 'First'), ?1, 1),
                    ((SELECT id FROM endpoints WHERE tag = 'First'), ?2, 0),
                    (?3, ?1, 1),
                    (?3, ?2, 1)",
            params![url_ids[0], url_ids[1], full_id],
        )
        .unwrap();

        items = list_items(&conn, summary.id, 0, 10).expect("items");
        assert_eq!(
            items.iter().map(|item| item.tag.as_str()).collect::<Vec<_>>(),
            vec!["Second", "Full", "First", "Third", "Dead"],
            "reachable by speed, then URL share desc; untested middle; dead last"
        );

        // pagination follows the same global order (page 1 of 2)
        let page = list_items(&conn, summary.id, 0, 2).expect("page");
        assert_eq!(
            page.iter().map(|item| item.tag.as_str()).collect::<Vec<_>>(),
            vec!["Second", "Full"]
        );
    }

    #[test]
    fn auto_update_due_and_next_wake() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Auto".into(), Some("https://example.com/sub".into()), None, SAMPLE)
            .expect("import");

        // no schedule yet
        assert!(due_profiles(&conn).expect("due").is_empty());
        assert!(next_wake_seconds(&conn).expect("next").is_none());

        // validation: bad interval rejected
        assert!(set_auto_update(&conn, summary.id, Some(45)).is_err());

        // enable auto-update, pretend the last fetch was an hour ago
        set_auto_update(&conn, summary.id, Some(30)).expect("enable");
        conn.execute(
            "UPDATE profiles SET last_fetched_at = datetime('now', '-1 hour') WHERE id = ?1",
            params![summary.id],
        )
        .map_err(db_err)
        .unwrap();

        assert_eq!(due_profiles(&conn).expect("due"), vec![summary.id]);

        // after a "fetch" it is no longer due and the next wake is ~30 minutes
        apply_update(&mut conn, summary.id, SAMPLE).expect("update");
        assert!(due_profiles(&conn).expect("due").is_empty());
        let next = next_wake_seconds(&conn).expect("next").expect("scheduled");
        assert!((1790.0..=1810.0).contains(&next), "next wake was {next}");

        // a fresh profile with no last_fetched_at never fires
        let other = import_profile(&mut conn, "NoFetch".into(), Some("https://example.com/2".into()), None, SAMPLE)
            .expect("import");
        conn.execute(
            "UPDATE profiles SET auto_update_minutes = 60, last_fetched_at = NULL WHERE id = ?1",
            params![other.id],
        )
        .map_err(db_err)
        .unwrap();
        assert!(!due_profiles(&conn).expect("due").contains(&other.id));

        // sourceless profiles cannot enable auto-update
        let text_only = import_profile(&mut conn, "Text".into(), None, None, SAMPLE).expect("import");
        assert!(set_auto_update(&conn, text_only.id, Some(60)).is_err());
    }

    #[test]
    fn auto_update_pauses_after_consecutive_failures() {
        let mut conn = test_db();
        let summary = import_profile(
            &mut conn,
            "Auto".into(),
            Some("https://example.com/sub".into()),
            None,
            SAMPLE,
        )
        .expect("import");
        set_auto_update(&conn, summary.id, Some(30)).expect("enable");
        conn.execute(
            "UPDATE profiles SET last_fetched_at = datetime('now', '-1 hour') WHERE id = ?1",
            params![summary.id],
        )
        .map_err(db_err)
        .unwrap();

        // two failed scheduled updates: the profile stays scheduled
        assert_eq!(bump_auto_update_failures(&conn, summary.id).expect("bump"), 1);
        assert_eq!(bump_auto_update_failures(&conn, summary.id).expect("bump"), 2);
        assert_eq!(due_profiles(&conn).expect("due"), vec![summary.id]);
        assert!(next_wake_seconds(&conn).expect("next").is_some());

        // the third consecutive failure pauses the schedule: the scheduler
        // neither runs the profile nor holds its wake target overdue
        assert_eq!(bump_auto_update_failures(&conn, summary.id).expect("bump"), 3);
        assert!(due_profiles(&conn).expect("due").is_empty());
        assert!(next_wake_seconds(&conn).expect("next").is_none());

        // a failed bump of a missing profile is an error, not a pause
        assert!(bump_auto_update_failures(&conn, 999).is_err());

        // a successful update (the manual Update button runs the same
        // apply path) clears the streak and unpauses the schedule
        apply_update(&mut conn, summary.id, SAMPLE).expect("update");
        let failures: (i64,) = conn
            .query_row(
                "SELECT auto_update_failures FROM profiles WHERE id = ?1",
                params![summary.id],
                |row| Ok((row.get(0)?,)),
            )
            .unwrap();
        assert_eq!(failures.0, 0);
        conn.execute(
            "UPDATE profiles SET last_fetched_at = datetime('now', '-1 hour') WHERE id = ?1",
            params![summary.id],
        )
        .map_err(db_err)
        .unwrap();
        assert_eq!(due_profiles(&conn).expect("due"), vec![summary.id]);
    }

    #[test]
    fn selection_mode_roundtrip_and_validation() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");

        // fresh profiles are automatic (the migration default)
        assert_eq!(selection_mode(&conn, summary.id).expect("mode"), SELECT_FASTEST);

        for mode in SELECTION_MODES {
            set_selection_mode(&conn, summary.id, mode).expect("set mode");
            assert_eq!(selection_mode(&conn, summary.id).expect("mode"), mode);
        }
        assert!(set_selection_mode(&conn, summary.id, "random").is_err());
        assert!(set_selection_mode(&conn, 999, SELECT_MANUAL).is_err());
        assert!(selection_mode(&conn, 999).is_err());
    }

    fn candidate(tag: &str, available: Option<bool>, latency_ms: Option<i64>) -> EndpointCandidate {
        EndpointCandidate {
            id: 0,
            raw: format!("raw-{tag}"),
            tag: tag.to_string(),
            available,
            latency_ms,
            url_ok: 0,
            url_total: 0,
            order_index: 0,
        }
    }

    fn fixture() -> Vec<EndpointCandidate> {
        vec![
            candidate("A", Some(true), Some(120)),
            candidate("B", Some(true), Some(80)),
            candidate("C", None, None),
            candidate("D", Some(false), None),
            candidate("E", Some(true), Some(200)),
        ]
    }

    #[test]
    fn pick_fastest_prefers_reachable_lowest_latency() {
        let items = fixture();
        // B is reachable and fastest
        assert_eq!(pick_fastest(&items).expect("pick").tag, "B");

        // B dies → next reachable by latency (A), never the dead D
        let mut dead_b = items.clone();
        dead_b[1].available = Some(false);
        dead_b[1].latency_ms = None;
        assert_eq!(pick_fastest(&dead_b).expect("pick").tag, "A");

        // nothing tested: the untested C outranks the dead D
        let fresh = vec![candidate("C", None, None), candidate("D", Some(false), None)];
        assert_eq!(pick_fastest(&fresh).expect("pick").tag, "C");

        assert!(pick_fastest(&[]).is_none());
    }

    #[test]
    fn pick_most_available_ranks_by_url_share() {
        let mut items = fixture();
        // A loads 1/2 URLs, E loads 2/2 — E wins despite being slower
        items[0].url_ok = 1;
        items[0].url_total = 2;
        items[4].url_ok = 2;
        items[4].url_total = 2;
        assert_eq!(pick_most_available(&items).expect("pick").tag, "E");

        // a probed zero share ranks below an unprobed reachable endpoint
        let mut zero = fixture();
        zero[1].url_ok = 0;
        zero[1].url_total = 2;
        assert_eq!(pick_most_available(&zero).expect("pick").tag, "A");

        // dead endpoints never win
        let dead = vec![candidate("D", Some(false), None)];
        assert_eq!(pick_most_available(&dead).expect("pick").tag, "D");
    }

    #[test]
    fn pick_round_robin_rotates_and_wraps() {
        let items = fixture();

        // from A → the next reachable is B; wrapping E → A
        assert_eq!(pick_round_robin(&items, Some("raw-A")).expect("pick").tag, "B");
        assert_eq!(pick_round_robin(&items, Some("raw-E")).expect("pick").tag, "A");

        // skips the dead D and the untested C
        assert_eq!(pick_round_robin(&items, Some("raw-B")).expect("pick").tag, "E");

        // nothing reachable after the cursor → the current one is kept
        let all_dead = vec![candidate("X", Some(false), None), candidate("Y", Some(false), None)];
        assert_eq!(pick_round_robin(&all_dead, Some("raw-X")).expect("pick").tag, "X");

        // no cursor: the first reachable endpoint
        assert_eq!(pick_round_robin(&items, None).expect("pick").tag, "A");

        // no cursor, nothing reachable, scan data exists → nothing to
        // rotate to (never an untested or known-dead endpoint)
        let scanned = vec![candidate("X", Some(false), None), candidate("Y", None, None)];
        assert!(pick_round_robin(&scanned, None).is_none());

        // no cursor, never scanned → the first listed endpoint
        let fresh = vec![candidate("X", None, None), candidate("Y", None, None)];
        assert_eq!(pick_round_robin(&fresh, None).expect("pick").tag, "X");

        assert!(pick_round_robin(&[], Some("raw-A")).is_none());
    }

    #[test]
    fn load_candidates_keeps_list_order_and_scores() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
        let url_ids = seed_proxy_urls(&conn);
        let items = list_items(&conn, summary.id, 0, 10).expect("items");

        // deep-probe the first endpoint 1/2
        conn.execute(
            "INSERT INTO endpoint_url_results (endpoint_id, url_id, available)
             VALUES (?1, ?2, 1), (?1, ?3, 0)",
            params![items[0].id, url_ids[0], url_ids[1]],
        )
        .unwrap();

        let candidates = load_candidates(&conn, summary.id).expect("candidates");
        assert_eq!(
            candidates.iter().map(|c| c.tag.as_str()).collect::<Vec<_>>(),
            vec!["First", "Second", "Third"],
            "list order, not the availability sort"
        );
        assert_eq!((candidates[0].url_ok, candidates[0].url_total), (1, 2));
        assert_eq!(candidates[0].tag, items[0].tag);
        assert!(selected_raw_key(&conn, summary.id).expect("raw key").is_none());

        // broken outbounds never become candidates
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
             VALUES (?1, 'Broken', 'vless', 'raw-broken', '{}', 9)",
            params![summary.id],
        )
        .unwrap();
        let candidates = load_candidates(&conn, summary.id).expect("candidates");
        assert!(!candidates.iter().any(|c| c.tag == "Broken"));
        assert!(load_candidates(&conn, 999).is_err(), "unknown profile");
    }

    #[test]
    fn load_reachable_candidates_keeps_only_proven_endpoints() {
        let conn = test_db();
        conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 3)", [])
            .expect("seed profile");
        let profile_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available)
             VALUES (?1, 'Live',  'vless',  'raw-live',  ?2, 0, 1),
                    (?1, 'Dead',  'vless',  'raw-dead',  ?2, 1, 0),
                    (?1, 'Fresh', 'vless',  'raw-fresh', ?2, 2, NULL),
                    (?1, 'Broken','vless',  'raw-broke', '{}', 3, 1)",
            params![
                profile_id,
                r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#
            ],
        )
        .unwrap();

        let candidates = load_reachable_candidates(&conn, profile_id).expect("candidates");
        assert_eq!(
            candidates.iter().map(|c| c.tag.as_str()).collect::<Vec<_>>(),
            vec!["Live"],
            "only reachable rows with a usable outbound, in list order"
        );
        assert_eq!(candidates[0].available, Some(true));
    }

    #[test]
    fn meets_availability_floor_boundaries() {
        // unprobed endpoints and a disabled floor accept everything
        assert!(meets_availability_floor(0, 0, 55));
        assert!(meets_availability_floor(0, 5, 0));
        assert!(meets_availability_floor(0, 5, -1));
        // exactly at the floor passes, one under it fails
        assert!(meets_availability_floor(55, 100, 55));
        assert!(!meets_availability_floor(54, 100, 55));
        assert!(meets_availability_floor(11, 20, 55));
        assert!(!meets_availability_floor(1, 2, 55));
        // a 100% floor demands every probed URL
        assert!(meets_availability_floor(2, 2, 100));
        assert!(!meets_availability_floor(1, 2, 100));
    }

    #[test]
    fn availability_floor_marks_low_share_endpoints_dead() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
        let url_ids = seed_proxy_urls(&conn);
        let items = list_items(&conn, summary.id, 0, 10).expect("items");

        // all reachable; First passes 1/2 (50%) and is the fastest,
        // Second passes 2/2, Third was never deep-probed
        conn.execute(
            "UPDATE endpoints SET available = 1, latency_ms = 50 WHERE tag = 'First'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE endpoints SET available = 1, latency_ms = 120 WHERE tag = 'Second'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE endpoints SET available = 1, latency_ms = 200 WHERE tag = 'Third'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO endpoint_url_results (endpoint_id, url_id, available) VALUES
             (?1, ?2, 1), (?1, ?3, 0), (?4, ?2, 1), (?4, ?3, 1)",
            params![items[0].id, url_ids[0], url_ids[1], items[1].id],
        )
        .unwrap();

        // the default floor is 55: First reads as dead to the strategies,
        // its raw score columns are still reported
        let candidates = load_candidates(&conn, summary.id).expect("candidates");
        let first = candidates.iter().find(|c| c.tag == "First").unwrap();
        assert_eq!(first.available, Some(false), "below the floor reads as dead");
        assert_eq!((first.url_ok, first.url_total), (1, 2), "the score itself is kept");
        assert_eq!(
            candidates
                .iter()
                .find(|c| c.tag == "Second")
                .unwrap()
                .available,
            Some(true)
        );
        assert_eq!(
            candidates
                .iter()
                .find(|c| c.tag == "Third")
                .unwrap()
                .available,
            Some(true),
            "an unprobed reachable endpoint stays alive"
        );

        // every strategy avoids the below-floor endpoint
        assert_eq!(pick_fastest(&candidates).expect("pick").tag, "Second");
        assert_eq!(pick_most_available(&candidates).expect("pick").tag, "Second");
        // round robin wrapping past First lands on Second
        let third_raw = candidates
            .iter()
            .find(|c| c.tag == "Third")
            .unwrap()
            .raw
            .clone();
        assert_eq!(
            pick_round_robin(&candidates, Some(&third_raw)).expect("pick").tag,
            "Second"
        );

        // the rescue list drops it outright
        let reachable = load_reachable_candidates(&conn, summary.id).expect("reachable");
        assert_eq!(
            reachable.iter().map(|c| c.tag.as_str()).collect::<Vec<_>>(),
            vec!["Second", "Third"]
        );

        // floor 0 (off) restores First as an ordinary reachable endpoint
        crate::db::set_setting(&conn, "min_endpoint_availability_percent", "0").unwrap();
        let candidates = load_candidates(&conn, summary.id).expect("candidates");
        assert_eq!(
            candidates
                .iter()
                .find(|c| c.tag == "First")
                .unwrap()
                .available,
            Some(true)
        );
        assert_eq!(pick_fastest(&candidates).expect("pick").tag, "First");
    }

    #[test]
    fn item_index_matches_list_order() {
        let mut conn = test_db();
        let summary = import_profile(&mut conn, "Sub".into(), None, None, SAMPLE).expect("import");
        let items = list_items(&conn, summary.id, 0, 10).expect("items");

        for (expected, item) in items.iter().enumerate() {
            let index = item_index(&conn, summary.id, item.id).expect("index");
            assert_eq!(index, Some(expected as i64));
        }
        assert_eq!(
            item_index(&conn, summary.id, 999),
            Ok(None),
            "unknown endpoint"
        );

        let other = import_profile(
            &mut conn,
            "Other".into(),
            None,
            None,
            "vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@1.2.3.4:443?security=tls&sni=example.com#Only",
        )
        .expect("import");
        assert_eq!(
            item_index(&conn, other.id, items[0].id),
            Ok(None),
            "endpoint of another profile"
        );
    }
}
