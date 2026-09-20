use std::sync::{Arc, Mutex};

use tauri::Manager;
use tauri_plugin_shell::ShellExt;
use tokio::sync::Notify;

mod auto_select;
mod connection;
mod db;
mod dpi;
mod dpi_test;
mod latency;
mod parser;
mod profiles;
mod routing;
mod scheduler;
mod settings;
mod sites;
mod tray;

pub struct AppState {
    db: Mutex<rusqlite::Connection>,
    auto_update_notify: Arc<Notify>,
}

impl AppState {
    /// Re-arm the auto-update scheduler (next wake is recomputed from the DB).
    pub fn notify_auto_update(&self) {
        self.auto_update_notify.notify_one();
    }
}

#[tauri::command]
async fn sing_box_version(app: tauri::AppHandle) -> Result<String, String> {
    let output = app
        .shell()
        .sidecar("sing-box")
        .map_err(|e| format!("failed to resolve sing-box sidecar: {e}"))?
        .args(["version"])
        .output()
        .await
        .map_err(|e| format!("failed to run sing-box: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "sing-box exited with {:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().next().unwrap_or_default().trim().to_string())
}

pub fn run() {
    let auto_update_notify = Arc::new(Notify::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup({
            let notify = auto_update_notify.clone();
            move |app| {
                let data_dir = app.path().app_data_dir()?;
                std::fs::create_dir_all(&data_dir)?;
                let connection = db::open(&data_dir.join("megathrone.db"))
                    .map_err(|e| format!("failed to initialize the profiles database: {e}"))?;
                // first run only: the bundled strategy presets and the
                // default test-site catalog (never into a user-touched table)
                dpi::seed_default_strategies(&connection)
                    .map_err(|e| format!("failed to seed the default DPI strategies: {e}"))?;
                sites::seed_default_sites(&connection)
                    .map_err(|e| format!("failed to seed the default test sites: {e}"))?;
                app.manage(AppState {
                    db: Mutex::new(connection),
                    auto_update_notify: notify,
                });
                scheduler::spawn(app.handle().clone(), auto_update_notify.clone());
                // undo whatever a hard-killed previous session left behind,
                // and make Ctrl+C/SIGTERM go through the graceful exit path
                connection::recover(app.handle());
                connection::install_exit_signals(app.handle());
                // the tray icon (status menu + the real quit); the window's
                // close button hides into it unless the user opted out
                tray::init(app.handle())?;
                Ok(())
            }
        })
        .invoke_handler(tauri::generate_handler![
            sing_box_version,
            profiles::profiles_list,
            profiles::profile_get,
            profiles::profile_import_from_url,
            profiles::profile_import_from_file,
            profiles::profile_import_from_text,
            profiles::profile_update,
            profiles::profile_update_status,
            profiles::profile_rename,
            profiles::profile_delete,
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
            routing::routing_list,
            routing::routing_set_fallback
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                // the tray is always alive, so defaulting to "hide" on a
                // broken settings read cannot strand the user
                let close_to_tray = app
                    .try_state::<AppState>()
                    .and_then(|state| {
                        let conn = state.db.lock().ok()?;
                        settings::load_general(&conn).ok()
                    })
                    .map(|settings| settings.close_to_tray)
                    .unwrap_or(settings::DEFAULT_CLOSE_TO_TRAY);
                if close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            match event {
                // never leave a running sidecar / a system proxy behind on exit
                tauri::RunEvent::Exit => connection::shutdown(app),
                // macOS: the dock icon was clicked while the window hides in
                // the tray — bring the window back
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => tray::show_main_window(app),
                _ => {}
            }
        });
}
