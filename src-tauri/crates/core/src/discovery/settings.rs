//! Discovery settings (the Discovery page's Run card): the persisted
//! "test after" default of the Run button and the optional auto-update
//! interval the background scheduler serves — when due, it re-runs
//! Discovery with the stored options. `next_due_seconds` computes the
//! wake target from `discovery_last_run_at` (stamped at every run start,
//! manual or scheduled) with the same `julianday` arithmetic the profile
//! auto-updates use; a missing or invalid stamp means "due now".

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use super::storage::db_err;
use crate::db::{get_setting, set_setting};

pub(super) const KEY_AUTO_UPDATE_MINUTES: &str = "discovery_auto_update_minutes";
pub(super) const KEY_TEST_AFTER: &str = "discovery_test_after";
pub(super) const KEY_LAST_RUN_AT: &str = "discovery_last_run_at";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverySettings {
    /// Minutes between scheduled discovery runs; `None` — manual only.
    pub auto_update_minutes: Option<i64>,
    /// Whether a run follows up with a latency scan on every touched
    /// profile.
    pub test_after: bool,
}

/// Loads the settings, falling back to the defaults on missing/junk
/// values (a hand-edited DB must not break the scheduler).
pub fn load(conn: &Connection) -> Result<DiscoverySettings, String> {
    let auto_update_minutes = get_setting(conn, KEY_AUTO_UPDATE_MINUTES)
        .map_err(db_err)?
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|minutes| *minutes >= 1);
    let test_after = get_setting(conn, KEY_TEST_AFTER)
        .map_err(db_err)?
        .map(|value| value == "1")
        .unwrap_or(false);
    Ok(DiscoverySettings { auto_update_minutes, test_after })
}

/// Persists both keys (the values are validated); `None` drops the
/// interval key so nothing is scheduled.
pub fn save(conn: &Connection, settings: &DiscoverySettings) -> Result<(), String> {
    if settings.auto_update_minutes.is_some_and(|minutes| minutes < 1) {
        return Err("the auto-update interval must be at least 1 minute".to_string());
    }
    match settings.auto_update_minutes {
        Some(minutes) => {
            set_setting(conn, KEY_AUTO_UPDATE_MINUTES, &minutes.to_string()).map_err(db_err)?;
        }
        None => {
            conn.execute(
                "DELETE FROM app_settings WHERE key = ?1",
                [KEY_AUTO_UPDATE_MINUTES],
            )
            .map_err(db_err)?;
        }
    }
    set_setting(conn, KEY_TEST_AFTER, if settings.test_after { "1" } else { "0" }).map_err(db_err)?;
    Ok(())
}

/// Seconds until the next scheduled discovery run (0.0 when due or
/// overdue); `None` when no interval is set. The last-run stamp lives in
/// the KV, so its value rides a scalar subselect into the `julianday`
/// arithmetic — a missing or invalid stamp is `julianday(NULL)`, i.e.
/// due now.
pub fn next_due_seconds(conn: &Connection) -> Result<Option<f64>, String> {
    let Some(minutes) = load(conn)?.auto_update_minutes else {
        return Ok(None);
    };
    let seconds: Option<f64> = conn
        .query_row(
            &format!(
                "SELECT (julianday((SELECT value FROM app_settings WHERE key = '{KEY_LAST_RUN_AT}'),
                                   '+' || ?1 || ' minutes') - julianday('now')) * 86400.0"
            ),
            params![minutes],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    Ok(Some(seconds.unwrap_or(0.0).max(0.0)))
}
