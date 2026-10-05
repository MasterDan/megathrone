use rusqlite::{params, Connection};

use crate::settings;

use super::endpoints::URL_SCORE_COLUMNS;
use super::model::{
    EndpointCandidate, SELECT_MANUAL, SELECT_MOST_AVAILABLE, SELECT_ROUND_ROBIN, SELECTION_MODES,
};
use super::storage::db_err;

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
