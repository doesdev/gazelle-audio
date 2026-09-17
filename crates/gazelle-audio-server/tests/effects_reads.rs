//! Effect chain reads (`specs/2026-09-17-effects-and-reverb.md`).
//!
//! The Quadro panel reads each effect chain on its own with `get_afx_strip_order`, naming the
//! chain in the header's `ext3` (`AfxModelController.get_device_data`), and never asks the Quadro
//! for `get_afx_order`. So a client must be able to set that selector, as it does for
//! `get_routing` and `get_mixer`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn app() -> axum::Router {
    let devices = DeviceManager::new(RegistrySet::builtin().expect("registries"));
    devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64);
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    http::router(AppState { devices, store, force_dry_run: false, backend: "loopback".into(), themes_dir: None })
}

async fn post(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(json!({}).to_string())).unwrap();
    let res = app.oneshot(request).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_quadro_effect_chain_is_read_by_its_index_in_ext3() {
    let (status, body) = post(app(), "/api/v1/devices/loopback-0/command/get_afx_strip_order?dry_run=true&ext3=5").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let hex = body["sent_hex"].as_str().expect("sent_hex");
    assert_eq!(&hex[24..32], "05000000", "the chain index goes in header bytes 12..16");

    // Not dry run: the loopback answers one chain of eight empty slots, whichever chain is named.
    let (status, body) = post(app(), "/api/v1/devices/loopback-0/command/get_afx_strip_order?ext3=3").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let slots = body["response"]["entries"][0]["slots"].as_array().expect("one chain's slots");
    assert_eq!(slots.len(), 8);
    assert!(slots.iter().all(|s| s["type"] == 0 && s["inst"] == 0), "an unset chain is empty: {slots:?}");
}

#[tokio::test]
async fn the_whole_chain_table_still_takes_no_selector() {
    // `get_afx_order` reads every chain at once; nothing selects within it.
    let (status, body) = post(app(), "/api/v1/devices/loopback-1/command/get_afx_order?dry_run=true&ext3=1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["error"]["code"], "bad_value");
}
