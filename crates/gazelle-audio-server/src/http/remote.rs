//! Phones over HTTP (`crate::remote`).
//!
//! - `GET /api/v1/remote`: the setting, whether a phone can reach Gazelle now, the addresses it
//!   would use, the paired phones, and the pairing running, if any.
//! - `PUT /api/v1/remote`: `{"allow_phones": true}` or `false`. Acted on at once: the listener on
//!   every interface starts or stops without a restart.
//! - `POST /api/v1/remote/pairing`: start pairing. Answers the code, when it expires, the pairing
//!   addresses and a QR code of the first. `DELETE` stops it.
//! - `POST /api/v1/remote/pair`: `{"code", "name"}`, from the phone. Exchanges a valid code for a
//!   token, which comes back in the body (for an app) and as a cookie (for a browser).
//! - `DELETE /api/v1/remote/phones/{id}`: revoke a phone.
//!
//! Every route but `pair` answers only this machine. The gate (`remote::guard`) already refuses
//! them to anything else; each handler checks again, so a router put together without the gate
//! still does not hand them out.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Request};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::handover;
use crate::remote::guard::{refuse, set_cookie};
use crate::remote::{PairRefusal, Remote};

pub fn routes(remote: Arc<Remote>) -> Router {
    Router::new()
        .route("/api/v1/remote", get(status).put(set))
        .route("/api/v1/remote/pairing", post(start_pairing).delete(cancel_pairing))
        .route("/api/v1/remote/pair", post(pair))
        .route("/api/v1/remote/phones/{id}", delete(revoke))
        .layer(Extension(remote))
}

fn peer(request: &Request) -> Option<SocketAddr> {
    request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(peer)| *peer)
}

fn refuse_unless_local(request: &Request) -> Option<Response> {
    (!handover::allowed(peer(request))).then(|| refuse(StatusCode::FORBIDDEN, "not_local", "Only Gazelle on the computer itself can do that.".into()))
}

async fn status(Extension(remote): Extension<Arc<Remote>>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    Json(remote.status()).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Setting {
    allow_phones: bool,
}

async fn set(Extension(remote): Extension<Arc<Remote>>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    let body = <Json<Setting> as axum::extract::FromRequest<()>>::from_request(request, &()).await;
    let Json(setting) = match body {
        Ok(body) => body,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, "bad_request", e.body_text()),
    };
    // Binding may take a moment, and the tray may be doing the same from its own thread.
    let result = tokio::task::spawn_blocking({
        let remote = remote.clone();
        move || remote.set_allow_phones(setting.allow_phones)
    })
    .await
    .unwrap_or_else(|e| Err(format!("changing the setting stopped part way: {e}")));
    match result {
        Ok(()) => Json(remote.status()).into_response(),
        Err(why) if remote.fixed_by_bind().is_some() => refuse(StatusCode::CONFLICT, "fixed_by_bind", why),
        Err(why) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", why),
    }
}

async fn start_pairing(Extension(remote): Extension<Arc<Remote>>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    match remote.start_pairing() {
        Ok(pairing) => no_store(Json(pairing).into_response()),
        Err(why) => refuse(StatusCode::CONFLICT, "phones_off", why),
    }
}

async fn cancel_pairing(Extension(remote): Extension<Arc<Remote>>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    Json(json!({"cancelled": remote.cancel_pairing()})).into_response()
}

#[derive(Deserialize)]
struct Pair {
    code: String,
    #[serde(default)]
    name: String,
}

async fn pair(Extension(remote): Extension<Arc<Remote>>, request: Request) -> Response {
    let from = peer(&request).map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |p| p.ip());
    let body = <Json<Pair> as axum::extract::FromRequest<()>>::from_request(request, &()).await;
    let Json(body) = match body {
        Ok(body) => body,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, "bad_request", e.body_text()),
    };
    match remote.pair(&body.code, &body.name, from) {
        Ok(paired) => {
            let mut response = no_store(Json(json!({"token": paired.token, "phone": paired.phone})).into_response());
            if let Ok(cookie) = HeaderValue::from_str(&set_cookie(&paired.token)) {
                response.headers_mut().insert(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(PairRefusal::TooMany) => refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            "Too many wrong codes lately. Wait a minute, then try again.".into(),
        ),
        Err(PairRefusal::Refused) => refuse(
            StatusCode::FORBIDDEN,
            "pairing_refused",
            "That code is not valid now. It may be mistyped, used or expired: start pairing again on the computer.".into(),
        ),
        Err(PairRefusal::Full) => refuse(
            StatusCode::CONFLICT,
            "too_many_phones",
            format!("{} phones are paired already. Revoke one on the computer first.", crate::remote::PHONE_LIMIT),
        ),
        Err(PairRefusal::Failed(why)) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", why),
    }
}

async fn revoke(Extension(remote): Extension<Arc<Remote>>, Path(id): Path<String>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    match remote.revoke(&id) {
        Ok(true) => Json(json!({"revoked": true})).into_response(),
        Ok(false) => refuse(StatusCode::NOT_FOUND, "unknown_phone", format!("no paired phone {id}")),
        Err(why) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", why),
    }
}

/// A token or a code must not be kept by anything between here and the page.
fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
