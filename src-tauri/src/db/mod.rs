// The SQLite layer, split by concern: `migrations` loads the versioned
// `vN.name.sql` files (resource dir on desktop, embedded copies on Android),
// `runner` opens connections and walks the migration chain, `settings` is
// the scalar-settings KV. Per the repo convention this file stays
// re-exports only.

mod migrations;
mod runner;
mod settings;
#[cfg(test)]
mod upgrade_tests;

pub use migrations::Migrations;
pub use runner::open;
pub use settings::{get_setting, set_setting};
