//! Liveness and server identity.

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::AppState;

pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": crate::VERSION,
        "backend": state.backend,
        "devices": state.devices.len(),
        "dry_run": state.force_dry_run,
    }))
}
