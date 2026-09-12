//! Command introspection and invocation.
//!
//! Both are driven entirely by the recovered registry, so the whole in-scope surface works
//! with no per-command Rust code: adding or correcting a command is a schema change.

use antelope_protocol::field::Field;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value as Json2};

use crate::device::descriptor::DeviceId;
use crate::error::ServerError;
use crate::value::{fields_to_json, json_to_payload_values, to_hex};
use crate::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct InvokeQuery {
    /// Return the bytes that would be sent without sending them.
    #[serde(default)]
    pub dry_run: bool,
}

/// Describe one field for the introspection endpoint.
fn field_json(f: &Field) -> Json2 {
    json!({
        "name": f.name(),
        "size": f.size(),
    })
}

fn command_json(c: &antelope_protocol::Command) -> Json2 {
    json!({
        "name": c.name,
        "report_id": format!("0x{:X}", c.report_id),
        "ext2": c.ext2,
        "ext3": c.ext3,
        "payload_id": c.payload_id,
        "auto_send_notification": c.auto_send_notification,
        "params": c.params.iter().map(field_json).collect::<Vec<_>>(),
        "returns": c.returns.iter().map(field_json).collect::<Vec<_>>(),
    })
}

/// Every command available on a given device, with its wire schema.
pub async fn device_commands(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Json2>, ServerError> {
    let id = DeviceId(id);
    let descriptor = state.devices.descriptor(&id)?;
    let model = state
        .devices
        .registries()
        .for_pid(descriptor.pid)
        .ok_or_else(|| ServerError::NoRegistry(id.to_string()))?;

    let mut names: Vec<&String> = model.registry.names().collect();
    names.sort();
    let commands: Vec<Json2> = names
        .iter()
        .filter_map(|n| model.registry.get(n))
        .map(command_json)
        .collect();

    Ok(Json(json!({
        "device_id": id,
        "slug": model.slug,
        "model": model.model,
        "count": commands.len(),
        "commands": commands,
    })))
}

/// Every registry the server knows, for clients discovering the surface up front.
pub async fn all_commands(State(state): State<AppState>) -> Json<Json2> {
    let mut models = Vec::new();
    for (pid, m) in state.devices.registries().models() {
        let mut names: Vec<&String> = m.registry.names().collect();
        names.sort();
        models.push(json!({
            "pid": format!("0x{pid:04x}"),
            "slug": m.slug,
            "model": m.model,
            "count": names.len(),
            "commands": names,
        }));
    }
    Json(json!({ "models": models }))
}

/// Invoke a command on a device.
pub async fn invoke(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
    Query(q): Query<InvokeQuery>,
    body: Option<Json<Json2>>,
) -> Result<Json<Json2>, ServerError> {
    let id = DeviceId(id);
    let handle = state.devices.handle(&id)?;

    let body = body.map(|Json(b)| b).unwrap_or(Json2::Null);
    let values = json_to_payload_values(&body)?;

    // A server-wide dry run cannot be overridden per request: the safe setting wins.
    let dry_run = q.dry_run || state.force_dry_run;

    let outcome = handle.request(&name, values, dry_run).await?;

    Ok(Json(json!({
        "device_id": id,
        "command": name,
        "sent_hex": to_hex(&outcome.sent),
        "sent_len": outcome.sent.len(),
        "dry_run": outcome.dry_run,
        "response": outcome.response.as_ref().map(fields_to_json),
        "response_error": outcome.response_error,
    })))
}
