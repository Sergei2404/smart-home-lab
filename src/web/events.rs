use crate::lab::{Event, Lab};
use axum::{
    Json,
    extract::State,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::{
    Stream, StreamExt,
    wrappers::{BroadcastStream, errors::BroadcastStreamRecvError},
};
use tokio_util::sync::CancellationToken;

/// Events are sent as default `message`s; a subscriber that fell behind gets `resync` and reloads everything.
pub async fn stream(
    State(lab): State<Arc<Lab>>,
    State(shutdown): State<CancellationToken>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let events = BroadcastStream::new(lab.subscribe()).map(|item| {
        Ok(match item {
            Ok(event) => SseEvent::default()
                .json_data(&event)
                .unwrap_or_else(|_| resync()),
            Err(BroadcastStreamRecvError::Lagged(_)) => resync(),
        })
    });
    // The stream never ends by itself; without this the server could not shut down gracefully.
    let events = futures_util::StreamExt::take_until(events, shutdown.cancelled_owned());
    Sse::new(events).keep_alive(KeepAlive::default())
}

pub async fn status(State(lab): State<Arc<Lab>>) -> Json<Value> {
    Json(json!({ "mqtt": lab.mqtt_state(), "project": lab.project() }))
}

pub async fn log(State(lab): State<Arc<Lab>>) -> Json<Vec<Event>> {
    Json(lab.log())
}

fn resync() -> SseEvent {
    SseEvent::default().event("resync").data("")
}
