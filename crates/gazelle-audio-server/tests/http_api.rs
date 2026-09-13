//! HTTP surface tests, driven against loopback devices so no hardware is involved.
//!
//! The key assertion is that a command's `sent_hex` matches the ground-truth vector the
//! protocol crate is validated against — proving the server sends the same bytes the
//! byte-level tests prove correct, rather than merely returning 200.

use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn app() -> axum::Router {
    let registries = RegistrySet::builtin().expect("registries");
    let devices = DeviceManager::new(registries);
    devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64);
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    http::router(AppState {
        devices,
        store,
        force_dry_run: false,
        backend: "loopback".into(),
    })
}

async fn get(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let res = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn send(app: axum::Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let res = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// Ground-truth vectors, the same file the protocol crate asserts against.
fn ground_truth() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../gazelle-audio-protocol/tests/ground_truth.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("ground_truth.json")).unwrap()
}

#[tokio::test]
async fn health_reports_backend_and_device_count() {
    let (status, body) = get(app(), "/api/v1/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["backend"], "loopback");
    assert_eq!(body["devices"], 2);
}

#[tokio::test]
async fn devices_enumerate_with_their_models() {
    let (status, body) = get(app(), "/api/v1/devices").await;
    assert_eq!(status, StatusCode::OK);
    let devices = body.as_array().expect("array");
    assert_eq!(devices.len(), 2);

    let slugs: Vec<&str> = devices.iter().map(|d| d["slug"].as_str().unwrap()).collect();
    assert!(slugs.contains(&"zenquadrosc_usb2"));
    assert!(slugs.contains(&"zenstudiotb"));

    // Command counts differ by model: this is the multi-device property in one assertion.
    for d in devices {
        let expected = match d["slug"].as_str().unwrap() {
            "zenquadrosc_usb2" => 63,
            "zenstudiotb" => 35,
            other => panic!("unexpected slug {other}"),
        };
        assert_eq!(d["command_count"], expected, "for {}", d["slug"]);
    }
}

#[tokio::test]
async fn unknown_device_is_404_with_a_code() {
    let (status, body) = get(app(), "/api/v1/devices/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "unknown_device");
}

#[tokio::test]
async fn device_commands_are_introspectable() {
    let (status, body) = get(app(), "/api/v1/devices/loopback-0/commands").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 63);
    assert_eq!(body["slug"], "zenquadrosc_usb2");

    let set_mixer = body["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "set_mixer")
        .expect("set_mixer present");
    assert_eq!(set_mixer["report_id"], "0x70");
    assert_eq!(set_mixer["payload_id"], 20);
    // mixer_id, channel, level, pan, mute, solo
    assert_eq!(set_mixer["params"].as_array().unwrap().len(), 6);
}

#[tokio::test]
async fn invoking_a_command_sends_ground_truth_bytes() {
    let gt = ground_truth();
    let expected = gt["set_mixer"].as_str().expect("set_mixer vector");

    let (status, body) = send(
        app(),
        "POST",
        "/api/v1/devices/loopback-0/command/set_mixer?dry_run=true",
        json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["sent_hex"], expected,
        "server must emit the same bytes the protocol crate is validated against"
    );
    assert_eq!(body["dry_run"], true);
    assert!(body["response"].is_null(), "dry run must not fabricate a response");
}

#[tokio::test]
async fn field_values_change_the_bytes() {
    let (_, a) = send(
        app(),
        "POST",
        "/api/v1/devices/loopback-0/command/set_mixer?dry_run=true",
        json!({"level": 64}),
    )
    .await;
    let (_, b) = send(
        app(),
        "POST",
        "/api/v1/devices/loopback-0/command/set_mixer?dry_run=true",
        json!({"level": 65}),
    )
    .await;
    assert_ne!(a["sent_hex"], b["sent_hex"], "a changed field must change the wire bytes");
}

#[tokio::test]
async fn quadro_only_command_is_absent_on_studio() {
    // set_mixer exists on Quadro (loopback-0) but not Studio+ (loopback-1).
    let (ok, _) = send(
        app(),
        "POST",
        "/api/v1/devices/loopback-0/command/set_mixer?dry_run=true",
        json!({}),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);

    let (missing, body) = send(
        app(),
        "POST",
        "/api/v1/devices/loopback-1/command/set_mixer?dry_run=true",
        json!({}),
    )
    .await;
    assert_eq!(missing, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "unknown_command");
}

#[tokio::test]
async fn shared_command_works_on_both_devices() {
    for id in ["loopback-0", "loopback-1"] {
        let (status, body) = send(
            app(),
            "POST",
            &format!("/api/v1/devices/{id}/command/set_routing?dry_run=true"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "set_routing on {id}: {body}");
    }
}

#[tokio::test]
async fn bad_field_value_is_rejected_with_guidance() {
    let (status, body) = send(
        app(),
        "POST",
        "/api/v1/devices/loopback-0/command/set_mixer?dry_run=true",
        json!({"level": null}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_value");
    assert!(body["error"]["message"].as_str().unwrap().contains("omit the field"));
}

#[tokio::test]
async fn server_wide_dry_run_cannot_be_overridden() {
    let registries = RegistrySet::builtin().unwrap();
    let devices = DeviceManager::new(registries);
    devices.attach_loopbacks(&[PID_QUADRO], 64);
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    let app = http::router(AppState {
        devices,
        store,
        force_dry_run: true,
        backend: "loopback".into(),
    });

    // Explicitly asking for dry_run=false must NOT defeat the server-wide safety setting.
    let (status, body) = send(
        app,
        "POST",
        "/api/v1/devices/loopback-0/command/set_mixer?dry_run=false",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dry_run"], true, "the safe setting must win");
}

#[tokio::test]
async fn workspace_roundtrips() {
    let app = app();
    let (status, body) = get(app.clone(), "/api/v1/workspace").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], 1);
    assert!(body["groups"].as_array().unwrap().is_empty());

    let ws = json!({
        "version": 1,
        "groups": [{
            "id": "g1", "name": "Drums", "collapsed": true, "hidden": false,
            "members": [{"device_id": "loopback-0", "channel": 3}],
            "children": []
        }],
        "links": [],
        "aliases": {"loopback-0": "Main Rig"}
    });
    let (status, saved) = send(app.clone(), "PUT", "/api/v1/workspace", ws).await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let (_, back) = get(app, "/api/v1/workspace").await;
    assert_eq!(back["groups"][0]["name"], "Drums");
    assert_eq!(back["groups"][0]["members"][0]["channel"], 3);
    assert_eq!(back["aliases"]["loopback-0"], "Main Rig");
}

#[tokio::test]
async fn all_commands_lists_every_model() {
    let (status, body) = get(app(), "/api/v1/commands").await;
    assert_eq!(status, StatusCode::OK);
    let models = body["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    let total: usize = models.iter().map(|m| m["count"].as_u64().unwrap() as usize).sum();
    assert_eq!(total, 63 + 35);
}
