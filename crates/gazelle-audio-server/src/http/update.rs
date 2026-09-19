//! What the web UI can ask about updates. Four routes, and only on a loopback bind.
//!
//! These are merged into the router by `main.rs`, and only when the listener is on a loopback
//! address: an update check, a download and a restart are the machine's own business, not
//! something a server exposed to a network should offer. On any other bind the routes are simply
//! not there, and `GET /api/v1/health` still names the running version.
//!
//! Checking and downloading are blocking (the updater speaks plain `ureq`), so both run on the
//! runtime's blocking pool and the request waits for the answer. Two downloads at once are
//! serialised by the updater itself, so a second POST is not a second copy of the file.
//!
//! # Answering before going away
//!
//! `POST /api/v1/update/restart` has to reach the browser before the process it is about to stop
//! does stop, or the page sees a dropped connection and cannot tell a restart from a crash. So
//! the handler does no restarting itself: it reads whether something is staged (that is the only
//! thing it can refuse for), returns, and leaves a task behind that waits [`RESTART_AFTER`] and
//! then asks [`Restart::request`], the one path a restart ever takes. By then the response has
//! been written and the page is already watching for the server to come back.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::json;

use crate::update::{Restart, Status, Updater, NOTHING_STAGED};

/// How long the restart waits after the handler has returned, so the answer is on the wire before
/// the server starts shutting down. Long enough for a loopback write, short enough that the page
/// is still showing "Reconnecting..." rather than wondering.
pub const RESTART_AFTER: Duration = Duration::from_millis(300);

/// The update routes, carrying the restart (and through it the updater) as an extension so
/// `AppState` stays as it was and every other caller of `http::router` is untouched.
pub fn routes(restart: Arc<Restart>) -> Router {
    Router::new()
        .route("/api/v1/update", get(status))
        .route("/api/v1/update/check", post(check))
        .route("/api/v1/update/download", post(download))
        .route("/api/v1/update/restart", post(restart_now))
        .layer(Extension(restart))
}

/// What is known now. Never asks the release source, so a page may poll it.
async fn status(Extension(restart): Extension<Arc<Restart>>) -> Json<Status> {
    Json(restart.updater().status())
}

/// Ask the release source. One request; whether what it finds is then fetched is the
/// `auto_download` setting's business, and by default it is.
async fn check(Extension(restart): Extension<Arc<Restart>>) -> Json<Status> {
    Json(on_the_blocking_pool(restart.updater().clone(), |updater| {
        updater.check(true);
    })
    .await)
}

/// Fetch and verify the release the last check found, and stage it for the next start. Nothing
/// is restarted; the answer says whether it is staged or why it is not.
async fn download(Extension(restart): Extension<Arc<Restart>>) -> Json<Status> {
    Json(on_the_blocking_pool(restart.updater().clone(), |updater| {
        updater.download();
    })
    .await)
}

/// Stop the server and start the staged binary. Refused, with nothing done, when nothing is
/// staged; otherwise the answer goes out first and the restart follows [`RESTART_AFTER`] later.
async fn restart_now(Extension(restart): Extension<Arc<Restart>>) -> Response {
    let Some(version) = restart.staged() else {
        let body = json!({"error": {"code": "nothing_staged", "message": NOTHING_STAGED}});
        return (StatusCode::CONFLICT, Json(body)).into_response();
    };
    tokio::spawn(async move {
        tokio::time::sleep(RESTART_AFTER).await;
        // Staged a moment ago and not now would mean something else took it away; the one restart
        // path decides, not this handler's earlier reading.
        if let Err(why) = restart.request() {
            tracing::warn!("the restart asked for over HTTP did not happen: {why}");
        }
    });
    Json(json!({"restarting": true, "version": version})).into_response()
}

/// The updater blocks; the runtime's workers must not. Runs `work` off the reactor and answers
/// with the status it leaves behind.
async fn on_the_blocking_pool(updater: Arc<Updater>, work: impl FnOnce(&Updater) + Send + 'static) -> Status {
    tokio::task::spawn_blocking(move || {
        work(&updater);
        updater.status()
    })
    .await
    .unwrap_or_else(|e| panic!("the update call panicked: {e}"))
}
