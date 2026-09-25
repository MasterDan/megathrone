//! DPI-bypass strategies for the byedpi (ciadpi) sidecar.
//!
//! A strategy is literally the argument line handed to the `ciadpi` binary
//! (e.g. `-s2 -d2`); the app manages the listener itself (`-i 127.0.0.1
//! -p <port>`), so the strategy line must not fight over those. Exactly one
//! strategy may be active — it is the one used whenever the proxy session
//! needs the DPI tunnel.

use rusqlite::{Connection, params};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::AppState;
use crate::connection;
use crate::db::{get_setting, set_setting};
use crate::routing;

pub const DEFAULT_DPI_PORT: u16 = 1080;

/// ciadpi short options the app owns (listener / process management). Both
/// `-p 1080` and the glued `-p1080` form are getopt-valid, so any short token
/// *starting* with these letters is rejected.
const FORBIDDEN_SHORT: [char; 5] = ['p', 'i', 'D', 'w', 'E'];
/// …and their long forms (`--port=…` etc.).
const FORBIDDEN_LONG: [&str; 5] = ["port", "ip", "daemon", "pidfile", "transparent"];

const MAX_STRATEGY_CHARS: usize = 2048;
const MAX_STRATEGY_TOKENS: usize = 64;
const MAX_NAME_CHARS: usize = 64;
/// Room for the bundled preset catalog (see `PRESET_STRATEGIES`) plus a
/// healthy number of the user's own lines.
const MAX_STRATEGIES: usize = 96;

