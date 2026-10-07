//! The command bridge: `POST /api/<command>` routes to the same core
//! command functions the Tauri IPC registry serves — identical command
//! names, identical camelCase arguments, identical JSON payloads. Each
//! domain module owns its slice of the surface; this module owns the HTTP
//! shape (JSON in, `T` or `{"error": "..."}` out).

mod connection;
mod discovery;
mod dpi;
mod dpi_test;
mod latency;
mod process;
mod profiles;
mod routing;
mod settings;
mod sites;

use axum::Json;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Extracts a required argument (camelCase key, matching the Tauri IPC
/// convention the frontend already uses).
///
/// Arg errors carry these prefixes so the HTTP layer can tell a malformed
/// request (400) from a command failure (500).
pub(super) const ARG_MISSING_PREFIX: &str = "missing required argument ";
pub(super) const ARG_INVALID_PREFIX: &str = "invalid argument ";

pub(super) fn req<T: DeserializeOwned>(body: &Value, key: &str) -> Result<T, String> {
    opt(body, key)?.ok_or_else(|| format!("{ARG_MISSING_PREFIX}`{key}`"))
}

/// Extracts an optional argument: a missing key and an explicit `null`
/// both mean `None` (like the Tauri IPC).
pub(super) fn opt<T: DeserializeOwned>(body: &Value, key: &str) -> Result<Option<T>, String> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| format!("{ARG_INVALID_PREFIX}`{key}`: {error}")),
    }
}

/// Runs a synchronous core command off the async worker (the Tauri IPC
/// does the same for non-async commands).
pub(super) async fn run<T, F>(job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|error| format!("command task failed: {error}"))?
}

/// Serializes a command's Ok payload into the JSON response value.
pub(super) fn to_json<T: serde::Serialize>(result: Result<T, String>) -> Result<Value, String> {
    result.and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
}

/// The HTTP handler: one route for every command, the command name in the
/// path. An empty body counts as no arguments.
pub async fn command(Path(command): Path<String>, body: Bytes) -> Response {
    let args: Value = if body.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("invalid JSON body: {error}"),
                );
            }
        }
    };
    match dispatch(&command, &args).await {
        Some(Ok(value)) => Json(value).into_response(),
        Some(Err(error)) => {
            let status = if error.starts_with(ARG_MISSING_PREFIX) || error.starts_with(ARG_INVALID_PREFIX) {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            error_response(status, error)
        }
        None => error_response(StatusCode::NOT_FOUND, format!("unknown command: {command}")),
    }
}

async fn dispatch(name: &str, body: &Value) -> Option<Result<Value, String>> {
    if let Some(result) = profiles::dispatch(name, body).await {
        return Some(result);
    }
    if let Some(result) = latency::dispatch(name, body).await {
        return Some(result);
    }
    if let Some(result) = connection::dispatch(name, body).await {
        return Some(result);
    }
    if let Some(result) = settings::dispatch(name, body).await {
        return Some(result);
    }
    if let Some(result) = dpi::dispatch(name, body).await {
        return Some(result);
    }
    if let Some(result) = dpi_test::dispatch(name, body).await {
        return Some(result);
    }
    if let Some(result) = sites::dispatch(name, body).await {
        return Some(result);
    }
    if let Some(result) = routing::dispatch(name, body).await {
        return Some(result);
    }
    if let Some(result) = discovery::dispatch(name, body).await {
        return Some(result);
    }
    process::dispatch(name, body).await
}

fn error_response(status: StatusCode, message: String) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn required_arguments_must_be_present() {
        let body = json!({ "other": 1 });
        assert_eq!(
            req::<i64>(&body, "profileId").unwrap_err(),
            "missing required argument `profileId`"
        );
    }

    #[test]
    fn optional_arguments_accept_missing_and_null() {
        let body = json!({ "name": null });
        assert_eq!(opt::<String>(&body, "name").unwrap(), None);
        assert_eq!(opt::<String>(&json!({}), "name").unwrap(), None);
        assert_eq!(
            opt::<String>(&json!({ "name": 5 }), "name").unwrap_err(),
            "invalid argument `name`: invalid type: integer `5`, expected a string"
        );
    }

    #[test]
    fn to_json_maps_serialization_failures_into_command_errors() {
        #[derive(serde::Serialize)]
        struct Fails {
            #[serde(skip_serializing)]
            _skipped: (),
            value: i32,
        }
        // () cannot fail in practice — assert the happy path shape instead
        assert_eq!(to_json(Ok(Fails { _skipped: (), value: 7 })).unwrap(), json!({ "value": 7 }));
        assert_eq!(to_json::<()>(Err("boom".to_string())).unwrap_err(), "boom");
    }
}
