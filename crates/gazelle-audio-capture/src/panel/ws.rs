//! Panel WebSocket: pushes every published state; accepts operator commands.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};

use super::{security, PanelApp};
use crate::session::authority::OperatorAuthority;
use crate::session::controller::{Controller, PanelState};
use crate::session::step::OperatorCommand;

/// Largest WebSocket message or frame the panel accepts. Operator commands are a few hundred
/// bytes; axum's default of 64 MiB would let any local client make the helper buffer that much.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
pub struct WsQuery {
    token: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage<'a> {
    State { state: &'a PanelState },
    Error { message: String },
}

pub async fn upgrade(State(app): State<PanelApp>, Query(query): Query<WsQuery>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if !security::constant_time_eq(query.token.as_bytes(), app.token.as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let origin = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok());
    if !security::origin_allowed(origin, app.port) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(MAX_MESSAGE_BYTES).max_frame_size(MAX_MESSAGE_BYTES).on_upgrade(move |socket| run(socket, app.controller))
}

async fn send(sink: &mut SplitSink<WebSocket, Message>, message: &ServerMessage<'_>) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).expect("server messages serialise");
    sink.send(Message::Text(text.into())).await
}

async fn run(socket: WebSocket, controller: Controller) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = controller.subscribe();
    let initial = rx.borrow_and_update().clone();
    if send(&mut sink, &ServerMessage::State { state: &initial }).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let state = rx.borrow_and_update().clone();
                if send(&mut sink, &ServerMessage::State { state: &state }).await.is_err() {
                    break;
                }
            }
            message = stream.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    let failure = match serde_json::from_str::<OperatorCommand>(text.as_str()) {
                        Ok(command) => {
                            let controller = controller.clone();
                            // `Controller::operator` can join the capture thread while holding
                            // the controller's inner lock, on the command that finishes the
                            // probe. Run it on a blocking thread so a capture source that is
                            // slow to stop cannot stall this connection's task (or, on a
                            // current-thread runtime, every other task sharing it).
                            match tokio::task::spawn_blocking(move || controller.operator(&OperatorAuthority::grant(), command)).await {
                                Ok(result) => result.err().map(|e| e.to_string()),
                                Err(join_error) => Some(format!("internal error: {join_error}")),
                            }
                        }
                        Err(e) => Some(format!("bad command: {e}")),
                    };
                    if let Some(message) = failure {
                        if send(&mut sink, &ServerMessage::Error { message }).await.is_err() {
                            break;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}
