//! OpenAI-compatible tools (spec §9): `GET /openai/tools` returns function definitions generated
//! from the operation set, and `POST /openai/call` executes one tool call. Both require the
//! helper's bearer token, checked before the body is read.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ops::service::{Ops, OpsError};
use crate::ops::types::operations;
use crate::panel::security;

#[derive(Clone)]
pub struct AgentApp {
    pub ops: Ops,
    pub token: Arc<str>,
}

/// Function definitions in the chat-completions `tools` format, one per operation. The JSON
/// Schema's `$schema` and `title` keys are dropped; some local servers reject them.
pub fn openai_tools() -> Vec<Value> {
    operations()
        .into_iter()
        .map(|op| {
            let mut parameters = op.input_schema;
            if let Some(object) = parameters.as_object_mut() {
                object.remove("$schema");
                object.remove("title");
            }
            json!({ "type": "function", "function": { "name": op.name, "description": op.description, "parameters": parameters } })
        })
        .collect()
}

/// One tool call as a chat-completions response carries it.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    pub name: String,
    /// An object, or the JSON-encoded string chat-completions tool calls use.
    #[serde(default)]
    pub arguments: Value,
}

impl ToolCall {
    /// The arguments as a JSON value; an empty string means none.
    pub fn arguments(&self) -> Result<Value, String> {
        match &self.arguments {
            Value::String(s) if s.trim().is_empty() => Ok(Value::Null),
            Value::String(s) => serde_json::from_str(s).map_err(|e| format!("arguments are not JSON: {e}")),
            other => Ok(other.clone()),
        }
    }
}

/// Runs a tool call on a blocking thread (operations may wait, e.g. `await_progress`).
pub async fn execute(ops: &Ops, call: &ToolCall) -> Result<Value, (StatusCode, String)> {
    let arguments = call.arguments().map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let ops = ops.clone();
    let name = call.name.clone();
    match tokio::task::spawn_blocking(move || ops.call(&name, arguments)).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => {
            let status = match e {
                OpsError::UnknownOperation(_) => StatusCode::NOT_FOUND,
                OpsError::BadArguments { .. } | OpsError::Invalid(_) | OpsError::NoSession => StatusCode::BAD_REQUEST,
                _ => StatusCode::UNPROCESSABLE_ENTITY,
            };
            Err((status, e.to_string()))
        }
        Err(join) => Err((StatusCode::INTERNAL_SERVER_ERROR, format!("internal error: {join}"))),
    }
}

/// `/openai/tools` and `/openai/call`, without the Host check (merge before `with_host_check`).
pub fn router(app: AgentApp) -> Router {
    Router::new().route("/openai/tools", get(tools)).route("/openai/call", post(call)).with_state(app)
}

async fn tools(State(app): State<AgentApp>, headers: HeaderMap) -> Response {
    if !security::bearer_ok(&headers, &app.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({ "tools": openai_tools() })).into_response()
}

async fn call(State(app): State<AgentApp>, headers: HeaderMap, body: Bytes) -> Response {
    if !security::bearer_ok(&headers, &app.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let call: ToolCall = match serde_json::from_slice(&body) {
        Ok(call) => call,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("bad tool call: {e}") }))).into_response(),
    };
    match execute(&app.ops, &call).await {
        Ok(result) => Json(json!({ "name": call.name, "result": result })).into_response(),
        Err((status, error)) => (status, Json(json!({ "name": call.name, "error": error }))).into_response(),
    }
}
