// Profiles & endpoint storage, split by concern: `model` holds the shared
// types and constants, `commands` the Tauri wrappers, `storage` the
// profile-level and `endpoints` the endpoint-level queries, `selection`
// the endpoint-picking strategies, `update` the refresh/diff flow and
// `sources` the fetch/read helpers plus mutation notifications. The
// re-exports keep the historical `crate::profiles::*` surface intact —
// `commands::*` also carries the hidden `__cmd__*` macro bindings
// `generate_handler!` looks up here. Per the repo convention this file
// stays re-exports only.

mod commands;
mod endpoints;
mod model;
mod selection;
mod sources;
mod storage;
mod update;

pub use commands::*;
pub(crate) use endpoints::set_selected_endpoint;
pub use model::*;
pub use selection::*;
pub use sources::*;
pub use storage::*;
pub use update::*;

#[cfg(test)]
mod endpoints_tests;
#[cfg(test)]
mod profiles_tests;
#[cfg(test)]
mod selection_tests;
#[cfg(test)]
mod update_tests;
