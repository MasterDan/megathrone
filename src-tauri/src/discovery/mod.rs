// The Discovery layer — an editable catalog of public proxy-subscription
// URLs (Settings → Discovery): `model` holds the shared types and the
// progress-event payloads, `seed` the bundled source catalog, `storage` the
// CRUD layer, `filter` the pure content filters (insecure-line detection,
// dedup), `run` the background discovery run with its single-run registry,
// `settings` the persisted run options (the "test after" default, the
// auto-update interval the background scheduler serves) and `commands`
// the Tauri wrappers. The glob re-exports keep the surface flat —
// `commands::*` also carries the hidden `__cmd__*` macro bindings
// `generate_handler!` looks up here. Per the repo convention this file
// stays re-exports only.

mod commands;
mod filter;
mod model;
mod run;
mod seed;
mod settings;
mod storage;

pub use commands::*;
pub use run::run_if_due;
pub use seed::seed_discovery_sources;
pub use settings::next_due_seconds;

#[cfg(test)]
mod filter_tests;
#[cfg(test)]
mod settings_tests;
