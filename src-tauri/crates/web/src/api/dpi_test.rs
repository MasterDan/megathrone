//! The DPI strategy-test command slice (core: `dpi_test/`).

use serde_json::Value;

use megathrone_core::dpi_test;

use super::{req, run, to_json};

pub(super) async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "dpi_test_strategies" => to_json(dpi_test::dpi_test_strategies().await),
        "dpi_test_status" => to_json(run(dpi_test::dpi_test_status).await),
        "dpi_cancel_test" => to_json(run(dpi_test::dpi_cancel_test).await),
        "dpi_strategy_urls" => async {
            let strategy_id = req::<i64>(body, "strategyId")?;
            to_json(run(move || dpi_test::dpi_strategy_urls(strategy_id)).await)
        }
        .await,
        _ => return None,
    })
}
