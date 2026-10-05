//! The sidecar command slice (core: `process.rs`).

use serde_json::Value;

use megathrone_core::process;

use super::to_json;

pub(super) async fn dispatch(name: &str, _body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "sing_box_version" => to_json(process::sing_box_version().await),
        _ => return None,
    })
}
