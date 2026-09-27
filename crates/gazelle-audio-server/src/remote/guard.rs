//! The one gate every request passes, on every listener: who is asking, whether they may, and
//! whether a browser was tricked into asking.
//!
//! In order:
//!
//! 1. **Who.** The socket's peer address, as the listener saw it. Nothing a request says about
//!    itself (`X-Forwarded-For`, `Forwarded`, `X-Real-IP`) is read. No peer at all counts as the
//!    network. Debug builds also let a test on this machine say it is a phone (`seam`).
//! 2. **Host.** A browser can be made to send a request to `127.0.0.1` from a page anywhere, by
//!    DNS rebinding: a name the attacker controls answers first with their own address and then
//!    with `127.0.0.1`, so their page and this server look like one origin to the browser. Such a
//!    request always carries the attacker's name in `Host`. So `Host` must be `localhost`, an IP
//!    address (which no rebinding can produce, since it is not a name), or this computer's own name
//!    (with `.local` or not). Anything else is 403 `bad_host`. A request with no `Host` (not from
//!    a browser) is let through to the checks below.
//! 3. **Origin.** A page from another site can still send requests straight to `127.0.0.1`
//!    (no rebinding needed): a form post, or a WebSocket, which the browser does not hold to the
//!    same origin rule as `fetch`. Browsers name the page's origin on both, and on any `fetch`
//!    that is not a plain GET. When `Origin` is present it must be this server itself, the same
//!    host and port as `Host`; otherwise 403 `bad_origin`. `Origin: null` (a sandboxed frame, a
//!    file) is refused too. Programs that are not browsers send no `Origin`, and are not affected.
//! 4. **This machine** needs nothing more.
//! 5. **Anything else** is refused outright (403 `remote_off`) when phones are off and `--bind`
//!    did not open Gazelle to the network, whatever it carries. Otherwise the web app's files are
//!    served to anyone, and `POST /api/v1/remote/pair` may be asked without a token; everything
//!    else under `/api` needs a valid token (401 `unauthorized` without one) and, for what stays
//!    on this machine, gets 403 `not_local` even with one.
//!
//! The paths are compared lower case and with `%` escapes decoded, so no spelling of a path slips
//! past a check that the router would still route.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;

use super::{seam, Remote, COOKIE};

/// The one route under `/api` a phone may ask without a token.
pub const PAIR_PATH: &str = "/api/v1/remote/pair";

/// What stays on this machine: a phone with a token still gets 403 for these, and for anything
/// under them.
///
/// The recording settings are here too: auto-arm holds this PC's drivers and start in the hub fills
/// its screen, so they are the computer's to change, while the recorder itself is a phone's to use.
pub const LOCAL_ONLY: [&str; 5] = ["/api/v1/update", "/api/v1/window", "/api/v1/aggregate", "/api/v1/remote", "/api/v1/recording/settings"];

/// Put the gate in front of every route of `app`, and of its fallback (the web app's files).
pub fn protect(app: Router, remote: Arc<Remote>) -> Router {
    app.layer(axum::middleware::from_fn_with_state(remote, guard))
}

/// Who a phone request is, for the handlers after the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer(pub Option<SocketAddr>);

async fn guard(State(remote): State<Arc<Remote>>, mut request: Request, next: Next) -> Response {
    let socket = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(peer)| *peer);
    let peer = match (socket, seam::test_peer(request.headers())) {
        // A test on this machine may say it is elsewhere; nothing elsewhere may say it is here.
        (Some(socket), Some(said)) if is_local(Some(socket)) => Some(said),
        (socket, _) => socket,
    };
    if let Some(peer) = peer {
        // Every handler after this sees the same answer, including the ones that check for
        // themselves (`handover::allowed`).
        request.extensions_mut().insert(ConnectInfo(peer));
    }
    request.extensions_mut().insert(Peer(peer));

    let headers = request.headers();
    let host = host_of(&request);
    if let Some(host) = &host {
        if !host_allowed(host, remote.host_names()) {
            return refuse(StatusCode::FORBIDDEN, "bad_host", format!("Gazelle does not answer to the name {host:?}. Open it by this computer's address instead."));
        }
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let same = origin.to_str().ok().zip(host.as_deref()).is_some_and(|(origin, host)| same_origin(origin, host));
        if !same {
            return refuse(StatusCode::FORBIDDEN, "bad_origin", "A page from another site cannot control Gazelle.".into());
        }
    }

    if is_local(peer) {
        return next.run(request).await;
    }

    if !remote.accepts_remote() {
        return refuse(
            StatusCode::FORBIDDEN,
            "remote_off",
            "Phones are not allowed on this network. Turn on Allow phones on this network in Gazelle on the computer.".into(),
        );
    }
    let path = normalise_path(request.uri().path());
    if !is_api(&path) || path == PAIR_PATH {
        return next.run(request).await;
    }
    let from = peer.unwrap_or(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0));
    let Some(phone) = tokens(request.headers()).into_iter().find_map(|token| remote.authenticate(&token, from)) else {
        let mut response = refuse(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "This device is not paired with Gazelle. Pair it from Gazelle on the computer: the Workspace page, Phones.".into(),
        );
        response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"gazelle\""));
        return response;
    };
    if local_only(&path) {
        return refuse(StatusCode::FORBIDDEN, "not_local", "Only Gazelle on the computer itself can do that.".into());
    }
    request.extensions_mut().insert(remote.session(phone));
    next.run(request).await
}

