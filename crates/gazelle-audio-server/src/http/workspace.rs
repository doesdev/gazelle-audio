//! Workspace read and replace.

use axum::extract::State;
use axum::Json;

use crate::error::ServerError;
use crate::workspace::model::{Group, Workspace};
use crate::AppState;

pub async fn get_workspace(State(state): State<AppState>) -> Result<Json<Workspace>, ServerError> {
    Ok(Json(state.store.load()?))
}

pub async fn put_workspace(
    State(state): State<AppState>,
    Json(workspace): Json<Workspace>,
) -> Result<Json<Workspace>, ServerError> {
    check_colours(&workspace.groups)?;
    state.store.save(&workspace)?;
    Ok(Json(workspace))
}

/// Every group colour, nested groups included, must be `#rrggbb`.
fn check_colours(groups: &[Group]) -> Result<(), ServerError> {
    for group in groups {
        if let Some(colour) = &group.color {
            let valid = colour.len() == 7 && colour.starts_with('#') && colour[1..].bytes().all(|b| b.is_ascii_hexdigit());
            if !valid {
                return Err(ServerError::BadValue(format!("group '{}': color must be #rrggbb, got {colour:?}", group.id)));
            }
        }
        check_colours(&group.children)?;
    }
    Ok(())
}
