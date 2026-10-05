//! The general-settings command slice (core: `settings/`).

use serde_json::Value;

use megathrone_core::settings;

use super::{req, run, to_json};

pub(super) async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "settings_get_general" => to_json(run(settings::settings_get_general).await),
        "settings_set_cadences" => async {
            let rotation_minutes = req::<i64>(body, "rotationMinutes")?;
            let recheck_minutes = req::<i64>(body, "recheckMinutes")?;
            let min_availability_percent = req::<i64>(body, "minAvailabilityPercent")?;
            to_json(run(move || {
                settings::settings_set_cadences(
                    rotation_minutes,
                    recheck_minutes,
                    min_availability_percent,
                )
            })
            .await)
        }
        .await,
        "settings_set_close_to_tray" => async {
            let close_to_tray = req::<bool>(body, "closeToTray")?;
            to_json(run(move || settings::settings_set_close_to_tray(close_to_tray)).await)
        }
        .await,
        "settings_set_raw_proxy" => async {
            let enabled = req::<bool>(body, "enabled")?;
            let port = req::<i64>(body, "port")?;
            to_json(settings::settings_set_raw_proxy(enabled, port).await)
        }
        .await,
        _ => return None,
    })
}
