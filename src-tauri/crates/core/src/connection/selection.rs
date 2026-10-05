//! The endpoint a connect dials: the stored selection and its resolution,
//! the dead-selection check, the strategy auto-pick that fills an absent or
//! dead one, and the check-loop's repick among loadable survivors.

use std::collections::HashSet;

use rusqlite::{Connection, params};
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::latency;
use crate::profiles;
use crate::AppState;

use super::config::{BakedEndpoint, bake_endpoints};

#[derive(Debug)]
pub(super) struct SelectedEndpoint {
    pub(super) id: i64,
    pub(super) tag: String,
}

/// The checkmark: the endpoint stored via `selected_endpoint_key`. With the
/// automatic strategies an absent / dangling / broken selection is not an
/// error — the connect flow picks a replacement (see `auto_pick_endpoint`).
pub(super) fn load_selected_endpoint(
    conn: &Connection,
    profile_id: i64,
) -> Result<(String, Option<SelectedEndpoint>), String> {
    let (profile_name, key) = conn
        .query_row(
            "SELECT name, selected_endpoint_key FROM profiles WHERE id = ?1",
            params![profile_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
            other => format!("database error: {other}"),
        })?;
    let Some(key) = key.filter(|key| !key.is_empty()) else {
        return Ok((profile_name, None));
    };
    let row = match conn.query_row(
        "SELECT id, tag, outbound_json FROM endpoints
         WHERE profile_id = ?1 AND raw = ?2
         ORDER BY order_index LIMIT 1",
        params![profile_id, key],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    ) {
        Ok(row) => row,
        // the endpoint vanished in a content update — same as no choice
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok((profile_name, None)),
        Err(other) => return Err(format!("database error: {other}")),
    };
    let (id, tag, outbound_json) = row;
    let outbound: Value = match serde_json::from_str(&outbound_json) {
        // a broken stored outbound is no choice either — the auto-pick
        // replaces it instead of failing the connect
        Ok(outbound) => outbound,
        Err(_) => return Ok((profile_name, None)),
    };
    let type_ok = outbound
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| !kind.is_empty());
    if !type_ok {
        return Ok((profile_name, None));
    }
    Ok((profile_name, Some(SelectedEndpoint { id, tag })))
}

/// Whether the stored selection resolves to an endpoint the selection data
/// marks unusable: the last scan marked it unreachable, or its deep-probe
/// share fell below the availability floor (`load_candidates` reports both
/// as dead). No selection, a never-tested endpoint or a lookup error read
/// as `false`.
pub(super) fn selection_known_dead(conn: &Connection, profile_id: i64) -> bool {
    let Some(raw) = profiles::selected_raw_key(conn, profile_id)
        .ok()
        .flatten()
        .filter(|key| !key.is_empty())
    else {
        return false;
    };
    profiles::load_candidates(conn, profile_id)
        .ok()
        .and_then(|candidates| candidates.into_iter().find(|candidate| candidate.raw == raw))
        .is_some_and(|candidate| candidate.available == Some(false))
}

/// Fills an absent selection with the strategy's pick so a connect always
/// has an endpoint to dial (a manual mode without a checkmark falls back
/// to the fastest pick; round robin rotates from the stored cursor).
/// Persists the choice — the UI shows what runs.
pub(super) fn auto_pick_endpoint(
    conn: &Connection,
    profile_id: i64,
    mode: &str,
) -> Result<Option<SelectedEndpoint>, String> {
    let candidates = profiles::load_candidates(conn, profile_id)?;
    let current = profiles::selected_raw_key(conn, profile_id).ok().flatten();
    let pick = profiles::pick_by_strategy(mode, &candidates, current.as_deref());
    if let Some(pick) = pick {
        profiles::set_selected_endpoint(conn, profile_id, Some(pick.id))?;
        return load_selected_endpoint(conn, profile_id).map(|(_, endpoint)| endpoint);
    }
    Ok(None)
}