/// An error in the shape every route uses.
pub fn refuse(status: StatusCode, code: &str, message: String) -> Response {
    (status, Json(json!({"error": {"code": code, "message": message}}))).into_response()
}

/// Whether a peer is this machine. An IPv4 address carried in IPv6 counts as itself.
pub fn is_local(peer: Option<SocketAddr>) -> bool {
    peer.is_some_and(|peer| peer.ip().to_canonical().is_loopback())
}

/// The `Host` header, or the authority of an absolute request target.
fn host_of(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string)
        .or_else(|| request.uri().authority().map(|a| a.as_str().to_string()))
}

/// Split `host[:port]` or `[v6][:port]` into the host and the port, if one is given.
fn split_authority(authority: &str) -> Option<(&str, Option<&str>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            after => Some(after.strip_prefix(':')?),
        };
        return Some((inside, port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Some((host, Some(port))),
        Some(_) => None,
        None => Some((authority, None)),
    }
}

/// Whether a `Host` value names this server: `localhost`, an IP address, or this computer's
/// name, with or without `.local`, and any port.
pub fn host_allowed(host: &str, names: &[String]) -> bool {
    let Some((name, port)) = split_authority(host.trim()) else { return false };
    if port.is_some_and(|p| p.parse::<u16>().is_err()) {
        return false;
    }
    if name.parse::<IpAddr>().is_ok() {
        return true;
    }
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    name == "localhost" || names.iter().any(|own| name == *own || name.strip_suffix(".local") == Some(own.as_str()))
}

/// `host:port`, lower case, with the scheme's default port written in.
fn with_port(authority: &str, default_port: &str) -> Option<String> {
    let (host, port) = split_authority(authority)?;
    let host = host.to_ascii_lowercase();
    let host = if host.contains(':') { format!("[{host}]") } else { host };
    Some(format!("{host}:{}", port.unwrap_or(default_port)))
}

/// Whether a request's `Origin` is this server itself.
pub fn same_origin(origin: &str, host: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else { return false };
    let default = match scheme.to_ascii_lowercase().as_str() {
        "http" => "80",
        "https" => "443",
        _ => return false,
    };
    let authority = rest.split('/').next().unwrap_or_default();
    // This server speaks plain HTTP, so its own origin is always `http`.
    default == "80" && with_port(authority, default).is_some_and(|origin| with_port(host.trim(), "80") == Some(origin))
}

/// A path lower case with its `%` escapes decoded, for comparing, never for routing.
pub fn normalise_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() && bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit() {
            let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
            out.push(hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]));
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_ascii_lowercase()
}

