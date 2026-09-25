// The DPI strategy test behind the Test button on Settings → DPI:
// `runner` drives the per-strategy ciadpi + sing-box sweep, `registry` is
// the single-entry status/cancel registry, `storage` persists and reads
// back the per-strategy site results. Re-exports only, per the repo
// convention. `generate_handler!` resolves each command's hidden macro
// companions (`#[tauri::command]` defines them next to the fn) through
// this module's path, so they ride along with the fns.

mod registry;
mod runner;
mod storage;

pub use registry::__cmd__dpi_cancel_test;
pub use registry::__cmd__dpi_test_status;
pub use registry::__tauri_command_name_dpi_cancel_test;
pub use registry::__tauri_command_name_dpi_test_status;
pub use registry::{dpi_cancel_test, dpi_test_status};
pub use runner::__cmd__dpi_test_strategies;
pub use runner::__tauri_command_name_dpi_test_strategies;
pub use runner::dpi_test_strategies;
pub use storage::__cmd__dpi_strategy_urls;
pub use storage::__tauri_command_name_dpi_strategy_urls;
pub use storage::dpi_strategy_urls;
