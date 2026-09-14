//! Panel HTTP surface and a scripted fake operator driving the WebSocket.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use gazelle_audio_capture::capture::import::MemorySource;
use gazelle_audio_capture::panel::security::{self, generate_token};
use gazelle_audio_capture::panel::{router, serve, with_host_check, PanelApp};
use gazelle_audio_capture::session::clock::ManualClock;
use gazelle_audio_capture::session::controller::Controller;
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, ProbePlan};
use gazelle_audio_capture::session::step::StepTiming;
use gazelle_audio_capture::session::store::{SessionInfo, SessionStore};
use gazelle_audio_capture::synth::device::{DeviceModel, SimpleDevice};
use gazelle_audio_capture::synth::frames::device_frames;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tower::ServiceExt;

const S: u64 = 1_000_000_000;
const T0: u64 = 1_700_000_000 * S;

struct Harness {
    controller: Controller,
    clock: Arc<ManualClock>,
    app: PanelApp,
    _dir: tempfile::TempDir,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::create(dir.path(), &SessionInfo { vid: 0x1234, pid: 0xABCD, ..SessionInfo::default() }).unwrap();
    store
        .declare_parameter(Parameter { id: "monitor_level".into(), label: "Monitor level".into(), kind: ParameterKind::Continuous, domain: ParameterDomain::default(), location: String::new() })
        .unwrap();
    store
        .declare_parameter(Parameter { id: "mute".into(), label: "Mute".into(), kind: ParameterKind::Toggle, domain: ParameterDomain::default(), location: String::new() })
        .unwrap();
    let clock = Arc::new(ManualClock::new(T0));
    let controller = Controller::new(store, clock.clone(), StepTiming::default(), Some(false)).unwrap();
    let plan = ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into()], sweep: vec![], repeats: 1, control_parameter: "mute".into() };
    controller.plan_probe(plan, 3).unwrap();
    let mut devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(SimpleDevice::new(0x1234, 0xABCD, 1, 5))];
    controller.start_probe("p1", Box::new(MemorySource::new("memory", device_frames(&mut devices, T0, S)))).unwrap();
    let app = PanelApp { controller: controller.clone(), token: generate_token().into(), port: 0 };
    Harness { controller, clock, app, _dir: dir }
}

