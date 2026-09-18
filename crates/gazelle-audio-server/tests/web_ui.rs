//! The server serves the web UI at `/` without shadowing the API.
//!
//! Runs against whatever is embedded: the real build when `web/apps/web/dist` exists, the
//! "not built" notice page otherwise. Both are HTML documents.
#![cfg(feature = "web-ui")]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::snapshot::store::MemorySnapshotStore;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO};
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, web, AppState};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

fn app() -> axum::Router {
    let devices = DeviceManager::new(RegistrySet::builtin().expect("registries"));
    devices.attach_loopbacks(&[PID_QUADRO], 64);
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    web::with_ui(http::router(AppState {
        devices,
        snapshots: Arc::new(MemorySnapshotStore::default()),
        store,
        force_dry_run: false,
        backend: "loopback".into(),
        themes_dir: None,
        show_window: None,
    }))
}

async fn fetch(uri: &str) -> (StatusCode, String, String) {
    let res = app()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ctype = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let body = String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).into_owned();
    (status, ctype, body)
}

#[tokio::test]
async fn root_serves_an_html_document() {
    let (status, ctype, body) = fetch("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ctype.starts_with("text/html"), "{ctype}");
    assert!(body.to_lowercase().contains("<html"), "{body}");
}

#[tokio::test]
async fn api_routes_still_answer() {
    let (status, ctype, _) = fetch("/api/v1/health").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ctype.starts_with("application/json"), "{ctype}");
}

#[tokio::test]
async fn unknown_api_paths_are_json_404s_not_the_app() {
    let (status, ctype, body) = fetch("/api/v1/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(ctype.starts_with("application/json"), "{ctype}");
    assert!(body.contains("not_found"), "{body}");
}

#[tokio::test]
async fn extensionless_paths_fall_back_to_the_app_shell() {
    let (status, ctype, _) = fetch("/devices/loopback-0").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ctype.starts_with("text/html"), "{ctype}");
}

#[tokio::test]
async fn missing_assets_are_404() {
    let (status, _, _) = fetch("/assets/missing.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