/// Ready-made strategies adapted from the ByeByeDPI Android app's
/// `proxytest_strategies.list` (github.com/romanvht/ByeByeDPI). The upstream
/// `{sni}` placeholder (its fake-SNI template value, default `google.com`)
/// is baked in — our ciadpi is spawned without a shell, so runtime
/// substitution would only add moving parts.
const PRESET_STRATEGIES: [(&str, &str); 61] = [
    ("Full Barrage", "-f-200 -Qr -s3:5+sm -a1 -As -d1 -s4+sm -s8+sh -f-300 -d6+sh -a1 -At,r,s -o2 -f-30 -As -r5 -Mh -r6+sh -f-250 -s2:7+s -s3:6+sm -a1 -At,r,s -s3:5+sm -s6+s -s7:9+s -q30+sm -a1"),
    ("Split-Disorder Ladder", "-d1 -d3+s -s6+s -d9+s -s12+s -d15+s -s20+s -d25+s -s30+s -d35+s -r1+s -S -a1 -As -d1 -d3+s -s6+s -d9+s -s12+s -d15+s -s20+s -d25+s -s30+s -d35+s -S -a1"),
    ("Split-TLSRec Weave", "-q2 -s2 -s3+s -r3 -s4 -r4 -s5+s -r5+s -s6 -s7+s -r8 -s9+s -Qr -Mh,d,r -a1 -At,r -s2+s -r2 -d2 -s3 -r3 -r4 -s4 -d5+s -r5 -d6 -s7+s -d7 -a1"),
    ("OOB Ladder", "-o1 -d1 -a1 -At,r,s -s1 -d1 -s5+s -s10+s -s15+s -s20+s -r1+s -S -a1 -As -s1 -d1 -s5+s -s10+s -s15+s -s20+s -S -a1"),
    ("Fake SNI Cascade", "-n google.com -Qr -f-204 -s1:5+sm -a1 -As -d1 -s3+s -s5+s -q7 -a1 -As -o2 -f-43 -a1 -As -r5 -Mh -s1:5+s -s3:7+sm -a1"),
    ("Fake SNI Multi-Hit", "-n google.com -Qr -f-205 -a1 -As -s1:3+sm -a1 -As -s5:8+sm -a1 -As -d3 -q7 -o2 -f-43 -f-85 -f-165 -r5 -Mh -a1"),
    ("Slow Motion Split", "-d1+s -s50+s -a1 -As -f20 -r2+s -a1 -At -d2 -s1+s -s5+s -s10+s -s15+s -s25+s -s35+s -s50+s -s60+s -a1"),
    ("Full Fake Takeover", "-o1 -a1 -At,r,s -f-1 -a1 -At,r,s -d1:11+sm -S -a1 -At,r,s -n google.com -Qr -f1 -d1:11+sm -s1:11+sm -S -a1"),
    ("Auto Split Ladder", "-d1 -s1 -q1 -a1 -Ar -s5 -o1+s -d3+s -s6+s -d9+s -s12+s -d15+s -s20+s -d25+s -s30+s -d35+s -a1"),
    ("Fake End Split", "-f1+nme -t6 -a1 -As -n google.com -Qr -s1:6+sm -a1 -As -s5:12+sm -a1 -As -d3 -q7 -r6 -Mh -a1"),
    ("Classic Ladder", "-d1 -s1+s -d3+s -s6+s -d9+s -s12+s -d15+s -s20+s -d25+s -s30+s -d35+s -a1"),
    ("Tight Ladder", "-d1 -s1+s -d1+s -s3+s -d6+s -s12+s -d14+s -s20+s -s24+s -s30+s -a1"),
    ("OOB Fake Bounce", "-o1 -a1 -At,r,s -f-1 -a1 -Ar,s -o1 -a1 -At -r1+s -f-1 -t6 -a1"),
    ("Wide Ladder", "-d1 -s1+s -s3+s -s6+s -s9+s -s12+s -s15+s -s20+s -s30+s -a1"),
    ("Early Bird", "-d1 -d3+s -s6+s -d6+s -s7+s -d8+s -s10+s -a1 -t12 -At,s -r3"),
    ("Fake TLS Hopscotch", "-f1 -t5 -n google.com -q3+h -Qr -f2 -q1 -r1+s -t15 -q1 -o2 -a1"),
    ("Host Shift", "-n google.com -d2:5:2+h -f-3 -r2+sm -o2 -o50+s -r2+s -f-4 -a1"),
    ("Fake First", "-f-1 -Qr -s1+sm -d3+s -s5+sm -o2 -a1 -As -r1+s -d8+s -a1"),
    ("Record Shuffle", "-r-1+s -o20+sm -s3:7+sm -d5:3+sm -f300+s -Qr -f-1 -a1"),
    ("Offset OOB", "-o2 -O4 -s1 -q1 -a1 -Ar -s5 -o1+s -f1+s -r20+s -a1"),
    ("Tail Record", "-o1 -r-5+se -a1 -At,r,s -d1 -n google.com -Qr -f-1 -a1"),
    ("Fake Classic", "--fake -1 --ttl 8 --split 1+s --disorder 3+s -a1"),
    ("Fake SNI Slices", "-n google.com -Qr -f6+nr -d2 -d11 -f9+hm -o3 -t7 -a1"),
    ("Double Record", "-r5+s -s25+s -a1 -At,r,s -s50 -r5+s -s50+s -a1"),
    ("Short Ladder", "-d1 -d3+s -s6+s -d9+s -s20+s -d25+s -s30+s -a1"),
    ("Midstream OOB", "-d9+s -q20+s -s25+s -t5 -a1 -At,r,s -r1+h -a1"),
    ("Tail Split OOB", "-q1+s -s29+s -s30+s -s14+s -o5+s -f-1 -S -a1"),
    ("Minor TLS Mix", "-d1 -s1+s -r1+s -e1 -m1 -o1+s -f-1 -t2 -a1"),
    ("Auto OOB", "-d1 -o1 -a1 -Ar -o1 -a1 -At -f-1 -r1+s -a1"),
    ("Front Load", "-d1 -s4 -d8 -s1+s -d5+s -s10+s -d20+s -a1"),
    ("Fake SNI Record", "-f-1 -n google.com -Qr -s2+s -r3 -o20 -t4 -a1"),
    ("SNI Middle Fake", "-n google.com -Qr -d5+sm -f3+sm -o2 -t4 -a1"),
    ("Auto Queue", "-o1 -a1 -Ar -q1 -a1 -At -f-1 -r1+s -a1"),
    ("Queue First", "-q1 -a1 -Ar -o1 -a1 -At -f-1 -r1+s -a1"),
    ("SNI Nudge", "-s4+sn -r9+s -Qr -n google.com -S -a1"),
    ("Compact Mix", "-o1 -d1 -r1+s -S -s1+s -d3+s -a1"),
    ("OOB Tail", "-q1+s -s29+s -o5+s -f-1 -S -a1"),
    ("TLS Minor Fake", "-n google.com -Qr -m2 -f-1 -d7 -a1"),
    ("Simple Fake", "-d1 -s1+s -r1+s -f-1 -t8 -a1"),
    ("OOB End Fake", "-o1 -a1 -An -f1+nme -t6 -a1"),
    ("Fake Record", "-n google.com -Qr -f-1 -r1+s -a1"),
    ("Triple Disorder", "-n google.com -Qr -d1:3 -f-1 -a1"),
    ("Minimal Auto", "-s1 -d3+s -a1 -At -r1+s -a1"),
    ("TTL Split", "-f-1 -t8 -n google.com -s1+s -a1"),
    ("Fake Duo", "-n google.com -Qr -d1 -f-1 -a1"),
    ("End Fake", "-f64+se -n google.com -t5 -a1"),
    ("Auto Disorder", "-o1 -a1 -At,r,s -d1 -a1"),
    ("Quick Mix", "-d1+s -o2 -s5 -r5 -a1"),
    ("Record Split", "-r8 -o2 -s7 -q4+s -a1"),
    ("Fake Tail", "-o1 -f-1 -r-5+se -a1"),
    ("Host OOB", "-d6+s -q4+hm -o2 -a1"),
    ("TLS Minor Split", "-s5+s -s35+s -m4 -a1"),
    ("Fake Middle", "-f-1+sm -t7 -m2 -a1"),
    ("End Record", "-o1 -r-5+se -a1"),
    ("Twin Split", "-o1+s -d3+s -a1"),
    ("Double Split", "-o1 -s4 -s6 -a1"),
    ("Late Record", "-q1 -r25+s -a1"),
    ("Basic Split", "-d1 -s3+s -a1"),
    ("OOB Disorder", "-o3 -d7 -a1"),
    ("Reverse Duo", "-d7 -s2 -a1"),
    (DESPAIR_NAME, DESPAIR_ARGS),
];

