// The automatic endpoint-selection supervisor, split by concern:
// `supervisor` owns the generation-keyed background task, its registry and
// its two clocks (re-check + rotation), `passes` applies strategy picks
// through the scan/switch machinery (including the immediate `apply_now`),
// `verdicts` holds the strategy decisions — quality hysteresis and the
// mid-scan rescue. Per the repo convention this file stays re-exports only.

mod passes;
mod supervisor;
mod verdicts;

pub use passes::apply_now;
pub use supervisor::{content_updated, ensure, stop};
pub use verdicts::rescue;

#[cfg(test)]
mod auto_select_tests;
