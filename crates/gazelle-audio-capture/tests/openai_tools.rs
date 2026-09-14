//! OpenAI-compatible tools: generated from the operation set, behind the token, executing calls.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gazelle_audio_capture::agent::openai::{openai_tools, router, AgentApp};
use gazelle_audio_capture::ops::service::Ops;
use gazelle_audio_capture::ops::types::operations;
use gazelle_audio_capture::session::controller::Environment;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "test-token";

fn app() -> AgentApp {
    AgentApp { ops: Ops::new(Environment::default()), token: Arc::from(TOKEN) }
}

async fn send(app: &AgentApp, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(t) = token {
        request = request.header("authorization", format!("Bearer {t}"));
    }
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let response = router(app.clone()).oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[test]
fn tools_are_the_operation_set() {
    let tools = openai_tools();
    let ops = operations();
    assert_eq!(tools.len(), ops.len());
    for (tool, op) in tools.iter().zip(&ops) {
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["function"]["name"], op.name);
        assert_eq!(tool["function"]["description"], op.description);
        let mut expected = op.input_schema.clone();
        let object = expected.as_object_mut().unwrap();
        object.remove("$schema");
        object.remove("title");
        assert_eq!(tool["function"]["parameters"], expected, "{}", op.name);
        assert_eq!(tool["function"]["parameters"]["type"], "object");
    }
}

#[tokio::test]
async fn tools_and_calls_require_the_token() {
    let app = app();
    assert_eq!(send(&app, "GET", "/openai/tools", None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(send(&app, "GET", "/openai/tools", Some("wrong"), None).await.0, StatusCode::UNAUTHORIZED);
    let (status, body) = send(&app, "GET", "/openai/tools", Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tools"].as_array().unwrap().len(), operations().len());
    let unauthorised = send(&app, "POST", "/openai/call", None, Some(json!({ "name": "session_status" }))).await;
    assert_eq!(unauthorised.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn calls_run_operations_and_map_errors_to_status_codes() {
    let app = app();
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("s").display().to_string();

    let (status, body) = send(&app, "POST", "/openai/call", Some(TOKEN), Some(json!({ "name": "session_open", "arguments": { "path": session, "vid": 4660, "pid": 43981 } }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["open"], true);

    // Chat-completions carry arguments as a JSON-encoded string.
    let parameter = json!({ "parameter": { "id": "mute", "label": "Mute", "kind": "toggle", "domain": { "values": ["off", "on"] } } }).to_string();
    let (status, body) = send(&app, "POST", "/openai/call", Some(TOKEN), Some(json!({ "name": "declare_parameter", "arguments": parameter }))).await;
    assert_eq!((status, body["result"]["declared"].clone()), (StatusCode::OK, json!("mute")), "{body}");
    let (status, body) = send(&app, "POST", "/openai/call", Some(TOKEN), Some(json!({ "name": "list_parameters", "arguments": "" }))).await;
    assert_eq!((status, body["result"].as_array().map(Vec::len)), (StatusCode::OK, Some(1)));

    let unknown = send(&app, "POST", "/openai/call", Some(TOKEN), Some(json!({ "name": "mark_step_done", "arguments": {} }))).await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND);
    let bad = send(&app, "POST", "/openai/call", Some(TOKEN), Some(json!({ "name": "get_field_map", "arguments": { "parameter": 3 } }))).await;
    assert_eq!(bad.0, StatusCode::BAD_REQUEST);
    let not_json = send(&app, "POST", "/openai/call", Some(TOKEN), Some(json!({ "name": "list_parameters", "arguments": "{oops" }))).await;
    assert_eq!(not_json.0, StatusCode::BAD_REQUEST);
    let plan = json!({ "plan": { "parameter": "gain", "value_a": "0", "value_b": ["1"], "control_parameter": "mute" } });
    let refused = send(&app, "POST", "/openai/call", Some(TOKEN), Some(json!({ "name": "plan_probe", "arguments": plan }))).await;
    assert_eq!(refused.0, StatusCode::UNPROCESSABLE_ENTITY, "{}", refused.1);
    assert!(refused.1["error"].as_str().unwrap().contains("gain"), "{}", refused.1);
}