/// The brute-force "Despair" preset: a giant `-Ku -l:<payload>` fake plus a
/// full split/disorder ladder — the closing entry of `PRESET_STRATEGIES`,
/// shipped like every other preset.
const DESPAIR_NAME: &str = "Despair";
const DESPAIR_ARGS: &str = "-Ku -l:\\xC2\\x00\\x00\\x00\\x01\\x14\\x2E\\xE3\\xE3\\x5F\\x6B\\xBB\\x23\\xA8\\xE6\\x5D\\xA9\\x78\\x21\\xCF\\xC2\\x72\\x4C\\x8F\\xC4\\x5E\\x14\\x00\\x00\\x00\\x00\\xC5\\x00\\x00\\x00\\x00\\x4C\\x00\\xA7\\x00\\x00\\x00\\x00\\x00\\x00\\x44\\x00\\x00\\x80\\x00\\x00\\x00\\x0D\\xFC\\xFA\\x1D\\xCD\\x73\\xBA\\x2A\\x90\\x93\\xB3\\xEE\\xF7\\x43\\xC5\\x85\\xDA\\xFF\\x45\\x3C\\x00\\x00\\x00\\x00\\x00\\x00\\x7C\\x00\\x9B\\x00\\xF6\\x00\\x00\\xDD\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x59\\xA8\\xE4\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x7B\\x00\\x0F\\x00\\x00\\x00\\x48\\x4E\\x00\\x00\\x00\\x06\\xF3\\x00\\x00\\x00\\x00\\xD9\\x5A\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00 -a3 -An -o1 -d1 -r1+s -t10 -b4000 -s1+s -s3+s -s6+s -s9+s -s12+s -s15+s -s20+s -s30+s -As -q1+s -s29+s -o5+s -f3 -S -As -d1+s -s3+s -d5+s -s7+s -r2+s -Mh,d -An";

