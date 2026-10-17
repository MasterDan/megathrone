//! The Megathrone core library: every piece of the backend that is not the
//! shell around it — the SQLite layer, the subscription parser, profiles,
//! routing, the sing-box/byedpi session management, scans and the scheduler.
//! Both shells consume it identically: the Tauri desktop/Android app
//! (`src-tauri`) and the console web server (`crates/web`) install the
//! [`env`] environment at startup and then drive the same command surface.
//!
//! The crate still leans on `tauri` types in its command layer
//! (`tauri::command` macros) — the commands are thin wrappers over pure
//! storage functions; state access and event broadcasts go through
//! [`env`] so either shell can host them.

use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

pub mod auto_select;
pub mod connection;
pub mod db;
pub mod discovery;
pub mod dpi;
pub mod dpi_test;
pub mod env;
pub mod latency;
pub mod mobile;
pub mod parser;
pub mod process;
pub mod profiles;
pub mod routing;
pub mod scheduler;
pub mod settings;
pub mod singleton;
pub mod sites;

/// The shared application state: the SQLite handle (behind a std mutex —
/// never held across network/file IO) and the auto-update scheduler poke.
pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    auto_update_notify: Arc<Notify>,
}

impl AppState {
    pub fn new(db: rusqlite::Connection, notify: Arc<Notify>) -> Self {
        Self {
            db: Mutex::new(db),
            auto_update_notify: notify,
        }
    }

    /// Re-arm the auto-update scheduler (next wake is recomputed from the DB).
    pub fn notify_auto_update(&self) {
        self.auto_update_notify.notify_one();
    }
}
