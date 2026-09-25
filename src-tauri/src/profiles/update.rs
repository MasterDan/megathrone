use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{params, Connection};
use tauri::{AppHandle, Emitter, Manager};

use crate::connection;
use crate::latency;
use crate::parser::{self, ParsedProfile};
use crate::AppState;

use super::model::{
    ProfileSummary, ProfileUpdateNotice, PROFILE_UPDATE_EVENT, UPDATE_NOTICE_FAILED,
    UPDATE_NOTICE_STARTED, UPDATE_NOTICE_SUCCEEDED,
};
use super::sources::{
    after_mutation, fetch_url, read_file, CONTENT, UPDATE_DONE, UPDATING,
};
use super::storage::{bump_auto_update_failures, db_err, summary_by_id, MAX_AUTO_UPDATE_FAILURES};

/// Where a profile's content comes from; URL wins if both are somehow set.
#[derive(Debug, Clone)]
pub enum Source {
    Url(String),
    Path(String),
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

pub(super) fn load_source(conn: &Connection, profile_id: i64) -> Result<Source, String> {
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
pub(super) fn apply_update(conn: &mut Connection, profile_id: i64, content: &str) -> Result<UpdateOutcome, String> {
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

// ---------------------------------------------------------------------------
// Update registry
// ---------------------------------------------------------------------------

/// Profile ids with an update currently in flight (button click or the
/// background scheduler), so a re-mounted page can adopt the "updating" state
/// via `profile_update_status`.
static RUNNING_UPDATES: LazyLock<Mutex<HashSet<i64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub(super) fn lock_running_updates() -> Result<MutexGuard<'static, HashSet<i64>>, String> {
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

fn lock_db_ref<'a>(
    state: &'a tauri::State<'_, AppState>,
) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}