const SETTING_DPI_ENABLED: &str = "dpi_enabled";

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DpiStrategy {
    pub id: i64,
    pub name: String,
    /// raw argument line, as typed by the user
    pub args: String,
    pub is_active: bool,
    /// test sites reachable through this strategy / configured total
    /// (filled from the last strategy test; `tested == 0` — never tested)
    pub url_ok: usize,
    pub url_total: usize,
    pub tested: usize,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DpiSettings {
    pub port: u16,
    /// master toggle: off — the byedpi tunnel never spawns and a connect is
    /// proxy-only (dpi-routed categories fall through to the fallback)
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Tauri commands (thin wrappers over the storage layer)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn dpi_list_strategies(state: State<AppState>) -> Result<Vec<DpiStrategy>, String> {
    let conn = lock_db(&state)?;
    list_strategies(&conn)
}

#[tauri::command]
pub fn dpi_add_strategy(
    state: State<AppState>,
    name: String,
    args: String,
) -> Result<DpiStrategy, String> {
    let name = validate_name(&name)?;
    let args = parse_strategy_args(&args)?;
    let conn = lock_db(&state)?;
    add_strategy(&conn, &name, &args)
}

/// Editing a strategy's argument line only touches the live session when
/// it is the active one.
#[tauri::command]
pub async fn dpi_update_strategy(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    name: String,
    args: String,
) -> Result<DpiStrategy, String> {
    let name = validate_name(&name)?;
    let args = parse_strategy_args(&args)?;
    let (updated, was_active) = {
        let conn = lock_db(&state)?;
        let updated = update_strategy(&conn, id, &name, &args)?;
        let was_active = updated.is_active;
        (updated, was_active)
    };
    if was_active {
        apply_dpi_change_live(&app).await;
    }
    Ok(updated)
}

/// Deleting the active strategy takes the tunnel down with it (the session
/// restarts if it ran the tunnel).
#[tauri::command]
pub async fn dpi_delete_strategy(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    let was_active = {
        let conn = lock_db(&state)?;
        let was_active = list_strategies(&conn)?
            .into_iter()
            .find(|strategy| strategy.id == id)
            .is_some_and(|strategy| strategy.is_active);
        delete_strategy(&conn, id)?;
        was_active
    };
    if was_active {
        apply_dpi_change_live(&app).await;
    }
    Ok(())
}

/// Marks exactly one strategy as the one to use (idempotent). A running
/// session restarts with the new tunnel when its DPI involvement changes —
/// the outcome lands in the global toast.
#[tauri::command]
pub async fn dpi_select_strategy(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    {
        let conn = lock_db(&state)?;
        select_strategy(&conn, id)?;
    }
    apply_dpi_change_live(&app).await;
    Ok(())
}

#[tauri::command]
pub fn dpi_get_settings(state: State<AppState>) -> Result<DpiSettings, String> {
    let conn = lock_db(&state)?;
    Ok(DpiSettings {
        port: load_dpi_port(&conn)?,
        enabled: dpi_enabled(&conn)?,
    })
}

#[tauri::command]
pub async fn dpi_set_port(
    app: AppHandle,
    state: State<'_, AppState>,
    port: u16,
) -> Result<DpiSettings, String> {
    if port == 0 {
        return Err("port must be between 1 and 65535".into());
    }
    {
        let conn = lock_db(&state)?;
        set_setting(&conn, "dpi_port", &port.to_string()).map_err(db_err)?;
    }
    apply_dpi_change_live(&app).await;
    Ok(DpiSettings {
        port,
        enabled: {
            let conn = lock_db(&state)?;
            dpi_enabled(&conn)?
        },
    })
}

/// Flips the DPI master toggle: off — the tunnel never starts and the main
/// connect button yields proxy-only sessions. Idempotent. A running session
/// restarts with or without the tunnel accordingly — the outcome lands in
/// the global toast.
#[tauri::command]
pub async fn dpi_set_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<DpiSettings, String> {
    {
        let conn = lock_db(&state)?;
        set_setting(&conn, SETTING_DPI_ENABLED, &(enabled as i64).to_string()).map_err(db_err)?;
    }
    apply_dpi_change_live(&app).await;
    Ok(DpiSettings {
        port: {
            let conn = lock_db(&state)?;
            load_dpi_port(&conn)?
        },
        enabled,
    })
}

/// A DPI-affecting setting just changed: restart the live session iff the
/// tunnel's runtime involvement actually changes — it runs now but would
/// not anymore (toggled off / strategy lost), or it does not run but would
/// now (toggled on / strategy picked / routing uses DPI), or it keeps
/// running with different parameters (strategy or port swapped). Everything
/// else (e.g. DPI toggled while nothing uses it) leaves the session alone.
/// Reports via `restart-result`.
async fn apply_dpi_change_live(app: &AppHandle) {
    let Some(profile_id) = connection::running_profile() else {
        return;
    };
    let runs_now = connection::dpi_tunnel_running();
    let would_run = {
        let Some(state) = app.try_state::<AppState>() else { return };
        let Ok(conn) = state.db.lock() else { return };
        let enabled = dpi_enabled(&conn).unwrap_or(false);
        let strategy = load_active_strategy(&conn).ok().flatten();
        let uses_dpi = routing::load_routing(&conn)
            .map(|config| routing::dpi_used(&config))
            .unwrap_or(false);
        enabled && strategy.is_some() && uses_dpi
    };
    if !runs_now && !would_run {
        return;
    }
    match connection::restart_if_running(app.clone(), profile_id).await {
        Ok(Some(_)) | Ok(None) => {
            let snapshot = connection::snapshot();
            let message = match &snapshot.dpi {
                Some(dpi) => format!("reconnected — DPI tunnel: {}", dpi.strategy),
                None => "reconnected — DPI tunnel off".to_string(),
            };
            connection::emit_restart_result(app, true, message);
        }
        Err(error) => connection::emit_restart_result(app, false, error),
    }
}

// ---------------------------------------------------------------------------
// Storage layer
// ---------------------------------------------------------------------------

fn row_to_strategy(row: &rusqlite::Row<'_>) -> rusqlite::Result<DpiStrategy> {
    Ok(DpiStrategy {
        id: row.get(0)?,
        name: row.get(1)?,
        args: row.get(2)?,
        is_active: row.get::<_, i64>(3)? != 0,
        url_ok: row.get::<_, i64>(4)? as usize,
        url_total: row.get::<_, i64>(5)? as usize,
        tested: row.get::<_, i64>(6)? as usize,
    })
}

/// Columns every strategy SELECT must produce, in `row_to_strategy` order:
/// identity + the test-site counters (last test's reachable rules, the
/// configured total and how many rules were probed at all). Only *dpi*
/// routed categories count — and only rules marked for testing that can be
/// probed at all (`url`, `domain_suffix`, `domain`); the others neither
/// get probed nor dilute the rate.
const STRATEGY_COLUMNS: &str = "s.id, s.name, s.args, s.is_active, \
     (SELECT COUNT(*) FROM dpi_url_results r \
        JOIN test_sites u ON u.id = r.url_id \
        JOIN test_site_categories c ON c.id = u.category_id \
        WHERE r.strategy_id = s.id AND r.ok = 1 AND c.action = 'dpi'), \
     (SELECT COUNT(*) FROM test_sites u \
        JOIN test_site_categories c ON c.id = u.category_id \
        WHERE c.action = 'dpi' AND u.test_enabled = 1 \
          AND u.rule_type IN ('url', 'domain_suffix', 'domain')), \
     (SELECT COUNT(*) FROM dpi_url_results r WHERE r.strategy_id = s.id)";

pub fn list_strategies(conn: &Connection) -> Result<Vec<DpiStrategy>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {STRATEGY_COLUMNS} FROM dpi_strategies s ORDER BY s.id"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], row_to_strategy)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

/// The one strategy marked active, if any.
pub fn load_active_strategy(conn: &Connection) -> Result<Option<DpiStrategy>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {STRATEGY_COLUMNS} FROM dpi_strategies s WHERE s.is_active = 1"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], row_to_strategy)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows.into_iter().next())
}

fn add_strategy(conn: &Connection, name: &str, args: &[String]) -> Result<DpiStrategy, String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM dpi_strategies", [], |row| row.get(0))
        .map_err(db_err)?;
    if count as usize >= MAX_STRATEGIES {
        return Err(format!("at most {MAX_STRATEGIES} strategies can be stored"));
    }
    conn.execute(
        "INSERT INTO dpi_strategies (name, args) VALUES (?1, ?2)",
        params![name, args.join(" ")],
    )
    .map_err(db_err)?;
    Ok(DpiStrategy {
        id: conn.last_insert_rowid(),
        name: name.to_string(),
        args: args.join(" "),
        is_active: false,
        url_ok: 0,
        url_total: 0,
        tested: 0,
    })
}

