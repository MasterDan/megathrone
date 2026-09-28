use std::path::Path;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

use super::model::{ProfilesChangedPayload, PROFILES_CHANGED_EVENT};

const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Sources & notifications
// ---------------------------------------------------------------------------

pub(super) fn fetch_url(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|e| format!("request failed: {e}"))?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_string()
        .map_err(|e| format!("failed to read response body: {e}"))
}

pub(super) fn read_file(path: &str) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if metadata.len() > MAX_DOWNLOAD_BYTES {
        return Err(format!("file is too large: {} bytes", metadata.len()));
    }
    std::fs::read_to_string(Path::new(path)).map_err(|e| format!("cannot read {path}: {e}"))
}

pub(super) fn default_name_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| "Imported profile".to_string())
}

pub(super) fn default_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Imported profile")
        .to_string()
}

/// Notify the UI and re-arm the auto-update scheduler after any mutation.
/// Use `CONTENT` when endpoints may have changed, `META` for settings-only
/// changes (rename, auto-update interval), `LATENCY` when only availability
/// test results were refreshed.
pub fn after_mutation(app: &AppHandle, profile_id: Option<i64>, kind: &'static str) {
    let _ = app.emit(PROFILES_CHANGED_EVENT, ProfilesChangedPayload { profile_id, kind });
    app.state::<AppState>().notify_auto_update();
}

pub const CONTENT: &str = "content";
pub const META: &str = "meta";
pub const LATENCY: &str = "latency";
pub const SELECTION: &str = "selection";
pub const UPDATING: &str = "updating";
pub const UPDATE_DONE: &str = "update-done";
