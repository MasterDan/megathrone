//! The long-lived proxy connection: runs sing-box with *every* endpoint of
//! the profile baked into the config behind a `selector` outbound (tag
//! `mt-proxy`, default = the selected endpoint) — so an endpoint change
//! (strategy rotation, rescue, a manual pick) is one Clash API call on the
//! running instance, no restart: the port, TUN device and system proxy stay
//! put, new connections ride the new endpoint immediately and open ones
//! drain on the old one. Only changes that alter the config itself (fresh
//! content, routing, DPI, mode, profile) reconnect. On top of that:
//! optional traffic splitting (Routing settings): every URL category's
//! hosts go through the proxy, the
//! byedpi DPI tunnel (a local ciadpi SOCKS server spawned alongside) or
//! direct, with a fallback action for everything else. Without a usable
//! selection the connect flow picks an endpoint automatically per the
//! profile's strategy (round robin / fastest / most available — see
//! `auto_select`); only a profile with nothing dialable at all falls back to
//! a DPI-only session (active strategy + DPI routing: no proxy outbound,
//! DPI rules intact, everything else direct). Traffic reaches sing-box
//! either through a local mixed port, the macOS system proxy, or a TUN
//! device. On top of the session port, the raw local proxy (Settings →
//! General, on by default) is a second mixed port whose rule rides above
//! every routing rule: apps that support a proxy point at
//! `127.0.0.1:<port>` and send *everything* — private ranges included —
//! through the currently selected endpoint, following its live switches.
//! The instance is watched: if it dies on its own, the UI hears
//! about it via `connection-changed` together with the stderr tail (this is
//! how e.g. missing TUN permissions surface).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};
use tauri::async_runtime::Receiver;
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;
use tokio::sync::Mutex as AsyncMutex;

use crate::AppState;
use crate::auto_select;
use crate::dpi;
use crate::latency;
use crate::profiles;
use crate::routing;
use crate::settings;
use crate::sites;

pub const CONNECTION_CHANGED_EVENT: &str = "connection-changed";

const MODE_OFF: &str = "off";
const MODE_SYSTEM_PROXY: &str = "system-proxy";
const MODE_TUN: &str = "tun";

/// Internal tags in the generated config: ASCII-only, collision-free.
const PROXY_TAG: &str = "mt-proxy";
const DIRECT_TAG: &str = "mt-direct";
const DPI_TAG: &str = "mt-dpi";
/// Inbound tag of the raw local proxy port (Settings → General): everything
/// arriving on it rides the `mt-proxy` selector — the selected endpoint —
/// above every routing rule, private ranges included.
const RAW_IN_TAG: &str = "mt-raw-in";

/// How much of sing-box's stderr is kept for error reporting.
const STDERR_TAIL_CHARS: usize = 1200;

/// How long ciadpi gets to open its listening port.
const DPI_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a suspicious early connect waits for proof that the answering
/// listener is not our freshly spawned child (see `probe_dpi_ready`).
const DPI_DEATH_GRACE: Duration = Duration::from_millis(300);

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSnapshot {
    pub connected: bool,
    pub profile_id: Option<i64>,
    pub profile_name: Option<String>,
    pub endpoint_tag: Option<String>,
    pub mode: Option<String>,
    pub mixed_port: Option<u16>,
    /// the raw local proxy port (Settings → General), when the session
    /// bakes one — apps point their proxy setting at `127.0.0.1:<port>`
    pub raw_proxy_port: Option<u16>,
    /// present when the byedpi DPI tunnel runs alongside sing-box
    pub dpi: Option<DpiSnapshot>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiSnapshot {
    pub strategy: String,
    pub port: u16,
}

/// What one proxy kind (web / secure web / socks) looked like before we
/// touched it, so the user's own settings can be put back on disconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum ProxyBackup {
    Disabled,
    Enabled { host: String, port: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MacServiceBackup {
    service: String,
    /// web, secure web, socks — aligned with `sysproxy::KINDS`
    kinds: [ProxyBackup; 3],
}

#[derive(Debug, Default, Clone)]
enum SystemProxyRestore {
    #[default]
    Untouched,
    MacOs(Vec<MacServiceBackup>),
}

/// The byedpi (ciadpi) sidecar spawned next to sing-box when routing needs
/// the DPI tunnel. `deliberate` marks kills initiated by the app itself so
/// the watcher can tell them from a crash.
struct DpiRunning {
    child: CommandChild,
    port: u16,
    strategy: String,
    deliberate: Arc<AtomicBool>,
}

struct Running {
    child: Option<CommandChild>,
    config_path: PathBuf,
    profile_id: i64,
    profile_name: String,
    /// `None` in a DPI-only session (no endpoint selected)
    endpoint_tag: Option<String>,
    mode: &'static str,
    mixed_port: u16,
    /// the raw local proxy port this session bakes (`None` — off, or a
    /// DPI-only session with no selector to ride)
    raw_port: Option<u16>,
    /// the clash API port of the running instance (live endpoint switches)
    api_port: u16,
    /// endpoint row id → synthetic outbound tag baked into the config; a
    /// live switch resolves its target through this map
    endpoint_tags: HashMap<i64, String>,
    restore: SystemProxyRestore,
    dpi: Option<DpiRunning>,
}

#[derive(Default)]
struct ProxyState {
    running: Option<Running>,
    last_error: Option<String>,
}

static STATE: LazyLock<Mutex<ProxyState>> = LazyLock::new(|| Mutex::new(ProxyState::default()));

/// Serializes whole connect/disconnect flows (the state mutex is only ever
/// held for short, await-free sections).
static FLOW: LazyLock<AsyncMutex<()>> = LazyLock::new(|| AsyncMutex::new(()));

fn lock_state() -> Result<MutexGuard<'static, ProxyState>, String> {
    STATE.lock().map_err(|_| "connection state lock poisoned".to_string())
}

fn snapshot_of(state: &ProxyState) -> ConnectionSnapshot {
    match &state.running {
        Some(run) => ConnectionSnapshot {
            connected: true,
            profile_id: Some(run.profile_id),
            profile_name: Some(run.profile_name.clone()),
            endpoint_tag: run.endpoint_tag.clone(),
            mode: Some(run.mode.to_string()),
            mixed_port: Some(run.mixed_port),
            raw_proxy_port: run.raw_port,
            dpi: run.dpi.as_ref().map(|dpi| DpiSnapshot {
                strategy: dpi.strategy.clone(),
                port: dpi.port,
            }),
            last_error: state.last_error.clone(),
        },
        None => ConnectionSnapshot {
            last_error: state.last_error.clone(),
            ..ConnectionSnapshot::default()
        },
    }
}

fn current_snapshot() -> ConnectionSnapshot {
    lock_state()
        .map(|state| snapshot_of(&state))
        .unwrap_or_else(|_| ConnectionSnapshot {
            last_error: Some("connection state lock poisoned".to_string()),
            ..ConnectionSnapshot::default()
        })
}

// ---------------------------------------------------------------------------
// Crash recovery
// ---------------------------------------------------------------------------

/// Written to the app data dir for every connect, removed on a clean
/// teardown. Its presence at startup means the previous session died
/// without unwinding (debugger stop, SIGKILL, crash) — the system proxy may
/// still point at the dead port.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMarker {
    /// the app process that wrote the marker (another live instance owns it)
    owner_pid: u32,
    /// the sing-box sidecar; 0 when unknown
    child_pid: u32,
    mixed_port: u16,
    config_path: String,
    /// empty when the system proxy was never touched
    macos_backups: Vec<MacServiceBackup>,
    /// the byedpi sidecar; 0 when the DPI tunnel was not running
    #[serde(default)]
    dpi_pid: u32,
}

fn marker_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("proxy-session.json"))
}

fn write_marker(app: &AppHandle, running: &Running) {
    let Some(path) = marker_path(app) else { return };
    let marker = SessionMarker {
        owner_pid: std::process::id(),
        child_pid: running.child.as_ref().map_or(0, CommandChild::pid),
        mixed_port: running.mixed_port,
        config_path: running.config_path.to_string_lossy().to_string(),
        macos_backups: match &running.restore {
            SystemProxyRestore::MacOs(backups) => backups.clone(),
            SystemProxyRestore::Untouched => Vec::new(),
        },
        dpi_pid: running.dpi.as_ref().map_or(0, |dpi| dpi.child.pid()),
    };
    if let Ok(json) = serde_json::to_string_pretty(&marker) {
        let _ = std::fs::write(&path, json);
    }
}

fn clear_marker(app: &AppHandle) {
    if let Some(path) = marker_path(app) {
        let _ = std::fs::remove_file(&path);
    }
}

/// Cleans up after a previous session that died without unwinding: restores
/// the system proxy, kills an orphaned sing-box, removes its temp config and
/// the marker itself. A no-op after a clean exit (no marker there).
pub fn recover(app: &AppHandle) {
    let Some(path) = marker_path(app) else { return };
    let Ok(raw) = std::fs::read_to_string(&path) else { return };
    let Ok(marker) = serde_json::from_str::<SessionMarker>(&raw) else {
        let _ = std::fs::remove_file(&path);
        return;
    };

    // another app instance still owns that session — hands off
    if marker.owner_pid != 0 && process_comm_is(marker.owner_pid, "megathrone") {
        return;
    }

    if !marker.macos_backups.is_empty() {
        let _ = restore_system_proxy(&marker.macos_backups);
    }
    if marker.child_pid != 0 {
        kill_if_process(marker.child_pid, "sing-box");
    }
    if marker.dpi_pid != 0 {
        // the needle must match the on-disk binary name (the comm is the
        // executable path) — the sidecar ships as `byedpi`, not `ciadpi`
        kill_if_process(marker.dpi_pid, "byedpi");
    }
    if !marker.config_path.is_empty() {
        let _ = std::fs::remove_file(&marker.config_path);
    }
    let _ = std::fs::remove_file(&path);
}

/// Ctrl+C / SIGTERM: route through the regular exit path so `RunEvent::Exit`
/// performs the same cleanup as a window close. (SIGKILL — e.g. a debugger
/// stop — cannot be caught; the session marker covers that case.)
#[cfg(unix)]
pub fn install_exit_signals(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        let (mut interrupt, mut terminate) = match (interrupt, terminate) {
            (Ok(interrupt), Ok(terminate)) => (interrupt, terminate),
            _ => return,
        };
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
        handle.exit(0);
    });
}

#[cfg(not(unix))]
pub fn install_exit_signals(_app: &AppHandle) {}

#[cfg(unix)]
fn process_comm_is(pid: u32, needle: &str) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .map(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim().contains(needle)
        })
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_comm_is(_pid: u32, _needle: &str) -> bool {
    false
}

