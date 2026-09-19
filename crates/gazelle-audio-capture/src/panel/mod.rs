//! The companion panel: one page, one WebSocket, one read-only state endpoint.

pub mod security;
mod ws;

pub use ws::MAX_MESSAGE_BYTES;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
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

/// Applies the Host check as the outermost layer over `router`, so it covers every
/// route already present, including ones a caller merged in before calling this. Callers that
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
        // A missed tick (e.g. while the tick below is running on a blocking thread) should not
        // fire a burst of catch-up ticks once it returns.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let controller = controller.clone();
            // `Controller::tick` can join the capture thread while holding the controller's
            // inner lock, on the tick that finishes a probe. Run it on a blocking thread so a
            // capture source that is slow to stop cannot stall this task (or, on a
            // current-thread runtime, every other task).
            match tokio::task::spawn_blocking(move || controller.tick()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "tick failed"),
                Err(e) => tracing::warn!(error = %e, "tick task panicked"),
            }
        }
    })
}

/// A deliberate exception to the token rule: this route is loadable without the
/// bearer token, since the token has to reach the browser somehow before the page's own script
/// can use it. It still passes the Host check (`with_host_check`, applied by `serve`) and, like
/// the WS upgrade, rejects a present-but-foreign `Origin` while allowing an absent one
/// (`security::origin_allowed`); every other HTTP and WS call still requires the bearer token.
///
/// Final review F1: a same-origin `GET /` carries no `Origin` at all when it is the top-level
/// navigation of an attacker's iframe, so the Origin check above cannot stop that framing (the
/// framed page then opens its own WS with its own, allowed, Origin). `frame-ancestors 'none'` /
/// `X-Frame-Options: DENY` refuse the framing itself, and `Cache-Control: no-store` keeps the
/// token-bearing HTML out of the disk cache.
async fn page(State(app): State<PanelApp>, headers: HeaderMap) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok());
    if !security::origin_allowed(origin, app.port) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = Html(PAGE.replace(TOKEN_PLACEHOLDER, &app.token)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("frame-ancestors 'none'"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn api_state(State(app): State<PanelApp>, headers: HeaderMap) -> Response {
    if !security::bearer_ok(&headers, &app.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(app.controller.state()).into_response()
}
