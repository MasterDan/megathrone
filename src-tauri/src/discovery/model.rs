use serde::Serialize;

/// Single event the discovery run narrates itself through (the sources
/// list page subscribes; the payloads below are the variants).
pub const DISCOVERY_PROGRESS_EVENT: &str = "discovery-progress";

/// One seeded/added subscription source of the catalog.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverySource {
    pub id: i64,
    pub url: String,
    pub name: String,
    pub enabled: bool,
    pub position: i64,
    /// the profile the last run created/updated for this source
    pub profile_id: Option<i64>,
    pub last_run_at: Option<String>,
    pub last_error: Option<String>,
    pub last_item_count: Option<i64>,
}

/// Snapshot of the (last) discovery run — the registry keeps the final
/// state around after completion so a remounted page can adopt it via
/// `discovery_run_status` until the next run replaces it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryRunStatus {
    /// SQLite `datetime('now')` style, captured at run start
    pub started_at: String,
    /// enabled sources this run
    pub total: usize,
    /// finished sources (ok + failed)
    pub done: usize,
    pub failed: usize,
    /// profiles imported this run
    pub created: usize,
    /// profiles diff-updated this run
    pub updated: usize,
    /// "fetch" | "testing" | "done"
    pub phase: &'static str,
    /// the profile the optional testing phase is scanning right now
    pub test_profile_id: Option<i64>,
    pub test_profile_name: Option<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum DiscoveryProgress {
    #[serde(rename = "source-started", rename_all = "camelCase")]
    SourceStarted { source_id: i64, name: String },
    #[serde(rename = "source-done", rename_all = "camelCase")]
    SourceDone {
        source_id: i64,
        name: String,
        ok: bool,
        item_count: usize,
        error: Option<String>,
    },
    #[serde(rename = "scan-started", rename_all = "camelCase")]
    ScanStarted { profile_id: i64, name: String },
    #[serde(rename = "finished", rename_all = "camelCase")]
    Finished { created: usize, updated: usize, failed: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn progress_payloads_serialize_to_the_contract_shape() {
        assert_eq!(
            serde_json::to_value(DiscoveryProgress::SourceStarted {
                source_id: 7,
                name: "Omega-7".into(),
            })
            .unwrap(),
            json!({"kind": "source-started", "sourceId": 7, "name": "Omega-7"})
        );
        assert_eq!(
            serde_json::to_value(DiscoveryProgress::SourceDone {
                source_id: 8,
                name: "Omega-8".into(),
                ok: false,
                item_count: 0,
                error: Some("request failed".into()),
            })
            .unwrap(),
            json!({
                "kind": "source-done", "sourceId": 8, "name": "Omega-8",
                "ok": false, "itemCount": 0, "error": "request failed"
            })
        );
        assert_eq!(
            serde_json::to_value(DiscoveryProgress::ScanStarted {
                profile_id: 3,
                name: "Omega-3".into(),
            })
            .unwrap(),
            json!({"kind": "scan-started", "profileId": 3, "name": "Omega-3"})
        );
        assert_eq!(
            serde_json::to_value(DiscoveryProgress::Finished {
                created: 4,
                updated: 5,
                failed: 6,
            })
            .unwrap(),
            json!({"kind": "finished", "created": 4, "updated": 5, "failed": 6})
        );
    }
}
