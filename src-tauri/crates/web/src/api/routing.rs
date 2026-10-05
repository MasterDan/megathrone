//! The routing command slice (core: `routing.rs`).

use serde_json::Value;

use megathrone_core::routing;

use super::{req, run, to_json};

pub(super) async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "routing_list" => to_json(run(routing::routing_list).await),
        "routing_set_fallback" => async {
            let action = req::<String>(body, "action")?;
            to_json(routing::routing_set_fallback(action).await)
        }
        .await,
        _ => return None,
    })
}
