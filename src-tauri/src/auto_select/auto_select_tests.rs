// Shared fixtures for the inline tests of the `auto_select` submodules
// (the tests themselves sit next to the private functions they exercise).

use crate::profiles::EndpointCandidate;

pub(super) fn candidate(
    tag: &str,
    available: Option<bool>,
    latency_ms: Option<i64>,
    url_ok: i64,
    url_total: i64,
) -> EndpointCandidate {
    EndpointCandidate {
        id: 0,
        raw: format!("raw-{tag}"),
        tag: tag.to_string(),
        available,
        latency_ms,
        url_ok,
        url_total,
        order_index: 0,
    }
}
