use serde::Serialize;

pub const PROFILES_CHANGED_EVENT: &str = "profiles-changed";

/// Global profile-update notifications (`ProfileUpdateNotice` payloads):
/// the update flow announces its start and its outcome (success or error)
/// so the toast can tell the user what a background refresh is doing.
pub const PROFILE_UPDATE_EVENT: &str = "profile-update";

pub const UPDATE_NOTICE_STARTED: &str = "started";
pub const UPDATE_NOTICE_SUCCEEDED: &str = "success";
pub const UPDATE_NOTICE_FAILED: &str = "error";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdateNotice {
    pub profile_id: i64,
    pub profile_name: String,
    /// one of UPDATE_NOTICE_* — the frontend styles and stacks by it
    pub kind: &'static str,
    /// the outcome detail ("2 added, 1 removed" / the error text)
    pub message: Option<String>,
}

/// How the profile's live endpoint is chosen (Settings of the endpoints
/// page): one of the automatic strategies or the manual checkmark.
pub const SELECT_MANUAL: &str = "manual";
pub const SELECT_FASTEST: &str = "fastest";
pub const SELECT_MOST_AVAILABLE: &str = "most_available";
pub const SELECT_ROUND_ROBIN: &str = "round_robin";

pub const SELECTION_MODES: [&str; 4] =
    [SELECT_ROUND_ROBIN, SELECT_FASTEST, SELECT_MOST_AVAILABLE, SELECT_MANUAL];

#[derive(Debug, Clone)]
pub struct EndpointCandidate {
    pub id: i64,
    pub raw: String,
    pub tag: String,
    pub available: Option<bool>,
    pub latency_ms: Option<i64>,
    pub url_ok: i64,
    pub url_total: i64,
    pub order_index: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub id: i64,
    pub name: String,
    pub source_url: Option<String>,
    pub source_path: Option<String>,
    pub auto_update_minutes: Option<i64>,
    pub last_fetched_at: Option<String>,
    pub item_count: i64,
    pub skipped_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointItem {
    pub id: i64,
    pub tag: String,
    pub protocol: String,
    pub server: Option<String>,
    pub server_port: Option<i64>,
    pub available: Option<bool>,
    pub latency_ms: Option<i64>,
    pub up_bytes: i64,
    pub down_bytes: i64,
    pub speed_bps: Option<f64>,
    /// how many of the deep-probed proxy-category URLs this endpoint reaches
    /// / how many were probed (0/0 — not deep-probed in the last scan)
    pub url_ok: i64,
    pub url_total: i64,
}

/// One category URL's latest deep-probe outcome for an endpoint; rows exist
/// only for URLs actually probed through it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointUrlStatus {
    pub url_id: i64,
    pub category: String,
    pub url: String,
    pub available: Option<bool>,
    pub latency_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetail {
    pub raw: String,
    pub outbound_json: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesChangedPayload {
    pub profile_id: Option<i64>,
    /// "content" — endpoints changed, "meta" — only profile settings changed,
    /// "latency" — only availability test results were refreshed,
    /// "selection" — the picked endpoint changed,
    /// "updating" — an update of this profile just started,
    /// "update-done" — the update finished (successfully or not)
    pub kind: &'static str,
}
