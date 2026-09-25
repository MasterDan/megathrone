//! General settings (Settings → General): the supervisor cadences of the
//! automatic endpoint strategies (`auto_select`), the close-to-tray
//! behavior, and the "raw" local proxy port.
//!
//! While a profile with an automatic strategy is connected, the
//! supervisor re-checks the endpoints the last scan proved reachable every
//! `recheck_minutes` (refreshing latency/availability data and the
//! fastest/most-available verdicts), and round robin additionally rotates
//! to the next reachable endpoint every `rotation_minutes`.
//!
//! With `close_to_tray` on (the default), closing the window hides it to
//! the tray icon instead of quitting the app.
//!
//! The raw local proxy (on by default) is an extra mixed port sing-box
//! opens next to the session: everything apps send there — private ranges
//! included — goes through the currently selected endpoint, ignoring the
//! routing categories entirely (see `connection`).
//!
//! `min_availability_percent` (default 55, 0 — off) is the availability
//! floor: an endpoint whose last deep-probe share over the proxy-routed
//! test rules falls below it is treated as dead by every automatic
//! endpoint pick (see `profiles::load_candidates`).

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::{get_setting, set_setting};

/// Minutes between round robin rotations to the next reachable endpoint.
pub const DEFAULT_ROTATION_MINUTES: i64 = 10;
/// Minutes between background re-checks of the scan-reachable endpoints.
pub const DEFAULT_RECHECK_MINUTES: i64 = 10;
/// Whether closing the window hides it to the tray instead of quitting.
pub const DEFAULT_CLOSE_TO_TRAY: bool = true;
/// Minimum deep-probe success share (percent) for an endpoint to stay
/// selectable by the automatic strategies; 0 disables the floor.
pub const DEFAULT_MIN_AVAILABILITY_PERCENT: i64 = 55;
/// Whether the raw local proxy port is opened alongside a session.
pub const DEFAULT_RAW_PROXY_ENABLED: bool = true;
/// The configured port of the raw local proxy (apps with a built-in proxy
/// setting point at `127.0.0.1:<port>`).
pub const DEFAULT_RAW_PROXY_PORT: i64 = 7890;

pub(super) const KEY_ROTATION_MINUTES: &str = "rotation_interval_minutes";
pub(super) const KEY_RECHECK_MINUTES: &str = "recheck_interval_minutes";
pub(super) const KEY_CLOSE_TO_TRAY: &str = "close_to_tray";
pub(super) const KEY_MIN_AVAILABILITY: &str = "min_endpoint_availability_percent";
pub(super) const KEY_RAW_PROXY_ENABLED: &str = "raw_proxy_enabled";
pub(super) const KEY_RAW_PROXY_PORT: &str = "raw_proxy_port";

const MAX_INTERVAL_MINUTES: i64 = 24 * 60;
const MAX_AVAILABILITY_PERCENT: i64 = 100;
const MAX_RAW_PROXY_PORT: i64 = 65535;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettings {
    pub rotation_minutes: i64,
    pub recheck_minutes: i64,
    pub close_to_tray: bool,
    pub min_availability_percent: i64,
    pub raw_proxy_enabled: bool,
    pub raw_proxy_port: i64,
}

/// The general settings, falling back to the defaults on missing/junk
/// values (hand-edited DBs must not break the scan or the supervisor).
pub fn load_general(conn: &Connection) -> Result<GeneralSettings, String> {
    let read = |key: &str, default: i64, min: i64, max: i64| -> Result<i64, String> {
        Ok(get_setting(conn, key)
            .map_err(db_err)?
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| (min..=max).contains(value))
            .unwrap_or(default))
    };
    let read_bool = |key: &str, default: bool| -> Result<bool, String> {
        Ok(get_setting(conn, key)
            .map_err(db_err)?
            .and_then(|value| match value.trim() {
                "1" | "true" => Some(true),
                "0" | "false" => Some(false),
                _ => None,
            })
            .unwrap_or(default))
    };
    Ok(GeneralSettings {
        rotation_minutes: read(
            KEY_ROTATION_MINUTES,
            DEFAULT_ROTATION_MINUTES,
            1,
            MAX_INTERVAL_MINUTES,
        )?,
        recheck_minutes: read(
            KEY_RECHECK_MINUTES,
            DEFAULT_RECHECK_MINUTES,
            1,
            MAX_INTERVAL_MINUTES,
        )?,
        close_to_tray: read_bool(KEY_CLOSE_TO_TRAY, DEFAULT_CLOSE_TO_TRAY)?,
        min_availability_percent: read(
            KEY_MIN_AVAILABILITY,
            DEFAULT_MIN_AVAILABILITY_PERCENT,
            0,
            MAX_AVAILABILITY_PERCENT,
        )?,
        raw_proxy_enabled: read_bool(KEY_RAW_PROXY_ENABLED, DEFAULT_RAW_PROXY_ENABLED)?,
        raw_proxy_port: read(
            KEY_RAW_PROXY_PORT,
            DEFAULT_RAW_PROXY_PORT,
            1,
            MAX_RAW_PROXY_PORT,
        )?,
    })
}

