//! The companion panel: one page, one WebSocket, one read-only state endpoint.

pub mod security;
mod ws;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio::net::TcpListener;

use crate::session::controller::Controller;

const PAGE: &str = include_str!("page.html");
const TOKEN_PLACEHOLDER: &str = "__GAZELLE_TOKEN__";

#[derive(Clone)]
pub struct PanelApp {
    pub controller: Controller,
    pub token: Arc<str>,
    /// The bound port; Host and Origin checks compare against it.
    pub port: u16,
}

/// Builds the panel's own routes. Deliberately does *not* apply the Host check (see
/// `with_host_check`): in axum a `layer` only covers routes added to the router before it is
/// called, so a caller that wants to merge more routes in (sub-project 3's `/mcp` and
/// `/openai/*`) must be able to do that before the Host check becomes the outermost layer.
pub fn router(app: PanelApp) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/panel/ws", get(ws::upgrade))
        .route("/api/state", get(api_state))
        .with_state(app)
}

/// Applies the Host check (spec §9) as the outermost layer over `router`, so it covers every
/// route already present — including ones a caller merged in before calling this. Callers that
/// merge additional routes onto `router`'s output must do so *before* calling this, since a
/// layer added here cannot retroactively cover routes merged in afterward.
pub fn with_host_check(router: Router, port: u16) -> Router {
    router.layer(middleware::from_fn(move |request: Request, next: Next| {
        let host = request.headers().get(header::HOST).and_then(|h| h.to_str().ok()).map(str::to_owned);
        async move {
            if !security::host_allowed(host.as_deref(), port) {
                return (StatusCode::FORBIDDEN, "host not allowed").into_response();
            }
            next.run(request).await
        }
    }))
}

pub async fn serve(listener: TcpListener, app: PanelApp) -> std::io::Result<()> {
    let port = app.port;
    axum::serve(listener, with_host_check(router(app), port)).await
}

/// Drives step timers and the packets/s indicator.
pub fn spawn_ticker(controller: Controller, period: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        loop {
            interval.tick().await;
            if let Err(e) = controller.tick() {
                tracing::warn!(error = %e, "tick failed");
            }
        }
    })
}

/// Accepted exception to spec §9 (plan amendments, Task 12): this route is loadable without the
/// bearer token, since the token has to reach the browser somehow before the page's own script
/// can use it. It stays 127.0.0.1-only and passes the Host/Origin checks like every other route;
/// every other HTTP and WS call still requires the bearer token.
async fn page(State(app): State<PanelApp>) -> Html<String> {
    Html(PAGE.replace(TOKEN_PLACEHOLDER, &app.token))
}

async fn api_state(State(app): State<PanelApp>, headers: HeaderMap) -> Response {
    if !security::bearer_ok(&headers, &app.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(app.controller.state()).into_response()
}