/// Requests go through `with_host_check` the same way `serve` applies it, so these tests
/// exercise the Host check as it actually runs in production, not just `router` alone.
async fn get(app: &PanelApp, uri: &str, host: &str, bearer: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::get(uri).header("host", host);
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let guarded = with_host_check(router(app.clone()), app.port);
    let resp = guarded.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn page_embeds_the_token_and_host_is_checked() {
    let mut h = harness().await;
    h.app.port = 8430;
    let (status, body) = get(&h.app, "/", "127.0.0.1:8430", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(&*h.app.token));
    assert!(!body.contains("__GAZELLE_TOKEN__"));
    for id in ["id=\"done\"", "id=\"redo\"", "id=\"skip\"", "id=\"actual-value\"", "id=\"rate\"", "id=\"elevation\"", "id=\"progress\""] {
        assert!(body.contains(id), "{id}");
    }
    assert_eq!(get(&h.app, "/", "localhost:8430", None).await.0, StatusCode::OK);
    assert_eq!(get(&h.app, "/", "attacker.example:8430", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get(&h.app, "/", "127.0.0.1:9999", None).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn state_endpoint_requires_the_bearer_token() {
    let mut h = harness().await;
    h.app.port = 8430;
    assert_eq!(get(&h.app, "/api/state", "127.0.0.1:8430", None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(get(&h.app, "/api/state", "127.0.0.1:8430", Some("wrong")).await.0, StatusCode::UNAUTHORIZED);
    let token = h.app.token.clone();
    let (status, body) = get(&h.app, "/api/state", "127.0.0.1:8430", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    let state: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(state["probe"]["probe_id"], "p1");
    assert_eq!(state["elevated"], false);
}

/// Plan amendments, Task 12: `router` no longer carries the Host layer, since in axum a layer
/// only covers routes added before it — a caller (sub-project 3) merges `/mcp` and `/openai/*`
/// onto `router`'s output before applying `with_host_check` as the outermost layer. This proves
/// that placement actually protects routes merged in ahead of it, not just the panel's own.
#[tokio::test]
async fn host_check_applies_to_routes_merged_before_it() {
    let mut h = harness().await;
    h.app.port = 8430;
    let extra = axum::Router::new().route("/extra", axum::routing::get(|| async { "ok" }));
    let combined = extra.merge(router(h.app.clone()));
    let guarded = with_host_check(combined, h.app.port);

    let bad = Request::get("/extra").header("host", "attacker.example:8430").body(Body::empty()).unwrap();
    let resp = guarded.clone().oneshot(bad).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let good = Request::get("/extra").header("host", "127.0.0.1:8430").body(Body::empty()).unwrap();
    let resp = guarded.oneshot(good).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[test]
fn security_helpers() {
    assert!(security::host_allowed(Some("127.0.0.1:1"), 1));
    assert!(!security::host_allowed(None, 1));
    assert!(security::origin_allowed(None, 1));
    assert!(security::origin_allowed(Some("http://localhost:1"), 1));
    assert!(!security::origin_allowed(Some("http://evil.example"), 1));
    assert!(!security::constant_time_eq(b"abc", b"abd"));
    assert_eq!(generate_token().len(), 64);
    assert_ne!(generate_token(), generate_token());
    let env = |k: &str| match k {
        "LOCALAPPDATA" => Some(r"C:\Users\op\AppData\Local".to_string()),
        _ => None,
    };
    assert!(security::token_path(env).unwrap().ends_with("capture-token"));
    let home = |k: &str| (k == "HOME").then(|| "/home/op".to_string());
    assert_eq!(security::token_path(home).unwrap(), std::path::PathBuf::from("/home/op/.local/state/gazelle/capture-token"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gazelle/capture-token");
    security::write_token(&path, "abc").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn live(mut h: Harness) -> (Harness, u16) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    h.app.port = port;
    tokio::spawn(serve(listener, h.app.clone()));
    (h, port)
}

async fn next_message(ws: &mut Ws) -> Value {
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("message within 5 s").unwrap().unwrap();
    match msg {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("unexpected {other:?}"),
    }
}

/// Reads until a state satisfies `pred`; returns it.
async fn wait_state(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    loop {
        let m = next_message(ws).await;
        if m["type"] == "state" && pred(&m["state"]) {
            return m["state"].clone();
        }
    }
}

async fn wait_error(ws: &mut Ws) -> String {
    loop {
        let m = next_message(ws).await;
        if m["type"] == "error" {
            return m["message"].as_str().unwrap().to_string();
        }
    }
}

async fn op(ws: &mut Ws, command: Value) {
    ws.send(Message::Text(command.to_string())).await.unwrap();
}

#[tokio::test]
async fn websocket_rejects_bad_token_and_foreign_origin() {
    let (h, port) = live(harness().await).await;
    let bad = format!("ws://127.0.0.1:{port}/panel/ws?token=nope");
    match tokio_tungstenite::connect_async(bad).await {
        Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 401),
        other => panic!("expected 401, got {other:?}"),
    }
    let mut req = format!("ws://127.0.0.1:{port}/panel/ws?token={}", h.app.token).into_client_request().unwrap();
    req.headers_mut().insert("origin", "http://evil.example".parse().unwrap());
    match tokio_tungstenite::connect_async(req).await {
        Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 403),
        other => panic!("expected 403, got {other:?}"),
    }
    let mut req = format!("ws://127.0.0.1:{port}/panel/ws?token={}", h.app.token).into_client_request().unwrap();
    req.headers_mut().insert("origin", format!("http://127.0.0.1:{port}").parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_ok());
}

#[tokio::test]
async fn scripted_operator_drives_a_probe_over_the_websocket() {
    let (h, port) = live(harness().await).await;
    let url = format!("ws://127.0.0.1:{port}/panel/ws?token={}", h.app.token);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let tick = |advance: u64| {
        h.clock.advance(advance);
        h.controller.tick().unwrap();
    };

    let s = wait_state(&mut ws, |s| s["probe"]["step_index"] == 0).await;
    assert_eq!(s["probe"]["kind"], "idle");
    op(&mut ws, json!({"op": "done"})).await;
    assert_eq!(wait_error(&mut ws).await, "idle steps finish on their own");

    tick(6_500_000_000);
    tick(1_500_000_000);
    let s = wait_state(&mut ws, |s| s["probe"]["step_index"] == 1).await;
    assert_eq!(s["probe"]["instruction"], "Set **Monitor level** to **0 dB**, then press Done");
    op(&mut ws, json!({"op": "done"})).await;
    assert!(wait_error(&mut ws).await.starts_with("pre-roll still running"));

    tick(1_500_000_000);
    op(&mut ws, json!({"op": "done"})).await;
    wait_state(&mut ws, |s| s["probe"]["step_state"] == "done").await;
    op(&mut ws, json!({"op": "redo"})).await;
    wait_state(&mut ws, |s| s["probe"]["attempt"] == 1 && s["probe"]["step_state"] == "armed").await;
    op(&mut ws, json!({"op": "skip", "reason": " "})).await;
    assert!(wait_error(&mut ws).await.contains("needs a reason"));
    op(&mut ws, json!({"op": "mark_done_for_agent"})).await;
    assert!(wait_error(&mut ws).await.starts_with("bad command"));
    op(&mut ws, json!({"op": "actual_value", "value": "-0.5 dB"})).await;
    wait_state(&mut ws, |s| s["probe"]["actual_value"] == "-0.5 dB").await;

    // Plan amendments, Task 12: don't compare `step_index` on possibly-stale queued states —
    // the websocket can have several published states queued up before the client reads them,
    // so a match on a captured `step_index` can be satisfied by a message that predates the
    // Done just sent. Instead wait for a state whose `seq` is strictly newer than the one on
    // which Done was sent, and send Done at most once per step.
    loop {
        let state = h.controller.state();
        let probe = state.probe.unwrap();
        if probe.status != gazelle_audio_capture::session::step::RunStatus::Running {
            break;
        }
        if probe.step_state == "armed" && probe.kind != gazelle_audio_capture::session::plan::StepKind::Idle {
            tick(1_500_000_000);
            let before = h.controller.state().seq;
            op(&mut ws, json!({"op": "done"})).await;
            wait_state(&mut ws, |s| s["seq"].as_u64().expect("seq is a u64") > before).await;
        } else {
            tick(6_500_000_000);
        }
        tick(1_500_000_000);
    }
    let s = wait_state(&mut ws, |s| s["probe"]["status"] != "running").await;
    assert_eq!(s["probe"]["status"], "completed");
}