fn update_strategy(conn: &Connection, id: i64, name: &str, args: &[String]) -> Result<DpiStrategy, String> {
    let updated = conn
        .execute(
            "UPDATE dpi_strategies SET name = ?2, args = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![id, name, args.join(" ")],
        )
        .map_err(db_err)?;
    if updated == 0 {
        return Err(format!("strategy {id} not found"));
    }
    conn.query_row(
        &format!("SELECT {STRATEGY_COLUMNS} FROM dpi_strategies s WHERE s.id = ?1"),
        params![id],
        row_to_strategy,
    )
    .map_err(db_err)
}

fn delete_strategy(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM dpi_strategies WHERE id = ?1", params![id])
        .map_err(db_err)?;
    Ok(())
}

fn select_strategy(conn: &Connection, id: i64) -> Result<(), String> {
    // the flag-flipping UPDATE touches every row (by design), so existence
    // is checked separately to keep "unknown id" an error
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dpi_strategies WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if exists == 0 {
        return Err(format!("strategy {id} not found"));
    }
    conn.execute(
        "UPDATE dpi_strategies SET is_active = CASE WHEN id = ?1 THEN 1 ELSE 0 END,
         updated_at = datetime('now')",
        params![id],
    )
    .map_err(db_err)?;
    Ok(())
}

