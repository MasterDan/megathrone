//! The process-wide core environment: the shared application state, the
//! event sink every backend broadcast goes through, and the shell's data
//! directory. Installed once at startup by whichever shell owns the
//! process — the Tauri app in its setup, the console web server before
//! its HTTP listener starts — after which all of core is shell-agnostic:
//! no `AppHandle` threading, no `State` extraction, just `env::state()` /
//! `env::emit` / `env::data_dir`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use crate::AppState;

/// Where backend broadcasts go: the Tauri shell emits them on its IPC
/// event bus (the webview listeners), the console web server forwards
/// them to the browser over its SSE stream. Payloads travel as JSON
/// values.
pub trait EventSink: Send + Sync + 'static {
    fn emit_json(&self, event: &str, payload: Value);
}

/// The Tauri shell's sink: the app's own event bus.
struct TauriSink(tauri::AppHandle);

impl EventSink for TauriSink {
    fn emit_json(&self, event: &str, payload: Value) {
        use tauri::Emitter as _;
        let _ = self.0.emit(event, payload);
    }
}

/// The installed environment (see [`install`]).
struct Environment {
    state: Arc<AppState>,
    sink: Arc<dyn EventSink>,
    data_dir: Option<PathBuf>,
}

static ENV: Mutex<Option<Environment>> = Mutex::new(None);

/// Installs the environment for the console web server (its own sink).
pub fn install(state: Arc<AppState>, sink: Arc<dyn EventSink>, data_dir: Option<PathBuf>) {
    *ENV.lock().expect("the core environment lock") =
        Some(Environment { state, sink, data_dir });
}

/// Installs the environment for the Tauri shell (events on the app bus).
pub fn install_tauri(handle: tauri::AppHandle, state: Arc<AppState>, data_dir: Option<PathBuf>) {
    install(state, Arc::new(TauriSink(handle)), data_dir);
}

/// The shared application state. Panics before `install` — installing the
/// environment is the first startup step of every shell.
pub fn state() -> Arc<AppState> {
    ENV.lock()
        .expect("the core environment lock")
        .as_ref()
        .expect("the core environment is not installed — the shell must install it at startup")
        .state
        .clone()
}

/// The shell's data directory (the crash-recovery session marker lives
/// there); `None` before `install` and for shells without one.
pub fn data_dir() -> Option<PathBuf> {
    ENV.lock()
        .expect("the core environment lock")
        .as_ref()
        .and_then(|env| env.data_dir.clone())
}

/// Broadcasts an event to whatever is listening: the UI event bus in the
/// Tauri app, the SSE stream of the web server. A no-op before `install`
/// (pure library use — unit tests never install one).
pub fn emit<S: Serialize>(event: &str, payload: S) {
    let sink = ENV
        .lock()
        .expect("the core environment lock")
        .as_ref()
        .map(|env| env.sink.clone());
    if let Some(sink) = sink {
        if let Ok(payload) = serde_json::to_value(payload) {
            sink.emit_json(event, payload);
        }
    }
}

/// Drops the environment (tests only — the production shells install
/// exactly once and never remove it).
#[cfg(test)]
fn reset() {
    *ENV.lock().expect("the core environment lock") = None;
}

// one test fn owns the environment: the global would race between
// parallel tests otherwise
#[cfg(test)]
mod tests {
    use super::*;

    struct RecordingSink {
        events: Mutex<Vec<(String, Value)>>,
    }

    impl EventSink for RecordingSink {
        fn emit_json(&self, event: &str, payload: Value) {
            self.events.lock().expect("sink lock").push((event.to_string(), payload));
        }
    }

    #[test]
    fn environment_lifecycle() {
        reset();
        // before install: broadcasts are dropped silently, no data dir, no panic
        emit("probe", true);
        assert_eq!(data_dir(), None);

        let conn =
            rusqlite::Connection::open_in_memory().expect("an in-memory database is enough here");
        let app_state = Arc::new(AppState::new(conn, Arc::new(tokio::sync::Notify::new())));
        let sink = Arc::new(RecordingSink {
            events: Mutex::new(Vec::new()),
        });
        install(
            Arc::clone(&app_state),
            Arc::clone(&sink) as Arc<dyn EventSink>,
            Some(PathBuf::from("/tmp/megathrone-env-test")),
        );
        assert_eq!(data_dir(), Some(PathBuf::from("/tmp/megathrone-env-test")));

        emit("probe", true);
        let recorded = sink.events.lock().expect("sink lock").clone();
        assert_eq!(recorded, vec![("probe".to_string(), Value::Bool(true))]);
        assert!(
            state().db.lock().is_ok(),
            "state() hands back the installed AppState"
        );

        // after a reset the environment is gone again: silent broadcasts,
        // no data dir
        reset();
        emit("probe", true);
        assert_eq!(data_dir(), None);
        assert_eq!(
            sink.events.lock().expect("sink lock").len(),
            1,
            "the dropped environment no longer forwards broadcasts"
        );
    }
}