/// (row id, display tag) of the stored selection; `None` when nothing is
/// selected or the key dangles (the same resolution as
/// `load_selected_endpoint`).
pub(super) fn selected_endpoint_brief(
    conn: &Connection,
    profile_id: i64,
) -> Result<Option<(i64, String)>, String> {
    let Some(key) = profiles::selected_raw_key(conn, profile_id).ok().flatten() else {
        return Ok(None);
    };
    match conn.query_row(
        "SELECT id, tag FROM endpoints
         WHERE profile_id = ?1 AND raw = ?2
         ORDER BY order_index LIMIT 1",
        params![profile_id, key],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    ) {
        Ok(brief) => Ok(Some(brief)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(other) => Err(format!("database error: {other}")),
    }
}

/// What the connect loop does with the endpoint selection after the bake
/// was cleaned of outbounds sing-box cannot load.
#[derive(Debug)]
pub(super) enum Repick {
    /// The selection survived the isolation — keep dialing it (an explicit
    /// manual pick and this connect's own auto-pick alike).
    Kept(SelectedEndpoint),
    /// The strategy re-picked among the loadable survivors (persisted).
    Switched(SelectedEndpoint),
    /// The explicit manual selection is itself among the unloadable rows —
    /// the caller surfaces the check error instead of replacing the user's
    /// pick behind their back.
    Lost,
    /// Nothing loadable survived and there is no manual pick to report.
    Empty,
    /// Loadable survivors exist but the strategy picks none of them (only
    /// round robin refuses — it rotates strictly among scan-reachable
    /// endpoints and never dials an untested or known-dead one).
    Refused,
}

/// The connect loop's landing when the run config was rejected by
/// `sing-box check`: validates every baked outbound with the same bisection
/// the latency scan uses, marks the rows sing-box cannot load as
/// unavailable (so scans and picks skip them from now on), and resolves the
/// selection against the survivors (see `decide_repick`). Returns the
/// rebaked survivors and the selection verdict.
pub(super) async fn isolate_and_repick(
    app: &AppHandle,
    profile_id: i64,
    mode: &str,
    manual_pick: bool,
    baked: Vec<BakedEndpoint>,
    current: Option<SelectedEndpoint>,
    untestable: Vec<i64>,
) -> Result<(Vec<BakedEndpoint>, Repick), String> {
    let rows: Vec<latency::OutboundRow> = baked
        .iter()
        .map(|endpoint| (endpoint.id, endpoint.outbound.clone()))
        .collect();
    let (loadable, mut broken) = latency::isolate_unloadable_outbounds(app, rows).await?;
    broken.extend(untestable);
    let loadable_ids: HashSet<i64> = loadable.iter().map(|(id, _)| *id).collect();
    let decision = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
        for id in &broken {
            conn.execute(
                "UPDATE endpoints SET available = 0, last_tested_at = datetime('now')
                 WHERE id = ?1",
                params![id],
            )
            .map_err(|e| format!("database error: {e}"))?;
        }
        decide_repick(&conn, profile_id, mode, manual_pick, current, &loadable_ids)?
    };
    Ok((bake_endpoints(loadable), decision))
}

/// The selection verdict over the loadable id set. A surviving selection is
/// kept untouched — the check failure came from some other row, so the
/// user's manual checkmark (and this connect's own auto-pick) must survive
/// the cleanup. Only a manual pick that sing-box cannot load itself is
/// reported as `Lost`; every other lost/absent selection re-picks per the
/// strategy among the survivors and persists that pick.
pub(super) fn decide_repick(
    conn: &Connection,
    profile_id: i64,
    mode: &str,
    manual_pick: bool,
    current: Option<SelectedEndpoint>,
    loadable_ids: &HashSet<i64>,
) -> Result<Repick, String> {
    if current.as_ref().is_some_and(|pick| loadable_ids.contains(&pick.id)) {
        return Ok(Repick::Kept(current.expect("the survival check just passed")));
    }
    if manual_pick && current.is_some() {
        return Ok(Repick::Lost);
    }
    let mut candidates = profiles::load_candidates(conn, profile_id)?;
    candidates.retain(|candidate| loadable_ids.contains(&candidate.id));
    let current_key = profiles::selected_raw_key(conn, profile_id)?;
    let Some(pick) = profiles::pick_by_strategy(mode, &candidates, current_key.as_deref()) else {
        return Ok(if candidates.is_empty() { Repick::Empty } else { Repick::Refused });
    };
    profiles::set_selected_endpoint(conn, profile_id, Some(pick.id))?;
    let (_, endpoint) = load_selected_endpoint(conn, profile_id)?;
    Ok(Repick::Switched(endpoint.expect("the pick was just persisted")))
}
