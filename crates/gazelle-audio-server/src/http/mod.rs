//! HTTP surface.

pub mod aggregate;
pub mod commands;
pub mod devices;
pub mod driver;
pub mod health;
pub mod recall;
pub mod snapshots;
pub mod themes;
pub mod update;
pub mod window;
pub mod workspace;

use axum::routing::{get, post};
use axum::Router;

use crate::AppState;

/// Build the full router. All routes live under `/api/v1`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health::health))
        .route("/api/v1/devices", get(devices::list_devices))
        .route("/api/v1/devices/{id}", get(devices::get_device))
        .route("/api/v1/devices/{id}/commands", get(commands::device_commands))
        .route(
            "/api/v1/devices/{id}/command/{name}",
            post(commands::invoke),
        )
        .route("/api/v1/commands", get(commands::all_commands))
        .route(
            "/api/v1/workspace",
            get(workspace::get_workspace).put(workspace::put_workspace),
        )
        .route(
            "/api/v1/snapshots",
            get(snapshots::list_snapshots).post(snapshots::create_snapshot),
        )
        .route("/api/v1/snapshots/import", post(snapshots::import_snapshots))
        .route(
            "/api/v1/snapshots/{id}",
            get(snapshots::get_snapshot).patch(snapshots::rename_snapshot).delete(snapshots::delete_snapshot),
        )
        .route("/api/v1/snapshots/{id}/compare", get(snapshots::compare_snapshot))
        // Recall: a plan is a description and sends nothing; the apply route is the seam, and it
        // refuses unless --enable-recall and the request body both say otherwise.
        .route("/api/v1/snapshots/{id}/recall/plan", post(recall::plan_recall))
        .route("/api/v1/snapshots/{id}/recall", post(recall::apply_recall))
        .route("/api/v1/themes", get(themes::list_themes))
        .route("/api/v1/window/show", post(window::show))
        .route("/api/v1/ws", get(crate::ws::ws_handler))
        .with_state(state)
}

/// Serve until the shutdown signal, with each connection's peer address available to handlers.
///
/// Every server in the project goes through this, so `/api/v1/window/show` can always see who is
/// asking: without the peer it refuses, which would otherwise be a silent, confusing refusal of a
/// handover from the same machine.
pub async fn serve(listener: tokio::net::TcpListener, app: Router) -> std::io::Result<()> {
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await
}

/// As [`serve`], stopping when `shutdown` resolves.
pub async fn serve_with_shutdown(
    listener: tokio::net::TcpListener,
    app: Router,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(shutdown)
        .await
}
