//! The system tray icon. Closing the window hides it to the tray (unless
//! the user turned that off in Settings → General); the tray menu carries
//! the session status, a way back into the window, an emergency
//! disconnect and the real quit — which goes through the graceful exit
//! path that tears the proxy session down, so it is always safe to reach
//! for it.
//!
//! The menu mirrors the proxy session by listening for
//! `connection-changed`: every transition (connect, disconnect, an
//! unexpected sing-box exit) rewrites the status row, the tooltip and the
//! Disconnect item's enabled state.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Listener, Manager, Wry};

use crate::connection::{self, ConnectionSnapshot, CONNECTION_CHANGED_EVENT};

const TRAY_ID: &str = "main";

/// The menu rows that change with the session. Menu handles are cheap
/// clones talking to the main thread, so they can live in the app state.
struct TrayState {
    status: MenuItem<Wry>,
    disconnect: MenuItem<Wry>,
}

/// Builds the tray icon and its menu, adopts the current session state
/// and subscribes to every later transition.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Disconnected", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Megathrone", true, None::<&str>)?;
    let disconnect = MenuItem::with_id(app, "disconnect", "Disconnect", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Megathrone", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &disconnect,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Megathrone — Disconnected")
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "disconnect" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    // errors land in the session state and come back as a
                    // `connection-changed` (lastError) — nothing to do here
                    let _ = connection::connection_disconnect(app).await;
                });
            }
            // RunEvent::Exit fires and unwinds the session gracefully
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    app.manage(TrayState { status, disconnect });

    // adopt whatever the backend is doing right now, then follow every
    // transition (the same `connection-changed` events the UI drinks)
    refresh(app, &connection::connection_status().unwrap_or_default());
    let listener_app = app.clone();
    app.listen(CONNECTION_CHANGED_EVENT, move |event| {
        if let Ok(snapshot) = serde_json::from_str::<ConnectionSnapshot>(event.payload()) {
            refresh(&listener_app, &snapshot);
        }
    });
    Ok(())
}

/// Rewrites the mutable rows (the status text, the tooltip, the
/// Disconnect item) to match `snapshot`. A broken tray is cosmetic, so
/// failures are ignored instead of failing the caller.
fn refresh(app: &AppHandle, snapshot: &ConnectionSnapshot) {
    let status_line = if snapshot.connected {
        match (&snapshot.profile_name, &snapshot.mode) {
            (Some(name), Some(mode)) => format!("Connected — {name} ({mode})"),
            (Some(name), None) => format!("Connected — {name}"),
            _ => "Connected".to_string(),
        }
    } else {
        "Disconnected".to_string()
    };
    let tooltip = if snapshot.connected {
        format!("Megathrone — {status_line}")
    } else {
        "Megathrone — Disconnected".to_string()
    };
    if let Some(state) = app.try_state::<TrayState>() {
        let _ = state.status.set_text(status_line);
        let _ = state.disconnect.set_enabled(snapshot.connected);
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

/// Brings the main window back from the tray (open menu item, macOS dock
/// icon click).
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
