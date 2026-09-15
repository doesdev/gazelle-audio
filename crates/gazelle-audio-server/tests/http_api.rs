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
    app_with_themes(None)
}

fn app_with_themes(themes_dir: Option<std::path::PathBuf>) -> axum::Router {
    let registries = RegistrySet::builtin().expect("registries");
    let devices = DeviceManager::new(registries);
    devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64);
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    http::router(AppState {
        devices,
        store,
        force_dry_run: false,
        backend: "loopback".into(),
        themes_dir,
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

/// Groups carry an optional colour (the web UI colour-codes them); an unset colour is omitted,
/// and anything but `#rrggbb` is rejected without changing the stored workspace.
#[tokio::test]
async fn group_colours_round_trip_and_are_validated() {
    let app = app();
    let workspace = json!({
        "version": 1,
        "groups": [{"id": "g1", "name": "Drums", "color": "#b5473a", "children": [{"id": "g2", "name": "Kick"}]}],
        "links": [],
        "aliases": {}
    });
    let (status, body) = send(app.clone(), "PUT", "/api/v1/workspace", workspace).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = get(app.clone(), "/api/v1/workspace").await;
    assert_eq!(body["groups"][0]["color"], "#b5473a");
    assert!(body["groups"][0]["children"][0].get("color").is_none(), "an unset colour is omitted: {body}");

    let bad = json!({
        "version": 1,
        "groups": [{"id": "g1", "name": "Drums", "children": [{"id": "g2", "name": "Kick", "color": "red"}]}],
        "links": [],
        "aliases": {}
    });
    let (status, body) = send(app.clone(), "PUT", "/api/v1/workspace", bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "bad_value");
    assert!(body["error"]["message"].as_str().unwrap().contains("g2"), "{body}");
    let (_, body) = get(app, "/api/v1/workspace").await;
    assert_eq!(body["groups"][0]["color"], "#b5473a", "a rejected save changes nothing");
}

/// User themes are JSON files in the themes directory: each is listed with its parsed theme,
/// or with the reason it could not be used. A missing or unconfigured directory lists nothing.
#[tokio::test]
async fn user_themes_are_listed_from_the_themes_directory() {
    let dir = std::env::temp_dir().join(format!("gazelle-themes-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("b-graphite.json"), r##"{"name": "Graphite", "type": "dark", "colors": {"accent": "#3fb6d9"}}"##).unwrap();
    std::fs::write(dir.join("a-broken.json"), "{ not json").unwrap();
    std::fs::write(dir.join("c-list.json"), "[1, 2]").unwrap();
    std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();

    let (status, body) = get(app_with_themes(Some(dir.clone())), "/api/v1/themes").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list = body.as_array().expect("a list");
    let files: Vec<&str> = list.iter().map(|e| e["file"].as_str().unwrap()).collect();
    assert_eq!(files, ["a-broken.json", "b-graphite.json", "c-list.json"]);
    assert!(list[0].get("theme").is_none() && !list[0]["error"].as_str().unwrap().is_empty(), "{body}");
    assert_eq!(list[1]["theme"]["name"], "Graphite");
    assert!(list[1].get("error").is_none());
    assert!(list[2]["error"].as_str().unwrap().contains("object"), "{body}");

    let (_, body) = get(app_with_themes(Some(dir.join("missing"))), "/api/v1/themes").await;
    assert_eq!(body, json!([]));
    let (_, body) = get(app(), "/api/v1/themes").await;
    assert_eq!(body, json!([]));
    std::fs::remove_dir_all(&dir).unwrap();
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
            "zenstudiotb" => 43,
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

/// Studio+ ground-truth vectors, generated from the same decompiled request builder.
fn ground_truth_studio() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../gazelle-audio-protocol/tests/ground_truth_studio.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("ground_truth_studio.json")).unwrap()
}

/// Phase 4 drives each family's own mixer command: Studio+ has `set_mixer_cfg` (with `send`)
/// instead of Quadro's `set_mixer`. A dry run must emit its ground-truth bytes, and a changed
/// level must change exactly the level byte.
#[tokio::test]
async fn studio_set_mixer_cfg_sends_ground_truth_bytes() {
    let expected = ground_truth_studio()["set_mixer_cfg"].as_str().expect("set_mixer_cfg vector").to_string();
    let (status, body) = send(app(), "POST", "/api/v1/devices/loopback-1/command/set_mixer_cfg?dry_run=true", json!({})).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["sent_hex"], expected);
    assert_eq!(body["dry_run"], true);

    let (_, level) = send(app(), "POST", "/api/v1/devices/loopback-1/command/set_mixer_cfg?dry_run=true", json!({"level": 0x40})).await;
    let (a, b) = (expected.as_str(), level["sent_hex"].as_str().unwrap());
    assert_eq!(a.len(), b.len());
    let differing: Vec<usize> = (0..a.len() / 2).filter(|i| a[i * 2..i * 2 + 2] != b[i * 2..i * 2 + 2]).collect();
    // 16-byte header, payload_id|nparams, nbytes, then mixer_id, channel, level: byte 20.
    assert_eq!(differing, vec![20], "only the level byte changes: {a} vs {b}");
    assert_eq!(&b[40..42], "40");

    let (missing, body) = send(app(), "POST", "/api/v1/devices/loopback-0/command/set_mixer_cfg?dry_run=true", json!({})).await;
    assert_eq!(missing, StatusCode::NOT_FOUND, "Quadro has set_mixer, not set_mixer_cfg: {body}");
    assert_eq!(body["error"]["code"], "unknown_command");
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

/// `get_routing` names the destination group in the header's `ext3` (the panel's
/// `get_device_data` asks once per group), so a client must be able to set it per request.
/// Only commands that take an `ext3` selector accept one: on any other command it would
/// silently change what the device is asked.
#[tokio::test]
async fn ext3_selector_sets_the_header_only_where_the_command_takes_one() {
    let base = ground_truth()["get_routing"].as_str().expect("get_routing vector").to_string();
    let (status, body) = send(app(), "POST", "/api/v1/devices/loopback-0/command/get_routing?dry_run=true&ext3=9", json!({})).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let expected = format!("{}09000000", &base[..24]);
    assert_eq!(body["sent_hex"], expected, "only ext3 (header bytes 12..16) changes");

    let (status, body) = send(app(), "POST", "/api/v1/devices/loopback-1/command/get_mixer?dry_run=true&ext3=2", json!({})).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(&body["sent_hex"].as_str().unwrap()[24..32], "02000000");

    let (status, body) = send(app(), "POST", "/api/v1/devices/loopback-0/command/get_routing?dry_run=true", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sent_hex"], base, "without ext3 the schema's value is used");

    let (refused, body) = send(app(), "POST", "/api/v1/devices/loopback-0/command/set_mixer?dry_run=true&ext3=1", json!({})).await;
    assert_eq!(refused, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["error"]["code"], "bad_value");
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
        themes_dir: None,
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
    assert_eq!(total, 63 + 43);
}

/// `set_routing` accepts its 32 routing pairs as an array of pairs, producing exactly the bytes
/// of the equivalent flat hex string.
#[tokio::test]
async fn element_arrays_encode_like_flat_hex() {
    let pairs: Vec<Value> = (0..32u8).map(|i| json!([i, 31 - i])).collect();
    let flat: String = (0..32u8).map(|i| format!("{i:02x}{:02x}", 31 - i)).collect();
    let uri = "/api/v1/devices/loopback-0/command/set_routing?dry_run=true";

    let (s1, as_pairs) = send(app(), "POST", uri, json!({"bank_idx": 2, "bank_configs": pairs})).await;
    let (s2, as_hex) = send(app(), "POST", uri, json!({"bank_idx": 2, "bank_configs": flat})).await;

    assert_eq!(s1, StatusCode::OK, "{as_pairs}");
    assert_eq!(s2, StatusCode::OK, "{as_hex}");
    assert_eq!(as_pairs["sent_hex"], as_hex["sent_hex"]);
}

/// Clients narrow command types on `family`; its values match the `--loopback-models` keys.
#[tokio::test]
async fn descriptors_carry_the_model_family() {
    let (status, body) = get(app(), "/api/v1/devices").await;
    assert_eq!(status, StatusCode::OK);
    let families: Vec<Value> = body.as_array().unwrap().iter().map(|d| d["family"].clone()).collect();
    assert_eq!(families, vec![json!("quadro"), json!("studio")]);
}
