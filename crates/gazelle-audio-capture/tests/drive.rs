//! The `drive` loop against a mock chat-completions server: scripted tool calls take a recorded
//! probe from `session_open` to `analyze_probe`, and the field map comes out right.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gazelle_audio_capture::agent::drive::{drive, DriveConfig, DriveError, DriveEvent};
use gazelle_audio_capture::capture::pipeline::PayloadPolicy;
use gazelle_audio_capture::ops::guidance::GUIDANCE;
use gazelle_audio_capture::ops::service::Ops;
use gazelle_audio_capture::ops::types::operations;
use gazelle_audio_capture::session::controller::Environment;
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, ProbePlan};
use gazelle_audio_capture::session::step::StepTiming;
use gazelle_audio_capture::synth::device::*;
use gazelle_audio_capture::synth::session::{generate_session, ScriptedOperator, SynthSpec};
use serde_json::{json, Value};

const VID: u16 = 0x1234;
const PID: u16 = 0xABCD;
const LEVELS: &[&str] = &["0 dB", "-6 dB", "-12 dB"];

fn parameters() -> Vec<Parameter> {
    vec![
        Parameter { id: "monitor_level".into(), label: "Monitor level".into(), kind: ParameterKind::Discrete, domain: ParameterDomain { values: LEVELS.iter().map(|s| s.to_string()).collect(), unit: Some("dB".into()) }, location: String::new() },
        Parameter { id: "mute".into(), label: "Mute".into(), kind: ParameterKind::Toggle, domain: ParameterDomain { values: vec!["off".into(), "on".into()], unit: None }, location: String::new() },
    ]
}

/// A recorded RichDevice probe in `<dir>/recorded`.
fn recorded_session(dir: &std::path::Path) -> std::path::PathBuf {
    let root = dir.join("recorded");
    let spec = SynthSpec {
        parameters: parameters(),
        plan: ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into(), "-12 dB".into()], sweep: vec![], repeats: 3, control_parameter: "mute".into() },
        seed: 5,
        timing: StepTiming::default(),
        start_ns: 1_700_000_000_000_000_000,
        operator: ScriptedOperator::default(),
        link_type: 249,
        policy: PayloadPolicy::default(),
    };
    let target = RichDevice::new(VID, PID, 1, 5).with_parameter("monitor_level", LEVELS).with_parameter("mute", &["off", "on"]);
    let devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(target), Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3))];
    generate_session(&root, spec, devices).unwrap();
    root
}

type Script = Arc<dyn Fn(usize, &Value) -> Response + Send + Sync>;
/// Each request's Authorization header and JSON body, in arrival order.
type Requests = Arc<Mutex<Vec<(Option<String>, Value)>>>;

#[derive(Clone)]
struct Mock {
    script: Script,
    requests: Requests,
}

async fn completions(State(mock): State<Mock>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string);
    let turn = {
        let mut requests = mock.requests.lock().unwrap();
        requests.push((auth, body.clone()));
        requests.len() - 1
    };
    (mock.script)(turn, &body)
}

/// Serves the script at `http://127.0.0.1:<port>/v1/chat/completions`.
async fn mock_server(script: Script) -> (String, Requests) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().route("/v1/chat/completions", post(completions)).with_state(Mock { script, requests: Arc::clone(&requests) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://127.0.0.1:{port}/v1"), requests)
}

/// An assistant message calling `calls` (name, arguments), arguments JSON-encoded as the API does.
fn tool_calls(turn: usize, calls: &[(&str, Value)]) -> Response {
    let calls: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(k, (name, arguments))| json!({ "id": format!("call_{turn}_{k}"), "type": "function", "function": { "name": name, "arguments": arguments.to_string() } }))
        .collect();
    Json(json!({ "choices": [{ "index": 0, "finish_reason": "tool_calls", "message": { "role": "assistant", "content": null, "tool_calls": calls } }] })).into_response()
}

fn answer(text: &str) -> Response {
    Json(json!({ "choices": [{ "index": 0, "finish_reason": "stop", "message": { "role": "assistant", "content": text } }] })).into_response()
}

fn config(base_url: &str) -> DriveConfig {
    DriveConfig { base_url: base_url.to_string(), model: "test-model".into(), api_key: Some("sk-test".into()), max_turns: 20 }
}

