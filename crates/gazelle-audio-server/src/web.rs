//! The embedded web UI.
//!
//! Mounted as the router's fallback, so every API route keeps priority. Unknown `/api/...`
//! paths stay JSON 404s rather than returning the app shell, and extensionless paths serve
//! `index.html` so client-side routes survive a reload.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$GAZELLE_WEB_DIST"]
struct Assets;

/// Add the web UI to an API router.
pub fn with_ui(api: Router) -> Router {
    api.fallback(serve)
}

async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        let body = serde_json::json!({
            "error": {"code": "not_found", "message": format!("no API route {}", uri.path())}
        });
        return (StatusCode::NOT_FOUND, axum::Json(body)).into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(res) = asset(path) {
        return res;
    }
    let last_segment = path.rsplit('/').next().unwrap_or_default();
    if !last_segment.contains('.') {
        if let Some(res) = asset("index.html") {
            return res;
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

fn asset(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = file.metadata.mimetype().to_string();
    Some(([(header::CONTENT_TYPE, mime)], Body::from(file.data.into_owned())).into_response())
}
