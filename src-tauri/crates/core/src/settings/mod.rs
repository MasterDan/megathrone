// General settings (Settings → General), split by concern: `general` is
// the storage layer (defaults, key constants, load/validate/store),
// `commands` the Tauri command wrappers, `test_url` the shared test-URL
// normalizer. Per the repo convention this file stays re-exports only.
// The commands glob also re-exports the `#[tauri::command]` hidden macro
// bindings (`__cmd__*`) that `generate_handler!` looks up at this path.

mod commands;
mod general;
mod test_url;
#[cfg(test)]
mod settings_tests;

pub use commands::*;
// `settings` is a private module, so names with no in-crate consumer by
// path (`GeneralSettings`, the raw-proxy defaults) would trip
// `unused_imports` in non-test builds — the list is the module's frozen
// surface, kept whole on purpose.
#[allow(unused_imports)]
pub use general::{
    load_general, GeneralSettings, DEFAULT_CLOSE_TO_TRAY, DEFAULT_MIN_AVAILABILITY_PERCENT,
    DEFAULT_RAW_PROXY_ENABLED, DEFAULT_RAW_PROXY_PORT, DEFAULT_RECHECK_MINUTES,
    DEFAULT_ROTATION_MINUTES,
};
pub use test_url::validate_test_url;
