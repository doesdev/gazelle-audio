//! HTTP surface.

pub mod commands;
pub mod devices;
pub mod health;
pub mod themes;
pub mod workspace;

use axum::routing::{get, post};
use axum::Router;

use crate::AppState;

/// Build the full router. All routes live under `/api/v1`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health::health))
        .route("/api/v1/devices", get(devices::list_devices))
        .route("/api/v1/devices/{id}", get(devices::get_device))
        .route("/api/v1/devices/{id}/commands", get(commands::device_commands))
        .route(
            "/api/v1/devices/{id}/command/{name}",
            post(commands::invoke),
        )
        .route("/api/v1/commands", get(commands::all_commands))
        .route(
            "/api/v1/workspace",
            get(workspace::get_workspace).put(workspace::put_workspace),
        )
        .route("/api/v1/themes", get(themes::list_themes))
        .route("/api/v1/ws", get(crate::ws::ws_handler))
        .with_state(state)
}
