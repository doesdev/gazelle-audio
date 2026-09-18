//! The server serves the web UI at `/` without shadowing the API.
//!
//! Runs against whatever is embedded: the real build when `web/apps/web/dist` exists, the
//! "not built" notice page otherwise. Both are HTML documents.
#![cfg(feature = "web-ui")]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use gazelle_audio_server::device::manager::DeviceManager;
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
        store,
        force_dry_run: false,
        backend: "loopback".into(),
        themes_dir: None,
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

/// The UI is more than one file: an entry chunk, its stylesheet, the fonts, and the chunk the
/// Effects page fetches when it opens. Every file the build wrote must be embedded and served at
/// its own path, byte for byte, or the app loads and then fails to fetch part of itself.
#[tokio::test]
async fn every_built_file_is_embedded_and_served_at_its_own_path() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/apps/web/dist");
    if !dist.join("index.html").is_file() {
        // Nothing is built, so the notice page is embedded instead (see this file's header).
        let (status, _, body) = fetch("/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Web UI not built"), "{body}");
        return;
    }
    let mut files = Vec::new();
    collect(&dist, &dist, &mut files);
    assert!(files.len() > 2, "the build wrote more than one file: {files:?}");
    let scripts = files.iter().filter(|(path, _)| path.ends_with(".js")).count();
    assert!(scripts >= 2, "the app is split into chunks, not one script: {files:?}");

    for (path, bytes) in &files {
        let res = app()
            .oneshot(Request::builder().uri(format!("/{path}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "GET /{path}");
        let ctype = res.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
        let served = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(served.as_ref(), bytes.as_slice(), "GET /{path} served other bytes than the build wrote");
        let expected = match path.rsplit('.').next() {
            Some("js") => "text/javascript",
            Some("css") => "text/css",
            Some("html") => "text/html",
            Some("woff2") => "font/woff2",
            _ => "",
        };
        assert!(ctype.starts_with(expected), "GET /{path}: {ctype}");
    }
}

/// Every file under `dist`, by its path relative to it, with the bytes the build wrote.
fn collect(dist: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).expect("read dist") {
        let path = entry.expect("dist entry").path();
        if path.is_dir() {
            collect(dist, &path, out);
        } else {
            let relative = path.strip_prefix(dist).expect("under dist").to_string_lossy().replace('\\', "/");
            out.push((relative, std::fs::read(&path).expect("read asset")));
        }
    }
}
