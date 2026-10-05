//! The profiles command slice (core: `profiles/commands.rs`).

use serde_json::Value;

use megathrone_core::profiles;

use super::{opt, req, run, to_json};

pub(super) async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "profiles_list" => to_json(run(profiles::profiles_list).await),
        "profile_get" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(run(move || profiles::profile_get(profile_id)).await)
        }
        .await,
        "profile_import_from_url" => async {
            let url = req::<String>(body, "url")?;
            let name = opt::<String>(body, "name")?;
            to_json(run(move || profiles::profile_import_from_url(url, name)).await)
        }
        .await,
        "profile_import_from_file" => async {
            let path = req::<String>(body, "path")?;
            let name = opt::<String>(body, "name")?;
            to_json(run(move || profiles::profile_import_from_file(path, name)).await)
        }
        .await,
        "profile_import_from_text" => async {
            let content = req::<String>(body, "content")?;
            let name = req::<String>(body, "name")?;
            to_json(run(move || profiles::profile_import_from_text(content, name)).await)
        }
        .await,
        "profile_update" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(profiles::profile_update(profile_id).await)
        }
        .await,
        "profile_update_status" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(run(move || profiles::profile_update_status(profile_id)).await)
        }
        .await,
        "profile_rename" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            let new_name = req::<String>(body, "newName")?;
            to_json(run(move || profiles::profile_rename(profile_id, new_name)).await)
        }
        .await,
        "profile_delete" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(run(move || profiles::profile_delete(profile_id)).await)
        }
        .await,
        "profiles_delete_bulk" => async {
            let profile_ids = req::<Vec<i64>>(body, "profileIds")?;
            to_json(profiles::profiles_delete_bulk(profile_ids).await)
        }
        .await,
        "profile_set_sidebar_visible" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            let visible = req::<bool>(body, "visible")?;
            to_json(run(move || profiles::profile_set_sidebar_visible(profile_id, visible)).await)
        }
        .await,
        "profile_set_sidebar_order" => async {
            let ordered_ids = req::<Vec<i64>>(body, "orderedIds")?;
            to_json(run(move || profiles::profile_set_sidebar_order(ordered_ids)).await)
        }
        .await,
        "profile_set_auto_update" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            let minutes = opt::<i64>(body, "minutes")?;
            to_json(run(move || profiles::profile_set_auto_update(profile_id, minutes)).await)
        }
        .await,
        "profile_items" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            let offset = req::<i64>(body, "offset")?;
            let limit = req::<i64>(body, "limit")?;
            to_json(run(move || profiles::profile_items(profile_id, offset, limit)).await)
        }
        .await,
        "profile_item_index" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            let item_id = req::<i64>(body, "itemId")?;
            to_json(run(move || profiles::profile_item_index(profile_id, item_id)).await)
        }
        .await,
        "profile_item_detail" => async {
            let item_id = req::<i64>(body, "itemId")?;
            to_json(run(move || profiles::profile_item_detail(item_id)).await)
        }
        .await,
        "profile_endpoint_urls" => async {
            let item_id = req::<i64>(body, "itemId")?;
            to_json(run(move || profiles::profile_endpoint_urls(item_id)).await)
        }
        .await,
        "profile_selected_endpoint" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(run(move || profiles::profile_selected_endpoint(profile_id)).await)
        }
        .await,
        "profile_select_endpoint" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            let item_id = opt::<i64>(body, "itemId")?;
            to_json(profiles::profile_select_endpoint(profile_id, item_id).await)
        }
        .await,
        "profile_selection_mode" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(run(move || profiles::profile_selection_mode(profile_id)).await)
        }
        .await,
        "profile_set_selection_mode" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            let mode = req::<String>(body, "mode")?;
            to_json(profiles::profile_set_selection_mode(profile_id, mode).await)
        }
        .await,
        _ => return None,
    })
}
