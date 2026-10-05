use tauri::Manager;

mod tray;

// the shell consumes the shared backend library (crates/core): the command
// registry below only re-exports its commands for the IPC handler
use megathrone_core::{connection, db, discovery, dpi, dpi_test, latency, profiles, settings, sites};

// the mobile entry point (Android/iOS): the attribute generates the JNI
// bootstrap the generated gradle/Xcode project loads
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK's DMA-BUF renderer breaks the Wayland connection on the
    // proprietary NVIDIA driver (GDK aborts the whole process with
    // "Error 71 Protocol error"); opt out before any webview exists,
    // unless the user set the knob themselves. Compositing goes too:
    // with DMABUF off it degrades into per-frame CPU→GPU copies (the
    // UI crawls), while a plain software path renders UI-type content
    // fast — the same pair tauri-forge ships.
    #[cfg(target_os = "linux")]
    if std::path::Path::new("/sys/module/nvidia").exists() {
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
    }

    let auto_update_notify = std::sync::Arc::new(tokio::sync::Notify::new());

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    // the Android foreground-service bridge (a no-op plugin elsewhere is not
    // registered at all)
    #[cfg(target_os = "android")]
    {
        builder = builder.plugin(megathrone_core::mobile::service_plugin());
    }

    builder
        .setup({
            let notify = auto_update_notify.clone();
            move |app| {
                let data_dir = app.path().app_data_dir()?;
                std::fs::create_dir_all(&data_dir)?;
                // the SQL migrations bundled as app resources (desktop); on
                // Android the resource dir is the virtual asset:// URI Rust
                // cannot read, so the compile-time copies serve instead. A
                // resource copy that trails the embedded chain is stale
                // (the dev CLI does not always refresh copied resources) —
                // the embedded set wins so the schema never lags the binary
                let migrations = app
                    .path()
                    .resolve("migrations", tauri::path::BaseDirectory::Resource)
                    .ok()
                    .and_then(|dir| db::Migrations::from_dir(&dir).ok())
                    .filter(|migrations| {
                        !migrations.is_empty()
                            && migrations.latest_version()
                                >= db::Migrations::embedded().latest_version()
                    })
                    .unwrap_or_else(db::Migrations::embedded);
                let connection = db::open(&data_dir.join("megathrone.db"), &migrations)
                    .map_err(|e| format!("failed to initialize the profiles database: {e}"))?;
                // first run only: the bundled strategy presets and the
                // default test-site catalog (never into a user-touched table)
                dpi::seed_default_strategies(&connection)
                    .map_err(|e| format!("failed to seed the default DPI strategies: {e}"))?;
                sites::seed_default_sites(&connection)
                    .map_err(|e| format!("failed to seed the default test sites: {e}"))?;
                discovery::seed_discovery_sources(&connection)
                    .map_err(|e| format!("failed to seed the discovery sources: {e}"))?;
                // the shell-agnostic core environment: state access, event
                // broadcasts (the app's IPC bus) and the data dir in one
                // place — the same contract the console web server installs
                megathrone_core::env::install_tauri(
                    app.handle().clone(),
                    std::sync::Arc::new(megathrone_core::AppState::new(
                        connection,
                        notify.clone(),
                    )),
                    Some(data_dir),
                );
                megathrone_core::scheduler::spawn(notify);
                // undo whatever a hard-killed previous session left behind,
                // and make Ctrl+C/SIGTERM go through the graceful exit path
                connection::recover();
                connection::install_exit_signals(app.handle());
                // the tray icon (status menu + the real quit); the window's
                // close button hides into it unless the user opted out
                // (desktop only — Android has no tray and no window close)
                #[cfg(desktop)]
                tray::init(app.handle())?;
                Ok(())
            }
        })
        .invoke_handler(tauri::generate_handler![
            megathrone_core::process::sing_box_version,
            profiles::profiles_list,
            profiles::profile_get,
            profiles::profile_import_from_url,
            profiles::profile_import_from_file,
            profiles::profile_import_from_text,
            profiles::profile_update,
            profiles::profile_update_status,
            profiles::profile_rename,
            profiles::profile_delete,
            profiles::profiles_delete_bulk,
            profiles::profile_set_sidebar_visible,
            profiles::profile_set_sidebar_order,
            profiles::profile_set_auto_update,
            profiles::profile_items,
            profiles::profile_item_index,
            profiles::profile_item_detail,
            profiles::profile_endpoint_urls,
            profiles::profile_selected_endpoint,
            profiles::profile_select_endpoint,
            profiles::profile_selection_mode,
            profiles::profile_set_selection_mode,
            latency::profile_check_latency,
            latency::profile_latency_status,
            latency::profile_cancel_latency,
            latency::profile_test_endpoint_urls,
            connection::connection_connect,
            connection::connection_disconnect,
            connection::connection_status,
            connection::connection_switch_profile,
            connection::connection_url_stats,
            settings::settings_get_general,
            settings::settings_set_cadences,
            settings::settings_set_close_to_tray,
            settings::settings_set_raw_proxy,
            dpi::dpi_list_strategies,
            dpi::dpi_add_strategy,
            dpi::dpi_update_strategy,
            dpi::dpi_delete_strategy,
            dpi::dpi_select_strategy,
            dpi::dpi_get_settings,
            dpi::dpi_set_port,
            dpi::dpi_set_enabled,
            dpi_test::dpi_test_strategies,
            dpi_test::dpi_test_status,
            dpi_test::dpi_cancel_test,
            dpi_test::dpi_strategy_urls,
            sites::sites_list,
            sites::sites_add_category,
            sites::sites_rename_category,
            sites::sites_delete_category,
            sites::sites_set_action,
            sites::sites_add_rule,
            sites::sites_update_rule,
            sites::sites_set_rule_test,
            sites::sites_delete_rule,
            megathrone_core::routing::routing_list,
            megathrone_core::routing::routing_set_fallback,
            discovery::discovery_sources_list,
            discovery::discovery_source_add,
            discovery::discovery_source_update,
            discovery::discovery_source_delete,
            discovery::discovery_source_delete_group,
            discovery::discovery_run,
            discovery::discovery_run_status,
            discovery::discovery_run_cancel,
            discovery::discovery_get_settings,
            discovery::discovery_set_settings
        ])
        .on_window_event(|window, event| {
            // desktop only: hiding into the tray instead of quitting
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // the tray is always alive, so defaulting to "hide" on a
                // broken settings read cannot strand the user
                let close_to_tray = megathrone_core::env::state()
                    .db
                    .lock()
                    .ok()
                    .and_then(|conn| settings::load_general(&conn).ok())
                    .map(|settings| settings.close_to_tray)
                    .unwrap_or(settings::DEFAULT_CLOSE_TO_TRAY);
                if close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            #[cfg(not(desktop))]
            {
                let _ = (window, event);
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            match event {
                // never leave a running sidecar / a system proxy behind on exit
                tauri::RunEvent::Exit => connection::shutdown(),
                // macOS: the dock icon was clicked while the window hides in
                // the tray — bring the window back
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => tray::show_main_window(_app),
                _ => {}
            }
        });
}
