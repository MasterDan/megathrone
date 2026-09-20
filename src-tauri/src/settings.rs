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
use tauri::{AppHandle, State};

use crate::AppState;
use crate::connection;
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

const KEY_ROTATION_MINUTES: &str = "rotation_interval_minutes";
const KEY_RECHECK_MINUTES: &str = "recheck_interval_minutes";
const KEY_CLOSE_TO_TRAY: &str = "close_to_tray";
const KEY_MIN_AVAILABILITY: &str = "min_endpoint_availability_percent";
const KEY_RAW_PROXY_ENABLED: &str = "raw_proxy_enabled";
const KEY_RAW_PROXY_PORT: &str = "raw_proxy_port";

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

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn settings_get_general(state: State<AppState>) -> Result<GeneralSettings, String> {
    let conn = lock_db(&state)?;
    load_general(&conn)
}

/// Persists a merged update: loads the stored settings, overlays `patch`,
/// validates the whole struct and writes every key back. `settings_set_*`
/// commands each own one card of the General tab, so saving one card never
/// commits (or rejects on) the other card's pending edits.
fn settings_set(
    state: State<AppState>,
    patch: impl FnOnce(&mut GeneralSettings),
) -> Result<GeneralSettings, String> {
    let conn = lock_db(&state)?;
    let mut settings = load_general(&conn)?;
    patch(&mut settings);
    validate_general(&settings)?;
    store_general(&conn, &settings)?;
    Ok(settings)
}

#[tauri::command]
pub fn settings_set_cadences(
    state: State<AppState>,
    rotation_minutes: i64,
    recheck_minutes: i64,
    min_availability_percent: i64,
) -> Result<GeneralSettings, String> {
    settings_set(state, |settings| {
        settings.rotation_minutes = rotation_minutes;
        settings.recheck_minutes = recheck_minutes;
        settings.min_availability_percent = min_availability_percent;
    })
}

#[tauri::command]
pub fn settings_set_close_to_tray(
    state: State<AppState>,
    close_to_tray: bool,
) -> Result<GeneralSettings, String> {
    settings_set(state, |settings| {
        settings.close_to_tray = close_to_tray;
    })
}

/// Saves the raw local proxy card (toggle + port). While a session with a
/// dialed endpoint runs, a change that alters the generated config (toggle
/// flip, port change) reconnects it — the outcome lands in the global
/// toast; a no-op save never restarts.
#[tauri::command]
pub async fn settings_set_raw_proxy(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
    port: i64,
) -> Result<GeneralSettings, String> {
    let before = {
        let conn = lock_db(&state)?;
        load_general(&conn)?
    };
    let settings = {
        let conn = lock_db(&state)?;
        let mut settings = load_general(&conn)?;
        settings.raw_proxy_enabled = enabled;
        settings.raw_proxy_port = port;
        validate_general(&settings)?;
        store_general(&conn, &settings)?;
        settings
    };
    apply_raw_proxy_change_live(
        &app,
        (before.raw_proxy_enabled, before.raw_proxy_port),
        (settings.raw_proxy_enabled, settings.raw_proxy_port),
    )
    .await;
    Ok(settings)
}

/// A raw-proxy setting just changed: reconnect the live session iff the
/// inbound this change would bake differs from what the config holds now —
/// the toggle flipped, or the port changed while on. Nothing running, a
/// DPI-only session (no endpoint → no raw inbound in its config) and
/// no-op saves leave the session alone. Reports via `restart-result`.
async fn apply_raw_proxy_change_live(
    app: &AppHandle,
    before: (bool, i64),
    after: (bool, i64),
) {
    let baked = |(enabled, port): (bool, i64)| enabled.then_some(port);
    if baked(before) == baked(after) {
        return;
    }
    let Some(profile_id) = connection::running_profile() else {
        return;
    };
    if connection::snapshot().endpoint_tag.is_none() {
        return;
    }
    match connection::restart_if_running(app.clone(), profile_id).await {
        Ok(Some(_)) | Ok(None) => {
            let message = match connection::snapshot().raw_proxy_port {
                Some(port) => format!("reconnected — local proxy on 127.0.0.1:{port}"),
                None => "reconnected — local proxy off".to_string(),
            };
            connection::emit_restart_result(app, true, message);
        }
        Err(error) => connection::emit_restart_result(app, false, error),
    }
}

// ---------------------------------------------------------------------------
// Storage layer
// ---------------------------------------------------------------------------

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
fn store_general(conn: &Connection, settings: &GeneralSettings) -> Result<(), String> {
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

fn validate_general(settings: &GeneralSettings) -> Result<(), String> {
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

/// Validates and normalizes a user-entered test URL: http/https scheme,
/// non-empty host, no control characters, sane length. Returns the parsed
/// (normalized) form used for storage and deduplication.
pub fn validate_test_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL must not be empty".into());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("URL must not contain control characters".into());
    }
    if trimmed.len() > 2048 {
        return Err("URL is too long (max 2048 characters)".into());
    }
    // bare "youtube.com" style input is fine — https:// is assumed whenever
    // no scheme is spelled out (prepending avoids `host:port` being read as
    // a scheme by the parser)
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&candidate).map_err(|_| "this does not look like a valid URL".to_string())?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("URL scheme must be http or https, got {other:?}")),
    }
    match parsed.host_str() {
        Some(host) if !host.is_empty() => {}
        _ => return Err("URL must include a host".into()),
    }
    Ok(parsed.to_string())
}

