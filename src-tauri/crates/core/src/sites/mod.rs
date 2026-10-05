// URL categories (Settings → Routing), split by concern: `model` holds the
// category/rule types, `validation` the rule-type constants and value
// checks, `queries`/`crud` the storage layer, `catalog` the seeded default
// and `commands` the Tauri wrappers. The glob re-exports keep the
// historical `crate::sites::*` surface intact — `commands::*` also carries
// the hidden `__cmd__*` macro bindings `generate_handler!` looks up here.
// Per the repo convention this file stays re-exports only.

mod catalog;
mod commands;
mod crud;
mod model;
mod queries;
mod validation;

pub use catalog::*;
pub use commands::*;
pub use crud::*;
pub use queries::*;
pub use validation::*;

#[cfg(test)]
mod sites_tests;