/// Writes every general-settings key back (the values are validated).
pub(super) fn store_general(conn: &Connection, settings: &GeneralSettings) -> Result<(), String> {
    set_setting(conn, KEY_ROTATION_MINUTES, &settings.rotation_minutes.to_string())
        .map_err(db_err)?;
    set_setting(conn, KEY_RECHECK_MINUTES, &settings.recheck_minutes.to_string())
        .map_err(db_err)?;
    set_setting(conn, KEY_CLOSE_TO_TRAY, &settings.close_to_tray.to_string())
        .map_err(db_err)?;
    set_setting(
        conn,
        KEY_MIN_AVAILABILITY,
        &settings.min_availability_percent.to_string(),
    )
    .map_err(db_err)?;
    set_setting(conn, KEY_RAW_PROXY_ENABLED, &(settings.raw_proxy_enabled as i64).to_string())
        .map_err(db_err)?;
    set_setting(conn, KEY_RAW_PROXY_PORT, &settings.raw_proxy_port.to_string())
        .map_err(db_err)?;
    Ok(())
}

pub(super) fn validate_general(settings: &GeneralSettings) -> Result<(), String> {
    if !(1..=MAX_INTERVAL_MINUTES).contains(&settings.rotation_minutes) {
        return Err(format!(
            "the rotation interval must be between 1 and {MAX_INTERVAL_MINUTES} minutes"
        ));
    }
    if !(1..=MAX_INTERVAL_MINUTES).contains(&settings.recheck_minutes) {
        return Err(format!(
            "the re-check interval must be between 1 and {MAX_INTERVAL_MINUTES} minutes"
        ));
    }
    if !(0..=MAX_AVAILABILITY_PERCENT).contains(&settings.min_availability_percent) {
        return Err(format!(
            "the minimum availability must be between 0 and {MAX_AVAILABILITY_PERCENT} percent"
        ));
    }
    if !(1..=MAX_RAW_PROXY_PORT).contains(&settings.raw_proxy_port) {
        return Err(format!(
            "the local proxy port must be between 1 and {MAX_RAW_PROXY_PORT}"
        ));
    }
    Ok(())
}

fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_validates_the_ranges() {
        let base = || GeneralSettings {
            rotation_minutes: DEFAULT_ROTATION_MINUTES,
            recheck_minutes: DEFAULT_RECHECK_MINUTES,
            close_to_tray: DEFAULT_CLOSE_TO_TRAY,
            min_availability_percent: DEFAULT_MIN_AVAILABILITY_PERCENT,
            raw_proxy_enabled: DEFAULT_RAW_PROXY_ENABLED,
            raw_proxy_port: DEFAULT_RAW_PROXY_PORT,
        };
        // the cadences share one sanity window: 1..=1440 minutes
        for minutes in [0, -5, 1441] {
            assert!(validate_general(&GeneralSettings {
                rotation_minutes: minutes, ..base()
            })
            .is_err(), "rotation {minutes} must be rejected");
            assert!(validate_general(&GeneralSettings {
                recheck_minutes: minutes, ..base()
            })
            .is_err(), "recheck {minutes} must be rejected");
        }
        assert!(validate_general(&GeneralSettings {
            rotation_minutes: 1,
            recheck_minutes: 1440,
            ..base()
        })
        .is_ok());
        // the availability floor: 0 (off) and 100 bracket the window
        for percent in [0, 100] {
            assert!(validate_general(&GeneralSettings {
                min_availability_percent: percent,
                ..base()
            })
            .is_ok(), "floor {percent} must be accepted");
        }
        for percent in [-1, 101] {
            assert!(validate_general(&GeneralSettings {
                min_availability_percent: percent,
                ..base()
            })
            .is_err(), "floor {percent} must be rejected");
        }
        // the raw proxy port: 1 and 65535 bracket the window
        for port in [1, 65535] {
            assert!(validate_general(&GeneralSettings {
                raw_proxy_port: port,
                ..base()
            })
            .is_ok(), "port {port} must be accepted");
        }
        for port in [0, -1, 65536] {
            assert!(validate_general(&GeneralSettings {
                raw_proxy_port: port,
                ..base()
            })
            .is_err(), "port {port} must be rejected");
        }
    }
}
