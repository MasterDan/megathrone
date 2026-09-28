// The byedpi (ciadpi) strategy layer, split by concern: `commands` holds the
// Tauri wrappers, `strategies` the strategy CRUD storage and the active
// selection, `settings` the dpi_port/dpi_enabled accessors, `validation` the
// name/argument-line checks, `presets` the bundled catalog and its seeding.
// Per the repo convention this file stays re-exports only. The glob over
// `commands` is load-bearing: it also re-exports the `#[tauri::command]`
// hidden macros so `generate_handler![dpi::…]` keeps resolving.

mod commands;
mod presets;
mod settings;
mod strategies;
mod validation;
#[cfg(test)]
mod dpi_tests;

pub use commands::*;
pub use presets::seed_default_strategies;
pub use settings::{DpiSettings, dpi_enabled, load_dpi_port};
pub use strategies::{DpiStrategy, list_strategies, load_active_strategy};
pub use validation::{parse_strategy_args, validate_name};