fn lock_db<'a>(state: &'a State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
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
        let path = std::env::temp_dir().join(format!(
            "megathrone-settings-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        crate::db::open(&path).expect("test db should open")
    }

    #[test]
    fn validate_accepts_http_urls_and_normalizes_them() {
        assert_eq!(
            validate_test_url("  https://Example.com/a?b=1  ").expect("valid"),
            "https://example.com/a?b=1"
        );
        assert_eq!(validate_test_url("http://1.2.3.4:8080/").expect("valid"), "http://1.2.3.4:8080/");

        // bare hosts get https:// assumed, host:port is not mistaken for a scheme
        assert_eq!(validate_test_url("youtube.com").expect("valid"), "https://youtube.com/");
        assert_eq!(
            validate_test_url("example.com:8080/check").expect("valid"),
            "https://example.com:8080/check"
        );

        for bad in [
            "",
            "   ",
            "ftp://example.com",
            "http://",
            "not a url",
        ] {
            assert!(validate_test_url(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn general_settings_roundtrip_with_defaults() {
        let conn = test_db();

        // a fresh database serves the defaults
        assert_eq!(
            load_general(&conn).expect("load"),
            GeneralSettings {
                rotation_minutes: DEFAULT_ROTATION_MINUTES,
                recheck_minutes: DEFAULT_RECHECK_MINUTES,
                close_to_tray: DEFAULT_CLOSE_TO_TRAY,
                min_availability_percent: DEFAULT_MIN_AVAILABILITY_PERCENT,
                raw_proxy_enabled: DEFAULT_RAW_PROXY_ENABLED,
                raw_proxy_port: DEFAULT_RAW_PROXY_PORT,
            }
        );

        // the command layer validates before storing — mirror that here
        set_setting(&conn, KEY_ROTATION_MINUTES, "30").expect("set");
        set_setting(&conn, KEY_RECHECK_MINUTES, "2").expect("set");
        set_setting(&conn, KEY_CLOSE_TO_TRAY, "false").expect("set");
        set_setting(&conn, KEY_MIN_AVAILABILITY, "70").expect("set");
        set_setting(&conn, KEY_RAW_PROXY_ENABLED, "0").expect("set");
        set_setting(&conn, KEY_RAW_PROXY_PORT, "2080").expect("set");
        assert_eq!(
            load_general(&conn).expect("load"),
            GeneralSettings {
                rotation_minutes: 30,
                recheck_minutes: 2,
                close_to_tray: false,
                min_availability_percent: 70,
                raw_proxy_enabled: false,
                raw_proxy_port: 2080,
            }
        );
    }

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

    #[test]
    fn junk_in_the_settings_table_falls_back_to_defaults() {
        let conn = test_db();
        set_setting(&conn, KEY_ROTATION_MINUTES, "later").expect("junk");
        set_setting(&conn, KEY_RECHECK_MINUTES, "0").expect("out of range");
        set_setting(&conn, KEY_CLOSE_TO_TRAY, "maybe").expect("junk bool");
        set_setting(&conn, KEY_MIN_AVAILABILITY, "soon").expect("junk");
        set_setting(&conn, KEY_RAW_PROXY_ENABLED, "perhaps").expect("junk bool");
        set_setting(&conn, KEY_RAW_PROXY_PORT, "not-a-port").expect("junk");
        assert_eq!(
            load_general(&conn).expect("load"),
            GeneralSettings {
                rotation_minutes: DEFAULT_ROTATION_MINUTES,
                recheck_minutes: DEFAULT_RECHECK_MINUTES,
                close_to_tray: DEFAULT_CLOSE_TO_TRAY,
                min_availability_percent: DEFAULT_MIN_AVAILABILITY_PERCENT,
                raw_proxy_enabled: DEFAULT_RAW_PROXY_ENABLED,
                raw_proxy_port: DEFAULT_RAW_PROXY_PORT,
            }
        );
    }

    #[test]
    fn close_to_tray_accepts_the_common_spellings() {
        let conn = test_db();
        for (stored, expected) in [("true", true), ("1", true), ("false", false), ("0", false)] {
            set_setting(&conn, KEY_CLOSE_TO_TRAY, stored).expect("set");
            assert_eq!(
                load_general(&conn).expect("load").close_to_tray,
                expected,
                "{stored:?} must read as {expected}"
            );
        }
    }
}