pub fn load_dpi_port(conn: &Connection) -> Result<u16, String> {
    let port = get_setting(conn, "dpi_port")
        .map_err(db_err)?
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_DPI_PORT);
    Ok(port)
}

/// The DPI master toggle; junk/missing values fall back to on (DPI is part
/// of the shipped experience). Read by the connect flow: off — no ciadpi
/// spawn at all, dpi-routed traffic goes through the routing fallback.
pub fn dpi_enabled(conn: &Connection) -> Result<bool, String> {
    Ok(get_setting(conn, SETTING_DPI_ENABLED)
        .map_err(db_err)?
        .map(|value| value != "0")
        .unwrap_or(true))
}

/// Inserts the bundled preset catalog once — only when the user has no
/// strategies of their own (an existing collection is never touched).
/// Called from app setup, not from `db::open`, so unit tests with throwaway
/// databases start from a clean slate.
pub fn seed_default_strategies(conn: &Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM dpi_strategies", [], |row| row.get(0))
        .map_err(db_err)?;
    if count > 0 {
        return Ok(());
    }
    for (name, args) in PRESET_STRATEGIES {
        // validated like user input — a preset the current rules reject must
        // fail loudly at startup instead of poisoning the spawn path later
        let tokens = parse_strategy_args(args)?;
        conn.execute(
            "INSERT INTO dpi_strategies (name, args) VALUES (?1, ?2)",
            params![name, tokens.join(" ")],
        )
        .map_err(db_err)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validates a user-entered strategy name.
pub fn validate_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("name must not be empty".into());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("name must not contain control characters".into());
    }
    if trimmed.chars().count() > MAX_NAME_CHARS {
        return Err(format!("name is too long (max {MAX_NAME_CHARS} characters)"));
    }
    Ok(trimmed.to_string())
}

