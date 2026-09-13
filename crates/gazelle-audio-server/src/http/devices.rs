//! Device enumeration.

use axum::extract::{Path, State};
use axum::Json;

use crate::device::descriptor::{DeviceDescriptor, DeviceId};
use crate::error::ServerError;
use crate::AppState;

pub async fn list_devices(State(state): State<AppState>) -> Json<Vec<DeviceDescriptor>> {
    Json(state.devices.descriptors())
}

pub async fn get_device(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DeviceDescriptor>, ServerError> {
    Ok(Json(state.devices.descriptor(&DeviceId(id))?))
}
