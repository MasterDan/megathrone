//! The latency-scan command slice (core: `latency/`).

use serde_json::Value;

use megathrone_core::latency;

use super::{req, run, to_json};

pub(super) async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "profile_check_latency" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(latency::profile_check_latency(profile_id).await)
        }
        .await,
        "profile_latency_status" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(run(move || latency::profile_latency_status(profile_id)).await)
        }
        .await,
        "profile_cancel_latency" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(run(move || latency::profile_cancel_latency(profile_id)).await)
        }
        .await,
        "profile_test_endpoint_urls" => async {
            let item_id = req::<i64>(body, "itemId")?;
            to_json(latency::profile_test_endpoint_urls(item_id).await)
        }
        .await,
        _ => return None,
    })
}