#[cfg(unix)]
fn kill_if_process(pid: u32, needle: &str) {
    if process_comm_is(pid, needle) {
        // SAFETY: kill(2) on a pid verified to be the expected process
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_if_process(_pid: u32, _needle: &str) {}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Starts the proxy: the profile's selected endpoint + the requested mode.
/// Without a usable selection the connect flow auto-picks one per the
/// profile's strategy (manual profiles fall back to the fastest pick); only
/// when nothing is dialable at all can a DPI-only session be started
/// instead (a DPI strategy active and routing needing the tunnel — no proxy
/// outbound, everything not routed into the DPI tunnel goes direct). Fails
/// (with a human-readable reason) when there is nothing to dial and the
/// DPI-only path is unavailable, the config is rejected by sing-box, the
/// instance dies on startup (e.g. TUN without privileges), or the system
/// proxy cannot be wired up.
#[tauri::command]
pub async fn connection_connect(
    app: AppHandle,
    profile_id: i64,
    mode: String,
) -> Result<ConnectionSnapshot, String> {
    let _flow = FLOW.lock().await;

    {
        let mut state = lock_state()?;
        if state.running.is_some() {
            return Err("already connected — disconnect first".to_string());
        }
        state.last_error = None;
    }

    let mode: &'static str = match mode.as_str() {
        MODE_OFF => MODE_OFF,
        MODE_SYSTEM_PROXY => MODE_SYSTEM_PROXY,
        MODE_TUN => MODE_TUN,
        other => return Err(format!("unknown proxy mode: {other}")),
    };

    // creating a TUN interface needs root on macOS/Windows — fail up front
    // with a clear reason instead of an obscure sing-box exit
    if mode == MODE_TUN && !running_as_root() {
        return Err(
            "TUN mode needs administrator privileges to create the network interface, \
             but the app is running as a regular user. For development launch the app \
             elevated (e.g. `sudo pnpm tauri dev`, or build once and run the binary with \
             sudo) — or use System Proxy mode instead."
                .to_string(),
        );
    }

    let (profile_name, mut endpoint, auto_picked, routing, strategy, dpi_port, dpi_enabled, selection_mode, outbounds, mut untestable, general) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;
        let (profile_name, mut endpoint) = load_selected_endpoint(&conn, profile_id)?;
        let selection_mode = profiles::selection_mode(&conn, profile_id)?;
        // no usable selection (fresh profile, update dropped it) — or one
        // the last scan marked dead, which an automatic strategy replaces
        // before dialing it (an explicit manual pick is the user's call);
        // manual falls back to the fastest pick only when there is no pick
        // at all
        let mut auto_picked = false;
        if endpoint.is_none()
            || (selection_mode != profiles::SELECT_MANUAL
                && selection_known_dead(&conn, profile_id))
        {
            endpoint = auto_pick_endpoint(&conn, profile_id, &selection_mode)?;
            auto_picked = endpoint.is_some();
        }
        let routing = routing::load_routing(&conn)?;
        let strategy = dpi::load_active_strategy(&conn)?;
        let dpi_port = dpi::load_dpi_port(&conn)?;
        let dpi_enabled = dpi::dpi_enabled(&conn)?;
        // every parseable endpoint is baked behind the selector — any of
        // them becomes a restart-free switch target later
        let (outbounds, untestable) = latency::load_outbounds(&conn, profile_id)?;
        let general = settings::load_general(&conn)?;
        (profile_name, endpoint, auto_picked, routing, strategy, dpi_port, dpi_enabled, selection_mode, outbounds, untestable, general)
    };

    // No endpoint to dial is only tolerated for a DPI-only session: DPI on,
    // an active strategy plus routing that needs the tunnel. Everything the
    // routing would send through the proxy then goes direct.
    if endpoint.is_none() && !(dpi_enabled && dpi_only_allowed(strategy.as_ref(), &routing)) {
        return Err(
            "the profile has no usable endpoint — check the subscription content, or connect \
             DPI-only by enabling DPI, selecting a DPI strategy and routing traffic to DPI"
                .to_string(),
        );
    }

    // The byedpi tunnel is spawned only when the DPI master toggle is on
    // (Settings → DPI) AND routing actually needs it AND a strategy is
    // selected. With DPI off the tunnel never starts: dpi categories stop
    // matching (their traffic falls through to the fallback) and a dpi
    // fallback degrades like any missing tunnel would. A dpi *fallback*
    // with DPI on but no strategy would silently send everything somewhere
    // broken — refuse instead; individual dpi rules without one simply stop
    // matching (traffic falls through to the fallback).
    let mut dpi_run = match (dpi_enabled, &strategy, routing::dpi_used(&routing)) {
        (false, _, _) => None,
        (_, Some(strategy), true) => {
            Some(start_byedpi(&app, dpi_port, &strategy.args, &strategy.name).await?)
        }
        (_, None, true) if routing.fallback == routing::ACTION_DPI => {
            return Err(
                "routing sends everything else into the DPI tunnel, but no DPI strategy is \
                 selected — pick one in Settings → DPI or change the fallback"
                    .to_string(),
            );
        }
        _ => None,
    };


    let mixed_port = latency::free_local_port()?;
    let api_port = latency::free_local_port()?;
    // the raw local proxy port (Settings → General): resolved once, before
    // the config loop — a busy configured port falls back to a random free
    // one so a stray listener never fails the connect. A DPI-only session
    // has no selector to ride, so it opens no port at all.
    let raw_port = (general.raw_proxy_enabled && endpoint.is_some())
        .then(|| resolve_raw_port(general.raw_proxy_port as u16));
    let route_context = Some((
        &routing,
        dpi_run.as_ref().map(|dpi| dpi.port),
    ));
    let config_path =
        std::env::temp_dir().join(format!("megathrone-run-{profile_id}-{mixed_port}.json"));

    // validate with the real sing-box before spawning anything. Every
    // endpoint is baked behind the `mt-proxy` selector, so one bad row
    // fails the whole config: the bisection the latency scan uses marks
    // the offenders unavailable and rotates the pick to a strategy
    // survivor — but only for automatic strategies or picks this connect
    // made itself; an explicit manual selection surfaces the error
    // untouched
    let manual_pick = selection_mode == profiles::SELECT_MANUAL && !auto_picked;
    let mut baked = bake_endpoints(outbounds);
    let mut isolated = false;
    loop {
        let default = endpoint
            .as_ref()
            .and_then(|pick| baked.iter().find(|endpoint| endpoint.id == pick.id))
            .map(|endpoint| endpoint.tag.clone());
        let config = build_run_config(
            &baked,
            default.as_deref(),
            mode,
            mixed_port,
            api_port,
            route_context,
            raw_port,
        );
        write_config(&config_path, &config)?;
        let config_arg = config_path.to_string_lossy().to_string();

        let check = latency::sidecar(&app)?;
        let output = check.args(["check", "-c", &config_arg]).output().await.map_err(|error| {
            let _ = std::fs::remove_file(&config_path);
            abort_dpi(dpi_run.take());
            format!("failed to run sing-box check: {error}")
        })?;
        if output.status.success() {
            break;
        }
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let give_up = manual_pick || isolated || endpoint.is_none();
        if give_up {
            let _ = std::fs::remove_file(&config_path);
            abort_dpi(dpi_run.take());
            let message = if manual_pick {
                format!(
                    "the selected endpoint cannot be loaded by sing-box: {reason} — pick \
                     another one or switch the profile to an automatic strategy"
                )
            } else {
                format!(
                    "the profile has no endpoint the current sing-box can load: {reason}"
                )
            };
            return Err(format!("sing-box rejected the config: {message}"));
        }
        isolated = true;
        let (survivors, pick) = match isolate_and_repick(
            &app,
            profile_id,
            &selection_mode,
            baked,
            std::mem::take(&mut untestable),
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                let _ = std::fs::remove_file(&config_path);
                abort_dpi(dpi_run.take());
                return Err(error);
            }
        };
        let Some(pick) = pick else {
            let _ = std::fs::remove_file(&config_path);
            abort_dpi(dpi_run.take());
            return Err(format!(
                "sing-box rejected the config: the profile has no endpoint the current \
                 sing-box can load: {reason}"
            ));
        };
        eprintln!(
            "[connect] profile {profile_id}: rotated away from endpoint(s) sing-box cannot load"
        );
        baked = survivors;
        endpoint = Some(pick);
        // the marked-dead rows and the new selection both show in the UI
        profiles::after_mutation(&app, Some(profile_id), profiles::LATENCY);
        profiles::after_mutation(&app, Some(profile_id), profiles::SELECTION);
    }
    let config_arg = config_path.to_string_lossy().to_string();

    let runner = latency::sidecar(&app)?;
    let (events, child) = runner.args(["run", "-c", &config_arg]).spawn().map_err(|error| {
        let _ = std::fs::remove_file(&config_path);
        abort_dpi(dpi_run.take());
        format!("failed to start sing-box: {error}")
    })?;

    // ready once the clash API answers; a fast exit (permissions, bad
    // config) is detected through the event channel and reported with the
    // stderr tail
    let (receiver, outcome) =
        tauri::async_runtime::spawn_blocking(move || probe_startup(events, api_port))
            .await
            .map_err(|error| format!("startup probe failed: {error}"))?;
    let events = match outcome {
        Startup::Ready => receiver.expect("a ready probe hands the receiver back"),
        Startup::Failed(reason) => {
            let _ = child.kill();
            let _ = std::fs::remove_file(&config_path);
            abort_dpi(dpi_run.take());
            return Err(permission_hint(mode, &reason));
        }
    };

    // wire the system proxy (macOS); on failure roll the instance back
    let restore = if mode == MODE_SYSTEM_PROXY {
        match tauri::async_runtime::spawn_blocking(move || enable_system_proxy(mixed_port))
            .await
            .map_err(|error| format!("system proxy setup failed: {error}"))?
        {
            Ok(restore) => restore,
            Err(error) => {
                let _ = child.kill();
                let _ = std::fs::remove_file(&config_path);
                abort_dpi(dpi_run.take());
                return Err(error);
            }
        }
    } else {
        SystemProxyRestore::Untouched
    };

    let running = Running {
        child: Some(child),
        config_path,
        profile_id,
        profile_name,
        endpoint_tag: endpoint.map(|endpoint| endpoint.tag),
        mode,
        mixed_port,
        raw_port,
        api_port,
        endpoint_tags: baked
            .iter()
            .map(|endpoint| (endpoint.id, endpoint.tag.clone()))
            .collect(),
        restore,
        dpi: dpi_run.take(),
    };
    // written before the state claim: a hard kill anywhere after the system
    // proxy is set must leave a trail for the next start to recover from
    write_marker(&app, &running);
    let snapshot = {
        let mut state = lock_state()?;
        state.last_error = None;
        state.running = Some(running);
        snapshot_of(&state)
    };

    spawn_watcher(app.clone(), events);
    // live traffic charts feed on the clash API of the running instance
    start_traffic_poller(&app, api_port);
    // automatic strategies keep the live endpoint fed by a background
    // supervisor (scan pass → switch); manual selections run bare
    if selection_mode != profiles::SELECT_MANUAL {
        auto_select::ensure(&app, profile_id);
    }
    let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
    Ok(snapshot)
}

/// Stops the proxy and puts the system proxy settings back; idempotent.
#[tauri::command]
pub async fn connection_disconnect(app: AppHandle) -> Result<ConnectionSnapshot, String> {
    let _flow = FLOW.lock().await;

    let taken = lock_state()?.running.take();
    if let Some(run) = &taken {
        // the supervisor has nothing to feed once the session is down
        auto_select::stop(run.profile_id);
        stop_traffic_poller();
    }
    let snapshot = match taken {
        Some(mut running) => {
            let restore_error = teardown(&app, &mut running).await.err();
            let mut state = lock_state()?;
            state.last_error = restore_error;
            snapshot_of(&state)
        }
        None => current_snapshot(),
    };

    let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
pub fn connection_status() -> Result<ConnectionSnapshot, String> {
    Ok(current_snapshot())
}

/// The current (or just-ended) session's per-URL summary: which host was
/// reached through which tunnel, and how many requests went there —
/// collected by the traffic poller off the clash `/connections` snapshots.
#[tauri::command]
pub fn connection_url_stats() -> Result<Vec<UrlStatEntry>, String> {
    URL_STATS.lock().map(|stats| stats.clone()).map_err(|e| e.to_string())
}

/// Read-only access for sibling modules (toast messages etc.).
pub fn snapshot() -> ConnectionSnapshot {
    current_snapshot()
}

/// One-shot report of a user-initiated live reconfiguration (endpoint pick,
/// strategy switch, DPI change, profile switch): the global toast shows it —
/// successes hide themselves, errors stay until dismissed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartResult {
    pub ok: bool,
    pub message: String,
}

pub const RESTART_RESULT_EVENT: &str = "restart-result";

pub fn emit_restart_result(app: &AppHandle, ok: bool, message: String) {
    let _ = app.emit(RESTART_RESULT_EVENT, RestartResult { ok, message });
}

/// Re-points the live session at another profile (the mode stays; the new
/// profile's strategy applies, its supervisor is armed by the connect
/// flow). Reports via `restart-result`. A no-op when the proxy is off or
/// already runs this profile.
#[tauri::command]
pub async fn connection_switch_profile(
    app: AppHandle,
    profile_id: i64,
) -> Result<ConnectionSnapshot, String> {
    let current = {
        let _flow = FLOW.lock().await;
        lock_state().ok().and_then(|state| {
            state
                .running
                .as_ref()
                .map(|run| (run.profile_id, run.mode.to_string()))
        })
    };
    let Some((running_id, mode)) = current else {
        return Ok(current_snapshot());
    };
    if running_id == profile_id {
        return Ok(current_snapshot());
    }

    let result = async {
        connection_disconnect(app.clone()).await?;
        connection_connect(app.clone(), profile_id, mode).await
    }
    .await;
    match result {
        Ok(fresh) => {
            let message = match (&fresh.profile_name, &fresh.endpoint_tag) {
                (Some(name), Some(tag)) => format!("switched to {name} — via {tag}"),
                (Some(name), None) => format!("switched to {name} — DPI only"),
                _ => "switched profile".to_string(),
            };
            emit_restart_result(&app, true, message);
        }
        Err(error) => emit_restart_result(&app, false, error),
    }
    Ok(current_snapshot())
}

/// Reconnects the running session when it uses this profile — the connect
/// flow re-reads the current settings, so endpoint/strategy/DPI changes
/// apply live. `Ok(Some(tag))` — the session now runs `tag` (`None` for a
/// DPI-only session); `Ok(None)` — nothing was running for this profile;
/// `Err` — the reconnect failed and the session is down (the reason is
/// also surfaced as `last_error` via `connection-changed`; the settings
/// themselves have already been persisted by the caller). Silent by
/// design: user-initiated flows report through `emit_restart_result`
/// themselves, background supervisor switches stay quiet.
pub async fn restart_if_running(
    app: AppHandle,
    profile_id: i64,
) -> Result<Option<String>, String> {
    let mode = {
        let _flow = FLOW.lock().await;
        lock_state().ok().and_then(|state| {
            state
                .running
                .as_ref()
                .filter(|run| run.profile_id == profile_id)
                .map(|run| run.mode.to_string())
        })
    };
    let Some(mode) = mode else { return Ok(None) };

    let result = async {
        connection_disconnect(app.clone()).await?;
        connection_connect(app.clone(), profile_id, mode)
            .await
            .map(|fresh| fresh.endpoint_tag)
    }
    .await;
    match result {
        Ok(tag) => Ok(tag),
        Err(error) => {
            let message = format!("reconnecting after a settings change failed: {error}");
            if let Ok(mut state) = lock_state() {
                state.last_error = Some(message.clone());
                let snapshot = snapshot_of(&state);
                let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
            }
            Err(message)
        }
    }
}

