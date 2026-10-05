//! The browser event bridge: core broadcasts (`env::emit`) land in a tokio
//! broadcast channel through the installed [`WebSink`], and every connected
//! browser drinks them from the `/api/events` SSE stream — the same named
//! events, the same JSON payloads, as the Tauri event bus.

use std::convert::Infallible;

use axum::extract::State;
use axum::response::Sse;
use axum::response::sse::{Event as SseEvent, KeepAlive};
use serde_json::Value;
use tokio::sync::broadcast::Sender;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{StreamExt as _, Stream};

use megathrone_core::env::EventSink;

/// One broadcast: the event name and its JSON payload.
pub type EventFrame = (String, Value);

/// How many broadcasts a slow client may fall behind before frames drop
/// (each payload is a fresh snapshot or an idempotent progress report, so
/// a gap heals on the next frame).
pub(crate) const CHANNEL_CAPACITY: usize = 256;

/// The web shell's [`EventSink`]: pushes every core broadcast into the
/// channel every SSE connection subscribes to.
pub struct WebSink {
    tx: Sender<EventFrame>,
}

impl WebSink {
    pub fn new(tx: Sender<EventFrame>) -> Self {
        Self { tx }
    }
}

impl EventSink for WebSink {
    fn emit_json(&self, event: &str, payload: Value) {
        let _ = self.tx.send((event.to_string(), payload));
    }
}

/// `GET /api/events`: the SSE stream of every backend broadcast.
pub async fn sse(
    State(tx): State<Sender<EventFrame>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(tx.subscribe()).filter_map(|frame| {
        match frame {
            Ok((event, payload)) => {
                Some(Ok(SseEvent::default().event(event).data(payload.to_string())))
            }
            // the client lagged past `CHANNEL_CAPACITY` — skip what it
            // missed instead of killing the stream
            Err(_lagged) => None,
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
