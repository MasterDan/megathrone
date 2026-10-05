// Endpoint availability / latency probing — a URL test through a throwaway
// sing-box instance, split by concern: `registry` the process-wide scan
// registry (status, cancel, wait-for-exit) plus its two commands, `scan` the
// whole-profile scan orchestration and payload types, `endpoint_probe` the
// on-demand single-endpoint deep probe, `storage` the DB persistence layer,
// `instance` the throwaway sing-box (config building, `check` bisection,
// sidecar guard) and `clash` the clash-API probe client. The glob re-exports
// keep the historical `crate::latency::*` surface intact — the command globs
// also carry the hidden `__cmd__*` macro bindings `generate_handler!` looks
// up here. Per the repo convention this file stays re-exports only.

mod clash;
mod endpoint_probe;
mod instance;
mod registry;
mod scan;
mod storage;

pub use clash::url_test_chunk;
pub use endpoint_probe::*;
pub use instance::{
    STARTUP_TIMEOUT, SingBoxGuard, build_test_config, free_local_port,
    isolate_unloadable_outbounds, sanitize_outbound, sidecar, wait_for_api, write_config_file,
};
pub use registry::*;
pub use scan::*;
pub use storage::{OutboundRow, load_outbounds};

#[cfg(test)]
mod latency_tests;