/// Whether a path is the API's rather than the web app's.
pub fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// Whether a path stays on this machine.
pub fn local_only(path: &str) -> bool {
    path != PAIR_PATH && LOCAL_ONLY.iter().any(|prefix| path == *prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/')))
}

/// The tokens a request offers: its bearer token, then its cookie.
pub fn tokens(headers: &HeaderMap) -> Vec<String> {
    let mut found = Vec::new();
    for value in headers.get_all(header::AUTHORIZATION) {
        if let Some((scheme, token)) = value.to_str().ok().and_then(|v| v.trim().split_once(' ')) {
            if scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty() {
                found.push(token.trim().to_string());
            }
        }
    }
    for value in headers.get_all(header::COOKIE) {
        let Ok(value) = value.to_str() else { continue };
        for pair in value.split(';') {
            if let Some((name, token)) = pair.trim().split_once('=') {
                if name.trim() == COOKIE && !token.trim().is_empty() {
                    found.push(token.trim().to_string());
                }
            }
        }
    }
    found
}

/// The `Set-Cookie` value pairing answers with: the token, readable by no script (`HttpOnly`), and
/// sent only with requests from this server's own pages (`SameSite=Strict`). Not `Secure`, since
/// the server speaks plain HTTP and a browser would not keep a secure cookie from it.
pub fn set_cookie(token: &str) -> String {
    format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}", super::COOKIE_MAX_AGE_S)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_must_be_localhost_an_address_or_this_computer() {
        let names = vec!["studio-pc".to_string()];
        for host in ["localhost", "localhost:8420", "LOCALHOST:8420", "127.0.0.1:8420", "[::1]:8420", "192.168.1.5:8420", "192.168.1.5", "studio-pc:8420", "Studio-PC.local:8420", "localhost.:8420"] {
            assert!(host_allowed(host, &names), "{host}");
        }
        for host in ["evil.example:8420", "evil.example", "127.0.0.1.nip.io:8420", "studio-pc.evil.example", "localhost:port", "", "[::1:8420", "a:b:c"] {
            assert!(!host_allowed(host, &names), "{host}");
        }
    }

    #[test]
    fn an_origin_must_be_this_server_itself() {
        assert!(same_origin("http://127.0.0.1:8420", "127.0.0.1:8420"));
        assert!(same_origin("http://192.168.1.5:8420", "192.168.1.5:8420"));
        assert!(same_origin("http://localhost", "localhost:80"));
        assert!(same_origin("http://[::1]:8420", "[::1]:8420"));
        assert!(same_origin("http://LocalHost:5173", "localhost:5173"), "the dev server's proxy keeps both as they were");
        assert!(!same_origin("http://evil.example", "127.0.0.1:8420"));
        assert!(!same_origin("http://127.0.0.1:9999", "127.0.0.1:8420"), "another port is another origin");
        assert!(!same_origin("https://127.0.0.1:8420", "127.0.0.1:8420"), "this server is plain HTTP");
        assert!(!same_origin("null", "127.0.0.1:8420"));
        assert!(!same_origin("file://", "127.0.0.1:8420"));
    }

    #[test]
    fn paths_are_compared_decoded_and_lower_case() {
        assert_eq!(normalise_path("/API/v1/%75pdate"), "/api/v1/update");
        assert_eq!(normalise_path("/%61pi/v1/ws"), "/api/v1/ws");
        assert_eq!(normalise_path("/index.html"), "/index.html");
        assert_eq!(normalise_path("/100%"), "/100%");
        assert!(is_api("/api/v1/health"));
        assert!(is_api("/api"));
        assert!(!is_api("/apiary.png"));
        assert!(!is_api("/pair"));
    }

    #[test]
    fn what_stays_on_this_machine() {
        for path in ["/api/v1/update", "/api/v1/update/restart", "/api/v1/window/show", "/api/v1/window/widget", "/api/v1/window/hub", "/api/v1/aggregate", "/api/v1/remote", "/api/v1/remote/pairing", "/api/v1/remote/phones/ab", "/api/v1/recording/settings"] {
            assert!(local_only(path), "{path}");
        }
        for path in [PAIR_PATH, "/api/v1/health", "/api/v1/ws", "/api/v1/devices", "/api/v1/updates", "/api/v1/workspace", "/api/v1/recording", "/api/v1/recording/arm", "/api/v1/recording/disarm", "/api/v1/recording/takes"] {
            assert!(!local_only(path), "{path}");
        }
    }

    #[test]
    fn a_token_comes_from_the_bearer_header_or_the_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        headers.insert(header::COOKIE, "theme=dark; gazelle_token=def ; other=1".parse().unwrap());
        assert_eq!(tokens(&headers), ["abc", "def"]);
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        headers.insert(header::COOKIE, "not_gazelle_token=x".parse().unwrap());
        assert!(tokens(&headers).is_empty());
    }

    #[test]
    fn the_cookie_is_http_only_and_same_site_strict() {
        let cookie = set_cookie("abc");
        assert!(cookie.starts_with("gazelle_token=abc;"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Path=/"));
    }

    #[test]
    fn only_loopback_is_this_machine() {
        assert!(is_local(Some("127.0.0.1:1".parse().unwrap())));
        assert!(is_local(Some("[::1]:1".parse().unwrap())));
        assert!(is_local(Some("[::ffff:127.0.0.1]:1".parse().unwrap())));
        assert!(!is_local(Some("192.168.1.5:1".parse().unwrap())));
        assert!(!is_local(None), "no peer counts as the network");
    }
}
