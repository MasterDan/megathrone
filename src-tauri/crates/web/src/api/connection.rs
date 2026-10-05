//! The proxy-session command slice (core: `connection/`).

use serde_json::Value;

use megathrone_core::connection;

use super::{req, run, to_json};

pub(super) async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "connection_connect" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            let mode = req::<String>(body, "mode")?;
            to_json(connection::connection_connect(profile_id, mode).await)
        }
        .await,
        "connection_disconnect" => to_json(connection::connection_disconnect().await),
        "connection_status" => to_json(run(connection::connection_status).await),
        "connection_switch_profile" => async {
            let profile_id = req::<i64>(body, "profileId")?;
            to_json(connection::connection_switch_profile(profile_id).await)
        }
        .await,
        "connection_url_stats" => to_json(run(connection::connection_url_stats).await),
        _ => return None,
    })
}
