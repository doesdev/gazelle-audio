//! What the web UI can ask about updates. Three routes, and only on a loopback bind.
//!
//! These are merged into the router by `main.rs`, and only when the listener is on a loopback
//! address: an update check and a download are the machine's own business, not something a
//! server exposed to a network should offer. On any other bind the routes are simply not there,
//! and `GET /api/v1/health` still names the running version.
//!
//! Checking and downloading are blocking (the updater speaks plain `ureq`), so both run on the
//! runtime's blocking pool and the request waits for the answer. Two downloads at once are
//! serialised by the updater itself, so a second POST is not a second copy of the file.

use std::sync::Arc;

use axum::routing::{get, post};
use axum::{Extension, Json, Router};

use crate::update::{Status, Updater};

/// The update routes, carrying the updater as an extension so `AppState` stays as it was and
/// every other caller of `http::router` is untouched.
pub fn routes(updater: Arc<Updater>) -> Router {
    Router::new()
        .route("/api/v1/update", get(status))
        .route("/api/v1/update/check", post(check))
        .route("/api/v1/update/download", post(download))
        .layer(Extension(updater))
}

/// What is known now. Never asks the release source, so a page may poll it.
async fn status(Extension(updater): Extension<Arc<Updater>>) -> Json<Status> {
    Json(updater.status())
}

/// Ask the release source. One request, and nothing is downloaded.
async fn check(Extension(updater): Extension<Arc<Updater>>) -> Json<Status> {
    let status = tokio::task::spawn_blocking(move || {
        updater.check(true);
        updater.status()
    })
    .await
    .unwrap_or_else(|e| panic!("the update check panicked: {e}"));
    Json(status)
}

/// Fetch and verify the release the last check found, and stage it for the next start. Nothing
/// is restarted; the answer says whether it is staged or why it is not.
async fn download(Extension(updater): Extension<Arc<Updater>>) -> Json<Status> {
    let status = tokio::task::spawn_blocking(move || {
        updater.download();
        updater.status()
    })
    .await
    .unwrap_or_else(|e| panic!("the update download panicked: {e}"));
    Json(status)
}