/// Applies a persisted endpoint change (strategy switch, rescue, a manual
/// pick) to the running session *without* a reconnect: the run config bakes
/// every endpoint behind the `mt-proxy` selector, so switching is a single
/// Clash API call — the listener, TUN device and system proxy stay as they
/// are, new connections ride the new endpoint immediately and open ones
/// drain on the old one (`interrupt_exist_connections` stays off). Falls
/// back to a reconnect when nothing runs, the target was not baked (fresh
/// content, DPI-only session) or the API call fails. `Ok(Some(tag))` — the
/// tag the session runs now; `Ok(None)` — nothing was running for this
/// profile; `Err` — the fallback reconnect failed. Like
/// `restart_if_running`, silent by design: callers report user-initiated
/// switches through `emit_restart_result` themselves.
pub async fn switch_endpoint_if_running(
    app: AppHandle,
    profile_id: i64,
) -> Result<Option<String>, String> {
    // (endpoint row id, display tag) of the stored selection; an unreadable
    // or cleared selection falls through to the reconnect path
    let selection: Option<(i64, String)> = app
        .state::<AppState>()
        .db
        .lock()
        .ok()
        .and_then(|conn| selected_endpoint_brief(&conn, profile_id).ok().flatten());
    let Some((id, display)) = selection else {
        return restart_if_running(app, profile_id).await;
    };
    let live = lock_state().ok().and_then(|state| {
        let run = state.running.as_ref().filter(|run| run.profile_id == profile_id)?;
        run.endpoint_tags.get(&id).map(|tag| (run.api_port, tag.clone()))
    });
    // nothing runs for this profile (the reconnect is a no-op), or the
    // endpoint was never baked into the live config
    let Some((api_port, tag)) = live else {
        return restart_if_running(app, profile_id).await;
    };
    let switched = match tauri::async_runtime::spawn_blocking(move || {
        select_on_clash_api(api_port, &tag)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(format!("endpoint switch failed: {error}")),
    };
    match switched {
        Ok(()) => {
            let snapshot = {
                let mut state = lock_state()?;
                if let Some(run) = state.running.as_mut() {
                    run.endpoint_tag = Some(display.clone());
                }
                snapshot_of(&state)
            };
            let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
            Ok(Some(display))
        }
        Err(error) => {
            eprintln!(
                "[connection] live endpoint switch failed ({error}) — reconnecting instead"
            );
            restart_if_running(app, profile_id).await
        }
    }
}

/// (row id, display tag) of the stored selection; `None` when nothing is
/// selected or the key dangles (the same resolution as
/// `load_selected_endpoint`).
fn selected_endpoint_brief(
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

/// Points the running instance's `mt-proxy` selector at `tag` via the
/// Clash API. Blocking (ureq) — run inside `spawn_blocking`.
fn select_on_clash_api(api_port: u16, tag: &str) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .build()
        .into();
    let url = format!("http://127.0.0.1:{api_port}/proxies/{PROXY_TAG}");
    let response = agent
        .put(&url)
        .send_json(json!({ "name": tag }))
        .map_err(|error| format!("clash api select failed: {error}"))?;
    if response.status().as_u16() / 100 == 2 {
        Ok(())
    } else {
        Err(format!(
            "clash api refused the switch: HTTP {}",
            response.status()
        ))
    }
}

/// Whether the long-lived proxy currently runs in TUN mode — its routes
/// would capture a DPI strategy test's outgoing probes and skew the
/// results, so the test refuses to run while it is active.
pub(crate) fn tun_active() -> bool {
    lock_state()
        .map(|state| state.running.as_ref().is_some_and(|run| run.mode == MODE_TUN))
        .unwrap_or(false)
}

/// Whether the long-lived proxy currently runs this profile (the supervisor
/// only lives as long as its session does).
pub(crate) fn runs_profile(profile_id: i64) -> bool {
    lock_state()
        .map(|state| state.running.as_ref().is_some_and(|run| run.profile_id == profile_id))
        .unwrap_or(false)
}

/// The profile the live session runs, if any (settings changes decide
/// whether a restart is needed).
pub fn running_profile() -> Option<i64> {
    lock_state()
        .ok()
        .and_then(|state| state.running.as_ref().map(|run| run.profile_id))
}

/// Whether the byedpi tunnel runs alongside the live session.
pub fn dpi_tunnel_running() -> bool {
    lock_state()
        .map(|state| {
            state
                .running
                .as_ref()
                .is_some_and(|run| run.dpi.is_some())
        })
        .unwrap_or(false)
}

/// Called from the app exit handler: kills the instance and restores the
/// system proxy synchronously (there is no async runtime to lean on).
pub fn shutdown(app: &AppHandle) {
    let Some(mut running) = lock_state().ok().and_then(|mut state| state.running.take()) else {
        return;
    };
    auto_select::stop(running.profile_id);
    stop_traffic_poller();
    let restore = std::mem::take(&mut running.restore);
    let restored = match &restore {
        SystemProxyRestore::Untouched => Ok(()),
        SystemProxyRestore::MacOs(backups) => restore_system_proxy(backups),
    };
    // keep the marker when the restore failed so the next start retries it
    if restored.is_ok() {
        clear_marker(app);
    }
    if let Some(child) = running.child.take() {
        let _ = child.kill();
    }
    abort_dpi(running.dpi.take());
    let _ = std::fs::remove_file(&running.config_path);
}

// ---------------------------------------------------------------------------
// The running instance
// ---------------------------------------------------------------------------

/// Kills a not-yet-committed byedpi child (connect aborted mid-flight).
fn abort_dpi(dpi: Option<DpiRunning>) {
    if let Some(dpi) = dpi {
        dpi.deliberate.store(true, Ordering::Relaxed);
        let _ = dpi.child.kill();
    }
}

/// Stops everything a `Running` owns: restores the system proxy first (so
/// traffic goes direct before the port disappears), then kills sing-box and
/// removes the temp config. Best effort — returns the first failure. The
/// session marker is removed on success (kept on failure → the next start
/// retries the restore).
async fn teardown(app: &AppHandle, running: &mut Running) -> Result<(), String> {
    let restore = std::mem::take(&mut running.restore);
    let restored = match &restore {
        SystemProxyRestore::Untouched => Ok(()),
        SystemProxyRestore::MacOs(backups) => {
            let backups = backups.clone();
            tauri::async_runtime::spawn_blocking(move || restore_system_proxy(&backups))
                .await
                .map_err(|error| format!("system proxy restore failed: {error}"))?
        }
    };
    if let Some(child) = running.child.take() {
        let _ = child.kill();
    }
    abort_dpi(running.dpi.take());
    let _ = std::fs::remove_file(&running.config_path);
    if restored.is_ok() {
        clear_marker(app);
    }
    Ok(())
}

/// Watches the spawned sing-box: keeps a stderr tail and reports an
/// unexpected exit (crash, OOM kill, permission revocation…) as
/// `connection-changed` with `connected: false` + the reason. A deliberate
/// disconnect removes `Running` from the state *before* killing, so the
/// watcher's `Terminated` finds nothing to report.
fn spawn_watcher(app: AppHandle, mut events: Receiver<CommandEvent>) {
    tauri::async_runtime::spawn(async move {
        let mut stderr: Vec<u8> = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stderr(chunk) => {
                    stderr.extend_from_slice(&chunk);
                    let overflow = stderr.len().saturating_sub(8 * 1024);
                    stderr.drain(..overflow);
                }
                CommandEvent::Terminated(_) => {
                    report_unexpected_exit(&app, tail(&stderr)).await;
                    break;
                }
                _ => {}
            }
        }
    });
}

async fn report_unexpected_exit(app: &AppHandle, reason: String) {
    let Some(mut running) = lock_state().ok().and_then(|mut state| state.running.take()) else {
        return;
    };
    auto_select::stop(running.profile_id);
    stop_traffic_poller();
    let restore_error = teardown(app, &mut running).await.err();
    let message = match (reason.is_empty(), restore_error) {
        (true, None) => "sing-box exited unexpectedly".to_string(),
        (true, Some(restore)) => {
            format!("sing-box exited unexpectedly (and restoring the system proxy failed: {restore})")
        }
        (false, None) => format!("sing-box exited unexpectedly: {reason}"),
        (false, Some(restore)) => format!(
            "sing-box exited unexpectedly: {reason} (and restoring the system proxy failed: {restore})"
        ),
    };
    let snapshot = match lock_state() {
        Ok(mut state) => {
            state.last_error = Some(message);
            snapshot_of(&state)
        }
        Err(error) => ConnectionSnapshot {
            last_error: Some(error),
            ..ConnectionSnapshot::default()
        },
    };
    let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
}

// ---------------------------------------------------------------------------
// Traffic stats (the live-session charts)
// ---------------------------------------------------------------------------

/// Emitted ~once a second while the proxy runs: cumulative bytes that went
/// through each of the session's outbounds (up = client → internet).
pub const TRAFFIC_STATS_EVENT: &str = "traffic-stats";

/// How often the clash API is polled for per-connection counters.
const TRAFFIC_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficStats {
    pub proxy_up: u64,
    pub proxy_down: u64,
    pub dpi_up: u64,
    pub dpi_down: u64,
    pub direct_up: u64,
    pub direct_down: u64,
}

/// One row of the per-URL session summary: how many connections (requests)
/// went to one host through one tunnel, split into successful (closed having
/// carried response bytes back) and failed (closed mute — connected, but not
/// a single byte came back). Session-scoped — a fresh connect clears the
/// table, a disconnect leaves the last snapshot in place so the page keeps
/// showing what the session did.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UrlStatEntry {
    pub host: String,
    /// "proxy" | "dpi" | "direct"
    pub tunnel: String,
    /// every connection seen this session — the still-open ones included
    pub requests: u64,
    /// closed with response bytes
    pub ok: u64,
    /// closed without a single response byte
    pub failed: u64,
}

/// Running counters behind one (host, tunnel) row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct UrlCounters {
    requests: u64,
    ok: u64,
    failed: u64,
}

/// The current session's per-URL summary, republished by the traffic poller
/// on every tick; readable from the UI at any time.
static URL_STATS: LazyLock<Mutex<Vec<UrlStatEntry>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Generation key for the poller task: connect bumps it and spawns a poller
/// holding the new value; every teardown path bumps it again so a poller of
/// a dead session exits on its next tick.
static TRAFFIC_POLL_GEN: AtomicU64 = AtomicU64::new(0);

fn start_traffic_poller(app: &AppHandle, api_port: u16) {
    // a fresh session counts from zero (a disconnect deliberately keeps the
    // last table for post-session viewing)
    if let Ok(mut stats) = URL_STATS.lock() {
        stats.clear();
    }
    let gen = TRAFFIC_POLL_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || traffic_poll_loop(&app, api_port, gen));
}

fn stop_traffic_poller() {
    TRAFFIC_POLL_GEN.fetch_add(1, Ordering::Relaxed);
}

/// Polls the clash `/connections` endpoint of the running sing-box: every
/// open connection carries its `chains` (the outbound it rides — `mt-proxy`
/// / `mt-dpi` / `mt-direct` in our config) and cumulative byte counters, so
/// per-connection deltas accumulate into per-outbound totals. Blocking —
/// runs on a `spawn_blocking` thread; exits once its generation is
/// superseded (disconnect, unexpected exit, a fresh connect).
fn traffic_poll_loop(app: &AppHandle, api_port: u16, gen: u64) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .into();
    let url = format!("http://127.0.0.1:{api_port}/connections");
    let mut seen: HashMap<String, (u64, u64)> = HashMap::new();
    let mut totals = TrafficStats::default();
    let mut urls: HashMap<(String, String), UrlCounters> = HashMap::new();
    // open connections' (host, tunnel) — kept until the id leaves the
    // snapshot, so its outcome can be finalized then
    let mut meta: HashMap<String, (String, String)> = HashMap::new();
    loop {
        std::thread::sleep(TRAFFIC_POLL_INTERVAL);
        if TRAFFIC_POLL_GEN.load(Ordering::Relaxed) != gen {
            return;
        }
        let body = agent.get(&url).call().ok().and_then(|mut response| {
            response.body_mut().with_config().limit(4 << 20).read_to_string().ok()
        });
        let Some(body) = body else { continue };
        let Ok(payload) = serde_json::from_str::<Value>(&body) else {
            continue;
        };
        accumulate_connections(&payload, &mut seen, &mut totals, &mut urls, &mut meta);
        publish_url_stats(&urls);
        let _ = app.emit(TRAFFIC_STATS_EVENT, totals);
    }
}

/// Republishes the per-URL counters as a list sorted by requests (ties by
/// host, then tunnel, so the order is stable between ticks).
fn publish_url_stats(urls: &HashMap<(String, String), UrlCounters>) {
    let mut entries: Vec<UrlStatEntry> = urls
        .iter()
        .map(|((host, tunnel), counters)| UrlStatEntry {
            host: host.clone(),
            tunnel: tunnel.clone(),
            requests: counters.requests,
            ok: counters.ok,
            failed: counters.failed,
        })
        .collect();
    entries.sort_by(|a, b| {
        b.requests
            .cmp(&a.requests)
            .then_with(|| a.host.cmp(&b.host))
            .then_with(|| a.tunnel.cmp(&b.tunnel))
    });
    if let Ok(mut stats) = URL_STATS.lock() {
        *stats = entries;
    }
}

