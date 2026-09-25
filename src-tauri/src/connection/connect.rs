//! The connect flow: mode validation and privilege checks, endpoint
//! auto-pick, the byedpi tunnel, the `sing-box check` loop with
//! isolate-and-repick, the startup probe, the system proxy wiring and the
//! session state claim.

use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;
use crate::auto_select;
use crate::dpi;
use crate::latency;
use crate::profiles;
use crate::routing;
use crate::settings;

use super::byedpi::{abort_dpi, start_byedpi};
use super::config::{bake_endpoints, build_run_config, dpi_only_allowed, resolve_raw_port, write_config};
use super::recovery::write_marker;
use super::selection::{
    auto_pick_endpoint, isolate_and_repick, load_selected_endpoint, selection_known_dead,
};
use super::startup::{Startup, permission_hint, probe_startup};
#[cfg(not(target_os = "android"))]
use super::startup::{elevation_hint, running_as_root};
use super::state::{
    CONNECTION_CHANGED_EVENT, ConnectionSnapshot, FLOW, MODE_OFF, MODE_SYSTEM_PROXY, MODE_TUN,
    Running, lock_state, snapshot_of,
};
use super::system_proxy::{SystemProxyRestore, enable_system_proxy};
use super::teardown::spawn_watcher;
use super::traffic::start_traffic_poller;

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

    // creating a TUN interface needs root on macOS/Linux and an elevated
    // token on Windows — fail up front with a clear reason instead of an
    // obscure sing-box exit
    #[cfg(not(target_os = "android"))]
    if mode == MODE_TUN && !running_as_root() {
        return Err(format!(
            "TUN mode needs administrator privileges to create the network interface, \
             but the app is running as a regular user. {}",
            elevation_hint()
        ));
    }

    // Android TUN needs a VpnService-based tunnel (a Kotlin service owning
    // the VPN), which is not wired up yet — refuse clearly instead of
    // failing inside sing-box
    #[cfg(target_os = "android")]
    if mode == MODE_TUN {
        return Err(
            "TUN mode is not available on Android yet — it needs a VpnService-based \
             tunnel planned for a future version. Use the local proxy instead."
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

        let check = latency::sidecar()?;
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

    let runner = latency::sidecar()?;
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

    // wire the system proxy (per-OS integration); on failure roll the
    // instance back
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
    // Android: a foreground service keeps the session alive in background
    crate::mobile::set_foreground_service(true);
    // automatic strategies keep the live endpoint fed by a background
    // supervisor (scan pass → switch); manual selections run bare
    if selection_mode != profiles::SELECT_MANUAL {
        auto_select::ensure(&app, profile_id);
    }
    let _ = app.emit(CONNECTION_CHANGED_EVENT, &snapshot);
    Ok(snapshot)
}
