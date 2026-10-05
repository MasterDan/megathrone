//! The DPI strategy command slice (core: `dpi/`).

use serde_json::Value;

use megathrone_core::dpi;

use super::{req, run, to_json};

pub(super) async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "dpi_list_strategies" => to_json(run(dpi::dpi_list_strategies).await),
        "dpi_add_strategy" => async {
            let name = req::<String>(body, "name")?;
            let args = req::<String>(body, "args")?;
            to_json(run(move || dpi::dpi_add_strategy(name, args)).await)
        }
        .await,
        "dpi_update_strategy" => async {
            let id = req::<i64>(body, "id")?;
            let name = req::<String>(body, "name")?;
            let args = req::<String>(body, "args")?;
            to_json(dpi::dpi_update_strategy(id, name, args).await)
        }
        .await,
        "dpi_delete_strategy" => async {
            let id = req::<i64>(body, "id")?;
            to_json(dpi::dpi_delete_strategy(id).await)
        }
        .await,
        "dpi_select_strategy" => async {
            let id = req::<i64>(body, "id")?;
            to_json(dpi::dpi_select_strategy(id).await)
        }
        .await,
        "dpi_get_settings" => to_json(run(dpi::dpi_get_settings).await),
        "dpi_set_port" => async {
            let port = req::<u16>(body, "port")?;
            to_json(dpi::dpi_set_port(port).await)
        }
        .await,
        "dpi_set_enabled" => async {
            let enabled = req::<bool>(body, "enabled")?;
            to_json(dpi::dpi_set_enabled(enabled).await)
        }
        .await,
        _ => return None,
    })
}