/// Folds one `/connections` payload into the totals: `seen` holds the last
/// snapshot's counters per connection id and is updated in place (closed
/// connections drop out — their bytes were counted while alive).
/// Connections riding none of our outbounds are ignored. Connection ids
/// absent from `seen` are new — each one counts as a request to its host
/// through its tunnel (`meta` remembers the id → host/tunnel pair). A
/// connection leaving the snapshot is finalized then: response bytes seen
/// (`download > 0` at its last sighting) make it a successful request,
/// closing mute makes it a failed one — still-open connections stay counted
/// as requests but neither ok nor failed.
fn accumulate_connections(
    payload: &Value,
    seen: &mut HashMap<String, (u64, u64)>,
    totals: &mut TrafficStats,
    urls: &mut HashMap<(String, String), UrlCounters>,
    meta: &mut HashMap<String, (String, String)>,
) {
    let Some(connections) = payload.get("connections").and_then(Value::as_array) else {
        return;
    };
    let mut present: HashSet<&str> = HashSet::new();
    for connection in connections {
        let Some(id) = connection.get("id").and_then(Value::as_str) else { continue };
        present.insert(id);
        let upload = connection.get("upload").and_then(Value::as_u64).unwrap_or(0);
        let download = connection.get("download").and_then(Value::as_u64).unwrap_or(0);
        let Some(chains) = connection.get("chains").and_then(Value::as_array) else {
            continue;
        };
        let (up, down) = seen.get(id).copied().unwrap_or((0, 0));
        let delta_up = upload.saturating_sub(up);
        let delta_down = download.saturating_sub(down);
        match connection_tunnel(chains) {
            Some("proxy") => {
                totals.proxy_up += delta_up;
                totals.proxy_down += delta_down;
            }
            Some("dpi") => {
                totals.dpi_up += delta_up;
                totals.dpi_down += delta_down;
            }
            Some("direct") => {
                totals.direct_up += delta_up;
                totals.direct_down += delta_down;
            }
            _ => {}
        }
        if !seen.contains_key(id) {
            if let (Some(tunnel), Some(host)) = (connection_tunnel(chains), connection_host(connection)) {
                urls.entry((host.clone(), tunnel.to_string())).or_default().requests += 1;
                meta.insert(id.to_string(), (host, tunnel.to_string()));
            }
        }
        seen.insert(id.to_string(), (upload, download));
    }
    // connections that left the snapshot since the last tick are done —
    // finalize each one's outcome by what it managed to carry
    let closed: Vec<String> = seen
        .keys()
        .filter(|id| !present.contains(id.as_str()))
        .cloned()
        .collect();
    for id in closed {
        let Some(key) = meta.remove(&id) else {
            seen.remove(&id); // never attributed (foreign chain / no host)
            continue;
        };
        let (_, download) = seen.remove(&id).unwrap_or((0, 0));
        let counters = urls.entry(key).or_default();
        if download > 0 {
            counters.ok += 1;
        } else {
            counters.failed += 1;
        }
    }
}

/// The session tunnel a connection rode — `"proxy"`, `"dpi"` or `"direct"`,
/// read off its clash `chains` (`mt-proxy` / `mt-dpi` / `mt-direct` in our
/// config). `None` for anything not one of our outbounds.
fn connection_tunnel(chains: &[Value]) -> Option<&'static str> {
    if chains.iter().any(|tag| tag.as_str() == Some(PROXY_TAG)) {
        Some("proxy")
    } else if chains.iter().any(|tag| tag.as_str() == Some(DPI_TAG)) {
        Some("dpi")
    } else if chains.iter().any(|tag| tag.as_str() == Some(DIRECT_TAG)) {
        Some("direct")
    } else {
        None
    }
}

/// The host a connection went to: the clash metadata's `host` (sniffed or
/// dialed domain), or the raw `destinationIP:destinationPort` when no
/// hostname is known. `None` when neither is present.
fn connection_host(connection: &Value) -> Option<String> {
    let metadata = connection.get("metadata")?;
    let host = metadata.get("host").and_then(Value::as_str).unwrap_or("");
    if !host.is_empty() {
        return Some(host.to_string());
    }
    let ip = metadata.get("destinationIP").and_then(Value::as_str).unwrap_or("");
    if ip.is_empty() {
        return None;
    }
    let port = metadata
        .get("destinationPort")
        .and_then(|value| value.as_str().map(str::to_string).or_else(|| value.as_u64().map(|n| n.to_string())))
        .unwrap_or_default();
    Some(if port.is_empty() { ip.to_string() } else { format!("{ip}:{port}") })
}

// ---------------------------------------------------------------------------
// The byedpi (DPI) sidecar
// ---------------------------------------------------------------------------

/// Spawns the ciadpi sidecar: a local SOCKS5 server with the user's strategy
/// as its argument line, on the configured port bound to loopback (the app
/// owns `-i`/`-p`, the strategy line is validated against them in `dpi.rs`).
/// Ready once the port accepts a TCP connection.
async fn start_byedpi(
    app: &AppHandle,
    port: u16,
    args: &str,
    strategy_name: &str,
) -> Result<DpiRunning, String> {
    let command = app
        .shell()
        .sidecar("byedpi")
        .map_err(|e| format!("failed to resolve the byedpi sidecar: {e}"))?;
    let command = command
        .args(["-i", "127.0.0.1", "-p", &port.to_string()])
        .args(args.split_whitespace());
    let (events, child) = command
        .spawn()
        .map_err(|e| format!("failed to start byedpi: {e}"))?;

    let deliberate = Arc::new(AtomicBool::new(false));
    let (receiver, outcome) = tauri::async_runtime::spawn_blocking(move || {
        probe_dpi_ready(events, port)
    })
    .await
    .map_err(|error| format!("byedpi startup probe failed: {error}"))?;

    match outcome {
        Ok(()) => {
            let events = receiver.expect("a ready probe hands the receiver back");
            spawn_dpi_watcher(app.clone(), events, deliberate.clone(), strategy_name.to_string());
            Ok(DpiRunning {
                child,
                port,
                strategy: strategy_name.to_string(),
                deliberate,
            })
        }
        Err(reason) => {
            let _ = child.kill();
            Err(reason)
        }
    }
}

