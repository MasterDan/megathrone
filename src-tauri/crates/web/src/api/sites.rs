//! The routing-category command slice (core: `sites/`).

use serde_json::Value;

use megathrone_core::sites;

use super::{req, run, to_json};

pub(super) async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "sites_list" => to_json(run(sites::sites_list).await),
        "sites_add_category" => async {
            let name = req::<String>(body, "name")?;
            to_json(run(move || sites::sites_add_category(name)).await)
        }
        .await,
        "sites_rename_category" => async {
            let category_id = req::<i64>(body, "categoryId")?;
            let name = req::<String>(body, "name")?;
            to_json(run(move || sites::sites_rename_category(category_id, name)).await)
        }
        .await,
        "sites_delete_category" => async {
            let category_id = req::<i64>(body, "categoryId")?;
            to_json(sites::sites_delete_category(category_id).await)
        }
        .await,
        "sites_set_action" => async {
            let category_id = req::<i64>(body, "categoryId")?;
            let action = req::<String>(body, "action")?;
            to_json(sites::sites_set_action(category_id, action).await)
        }
        .await,
        "sites_add_rule" => async {
            let category_id = req::<i64>(body, "categoryId")?;
            let rule_type = req::<String>(body, "ruleType")?;
            let value = req::<String>(body, "value")?;
            to_json(sites::sites_add_rule(category_id, rule_type, value).await)
        }
        .await,
        "sites_update_rule" => async {
            let rule_id = req::<i64>(body, "ruleId")?;
            let rule_type = req::<String>(body, "ruleType")?;
            let value = req::<String>(body, "value")?;
            to_json(sites::sites_update_rule(rule_id, rule_type, value).await)
        }
        .await,
        "sites_set_rule_test" => async {
            let rule_id = req::<i64>(body, "ruleId")?;
            let test_enabled = req::<bool>(body, "testEnabled")?;
            to_json(run(move || sites::sites_set_rule_test(rule_id, test_enabled)).await)
        }
        .await,
        "sites_delete_rule" => async {
            let rule_id = req::<i64>(body, "ruleId")?;
            to_json(sites::sites_delete_rule(rule_id).await)
        }
        .await,
        _ => return None,
    })
}
