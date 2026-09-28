//! URL categories (Settings → Routing) — the single catalog both
//! tests and routing draw from.
//!
//! Every category carries a routing action (`dpi` | `proxy` | `direct`,
//! switched in the Routing list) and a list of typed routing rules.
//! `dpi` categories are probed by the DPI strategy test (Settings → DPI)
//! and their rules route into the DPI tunnel while the proxy is connected;
//! `proxy` categories are deep-probed through endpoints during latency
//! scans (`latency.rs`) and their rules go through the proxy; `direct`
//! categories match traffic that bypasses both. Only rules marked
//! `test_enabled` join the tests — and only rule types that can be turned
//! into a probe URL (`url`, `domain_suffix`, `domain`) at that. The
//! default catalog is adapted from the ByeByeDPI Android app's
//! `proxytest_*.sites` assets (github.com/romanvht/ByeByeDPI). Deleting a
//! category (or a single rule) cascades the matching result rows away, so
//! strategy percentages and endpoint scores always reflect the currently
//! configured list.

use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRule {
    pub id: i64,
    pub rule_type: String,
    pub value: String,
    /// joins the DPI strategy test / endpoint deep probe
    pub test_enabled: bool,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TestSiteCategory {
    pub id: i64,
    pub name: String,
    /// dpi | proxy | direct — switched in Settings → Routing
    pub action: String,
    pub rules: Vec<CategoryRule>,
}

/// A category with its typed rules and its routing action — what
/// `routing.rs` builds the effective config from.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRules {
    pub id: i64,
    pub name: String,
    pub action: String,
    pub rules: Vec<CategoryRule>,
}
