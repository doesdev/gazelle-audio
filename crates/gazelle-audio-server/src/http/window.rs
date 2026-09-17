//! The desktop window, as far as HTTP is concerned: one request, which a second launch of the
//! binary makes so the copy already running comes to the front (`crate::handover`).
//!
//! Loopback peers only. The request moves a window on someone's desk, so a server bound to every
//! interface must not let the network make it.

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::net::SocketAddr;

use crate::handover;
use crate::AppState;

/// Bring this server's window to the front, if it has one.
///
/// Always says what happened rather than failing: a headless server answers 200 with
/// `shown: false` and its reason, which is what the second launch prints before exiting.
pub async fn show(State(state): State<AppState>, request: Request) -> Response {
    // The peer as the listener saw it. Absent unless the server was served with connect info,
    // which `crate::http::serve` always does; an absent peer is refused rather than trusted.
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(peer)| *peer);
    if !handover::allowed(peer) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"shown": false, "reason": "only a program on this machine may show the window"})),
        )
            .into_response();
    }
    match &state.show_window {
        Some(show) => {
            tracing::info!("another Gazelle was started; showing this one's window");
            show();
            Json(json!({"shown": true})).into_response()
        }
        None => Json(json!({
            "shown": false,
            "reason": "this Gazelle is running without a window; open it in a browser instead",
        }))
        .into_response(),
    }
}