/// Validates a strategy argument line and splits it into argv entries. The
/// process is spawned without a shell, so no shell-quoting hazards exist —
/// but the app-managed options (listen address/port, daemonizing, pidfile,
/// transparent mode) must not appear in the user line.
pub fn parse_strategy_args(raw: &str) -> Result<Vec<String>, String> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > MAX_STRATEGY_CHARS {
        return Err(format!("argument line is too long (max {MAX_STRATEGY_CHARS} characters)"));
    }
    let tokens: Vec<String> = trimmed.split_whitespace().map(str::to_string).collect();
    if tokens.len() > MAX_STRATEGY_TOKENS {
        return Err(format!("too many arguments (max {MAX_STRATEGY_TOKENS})"));
    }
    for token in &tokens {
        if let Some(long) = token.strip_prefix("--") {
            if long.is_empty() {
                return Err("bare `--` is not a valid argument".into());
            }
            let name = long.split('=').next().unwrap_or(long);
            if FORBIDDEN_LONG.contains(&name) {
                return Err(format!(
                    "`{token}` conflicts with an option managed by the app \
                     (listen address/port, daemonizing, pidfile, transparent mode)"
                ));
            }
        } else if let Some(short) = token.strip_prefix('-') {
            let Some(first) = short.chars().next() else {
                return Err("bare `-` is not a valid argument".into());
            };
            if FORBIDDEN_SHORT.contains(&first) {
                return Err(format!(
                    "`{token}` conflicts with an option managed by the app \
                     (listen address/port, daemonizing, pidfile, transparent mode)"
                ));
            }
        }
    }
    Ok(tokens)
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
            "megathrone-dpi-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
    }

    #[test]
    fn strategy_args_allow_real_flags_and_reject_managed_ones() {
        let ok = parse_strategy_args("  -s2  -d2 ").expect("valid");
        assert_eq!(ok, vec!["-s2".to_string(), "-d2".to_string()]);

        assert_eq!(parse_strategy_args("").expect("empty is fine"), Vec::<String>::new());
        assert_eq!(
            parse_strategy_args("--fake -1 --ttl 8").expect("long flags"),
            vec!["--fake", "-1", "--ttl", "8"]
        );
        // case-sensitive getopt: -d is disorder, -D would be the daemon flag
        assert!(parse_strategy_args("-d3 --disorder 3+s").is_ok());
        assert!(parse_strategy_args("--fake-data=:GET / HTTP/1.1").is_ok());

        for bad in [
            "-p 2080",
            "-p2080",
            "--port 2080",
            "--port=2080",
            "-i 127.0.0.1",
            "--ip=0.0.0.0",
            "-D",
            "-w /tmp/pid",
            "--pidfile=/tmp/pid",
            "--daemon",
            "-E",
            "--transparent",
            "--",
            "-",
        ] {
            assert!(parse_strategy_args(bad).is_err(), "{bad:?} must be rejected");
        }

        let too_long = "-s 1 ".repeat(MAX_STRATEGY_TOKENS + 1);
        assert!(parse_strategy_args(&too_long).is_err());
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(validate_name("  Fake 1 ").expect("valid"), "Fake 1");
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"x".repeat(MAX_NAME_CHARS + 1)).is_err());
    }

    #[test]
    fn strategies_crud_and_single_selection() {
        let conn = test_db();

        let first = add_strategy(&conn, "Split", &parse_strategy_args("-s2 -d2").unwrap())
            .expect("add");
        let second =
            add_strategy(&conn, "Fake", &parse_strategy_args("--fake -1 --ttl 8").unwrap())
                .expect("add");
        assert!(!first.is_active && !second.is_active);
        assert!(load_active_strategy(&conn).expect("active").is_none());

        select_strategy(&conn, second.id).expect("select");
        let active = load_active_strategy(&conn).expect("active").expect("some");
        assert_eq!(active.id, second.id);
        // selecting the same one again keeps it the only active strategy
        select_strategy(&conn, second.id).expect("re-select");
        let list = list_strategies(&conn).expect("list");
        assert_eq!(list.iter().filter(|entry| entry.is_active).count(), 1);

        // editing keeps the selection
        let edited = update_strategy(&conn, second.id, "Fake v2", &[]).expect("update");
        assert_eq!(edited.name, "Fake v2");
        assert_eq!(edited.args, "");
        assert!(edited.is_active);

        // deleting the active strategy leaves none selected
        delete_strategy(&conn, second.id).expect("delete");
        assert!(load_active_strategy(&conn).expect("active").is_none());
        assert_eq!(list_strategies(&conn).expect("list").len(), 1);

        assert!(select_strategy(&conn, 999).is_err());
        assert!(update_strategy(&conn, 999, "x", &[]).is_err());
        // deleting an unknown id is fine (idempotent)
        delete_strategy(&conn, 999).expect("idempotent delete");
    }

    #[test]
    fn add_enforces_the_cap() {
        let conn = test_db();
        for index in 0..MAX_STRATEGIES {
            add_strategy(&conn, &format!("S{index}"), &[]).expect("add below cap");
        }
        let error = add_strategy(&conn, "One too many", &[]).expect_err("cap");
        assert!(error.contains("at most"));
    }

    #[test]
    fn preset_catalog_is_valid_and_seeds_once() {
        for (name, args) in PRESET_STRATEGIES {
            assert!(validate_name(name).is_ok(), "{name} must be a valid name");
            assert!(parse_strategy_args(args).is_ok(), "{args} must pass validation");
        }

        let conn = test_db();
        seed_default_strategies(&conn).expect("seed");
        let seeded = list_strategies(&conn).expect("list");
        assert_eq!(seeded.len(), PRESET_STRATEGIES.len());
        assert_eq!(seeded[0].name, PRESET_STRATEGIES[0].0);
        assert_eq!(seeded[0].args, PRESET_STRATEGIES[0].1);
        assert!(seeded.iter().all(|strategy| !strategy.is_active));
        assert!(
            seeded.iter().any(|strategy| strategy.name == DESPAIR_NAME),
            "the Despair preset ships with the catalog"
        );

        // re-seeding is a no-op, and any existing collection blocks seeding
        seed_default_strategies(&conn).expect("seed again");
        assert_eq!(list_strategies(&conn).expect("list").len(), PRESET_STRATEGIES.len());
    }

    #[test]
    fn list_reports_site_result_counts() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO test_site_categories (name, position, action)
             VALUES ('DpiCat', 0, 'dpi'), ('ProxyCat', 1, 'proxy')",
            [],
        )
        .expect("seed categories");
        for index in 0..3 {
            conn.execute(
                "INSERT INTO test_sites (category_id, rule_type, value) VALUES (1, 'url', ?1)",
                params![format!("https://s{index}.example/")],
            )
            .expect("seed site");
        }
        conn.execute(
            "INSERT INTO test_sites (category_id, rule_type, value)
             VALUES (2, 'url', 'https://proxy.example/')",
            [],
        )
        .expect("seed proxy site");
        let strategy = add_strategy(&conn, "S", &[]).expect("add");
        conn.execute(
            "INSERT INTO dpi_url_results (strategy_id, url_id, ok)
             VALUES (?1, 1, 1), (?1, 2, 0), (?1, 3, 1), (?1, 4, 1)",
            params![strategy.id],
        )
        .expect("seed results");

        // only dpi-routed sites count towards ok/total; every probed row counts as tested
        let list = list_strategies(&conn).expect("list");
        assert_eq!((list[0].url_ok, list[0].url_total, list[0].tested), (2, 3, 4));

        // results cascade away with the strategy
        delete_strategy(&conn, strategy.id).expect("delete");
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM dpi_url_results", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn dpi_port_roundtrip_with_default() {
        let conn = test_db();
        assert_eq!(load_dpi_port(&conn).expect("port"), DEFAULT_DPI_PORT);
        set_setting(&conn, "dpi_port", "2080").expect("set");
        assert_eq!(load_dpi_port(&conn).expect("port"), 2080);
        // junk in the settings table falls back to the default instead of
        // breaking the connect flow
        set_setting(&conn, "dpi_port", "not-a-port").expect("set junk");
        assert_eq!(load_dpi_port(&conn).expect("port"), DEFAULT_DPI_PORT);
    }

    #[test]
    fn dpi_toggle_roundtrip_with_default() {
        let conn = test_db();
        assert!(dpi_enabled(&conn).expect("toggle"), "on by default");

        set_setting(&conn, SETTING_DPI_ENABLED, "0").expect("off");
        assert!(!dpi_enabled(&conn).expect("toggle"));

        set_setting(&conn, SETTING_DPI_ENABLED, "1").expect("on");
        assert!(dpi_enabled(&conn).expect("toggle"));

        // junk in the settings table reads as on instead of breaking the
        // connect flow
        set_setting(&conn, SETTING_DPI_ENABLED, "junk").expect("junk");
        assert!(dpi_enabled(&conn).expect("toggle"));
    }
}