/// Polls the ciadpi listening port; a fast exit (bad flag combination, port
/// taken) surfaces through the event channel with the stderr tail. Blocking —
/// run inside `spawn_blocking`. Hands the receiver back on success.
///
/// A connect that succeeds before the port ever refused one is not trusted
/// blindly: a stale tunnel from a crashed run keeps answering while our own
/// child dies with "bind: address already in use" — the false-ready that used
/// to surface later as an unexpected-exit toast. When the port is first seen
/// occupied, the probe waits out `DPI_DEATH_GRACE` for our child's
/// termination before calling the listener ours; once the port has refused a
/// connection, any listener appearing afterwards can only be ours.
pub(crate) fn probe_dpi_ready(
    mut events: Receiver<CommandEvent>,
    port: u16,
) -> (Option<Receiver<CommandEvent>>, Result<(), String>) {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stderr = Vec::new();
    let mut port_was_seen_free = false;
    let deadline = Instant::now() + DPI_STARTUP_TIMEOUT;
    loop {
        if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok() {
            let our_child_died =
                !port_was_seen_free && wait_out_death_grace(&mut events, &mut stderr);
            if !our_child_died {
                return (Some(events), Ok(()));
            }
            break; // the answering listener is not ours; the reason is in stderr
        }
        port_was_seen_free = true;
        match events.try_recv() {
            Ok(CommandEvent::Stderr(chunk)) => stderr.extend_from_slice(&chunk),
            Ok(CommandEvent::Terminated(_)) | Ok(CommandEvent::Error(_)) => break,
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // the process is gone, but its last stderr lines may still be in flight
    let flush_deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < flush_deadline {
        match events.try_recv() {
            Ok(CommandEvent::Stderr(chunk)) => stderr.extend_from_slice(&chunk),
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    let reason = match tail(&stderr).is_empty() {
        true => format!(
            "byedpi did not start within {:?} (is port {port} already in use?)",
            DPI_STARTUP_TIMEOUT
        ),
        false => format!("byedpi failed to start: {}", tail(&stderr)),
    };
    (None, Err(reason))
}

/// Drains the event channel for `DPI_DEATH_GRACE`, collecting stderr along
/// the way; true when the child terminated (or the channel died) in that
/// window — a dead child cannot own the port that just answered.
fn wait_out_death_grace(events: &mut Receiver<CommandEvent>, stderr: &mut Vec<u8>) -> bool {
    let deadline = Instant::now() + DPI_DEATH_GRACE;
    loop {
        match events.try_recv() {
            Ok(CommandEvent::Stderr(chunk)) => stderr.extend_from_slice(&chunk),
            Ok(CommandEvent::Terminated(_)) | Ok(CommandEvent::Error(_)) => return true,
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return true,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

/// Watches the byedpi sidecar: an unexpected exit (crash, port stolen
/// mid-run) is reported as `last_error` + `connection-changed`, but does not
/// tear the whole session down — proxy and direct routes keep working.
/// Deliberate kills (disconnect/reconnect) are marked on `deliberate` and
/// stay silent.
fn spawn_dpi_watcher(
    app: AppHandle,
    mut events: Receiver<CommandEvent>,
    deliberate: Arc<AtomicBool>,
    strategy: String,
) {
    tauri::async_runtime::spawn(async move {
        let mut stderr: Vec<u8> = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stderr(chunk) => {
                    stderr.extend_from_slice(&chunk);
                    let overflow = stderr.len().saturating_sub(4 * 1024);
                    stderr.drain(..overflow);
                }
                CommandEvent::Terminated(_) => {
                    if !deliberate.load(Ordering::Relaxed) {
                        report_dpi_exit(&app, &strategy, tail(&stderr));
                    }
                    break;
                }
                _ => {}
            }
        }
    });
}

fn report_dpi_exit(app: &AppHandle, strategy: &str, reason: String) {
    let message = if reason.is_empty() {
        format!(
            "the DPI tunnel (strategy {strategy:?}) exited unexpectedly — DPI routes no \
             longer work; reconnect to restart it"
        )
    } else {
        format!(
            "the DPI tunnel (strategy {strategy:?}) exited unexpectedly: {reason} — DPI \
             routes no longer work; reconnect to restart it"
        )
    };
    let snapshot = match lock_state() {
        Ok(mut state) => {
            state.last_error = Some(message);
            if let Some(run) = state.running.as_mut() {
                run.dpi = None;
            }
            snapshot_of(&state)
        }
        Err(error) => ConnectionSnapshot {
            last_error: Some(error),
            ..ConnectionSnapshot::default()
        },
    };
    let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
}

#[derive(Debug)]
enum Startup {
    Ready,
    Failed(String),
}

/// Polls the clash API until the instance is up. A fast exit (permissions,
/// bad config) is detected through the event channel; the buffered stderr
/// becomes the failure reason. The `Terminated` event can overtake the last
/// pipe chunks, so after a death the probe keeps draining for a short grace
/// window — otherwise the "why" is lost. Blocking (ureq + sleeps) — run
/// inside `spawn_blocking`. Hands the receiver back on success so the
/// watcher can keep reading it.
fn probe_startup(
    mut events: Receiver<CommandEvent>,
    api_port: u16,
) -> (Option<Receiver<CommandEvent>>, Startup) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .into();
    let mut stderr = Vec::new();
    let deadline = Instant::now() + latency::STARTUP_TIMEOUT;
    let died = loop {
        let up = matches!(
            agent.get(&format!("http://127.0.0.1:{api_port}/version")).call(),
            Ok(response) if response.status().as_u16() == 200
        );
        if up {
            return (Some(events), Startup::Ready);
        }
        match events.try_recv() {
            Ok(CommandEvent::Stderr(chunk)) => stderr.extend_from_slice(&chunk),
            Ok(CommandEvent::Terminated(_)) | Ok(CommandEvent::Error(_)) => break true,
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break true,
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    if !died {
        return (
            None,
            Startup::Failed(format!(
                "sing-box did not become ready within {:?}",
                latency::STARTUP_TIMEOUT
            )),
        );
    }

    // the process is gone, but its last stderr lines may still be in flight
    let flush_deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < flush_deadline {
        match events.try_recv() {
            Ok(CommandEvent::Stderr(chunk)) => stderr.extend_from_slice(&chunk),
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    let reason = match tail(&stderr).is_empty() {
        true => "sing-box failed to start (no error output)".to_string(),
        false => format!("sing-box failed to start: {}", tail(&stderr)),
    };
    (None, Startup::Failed(reason))
}

/// TUN failures caused by missing privileges get an actionable hint appended
/// (covers privileged environments the up-front check cannot see).
fn permission_hint(mode: &str, reason: &str) -> String {
    let denied =
        reason.contains("not permitted") || reason.contains("permission denied");
    if mode == MODE_TUN && denied {
        format!(
            "{reason}\n\nTUN mode needs administrator privileges to create the network \
             interface. Launch the app elevated (on macOS: build it and run the binary \
             with sudo, or `sudo pnpm tauri dev` for a quick dev loop) — or use System \
             Proxy mode instead."
        )
    } else {
        reason.to_string()
    }
}

#[cfg(unix)]
fn running_as_root() -> bool {
    // SAFETY: geteuid is a side-effect-free system call
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn running_as_root() -> bool {
    false
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() <= STDERR_TAIL_CHARS {
        return trimmed.to_string();
    }
    let skip = trimmed.chars().count() - STDERR_TAIL_CHARS;
    trimmed.chars().skip(skip).collect()
}

// ---------------------------------------------------------------------------
// Config & storage
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct SelectedEndpoint {
    id: i64,
    tag: String,
}

/// One endpoint baked into the run config behind the `mt-proxy` selector.
#[derive(Debug)]
struct BakedEndpoint {
    /// endpoint row id — the live-switch map key
    id: i64,
    /// synthetic outbound tag (`mt-1`, `mt-2`, … — unique, ASCII-only and
    /// never colliding with the reserved `mt-proxy` / `mt-direct` /
    /// `mt-dpi`)
    tag: String,
    outbound: Value,
}

/// Retags a profile's outbound rows into baked endpoints, in list order.
fn bake_endpoints(rows: Vec<latency::OutboundRow>) -> Vec<BakedEndpoint> {
    rows.into_iter()
        .enumerate()
        .map(|(index, (id, outbound))| BakedEndpoint {
            id,
            tag: format!("mt-{}", index + 1),
            outbound,
        })
        .collect()
}

/// The checkmark: the endpoint stored via `selected_endpoint_key`. With the
/// automatic strategies an absent / dangling / broken selection is not an
/// error — the connect flow picks a replacement (see `auto_pick_endpoint`).
fn load_selected_endpoint(
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
fn selection_known_dead(conn: &Connection, profile_id: i64) -> bool {
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
fn auto_pick_endpoint(
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

/// The connect loop's landing when the run config was rejected by
/// `sing-box check`: validates every baked outbound with the same bisection
/// the latency scan uses, marks the rows sing-box cannot load as
/// unavailable (so scans and picks skip them from now on), and re-picks per
/// the strategy among the survivors. Returns the rebaked survivors and the
/// new pick (the pick is `None` when nothing survived).
async fn isolate_and_repick(
    app: &AppHandle,
    profile_id: i64,
    mode: &str,
    baked: Vec<BakedEndpoint>,
    untestable: Vec<i64>,
) -> Result<(Vec<BakedEndpoint>, Option<SelectedEndpoint>), String> {
    let rows: Vec<latency::OutboundRow> = baked
        .iter()
        .map(|endpoint| (endpoint.id, endpoint.outbound.clone()))
        .collect();
    let (loadable, mut broken) = latency::isolate_unloadable_outbounds(app, rows).await?;
    broken.extend(untestable);
    let loadable_ids: HashSet<i64> = loadable.iter().map(|(id, _)| *id).collect();
    let pick = {
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
        let mut candidates = profiles::load_candidates(&conn, profile_id)?;
        candidates.retain(|candidate| loadable_ids.contains(&candidate.id));
        let current = profiles::selected_raw_key(&conn, profile_id).ok().flatten();
        match profiles::pick_by_strategy(mode, &candidates, current.as_deref()) {
            Some(pick) => {
                profiles::set_selected_endpoint(&conn, profile_id, Some(pick.id))?;
                load_selected_endpoint(&conn, profile_id).map(|(_, endpoint)| endpoint)?
            }
            None => None,
        }
    };
    Ok((bake_endpoints(loadable), pick))
}

/// A DPI-only connect (no endpoint selected) is allowed exactly when a DPI
/// strategy is active and routing actually needs the tunnel — otherwise the
/// session would be an elaborate "direct" pipe. The caller additionally
/// requires the DPI master toggle to be on.
fn dpi_only_allowed(strategy: Option<&dpi::DpiStrategy>, routing: &routing::RoutingConfig) -> bool {
    strategy.is_some() && routing::dpi_used(routing)
}

/// The real proxy config: every endpoint of the profile as an outbound
/// with a synthetic tag (all switchable without a restart), a `selector`
/// (tag `mt-proxy`, default = the selected endpoint) as the single outbound
/// the routing rules reference, a mixed inbound for manual/browser use,
/// the optional raw local proxy port (an extra mixed inbound whose rule
/// rides above everything, straight into the selector — see
/// `RAW_IN_TAG`), optional TUN, the optional DPI socks outbound, one
/// domain-suffix rule per URL category on top of "private ranges direct",
/// and the clash API for the readiness probe and live endpoint switches.
///
/// `default` is `None` in a DPI-only session: no proxy outbound exists,
/// and `proxy` actions (categories and fallback) degrade to direct. (No
/// pick also means no parseable endpoints, so there is nothing to bake
/// anyway.)
///
/// `routing` is `(config, dpi_port)`: `dpi_port` is `Some` only while the
/// ciadpi tunnel runs — dpi-routed categories (and the dpi fallback, which
/// the caller must reject earlier) are skipped without it.
fn build_run_config(
    endpoints: &[BakedEndpoint],
    default: Option<&str>,
    mode: &str,
    mixed_port: u16,
    api_port: u16,
    routing: Option<(&routing::RoutingConfig, Option<u16>)>,
    raw_port: Option<u16>,
) -> Value {
    let mut outbounds: Vec<Value> = endpoints
        .iter()
        .map(|endpoint| {
            let mut outbound = endpoint.outbound.clone();
            latency::sanitize_outbound(&mut outbound);
            outbound["tag"] = Value::String(endpoint.tag.clone());
            outbound
        })
        .collect();
    // the selector is what makes endpoint switches restart-free: the Clash
    // API can repoint it at any baked outbound while the instance runs
    // (open connections drain on the previous outbound — interruptions
    // stay off)
    if let Some(default) = default {
        outbounds.push(json!({
            "type": "selector",
            "tag": PROXY_TAG,
            "outbounds": endpoints.iter().map(|endpoint| endpoint.tag.as_str()).collect::<Vec<_>>(),
            "default": default,
            "interrupt_exist_connections": false,
        }));
    }
    let has_proxy = default.is_some();
    // the raw port rides the selector: without one (DPI-only session) the
    // port is not opened at all
    let raw_port = raw_port.filter(|_| has_proxy);
    outbounds.push(json!({ "type": "direct", "tag": DIRECT_TAG }));
    if let Some((_, Some(dpi_port))) = routing {
        outbounds.push(json!({
            "type": "socks",
            "tag": DPI_TAG,
            "server": "127.0.0.1",
            "server_port": dpi_port,
            "version": "5",
        }));
    }

    // the raw port's rule sits above everything — the private-range direct
    // rule included: an app pointed at it wants *all* of its traffic on
    // the selected endpoint
    let mut rules = raw_port
        .map(|_| json!({ "inbound": [RAW_IN_TAG], "outbound": PROXY_TAG }))
        .into_iter()
        .collect::<Vec<_>>();
    rules.push(json!({ "ip_is_private": true, "outbound": DIRECT_TAG }));
    let final_tag = match routing {
        Some((config, dpi_port)) => {
            // the categories carry every domain rule: one sing-box rule per
            // (category, matcher kind), in category order (an overlapping
            // host is claimed by the first category that lists it)
            let category_rules: Vec<(&routing::RouteCategory, &str)> = config
                .categories
                .iter()
                .filter_map(|category| {
                    if category.rules.is_empty() {
                        return None;
                    }
                    let outbound = match category.action.as_str() {
                        routing::ACTION_DPI if dpi_port.is_some() => DPI_TAG,
                        routing::ACTION_DPI => return None,
                        routing::ACTION_PROXY if has_proxy => PROXY_TAG,
                        routing::ACTION_PROXY | routing::ACTION_DIRECT => DIRECT_TAG,
                        _ => return None,
                    };
                    Some((category, outbound))
                })
                .collect();
            // domain rules need the hostname: sniff it out of the TLS/HTTP
            // payload (TUN and bare-IP SOCKS connections carry no domain)
            if !category_rules.is_empty() {
                rules.push(json!({ "action": "sniff" }));
            }
            for (category, outbound) in category_rules {
                // bucket the typed rules per matcher kind — `url` rules
                // route by their hostname as a domain suffix
                let mut suffixes: Vec<String> = Vec::new();
                let mut domains: Vec<String> = Vec::new();
                let mut keywords: Vec<String> = Vec::new();
                let mut regexes: Vec<String> = Vec::new();
                for rule in &category.rules {
                    match rule.rule_type.as_str() {
                        sites::RULE_TYPE_DOMAIN_SUFFIX => suffixes.push(rule.value.clone()),
                        sites::RULE_TYPE_URL => {
                            suffixes.push(sites::host_of(&rule.value));
                        }
                        sites::RULE_TYPE_DOMAIN => domains.push(rule.value.clone()),
                        sites::RULE_TYPE_KEYWORD => keywords.push(rule.value.clone()),
                        sites::RULE_TYPE_REGEX => regexes.push(rule.value.clone()),
                        _ => {} // unknown types are filtered by the loader
                    }
                }
                for (field, values) in [
                    ("domain_suffix", &suffixes),
                    ("domain", &domains),
                    ("domain_keyword", &keywords),
                    ("domain_regex", &regexes),
                ] {
                    if !values.is_empty() {
                        rules.push(json!({ field: values, "outbound": outbound }));
                    }
                }
            }
            match config.fallback.as_str() {
                routing::ACTION_DIRECT => DIRECT_TAG,
                routing::ACTION_DPI if dpi_port.is_some() => DPI_TAG,
                _ if has_proxy => PROXY_TAG,
                _ => DIRECT_TAG,
            }
        }
        None if has_proxy => PROXY_TAG,
        None => DIRECT_TAG,
    };

    let mut inbounds = vec![json!({
        "type": "mixed",
        "tag": "mixed-in",
        "listen": "127.0.0.1",
        "listen_port": mixed_port,
    })];
    if let Some(raw_port) = raw_port {
        inbounds.push(json!({
            "type": "mixed",
            "tag": RAW_IN_TAG,
            "listen": "127.0.0.1",
            "listen_port": raw_port,
        }));
    }
    if mode == MODE_TUN {
        inbounds.push(json!({
            "type": "tun",
            "tag": "tun-in",
            "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
            "auto_route": true,
            "strict_route": true,
            "stack": "system",
        }));
    }

    json!({
        "log": { "level": "warn" },
        "inbounds": inbounds,
        "outbounds": outbounds,
        "route": {
            "auto_detect_interface": true,
            "final": final_tag,
            "rules": rules,
        },
        "experimental": {
            "clash_api": { "external_controller": format!("127.0.0.1:{api_port}") }
        },
    })
}

fn write_config(path: &PathBuf, config: &Value) -> Result<(), String> {
    let content =
        serde_json::to_string_pretty(config).map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// The port the raw local proxy listens on: the configured one when it is
/// free, a random free one otherwise (the snapshot carries the actual
/// port, so the UI never lies). Binding and dropping a listener is only a
/// probe — the real listener is sing-box's. A reconnect races the dying
/// previous instance for the port, so a busy port gets a short window to
/// let go before the fallback picks a stranger.
fn resolve_raw_port(configured: u16) -> u16 {
    for attempt in 0..5 {
        if std::net::TcpListener::bind(("127.0.0.1", configured)).is_ok() {
            return configured;
        }
        if attempt < 4 {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    latency::free_local_port().unwrap_or(configured)
}

// ---------------------------------------------------------------------------
// macOS system proxy (networksetup)
// ---------------------------------------------------------------------------

/// Parses `-getwebproxy`-style output into the previous state; `None` when
/// the output cannot be understood (→ the service must not be touched).
fn parse_proxy_state(output: &str) -> Option<ProxyBackup> {
    let mut enabled: Option<bool> = None;
    let mut host: Option<String> = None;
    let mut port: Option<u16> = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Enabled:") {
            enabled = Some(value.trim() == "Yes");
        } else if let Some(value) = line.strip_prefix("Server:") {
            host = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Port:") {
            port = value.trim().parse::<u16>().ok().filter(|port| *port != 0);
        }
    }
    match (enabled?, host?, port) {
        (false, _, _) => Some(ProxyBackup::Disabled),
        (true, host, Some(port)) if !host.is_empty() => Some(ProxyBackup::Enabled { host, port }),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
mod sysproxy {
    use super::{MacServiceBackup, ProxyBackup, SystemProxyRestore, parse_proxy_state};

    /// (get, set, set-state) for web, secure-web and socks proxies; the
    /// order is what `MacServiceBackup::kinds` is aligned with.
    const KINDS: [(&str, &str, &str); 3] = [
        ("-getwebproxy", "-setwebproxy", "-setwebproxystate"),
        ("-getsecurewebproxy", "-setsecurewebproxy", "-setsecurewebproxystate"),
        (
            "-getsocksfirewallproxy",
            "-setsocksfirewallproxy",
            "-setsocksfirewallproxystate",
        ),
    ];

    fn run(args: &[&str]) -> Result<String, String> {
        let output = std::process::Command::new("networksetup")
            .args(args)
            .output()
            .map_err(|error| format!("failed to run networksetup: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "networksetup {} failed: {}",
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn list_services() -> Result<Vec<String>, String> {
        let output = run(&["-listallnetworkservices"])?;
        Ok(output
            .lines()
            .skip(1) // "An asterisk (*) denotes..." hint line
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('*')) // disabled services
            .map(str::to_string)
            .collect())
    }

    pub(super) fn enable(port: u16) -> Result<SystemProxyRestore, String> {
        let services = list_services()?;
        let host = "127.0.0.1";
        // networksetup is slow (~hundreds of ms per call) — configure the
        // services in parallel
        let backups: Vec<MacServiceBackup> = std::thread::scope(|scope| {
            let handles: Vec<_> = services
                .iter()
                .map(|service| scope.spawn(move || enable_service(service, host, port)))
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok().flatten())
                .collect()
        });
        if backups.is_empty() {
            return Err("failed to set the system proxy on any network service".to_string());
        }
        Ok(SystemProxyRestore::MacOs(backups))
    }

    /// Snapshots the service's current proxy state, then points all three
    /// proxy kinds at the local mixed port. `None` = do not touch (and do
    /// not restore) this service at all.
    fn enable_service(service: &str, host: &str, port: u16) -> Option<MacServiceBackup> {
        let mut kinds = Vec::with_capacity(KINDS.len());
        for (get, _, _) in KINDS {
            kinds.push(parse_proxy_state(&run(&[get, service]).ok()?)?);
        }
        let mut changed = false;
        for (_, set, _) in KINDS {
            if run(&[set, service, host, &port.to_string()]).is_ok() {
                changed = true;
            }
        }
        if !changed {
            return None;
        }
        let [web, secure, socks] = kinds.try_into().ok()?;
        Some(MacServiceBackup { service: service.to_string(), kinds: [web, secure, socks] })
    }

    pub(super) fn restore(backups: &[MacServiceBackup]) -> Result<(), String> {
        let errors: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = backups
                .iter()
                .map(|backup| scope.spawn(move || restore_service(backup)))
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| match handle.join() {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some("restore worker panicked".to_string()),
                })
                .collect()
        });
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn restore_service(backup: &MacServiceBackup) -> Result<(), String> {
        for (index, (_, set, set_state)) in KINDS.iter().enumerate() {
            match &backup.kinds[index] {
                ProxyBackup::Disabled => run(&[set_state, &backup.service, "off"])?,
                ProxyBackup::Enabled { host, port } => {
                    run(&[set, &backup.service, host, &port.to_string()])?
                }
            };
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn enable_system_proxy(port: u16) -> Result<SystemProxyRestore, String> {
    sysproxy::enable(port)
}

#[cfg(not(target_os = "macos"))]
fn enable_system_proxy(_port: u16) -> Result<SystemProxyRestore, String> {
    Err("system proxy mode is only supported on macOS".to_string())
}

#[cfg(target_os = "macos")]
fn restore_system_proxy(backups: &[MacServiceBackup]) -> Result<(), String> {
    sysproxy::restore(backups)
}

#[cfg(not(target_os = "macos"))]
fn restore_system_proxy(_backups: &[MacServiceBackup]) -> Result<(), String> {
    Ok(())
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
            "megathrone-connection-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        crate::db::open(&path).expect("test db should open")
    }

    #[test]
    fn load_selected_endpoint_treats_missing_choice_as_none() {
        let conn = test_db();
        conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 2)", [])
            .expect("seed profile");
        let profile_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
             VALUES (?1, 'First', 'vless', 'raw-first', ?2, 0)",
            params![profile_id, r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#],
        )
        .expect("seed endpoint");

        // no selection → None (the connect flow decides whether that is fatal)
        let (name, endpoint) = load_selected_endpoint(&conn, profile_id).expect("load");
        assert_eq!(name, "Sub");
        assert!(endpoint.is_none(), "no checkmark means no endpoint");

        // selected → loads the stored endpoint
        conn.execute(
            "UPDATE profiles SET selected_endpoint_key = 'raw-first' WHERE id = ?1",
            params![profile_id],
        )
        .expect("select");
        let (_, endpoint) = load_selected_endpoint(&conn, profile_id).expect("selected endpoint");
        let endpoint = endpoint.expect("the checkmark resolves");
        assert_eq!(endpoint.id, 1);
        assert_eq!(endpoint.tag, "First");

        // dangling selection (endpoint vanished in an update) → None
        conn.execute(
            "UPDATE profiles SET selected_endpoint_key = 'gone' WHERE id = ?1",
            params![profile_id],
        )
        .expect("dangle");
        let (_, endpoint) = load_selected_endpoint(&conn, profile_id).expect("dangling key");
        assert!(endpoint.is_none(), "a dangling key is no selection");

        // unknown profile
        assert!(load_selected_endpoint(&conn, 999).is_err());

        // stored outbound without a type is unusable — treated as no
        // selection so the auto-pick can replace it
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
             VALUES (?1, 'Broken', 'vless', 'raw-broken', '{}', 1)",
            params![profile_id],
        )
        .expect("seed broken");
        conn.execute(
            "UPDATE profiles SET selected_endpoint_key = 'raw-broken' WHERE id = ?1",
            params![profile_id],
        )
        .expect("select broken");
        let (_, endpoint) = load_selected_endpoint(&conn, profile_id).expect("broken outbound");
        assert!(endpoint.is_none(), "a broken selection reads as no selection");
    }

    /// Without a usable selection the connect flow picks one itself: the
    /// strategy's pick is persisted and loaded, broken rows are skipped.
    #[test]
    fn auto_pick_endpoint_fills_missing_selections() {
        let conn = test_db();
        conn.execute("INSERT INTO profiles (name, item_count) VALUES ('Sub', 2)", [])
            .expect("seed profile");
        let profile_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available, latency_ms)
             VALUES (?1, 'Slow', 'vless', 'raw-slow', ?2, 0, 1, 500),
                    (?1, 'Quick', 'trojan', 'raw-quick', ?3, 1, 1, 30),
                    (?1, 'Broken', 'vless', 'raw-broken', '{}', 2, NULL, NULL)",
            params![
                profile_id,
                r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#,
                r#"{"type":"trojan","server":"5.6.7.8","server_port":443}"#
            ],
        )
        .expect("seed endpoints");

        // fastest (also the manual fallback): Quick wins, and it is persisted
        let endpoint = auto_pick_endpoint(&conn, profile_id, profiles::SELECT_FASTEST)
            .expect("auto pick")
            .expect("a pick exists");
        assert_eq!(endpoint.tag, "Quick");
        assert_eq!(endpoint.id, 2);
        assert_eq!(
            profiles::selected_raw_key(&conn, profile_id).expect("raw key").as_deref(),
            Some("raw-quick")
        );

        // round robin from nothing: the first reachable endpoint
        let endpoint = auto_pick_endpoint(&conn, profile_id, profiles::SELECT_ROUND_ROBIN)
            .expect("auto pick")
            .expect("a pick exists");
        assert_eq!(endpoint.tag, "Slow");

        // nothing pickable at all → None (the connect flow decides the rest)
        conn.execute("DELETE FROM endpoints WHERE profile_id = ?1", params![profile_id])
            .expect("wipe");
        assert!(auto_pick_endpoint(&conn, profile_id, profiles::SELECT_FASTEST)
            .expect("auto pick")
            .is_none());
    }

    /// A stored selection the last scan marked dead is replaced by the
    /// strategy's pick at connect time (automatic modes only): round robin
    /// rotates from the dead cursor onto the next reachable endpoint.
    #[test]
    fn auto_pick_rotates_off_a_dead_selection() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO profiles (name, item_count, selected_endpoint_key)
             VALUES ('Sub', 3, 'raw-dead')",
            [],
        )
        .expect("seed profile");
        let profile_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index, available)
             VALUES (?1, 'Dead',     'vless',  'raw-dead',  ?2, 0, 0),
                    (?1, 'Alive',    'trojan', 'raw-alive', ?3, 1, 1),
                    (?1, 'Untested', 'socks5', 'raw-new',   ?4, 2, NULL)",
            params![
                profile_id,
                r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#,
                r#"{"type":"trojan","server":"5.6.7.8","server_port":443}"#,
                r#"{"type":"socks","server":"9.9.9.9","server_port":1080}"#
            ],
        )
        .expect("seed endpoints");

        // only a known-dead resolution reads as dead
        assert!(selection_known_dead(&conn, profile_id));
        for key in ["raw-alive", "raw-new"] {
            conn.execute(
                "UPDATE profiles SET selected_endpoint_key = ?2 WHERE id = ?1",
                params![profile_id, key],
            )
            .expect("reselect");
            assert!(!selection_known_dead(&conn, profile_id), "{key} is not dead");
        }
        conn.execute(
            "UPDATE profiles SET selected_endpoint_key = NULL WHERE id = ?1",
            params![profile_id],
        )
        .expect("clear");
        assert!(!selection_known_dead(&conn, profile_id));

        // round robin from the dead cursor: the next reachable endpoint
        conn.execute(
            "UPDATE profiles SET selected_endpoint_key = 'raw-dead' WHERE id = ?1",
            params![profile_id],
        )
        .expect("select dead");
        let endpoint = auto_pick_endpoint(&conn, profile_id, profiles::SELECT_ROUND_ROBIN)
            .expect("auto pick")
            .expect("a pick exists");
        assert_eq!(endpoint.tag, "Alive");
        assert_eq!(
            profiles::selected_raw_key(&conn, profile_id)
                .expect("raw key")
                .as_deref(),
            Some("raw-alive")
        );

        // nothing reachable anywhere: the dead current is kept as the last
        // resort (something must be dialed), the untested one is not chosen
        conn.execute(
            "UPDATE endpoints SET available = 0 WHERE raw = 'raw-alive'",
            [],
        )
        .expect("kill alive");
        let endpoint = auto_pick_endpoint(&conn, profile_id, profiles::SELECT_ROUND_ROBIN)
            .expect("auto pick")
            .expect("kept current");
        assert_eq!(endpoint.tag, "Alive");
    }

    #[test]
    fn dpi_only_needs_a_strategy_and_dpi_routing() {
        // routing with no dpi categories and a proxy fallback
        let no_dpi = routing::RoutingConfig {
            categories: vec![routing::RouteCategory {
                id: 1,
                name: "Direct Cat".to_string(),
                action: routing::ACTION_DIRECT.to_string(),
                rules: vec![routing::RouteRule {
                    rule_type: sites::RULE_TYPE_DOMAIN.to_string(),
                    value: "example.org".to_string(),
                }],
            }],
            fallback: routing::ACTION_PROXY.to_string(),
        };
        assert!(!dpi_only_allowed(None, &no_dpi));

        let strategy = dpi::DpiStrategy {
            id: 1,
            name: "S".to_string(),
            args: String::new(),
            is_active: true,
            url_ok: 0,
            url_total: 0,
            tested: 0,
        };
        // strategy but no dpi routing → still refused
        assert!(!dpi_only_allowed(Some(&strategy), &no_dpi));

        // a dpi category or a dpi fallback makes it viable
        let mut dpi_category = no_dpi.clone();
        dpi_category.categories[0].action = routing::ACTION_DPI.to_string();
        assert!(dpi_only_allowed(Some(&strategy), &dpi_category));

        let dpi_fallback =
            routing::RoutingConfig { fallback: routing::ACTION_DPI.to_string(), ..no_dpi.clone() };
        assert!(dpi_only_allowed(Some(&strategy), &dpi_fallback));
    }

    /// One baked endpoint per outbound value, tags `mt-1`…`mt-N` by position.
    fn baked(outbounds: &[Value]) -> Vec<BakedEndpoint> {
        outbounds
            .iter()
            .enumerate()
            .map(|(index, outbound)| BakedEndpoint {
                id: index as i64 + 1,
                tag: format!("mt-{}", index + 1),
                outbound: outbound.clone(),
            })
            .collect()
    }

    #[test]
    fn bake_endpoints_assigns_sequential_unique_tags() {
        let rows = vec![
            (7, json!({ "type": "vless", "server": "1.1.1.1", "server_port": 443 })),
            (9, json!({ "type": "trojan", "server": "2.2.2.2", "server_port": 443 })),
        ];
        let baked = bake_endpoints(rows);
        assert_eq!(baked[0].id, 7);
        assert_eq!(baked[0].tag, "mt-1");
        assert_eq!(baked[1].id, 9);
        assert_eq!(baked[1].tag, "mt-2");
    }

    #[test]
    fn build_run_config_wires_mixed_and_direct_rules() {
        let endpoints = baked(&[json!({
            "type": "vless", "tag": "original #2", "server": "1.2.3.4", "server_port": 443
        })]);

        let config = build_run_config(&endpoints, Some("mt-1"), MODE_OFF, 7897, 45678, None, None);

        assert_eq!(config["inbounds"].as_array().expect("inbounds").len(), 1);
        assert_eq!(config["inbounds"][0]["type"], "mixed");
        assert_eq!(config["inbounds"][0]["listen_port"], 7897);
        // the endpoint keeps its synthetic tag; the selector rides on top
        // of it as the outbound the routing rules reference
        let outbounds = config["outbounds"].as_array().expect("outbounds");
        assert_eq!(outbounds.len(), 3);
        assert_eq!(outbounds[0]["tag"], "mt-1", "source tags are replaced");
        assert_eq!(outbounds[0]["type"], "vless");
        assert_eq!(outbounds[1]["type"], "selector");
        assert_eq!(outbounds[1]["tag"], PROXY_TAG);
        assert_eq!(outbounds[1]["default"], "mt-1");
        assert_eq!(outbounds[1]["outbounds"], json!(["mt-1"]));
        assert_eq!(outbounds[1]["interrupt_exist_connections"], false);
        assert_eq!(outbounds[2]["type"], "direct");
        assert_eq!(config["route"]["final"], PROXY_TAG);
        assert_eq!(config["route"]["rules"].as_array().expect("rules").len(), 1);
        assert_eq!(config["route"]["rules"][0]["outbound"], DIRECT_TAG);
        assert_eq!(
            config["experimental"]["clash_api"]["external_controller"],
            "127.0.0.1:45678"
        );
    }

    /// The selector lists every baked endpoint — any of them is a
    /// restart-free switch target — and the default may be any of them.
    #[test]
    fn build_run_config_selector_lists_every_baked_endpoint() {
        let endpoints = baked(&[
            json!({ "type": "vless", "server": "1.2.3.4", "server_port": 443 }),
            json!({ "type": "trojan", "server": "5.6.7.8", "server_port": 443 }),
        ]);

        let config = build_run_config(&endpoints, Some("mt-2"), MODE_OFF, 7897, 45678, None, None);

        let outbounds = config["outbounds"].as_array().expect("outbounds");
        let selector = &outbounds[2];
        assert_eq!(selector["type"], "selector");
        assert_eq!(selector["outbounds"], json!(["mt-1", "mt-2"]));
        assert_eq!(selector["default"], "mt-2");
    }

    #[test]
    fn build_run_config_tun_adds_the_tun_inbound() {
        let endpoints = baked(&[json!({
            "type": "trojan", "server": "1.2.3.4", "server_port": 443
        })]);
        let config =
            build_run_config(&endpoints, Some("mt-1"), MODE_TUN, 7897, 45678, None, None);
        let inbounds = config["inbounds"].as_array().expect("inbounds");
        assert_eq!(inbounds.len(), 2);
        assert_eq!(inbounds[0]["type"], "mixed");
        assert_eq!(inbounds[1]["type"], "tun");
        assert_eq!(inbounds[1]["auto_route"], true);
    }

    #[test]
    fn build_run_config_sanitizes_junk_fingerprints() {
        let poisoned = json!({
            "type": "vless", "server": "1.2.3.4", "server_port": 443,
            "tls": { "enabled": true, "utls": { "enabled": true, "fingerprint": "unsafe" } }
        });

        let config = build_run_config(&baked(&[poisoned]), Some("mt-1"), MODE_OFF, 7897, 45678, None, None);

        let utls = &config["outbounds"][0]["tls"]["utls"];
        assert!(utls.get("fingerprint").is_none(), "junk fingerprint must be dropped");
        assert_eq!(utls["enabled"], true);
    }

    /// A routing fixture with one category per action, in this order — the
    /// proxy category mixes every rule kind to cover the bucketing.
    fn routing_fixture(fallback: &str) -> routing::RoutingConfig {
        let rule = |rule_type: &str, value: &str| routing::RouteRule {
            rule_type: rule_type.to_string(),
            value: value.to_string(),
        };
        routing::RoutingConfig {
            categories: vec![
                routing::RouteCategory {
                    id: 1,
                    name: "DPI Cat".to_string(),
                    action: routing::ACTION_DPI.to_string(),
                    rules: vec![
                        rule(sites::RULE_TYPE_DOMAIN_SUFFIX, "youtube.com"),
                        rule(sites::RULE_TYPE_DOMAIN_SUFFIX, "youtu.be"),
                    ],
                },
                routing::RouteCategory {
                    id: 2,
                    name: "Direct Cat".to_string(),
                    action: routing::ACTION_DIRECT.to_string(),
                    rules: vec![rule(sites::RULE_TYPE_DOMAIN, "example.org")],
                },
                routing::RouteCategory {
                    id: 3,
                    name: "Proxy Cat".to_string(),
                    action: routing::ACTION_PROXY.to_string(),
                    rules: vec![
                        rule(sites::RULE_TYPE_DOMAIN_SUFFIX, "speedtest.net"),
                        rule(sites::RULE_TYPE_URL, "https://media.example/clip"),
                        rule(sites::RULE_TYPE_KEYWORD, "blocked"),
                        rule(sites::RULE_TYPE_REGEX, r"(^|\.)vk\.com$"),
                    ],
                },
            ],
            fallback: fallback.to_string(),
        }
    }

    #[test]
    fn build_run_config_routes_categories_by_action() {
        let fixture = routing_fixture("proxy");
        let endpoints = baked(&[json!({ "type": "vless", "server": "1.2.3.4", "server_port": 443 })]);

        // with the tunnel up: private → direct, sniff, then one rule per
        // (category, matcher kind) in category order — `url` rules ride
        // the suffix bucket by their hostname
        let config = build_run_config(
            &endpoints,
            Some("mt-1"),
            MODE_OFF,
            7897,
            45678,
            Some((&fixture, Some(1080))),
            None,
        );
        let rules = config["route"]["rules"].as_array().expect("rules");
        assert_eq!(rules.len(), 7);
        assert_eq!(rules[0]["outbound"], DIRECT_TAG);
        assert_eq!(rules[0]["ip_is_private"], true);
        assert_eq!(rules[1]["action"], "sniff");
        assert_eq!(rules[2]["domain_suffix"], json!(["youtube.com", "youtu.be"]));
        assert_eq!(rules[2]["outbound"], DPI_TAG);
        assert_eq!(rules[3]["domain"], json!(["example.org"]));
        assert_eq!(rules[3]["outbound"], DIRECT_TAG);
        assert_eq!(rules[4]["domain_suffix"], json!(["speedtest.net", "media.example"]));
        assert_eq!(rules[4]["outbound"], PROXY_TAG);
        assert_eq!(rules[5]["domain_keyword"], json!(["blocked"]));
        assert_eq!(rules[5]["outbound"], PROXY_TAG);
        assert_eq!(rules[6]["domain_regex"], json!([r"(^|\.)vk\.com$"]));
        assert_eq!(rules[6]["outbound"], PROXY_TAG);
        assert_eq!(config["route"]["final"], PROXY_TAG);

        // without a running tunnel the dpi category rule disappears; the
        // other domain rules still sniff
        let config = build_run_config(
            &endpoints,
            Some("mt-1"),
            MODE_OFF,
            7897,
            45678,
            Some((&fixture, None)),
            None,
        );
        let rules = config["route"]["rules"].as_array().expect("rules");
        assert!(
            !rules.iter().any(|rule| rule["outbound"] == DPI_TAG),
            "dpi rules vanish without the tunnel"
        );
        assert!(
            rules.iter().any(|rule| rule.get("action") == Some(&json!("sniff"))),
            "the surviving domain rules still need the hostname"
        );

        // categories without rules contribute nothing
        let mut empty = routing_fixture("proxy");
        empty.categories.clear();
        let config = build_run_config(
            &endpoints,
            Some("mt-1"),
            MODE_OFF,
            7897,
            45678,
            Some((&empty, Some(1080))),
            None,
        );
        let rules = config["route"]["rules"].as_array().expect("rules");
        assert_eq!(rules.len(), 1, "no rules anywhere — only the private rule");
    }

    #[test]
    fn build_run_config_dpi_fallback_targets_the_tunnel() {
        let endpoints = baked(&[json!({ "type": "vless", "server": "1.2.3.4", "server_port": 443 })]);
        let config = build_run_config(
            &endpoints,
            Some("mt-1"),
            MODE_OFF,
            7897,
            45678,
            Some((&routing_fixture("dpi"), Some(1080))),
            None,
        );
        assert_eq!(config["route"]["final"], DPI_TAG);
    }

    /// The raw local proxy: a second mixed inbound whose rule sits above
    /// everything — the private-range direct rule included — and rides the
    /// `mt-proxy` selector, so it always follows the selected endpoint.
    #[test]
    fn build_run_config_bakes_the_raw_proxy_above_all_rules() {
        let endpoints = baked(&[json!({ "type": "vless", "server": "1.2.3.4", "server_port": 443 })]);

        let config = build_run_config(
            &endpoints,
            Some("mt-1"),
            MODE_OFF,
            7897,
            45678,
            Some((&routing_fixture("proxy"), Some(1080))),
            Some(7890),
        );

        let inbounds = config["inbounds"].as_array().expect("inbounds");
        assert_eq!(inbounds.len(), 2);
        assert_eq!(inbounds[0]["tag"], "mixed-in", "the session port comes first");
        assert_eq!(inbounds[1]["type"], "mixed");
        assert_eq!(inbounds[1]["tag"], RAW_IN_TAG);
        assert_eq!(inbounds[1]["listen"], "127.0.0.1");
        assert_eq!(inbounds[1]["listen_port"], 7890);

        // the raw rule precedes even the private-range rule, and targets
        // the selector (not a concrete endpoint — live switches apply)
        let rules = config["route"]["rules"].as_array().expect("rules");
        assert_eq!(rules[0]["inbound"], json!([RAW_IN_TAG]));
        assert_eq!(rules[0]["outbound"], PROXY_TAG);
        assert_eq!(rules[1]["ip_is_private"], true);
        assert_eq!(rules[1]["outbound"], DIRECT_TAG);

        // without the setting neither the inbound nor its rule exist
        let config = build_run_config(
            &endpoints,
            Some("mt-1"),
            MODE_OFF,
            7897,
            45678,
            Some((&routing_fixture("proxy"), Some(1080))),
            None,
        );
        assert_eq!(config["inbounds"].as_array().expect("inbounds").len(), 1);
        assert!(
            !config["route"]["rules"]
                .as_array()
                .expect("rules")
                .iter()
                .any(|rule| rule.get("inbound").is_some())
        );

        // a DPI-only session has no selector to ride — no raw port either
        let config = build_run_config(
            &[],
            None,
            MODE_OFF,
            7897,
            45678,
            Some((&routing_fixture("dpi"), Some(1080))),
            Some(7890),
        );
        let inbounds = config["inbounds"].as_array().expect("inbounds");
        assert_eq!(inbounds.len(), 1);
        assert_eq!(inbounds[0]["tag"], "mixed-in");
        assert!(
            !config["route"]["rules"]
                .as_array()
                .expect("rules")
                .iter()
                .any(|rule| rule.get("inbound").is_some())
        );
    }

    /// A free configured port is kept; a busy one falls back to some other
    /// (free) port instead of failing the connect.
    #[test]
    fn resolve_raw_port_falls_back_when_busy() {
        let free = latency::free_local_port().expect("free port");
        assert_eq!(resolve_raw_port(free), free);

        let holder = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("holder");
        let busy = holder.local_addr().expect("addr").port();
        let resolved = resolve_raw_port(busy);
        assert_ne!(resolved, busy, "a busy port must not be baked");
        assert!(resolved != 0);
        assert_ne!(resolved, free, "the fallback picks its own port");
    }

    /// DPI-only session (no endpoint): no proxy outbound, proxy actions
    /// degrade to direct, dpi actions keep targeting the tunnel.
    #[test]
    fn build_run_config_dpi_only_degrades_proxy_to_direct() {
        let config = build_run_config(
            &[],
            None,
            MODE_OFF,
            7897,
            45678,
            Some((&routing_fixture("proxy"), Some(1080))),
            None,
        );

        // direct + dpi socks only — nothing references the proxy tag
        let outbounds = config["outbounds"].as_array().expect("outbounds");
        assert_eq!(outbounds.len(), 2);
        assert_eq!(outbounds[0]["type"], "direct");
        assert_eq!(outbounds[1]["tag"], DPI_TAG);
        assert_eq!(outbounds[1]["server_port"], 1080);

        // the dpi category keeps the tunnel; the proxy category degrades to direct
        let rules = config["route"]["rules"].as_array().expect("rules");
        let youtube = rules
            .iter()
            .find(|rule| rule.get("domain_suffix") == Some(&json!(["youtube.com", "youtu.be"])))
            .expect("the dpi rule survives");
        assert_eq!(youtube["outbound"], DPI_TAG);
        let speedtest = rules
            .iter()
            .find(|rule| {
                rule.get("domain_suffix")
                    == Some(&json!(["speedtest.net", "media.example"]))
            })
            .expect("the proxy rule survives");
        assert_eq!(speedtest["outbound"], DIRECT_TAG);

        // a proxy fallback also degrades to direct
        assert_eq!(config["route"]["final"], DIRECT_TAG);

        // a dpi fallback still targets the tunnel
        let config = build_run_config(
            &[],
            None,
            MODE_OFF,
            7897,
            45678,
            Some((&routing_fixture("dpi"), Some(1080))),
            None,
        );
        assert_eq!(config["route"]["final"], DPI_TAG);
    }

    /// The live-switch target resolver: the stored key maps to the row's id
    /// and display tag; a missing or dangling key is no selection.
    #[test]
    fn selected_endpoint_brief_resolves_the_stored_key() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO profiles (name, item_count, selected_endpoint_key)
             VALUES ('Sub', 2, 'raw-first')",
            [],
        )
        .expect("seed profile");
        let profile_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO endpoints (profile_id, tag, protocol, raw, outbound_json, order_index)
             VALUES (?1, 'First', 'vless', 'raw-first', ?2, 0),
                    (?1, 'Second', 'trojan', 'raw-second', ?3, 1)",
            params![
                profile_id,
                r#"{"type":"vless","server":"1.2.3.4","server_port":443}"#,
                r#"{"type":"trojan","server":"5.6.7.8","server_port":443}"#
            ],
        )
        .expect("seed endpoints");

        assert_eq!(
            selected_endpoint_brief(&conn, profile_id).expect("brief"),
            Some((1, "First".to_string()))
        );

        // dangling key → None
        conn.execute(
            "UPDATE profiles SET selected_endpoint_key = 'gone' WHERE id = ?1",
            params![profile_id],
        )
        .expect("dangle");
        assert_eq!(selected_endpoint_brief(&conn, profile_id).expect("brief"), None);

        // no key → None
        conn.execute(
            "UPDATE profiles SET selected_endpoint_key = NULL WHERE id = ?1",
            params![profile_id],
        )
        .expect("clear");
        assert_eq!(selected_endpoint_brief(&conn, profile_id).expect("brief"), None);
    }

    #[test]
    fn parse_proxy_state_handles_enabled_and_disabled() {
        let disabled = "\
Enabled: No
Server:
Port: 0
Authenticated Proxy: 0";
        assert_eq!(parse_proxy_state(disabled), Some(ProxyBackup::Disabled));

        let enabled = "\
Enabled: Yes
Server: 10.0.0.2
Port: 3128
Authenticated Proxy: 0";
        assert_eq!(
            parse_proxy_state(enabled),
            Some(ProxyBackup::Enabled { host: "10.0.0.2".to_string(), port: 3128 })
        );

        // enabled but unparseable host/port → unknown, must not be touched
        let broken = "Enabled: Yes\nServer: \nPort: applesauce";
        assert_eq!(parse_proxy_state(broken), None);

        // garbage output → unknown
        assert_eq!(parse_proxy_state("not networksetup output"), None);
    }

    #[test]
    fn tail_keeps_the_last_chars_only() {
        assert_eq!(tail(b"  short message \n"), "short message");
        let long = "x".repeat(STDERR_TAIL_CHARS + 500);
        assert_eq!(tail(long.as_bytes()).chars().count(), STDERR_TAIL_CHARS);
        assert_eq!(tail(&[]), "");
    }

    /// The marker is the crash-recovery source of truth — it must survive a
    /// serialize/deserialize roundtrip with the proxy backups intact.
    #[test]
    fn session_marker_roundtrips() {
        let marker = SessionMarker {
            owner_pid: 111,
            child_pid: 222,
            mixed_port: 7897,
            config_path: "/tmp/megathrone-run-1-7897.json".to_string(),
            macos_backups: vec![MacServiceBackup {
                service: "Wi-Fi".to_string(),
                kinds: [
                    ProxyBackup::Disabled,
                    ProxyBackup::Enabled { host: "10.0.0.2".to_string(), port: 3128 },
                    ProxyBackup::Disabled,
                ],
            }],
            dpi_pid: 333,
        };

        let json = serde_json::to_string(&marker).expect("serialize");
        let back: SessionMarker = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.owner_pid, 111);
        assert_eq!(back.child_pid, 222);
        assert_eq!(back.dpi_pid, 333);
        assert_eq!(back.mixed_port, 7897);
        assert_eq!(back.macos_backups.len(), 1);
        assert_eq!(back.macos_backups[0].service, "Wi-Fi");
        assert_eq!(
            back.macos_backups[0].kinds[1],
            ProxyBackup::Enabled { host: "10.0.0.2".to_string(), port: 3128 }
        );
    }

    /// Markers written before the DPI integration have no `dpi_pid` — they
    /// must still deserialize (serde default).
    #[test]
    fn session_marker_tolerates_legacy_payloads() {
        let legacy = serde_json::json!({
            "ownerPid": 111,
            "childPid": 222,
            "mixedPort": 7897,
            "configPath": "/tmp/x.json",
            "macosBackups": [],
        });
        let marker: SessionMarker = serde_json::from_value(legacy).expect("legacy marker");
        assert_eq!(marker.dpi_pid, 0);
    }

    #[test]
    fn process_comm_is_rejects_unknown_pids() {
        assert!(!process_comm_is(u32::MAX, "sing-box"));
    }

    #[test]
    fn permission_hint_only_extends_tun_denials() {
        let tun_denied = permission_hint(
            MODE_TUN,
            "sing-box failed to start: FATAL create tun: operation not permitted",
        );
        assert!(tun_denied.contains("administrator privileges"));

        // same sing-box error in another mode: no hint
        let proxy_denied = permission_hint(
            MODE_SYSTEM_PROXY,
            "sing-box failed to start: operation not permitted",
        );
        assert_eq!(proxy_denied, "sing-box failed to start: operation not permitted");

        // tun dying for another reason: untouched
        let tun_other = permission_hint(MODE_TUN, "sing-box failed to start: bad config");
        assert_eq!(tun_other, "sing-box failed to start: bad config");
    }

    /// A dying "sing-box": stderr chunks, then Terminated — and one more
    /// chunk arriving *after* Terminated (the overtake race the grace window
    /// exists for). The probe must report the whole reason.
    #[test]
    fn probe_startup_reports_the_death_reason() {
        let (tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);
        let api_port = latency::free_local_port().expect("free port");
        tx.blocking_send(CommandEvent::Stderr(
            b"FATAL[0000] start service: create tun interface: ".to_vec(),
        ))
        .expect("send chunk 1");
        tx.blocking_send(CommandEvent::Terminated(tauri_plugin_shell::process::TerminatedPayload {
            code: Some(1),
            signal: None,
        }))
        .expect("send terminated");
        tx.blocking_send(CommandEvent::Stderr(b"operation not permitted\n".to_vec()))
            .expect("send chunk 2 (late)");

        let (receiver, outcome) = probe_startup(rx, api_port);

        assert!(receiver.is_none(), "a failed probe must not hand the receiver back");
        let Startup::Failed(reason) = outcome else {
            panic!("a dead process must fail the probe, got {outcome:?}");
        };
        assert!(reason.contains("sing-box failed to start"), "got: {reason}");
        assert!(reason.contains("create tun interface"), "got: {reason}");
        assert!(reason.contains("operation not permitted"), "got: {reason}");
    }

    /// No events, no API: the probe runs into its deadline and reports a
    /// timeout (without stderr).
    #[test]
    fn probe_startup_times_out_when_nothing_answers() {
        let (_tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);
        let api_port = latency::free_local_port().expect("free port");

        let started = Instant::now();
        let (receiver, outcome) = probe_startup(rx, api_port);

        assert!(receiver.is_none());
        let Startup::Failed(reason) = outcome else {
            panic!("expected a failure, got {outcome:?}");
        };
        assert!(reason.contains("did not become ready"), "got: {reason}");
        assert!(
            started.elapsed() >= latency::STARTUP_TIMEOUT,
            "the probe must wait out its deadline, took {:?}",
            started.elapsed()
        );
    }

    /// A stale tunnel from a crashed run keeps the port answering while the
    /// fresh child dies with a bind error: the first connect succeeds before
    /// the port ever refused one, then Terminated arrives in the grace
    /// window — the probe must fail with the stderr reason, not go ready.
    #[test]
    fn probe_dpi_ready_rejects_a_stale_listener() {
        let stale = std::net::TcpListener::bind("127.0.0.1:0").expect("stale listener");
        let port = stale.local_addr().expect("addr").port();
        let (tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);
        tx.blocking_send(CommandEvent::Stderr(b"bind: Address already in use\n".to_vec()))
            .expect("send stderr");
        tx.blocking_send(CommandEvent::Terminated(tauri_plugin_shell::process::TerminatedPayload {
            code: Some(1),
            signal: None,
        }))
        .expect("send terminated");

        let (receiver, outcome) = probe_dpi_ready(rx, port);

        assert!(receiver.is_none(), "a dead child must not hand the receiver back");
        let Err(reason) = outcome else {
            panic!("a stale listener must fail the probe, got {outcome:?}");
        };
        assert!(reason.contains("bind: Address already in use"), "got: {reason}");
    }

    /// Early success with a living child (no termination within the grace)
    /// is accepted — the answering listener is ours.
    #[test]
    fn probe_dpi_ready_accepts_a_live_child_on_an_occupied_port() {
        let ours = std::net::TcpListener::bind("127.0.0.1:0").expect("our listener");
        let port = ours.local_addr().expect("addr").port();
        // an open sender keeps the channel alive — the child is doing fine
        let (_tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);

        let (receiver, outcome) = probe_dpi_ready(rx, port);

        assert!(outcome.is_ok(), "a live child must pass the probe, got {outcome:?}");
        assert!(receiver.is_some(), "a ready probe must hand the receiver back");
    }

    /// The regular startup: the port refuses first (proving it was free),
    /// then the child binds and the probe succeeds — no grace detour.
    #[test]
    fn probe_dpi_ready_accepts_a_late_listener_on_a_free_port() {
        let port = latency::free_local_port().expect("free port");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind");
            // hold the port until the probe has connected
            std::thread::sleep(Duration::from_secs(2));
            drop(listener);
        });

        // the sender stays alive for the whole test — the child is healthy
        let (_tx, rx) = tauri::async_runtime::channel::<CommandEvent>(16);
        let (receiver, outcome) = probe_dpi_ready(rx, port);

        assert!(outcome.is_ok(), "a late listener on a free port must pass, got {outcome:?}");
        assert!(receiver.is_some(), "a ready probe must hand the receiver back");
    }

    /// Consecutive `/connections` snapshots accumulate per-outbound deltas:
    /// counters grow, connections close (their last bytes were already
    /// counted), new ones appear, unknown chains are ignored.
    #[test]
    fn traffic_totals_accumulate_deltas_per_outbound() {
        let mut seen = HashMap::new();
        let mut totals = TrafficStats::default();
        let mut urls = HashMap::new();
        let mut meta = HashMap::new();

        let first = json!({
            "connections": [
                { "id": "a", "upload": 100, "download": 500, "chains": ["mt-proxy"] },
                { "id": "b", "upload": 10, "download": 20, "chains": ["mt-dpi"] },
                { "id": "c", "upload": 7, "download": 9, "chains": ["mt-direct"] },
                { "id": "d", "upload": 1, "download": 1, "chains": ["foreign"] }
            ]
        });
        accumulate_connections(&first, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(
            totals,
            TrafficStats {
                proxy_up: 100, proxy_down: 500,
                dpi_up: 10, dpi_down: 20,
                direct_up: 7, direct_down: 9,
            }
        );

        // "a" grows, "b" stalls, "c" closed (final bytes counted last tick),
        // "e" is new, "d" (unknown chain) is gone
        let second = json!({
            "connections": [
                { "id": "a", "upload": 300, "download": 900, "chains": ["mt-proxy"] },
                { "id": "b", "upload": 10, "download": 40, "chains": ["mt-dpi"] },
                { "id": "e", "upload": 5, "download": 5, "chains": ["mt-direct"] }
            ]
        });
        accumulate_connections(&second, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(
            totals,
            TrafficStats {
                proxy_up: 300, proxy_down: 900,
                dpi_up: 10, dpi_down: 40,
                direct_up: 12, direct_down: 14,
            }
        );
        assert_eq!(seen.len(), 3, "closed and foreign connections drop out");

        // a payload without connections leaves everything untouched
        accumulate_connections(&json!({}), &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(seen.len(), 3);
        assert_eq!(totals.proxy_up, 300);
    }

    /// Each connection id unseen in the previous snapshot is one request:
    /// counted per (host, tunnel), surviving ids never double-count, hosts
    /// fall back to the raw IP:port when no hostname is known — and a
    /// connection leaving the snapshot finalizes its outcome by whether it
    /// ever carried response bytes back.
    #[test]
    fn url_stats_count_new_connections_per_host_and_tunnel() {
        fn counters(
            urls: &HashMap<(String, String), UrlCounters>,
            host: &str,
            tunnel: &str,
        ) -> UrlCounters {
            *urls.get(&(host.to_string(), tunnel.to_string())).unwrap()
        }

        let mut seen = HashMap::new();
        let mut totals = TrafficStats::default();
        let mut urls = HashMap::new();
        let mut meta = HashMap::new();

        let first = json!({
            "connections": [
                {
                    "id": "a", "upload": 1, "download": 1, "chains": ["mt-proxy"],
                    "metadata": { "host": "example.com", "destinationIP": "1.2.3.4", "destinationPort": "443" }
                },
                {
                    "id": "b", "upload": 1, "download": 1, "chains": ["mt-dpi"],
                    "metadata": { "host": "example.com", "destinationIP": "1.2.3.4", "destinationPort": "443" }
                },
                {
                    "id": "c", "upload": 1, "download": 1, "chains": ["mt-direct"],
                    "metadata": { "host": "", "destinationIP": "5.6.7.8", "destinationPort": "853" }
                },
                {
                    "id": "d", "upload": 1, "download": 1, "chains": ["mt-proxy"],
                    "metadata": { "host": "", "destinationIP": "" }
                }
            ]
        });
        accumulate_connections(&first, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(counters(&urls, "example.com", "proxy").requests, 1);
        assert_eq!(counters(&urls, "example.com", "dpi").requests, 1);
        assert_eq!(counters(&urls, "5.6.7.8:853", "direct").requests, 1);
        assert_eq!(urls.len(), 3, "no host to attribute — not counted");
        assert_eq!(
            counters(&urls, "example.com", "proxy"),
            UrlCounters { requests: 1, ok: 0, failed: 0 },
            "still-open connections are neither ok nor failed yet"
        );

        // "a" rides on, "b" and "c" closed (both had response bytes → ok),
        // "d" never had a host so its closing counts for nothing; "e" and
        // "f" are new (f proves numeric ports count too)
        let second = json!({
            "connections": [
                {
                    "id": "a", "upload": 9, "download": 9, "chains": ["mt-proxy"],
                    "metadata": { "host": "example.com" }
                },
                {
                    "id": "e", "upload": 5, "download": 0, "chains": ["mt-proxy"],
                    "metadata": { "host": "example.com" }
                },
                {
                    "id": "f", "upload": 1, "download": 1, "chains": ["mt-direct"],
                    "metadata": { "destinationIP": "5.6.7.8", "destinationPort": 853 }
                }
            ]
        });
        accumulate_connections(&second, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(counters(&urls, "example.com", "proxy").requests, 2);
        assert_eq!(counters(&urls, "example.com", "dpi"), UrlCounters { requests: 1, ok: 1, failed: 0 });
        assert_eq!(counters(&urls, "5.6.7.8:853", "direct"), UrlCounters { requests: 2, ok: 1, failed: 0 });
        assert_eq!(seen.len(), 3);

        // "e" closed mute (sent 5 bytes, nothing came back → failed), "f"
        // closed with response bytes (→ its second ok); "a" still open
        let third = json!({
            "connections": [
                {
                    "id": "a", "upload": 12, "download": 10, "chains": ["mt-proxy"],
                    "metadata": { "host": "example.com" }
                }
            ]
        });
        accumulate_connections(&third, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(
            counters(&urls, "example.com", "proxy"),
            UrlCounters { requests: 2, ok: 0, failed: 1 },
            "the open \"a\" is still neither — only \"e\" finalized, as failed"
        );
        assert_eq!(counters(&urls, "5.6.7.8:853", "direct"), UrlCounters { requests: 2, ok: 2, failed: 0 });
        assert_eq!(seen.len(), 1);
        assert_eq!(meta.len(), 1);
    }
}