#[tokio::test]
async fn a_scripted_model_maps_a_recorded_probe() {
    let dir = tempfile::tempdir().unwrap();
    let capture = recorded_session(dir.path()).join("captures").join("p1.pcapng").display().to_string();
    let session = dir.path().join("session").display().to_string();
    let declared: Vec<Value> = parameters().into_iter().map(|p| json!({ "parameter": p })).collect();

    let script: Script = Arc::new(move |turn, _request| match turn {
        0 => tool_calls(turn, &[("session_open", json!({ "path": session, "vid": VID, "pid": PID }))]),
        1 => tool_calls(turn, &[("declare_parameter", declared[0].clone()), ("declare_parameter", declared[1].clone())]),
        2 => tool_calls(turn, &[("get_field_map", json!({ "parameter": "monitor_level" }))]),
        3 => tool_calls(turn, &[("import_capture", json!({ "capture": capture }))]),
        4 => tool_calls(turn, &[("analyze_probe", json!({}))]),
        _ => answer("done"),
    });
    let (base_url, requests) = mock_server(script).await;

    let ops = Ops::new(Environment::default());
    let mut events = Vec::new();
    let outcome = drive(&ops, &config(&base_url), "Map monitor_level from the recording.", &mut |e| events.push(e.clone())).await.expect("the loop reaches an answer");
    assert_eq!((outcome.answer.as_str(), outcome.tool_calls), ("done", 6));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    let (auth, first) = &requests[0];
    assert_eq!(auth.as_deref(), Some("Bearer sk-test"));
    assert_eq!(first["model"], "test-model");
    assert_eq!(first["messages"][0], json!({ "role": "system", "content": GUIDANCE }));
    assert_eq!(first["messages"][1]["content"], "Map monitor_level from the recording.");
    assert_eq!(first["tools"].as_array().unwrap().len(), operations().len());

    // Every tool call is answered, in order, before the next request.
    let last = requests[5].1["messages"].as_array().unwrap();
    let tool_messages: Vec<&Value> = last.iter().filter(|m| m["role"] == "tool").collect();
    let ids: Vec<&str> = tool_messages.iter().map(|m| m["tool_call_id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["call_0_0", "call_1_0", "call_1_1", "call_2_0", "call_3_0", "call_4_0"]);
    let early: Value = serde_json::from_str(tool_messages[3]["content"].as_str().unwrap()).unwrap();
    assert!(early["error"].as_str().unwrap().contains("run analyze_probe first"), "{early}");
    let analysed: Value = serde_json::from_str(tool_messages[5]["content"].as_str().unwrap()).unwrap();
    assert_eq!(analysed["field_map"]["command"]["field"]["byte"], json!(RICH_VALUE));
    assert_eq!(analysed["field_map"]["readback"]["field"]["byte"], json!(RICH_READBACK_BASE));
    assert_eq!(outcome.messages.len(), last.len() + 1, "the outcome adds the final answer");

    let results: Vec<(&str, bool)> = events.iter().filter_map(|e| match e { DriveEvent::ToolResult { name, ok } => Some((name.as_str(), *ok)), _ => None }).collect();
    assert_eq!(results, [("session_open", true), ("declare_parameter", true), ("declare_parameter", true), ("get_field_map", false), ("import_capture", true), ("analyze_probe", true)]);
    assert_eq!(events.last(), Some(&DriveEvent::Assistant("done".into())));

    let map = ops.call("get_field_map", json!({ "parameter": "monitor_level" })).unwrap();
    assert_eq!(map["command"]["field"]["byte"], json!(RICH_VALUE));
}

#[tokio::test]
async fn a_model_that_never_answers_hits_the_turn_limit() {
    let (base_url, requests) = mock_server(Arc::new(|turn, _| tool_calls(turn, &[("session_status", json!({}))]))).await;
    let config = DriveConfig { max_turns: 3, ..config(&base_url) };
    let result = drive(&Ops::new(Environment::default()), &config, "loop", &mut |_| {}).await;
    assert!(matches!(result, Err(DriveError::TurnLimit(3))), "{result:?}");
    assert_eq!(requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn api_errors_and_https_urls_are_reported() {
    let (base_url, _) = mock_server(Arc::new(|_, _| (StatusCode::UNAUTHORIZED, "invalid api key").into_response())).await;
    let result = drive(&Ops::new(Environment::default()), &config(&base_url), "task", &mut |_| {}).await;
    assert!(matches!(&result, Err(DriveError::Status { status: 401, body }) if body == "invalid api key"), "{result:?}");

    let result = drive(&Ops::new(Environment::default()), &config("https://api.example.com/v1"), "task", &mut |_| {}).await;
    assert!(matches!(result, Err(DriveError::Scheme(_))), "{result:?}");
}
