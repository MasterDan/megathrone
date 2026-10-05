//! The subscription-discovery command slice (core: `discovery/`).

use serde_json::Value;

use megathrone_core::discovery::{self, DiscoverySettings};

use super::{opt, req, run, to_json};

pub(super) async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "discovery_sources_list" => to_json(run(discovery::discovery_sources_list).await),
        "discovery_source_add" => async {
            let url = req::<String>(body, "url")?;
            let name = opt::<String>(body, "name")?;
            let merge_group = opt::<String>(body, "mergeGroup")?;
            to_json(run(move || discovery::discovery_source_add(url, name, merge_group)).await)
        }
        .await,
        "discovery_source_update" => async {
            let id = req::<i64>(body, "id")?;
            let url = opt::<String>(body, "url")?;
            let name = opt::<String>(body, "name")?;
            let enabled = opt::<bool>(body, "enabled")?;
            to_json(run(move || discovery::discovery_source_update(id, url, name, enabled)).await)
        }
        .await,
        "discovery_source_delete" => async {
            let id = req::<i64>(body, "id")?;
            let delete_profile = req::<bool>(body, "deleteProfile")?;
            to_json(run(move || discovery::discovery_source_delete(id, delete_profile)).await)
        }
        .await,
        "discovery_source_delete_group" => async {
            let group = req::<String>(body, "group")?;
            to_json(run(move || discovery::discovery_source_delete_group(group)).await)
        }
        .await,
        "discovery_run" => async {
            let test_after = req::<bool>(body, "testAfter")?;
            to_json(run(move || discovery::discovery_run(test_after)).await)
        }
        .await,
        "discovery_run_status" => to_json(run(discovery::discovery_run_status).await),
        "discovery_run_cancel" => to_json(run(discovery::discovery_run_cancel).await),
        "discovery_get_settings" => to_json(run(discovery::discovery_get_settings).await),
        "discovery_set_settings" => async {
            let settings = req::<DiscoverySettings>(body, "settings")?;
            to_json(run(move || discovery::discovery_set_settings(settings)).await)
        }
        .await,
        _ => return None,
    })
}
