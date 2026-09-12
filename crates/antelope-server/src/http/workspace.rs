//! Workspace read and replace.

use axum::extract::State;
use axum::Json;

use crate::error::ServerError;
use crate::workspace::model::Workspace;
use crate::AppState;

pub async fn get_workspace(State(state): State<AppState>) -> Result<Json<Workspace>, ServerError> {
    Ok(Json(state.store.load()?))
}

pub async fn put_workspace(
    State(state): State<AppState>,
    Json(workspace): Json<Workspace>,
) -> Result<Json<Workspace>, ServerError> {
    state.store.save(&workspace)?;
    Ok(Json(workspace))
}
