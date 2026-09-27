//! Phones on the network, through the gate every request passes (`remote::guard`).
//!
//! No test here listens on anything but 127.0.0.1. Where a request comes from is the socket's peer
//! address, so a phone is played by setting that address: on a request sent through `oneshot`, by
//! inserting it as the listener would; on a real WebSocket, by a layer in front of the gate that
//! rewrites the peer of a connection that really came from this machine.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::http::{header, HeaderMap, StatusCode};
use axum::Router;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO};
use gazelle_audio_server::remote::addresses::LanAddress;
use gazelle_audio_server::remote::store::Backing;
use gazelle_audio_server::remote::{guard, Exposure, Options, Remote};
use gazelle_audio_server::snapshot::store::MemorySnapshotStore;
use gazelle_audio_server::update::settings::Settings;
use gazelle_audio_server::update::{Restart, Updater, TARGET};
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};

const HERE: &str = "127.0.0.1:50000";
const PHONE: &str = "192.168.1.50:51000";
const HOST: &str = "127.0.0.1:8420";

struct Server {
    app: Router,
    remote: Arc<Remote>,
    /// How many times the restart route asked the server to stop.
    stops: Arc<AtomicUsize>,
}

fn remote_with(backing: Backing, exposure: Exposure) -> Arc<Remote> {
    let clock = Arc::new(AtomicU64::new(1_789_700_000_000));
    let (remote, warning) = Remote::new(Options {
        backing,
        exposure,
        port: 8420,
        phone_listen: "127.0.0.1:0".parse().unwrap(),
        clock: Arc::new(move || clock.load(Ordering::SeqCst)),
        addresses: Arc::new(|| vec![LanAddress { ip: "192.168.1.5".parse().unwrap(), primary: true }]),
        host_names: vec!["studio-pc".into()],
    });
    assert_eq!(warning, None);
    remote
}

/// The app as `main.rs` builds it: the API, the updater's routes, the phones' routes, the web
/// app's files, and the gate in front of all of it.
fn server_with(remote: Arc<Remote>) -> Server {
    let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
    devices.attach_loopbacks(&[PID_QUADRO], 64);
    let state = AppState {
        devices,
        store: Arc::new(MemoryStore::default()) as Arc<dyn WorkspaceStore>,
        snapshots: Arc::new(MemorySnapshotStore::default()),
        force_dry_run: true,
        enable_recall: false,
        backend: "loopback".into(),
        themes_dir: None,
        show_window: None,
    };
    let settings = Settings { api_base: "http://127.0.0.1:9".into(), ..Settings::default() };
    let updater = Updater::new(settings, PathBuf::from("gazelle-audio-server.exe"), semver::Version::new(1, 0, 0), TARGET.into(), None);
    let stops = Arc::new(AtomicUsize::new(0));
    let counted = stops.clone();
    let restart = Arc::new(Restart::new(Arc::new(updater), Box::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    })));
    let app = http::router(state).merge(http::update::routes(restart)).merge(http::remote::routes(remote.clone()));
    #[cfg(feature = "web-ui")]
    let app = gazelle_audio_server::web::with_ui(app);
    Server { app: guard::protect(app, remote.clone()), remote, stops }
}

/// As if started with `--bind 0.0.0.0:8420`: phones can reach it without starting a listener.
fn server() -> Server {
    server_with(remote_with(Backing::Memory, Exposure::Everywhere("0.0.0.0:8420".parse().unwrap())))
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn send(app: &Router, method: &str, uri: &str, from: &str, headers: &[(&str, &str)], body: Option<Value>) -> Answer {
    let mut request = Request::builder().method(method).uri(uri);
    if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("host")) {
        request = request.header(header::HOST, HOST);
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let mut request = request.body(body.map_or(Body::empty(), |b| Body::from(b.to_string()))).unwrap();
    request.extensions_mut().insert(ConnectInfo(from.parse::<SocketAddr>().unwrap()));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Answer { status, headers, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null) }
}

async fn get(app: &Router, uri: &str, from: &str, headers: &[(&str, &str)]) -> Answer {
    send(app, "GET", uri, from, headers, None).await
}

/// Pair a phone the way the page and the phone do: the code started here, the pair from there.
async fn pair(s: &Server, name: &str) -> (String, String, Answer) {
    let started = send(&s.app, "POST", "/api/v1/remote/pairing", HERE, &[], None).await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let code = started.body["code"].as_str().unwrap().to_string();
    let paired = send(&s.app, "POST", "/api/v1/remote/pair", PHONE, &[], Some(json!({"code": code, "name": name}))).await;
    assert_eq!(paired.status, StatusCode::OK, "{}", paired.body);
    let token = paired.body["token"].as_str().unwrap().to_string();
    let id = paired.body["phone"]["id"].as_str().unwrap().to_string();
    (token, id, paired)
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

#[tokio::test]
async fn this_machine_needs_no_token() {
    let s = server();
    for uri in ["/api/v1/health", "/api/v1/devices", "/api/v1/workspace", "/api/v1/remote", "/api/v1/update"] {
        let answer = get(&s.app, uri, HERE, &[]).await;
        assert_eq!(answer.status, StatusCode::OK, "{uri}: {}", answer.body);
    }
    // IPv6 loopback too.
    assert_eq!(get(&s.app, "/api/v1/devices", "[::1]:50000", &[("host", "[::1]:8420")]).await.status, StatusCode::OK);
}

#[tokio::test]
async fn another_machine_without_a_token_gets_401_in_the_apis_own_shape() {
    let s = server();
    for uri in ["/api/v1/health", "/api/v1/devices", "/api/v1/workspace", "/api/v1/ws", "/api/v1/nothing-here", "/API/v1/devices", "/%61pi/v1/devices"] {
        let answer = get(&s.app, uri, PHONE, &[]).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(answer.body["error"]["code"], "unauthorized", "{uri}");
        assert!(answer.body["error"]["message"].as_str().unwrap().contains("not paired"));
        assert_eq!(answer.headers[header::WWW_AUTHENTICATE], "Bearer realm=\"gazelle\"");
    }
}

#[cfg(feature = "web-ui")]
#[tokio::test]
async fn the_web_apps_own_files_need_no_token() {
    let s = server();
    for uri in ["/", "/pair"] {
        let answer = get(&s.app, uri, PHONE, &[]).await;
        assert_eq!(answer.status, StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn a_header_never_changes_who_is_asking() {
    let s = server();
    for (name, value) in [("x-forwarded-for", "127.0.0.1"), ("x-real-ip", "127.0.0.1"), ("forwarded", "for=127.0.0.1"), ("x-gazelle-test-peer", "127.0.0.1")] {
        let answer = get(&s.app, "/api/v1/devices", PHONE, &[(name, value)]).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{name}");
    }
}

#[tokio::test]
async fn pairing_gives_a_token_that_works_by_header_and_by_cookie() {
    let s = server();
    let (token, _, paired) = pair(&s, "Pixel").await;
    assert_eq!(paired.body["phone"]["name"], "Pixel");
    let cookie = paired.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with(&format!("gazelle_token={token};")), "{cookie}");
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"), "{cookie}");
    assert_eq!(paired.headers[header::CACHE_CONTROL], "no-store");

    let by_header = get(&s.app, "/api/v1/devices", PHONE, &[("authorization", &bearer(&token))]).await;
    assert_eq!(by_header.status, StatusCode::OK, "{}", by_header.body);
    let by_cookie = get(&s.app, "/api/v1/devices", PHONE, &[("cookie", &format!("theme=x; gazelle_token={token}"))]).await;
    assert_eq!(by_cookie.status, StatusCode::OK);
    let wrong = get(&s.app, "/api/v1/devices", PHONE, &[("authorization", &bearer(&"0".repeat(64)))]).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

    // The list, on this machine, says who and from where, and never the token.
    let listed = get(&s.app, "/api/v1/remote", HERE, &[]).await;
    assert_eq!(listed.body["phones"][0]["name"], "Pixel");
    assert_eq!(listed.body["phones"][0]["last_address"], "192.168.1.50");
    assert!(!listed.body.to_string().contains(&token));
}

#[tokio::test]
async fn a_phone_can_do_what_the_pages_do_to_the_devices() {
    let s = server();
    let (token, _, _) = pair(&s, "Pixel").await;
    let answer = send(
        &s.app,
        "POST",
        "/api/v1/devices/loopback-0/command/set_mute?dry_run=true",
        PHONE,
        &[("authorization", &bearer(&token))],
        Some(json!({"id": 0, "mute": 1})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
}

#[tokio::test]
async fn updating_restarting_and_managing_phones_stay_on_this_machine() {
    let s = server();
    let (token, id, _) = pair(&s, "Pixel").await;
    let auth = bearer(&token);
    for (method, uri) in [
        ("GET", "/api/v1/update"),
        ("POST", "/api/v1/update/check"),
        ("POST", "/api/v1/update/download"),
        ("POST", "/api/v1/update/restart"),
        ("POST", "/api/v1/window/show"),
        ("GET", "/api/v1/aggregate"),
        ("GET", "/api/v1/remote"),
        ("PUT", "/api/v1/remote"),
        ("POST", "/api/v1/remote/pairing"),
        ("DELETE", &format!("/api/v1/remote/phones/{id}")),
        ("GET", "/api/v1/%55pdate"),
    ] {
        let answer = send(&s.app, method, uri, PHONE, &[("authorization", &auth)], Some(json!({"allow_phones": false}))).await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{method} {uri}: {}", answer.body);
        assert_eq!(answer.body["error"]["code"], "not_local", "{method} {uri}");
    }
    assert_eq!(s.stops.load(Ordering::SeqCst), 0, "nothing was restarted");
    assert_eq!(s.remote.phones().len(), 1, "and nothing about phones changed");
    // This machine still has all of it.
    assert_eq!(get(&s.app, "/api/v1/update", HERE, &[]).await.status, StatusCode::OK);
    let restart = send(&s.app, "POST", "/api/v1/update/restart", HERE, &[], None).await;
    assert_eq!(restart.body["error"]["code"], "nothing_staged", "answered, not refused");
}

#[tokio::test]
async fn a_revoked_phone_is_refused_at_its_next_request() {
    let s = server();
    let (token, id, _) = pair(&s, "Pixel").await;
    let auth = bearer(&token);
    assert_eq!(get(&s.app, "/api/v1/devices", PHONE, &[("authorization", &auth)]).await.status, StatusCode::OK);
    let revoked = send(&s.app, "DELETE", &format!("/api/v1/remote/phones/{id}"), HERE, &[], None).await;
    assert_eq!(revoked.body, json!({"revoked": true}));
    assert_eq!(get(&s.app, "/api/v1/devices", PHONE, &[("authorization", &auth)]).await.status, StatusCode::UNAUTHORIZED);
    let again = send(&s.app, "DELETE", &format!("/api/v1/remote/phones/{id}"), HERE, &[], None).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    assert_eq!(again.body["error"]["code"], "unknown_phone");
}

#[tokio::test]
async fn wrong_codes_are_refused_then_rate_limited() {
    let s = server();
    let started = send(&s.app, "POST", "/api/v1/remote/pairing", HERE, &[], None).await;
    let code = started.body["code"].as_str().unwrap().to_string();
    for _ in 0..5 {
        let wrong = send(&s.app, "POST", "/api/v1/remote/pair", PHONE, &[], Some(json!({"code": "0000-0000", "name": "x"}))).await;
        assert_eq!(wrong.status, StatusCode::FORBIDDEN);
        assert_eq!(wrong.body["error"]["code"], "pairing_refused");
    }
    let limited = send(&s.app, "POST", "/api/v1/remote/pair", PHONE, &[], Some(json!({"code": code, "name": "x"}))).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.body["error"]["code"], "too_many_attempts");
    assert!(limited.body.get("token").is_none());
}

#[tokio::test]
async fn a_host_that_is_not_this_server_is_refused_even_from_this_machine() {
    let s = server();
    for host in ["evil.example", "evil.example:8420", "127.0.0.1.evil.example:8420"] {
        let answer = get(&s.app, "/api/v1/devices", HERE, &[("host", host)]).await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{host}");
        assert_eq!(answer.body["error"]["code"], "bad_host");
    }
    for host in ["127.0.0.1:8420", "localhost:8420", "studio-pc:8420", "STUDIO-PC.local:8420", "192.168.1.5:8420"] {
        assert_eq!(get(&s.app, "/api/v1/devices", HERE, &[("host", host)]).await.status, StatusCode::OK, "{host}");
    }
}

#[tokio::test]
async fn a_page_from_another_origin_cannot_reach_this_machines_api() {
    let s = server();
    for origin in ["http://evil.example", "http://127.0.0.1:9999", "null", "https://127.0.0.1:8420"] {
        let answer = send(&s.app, "POST", "/api/v1/remote/pairing", HERE, &[("origin", origin)], None).await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{origin}");
        assert_eq!(answer.body["error"]["code"], "bad_origin", "{origin}");
    }
    assert_eq!(s.remote.status().pairing, None, "no pairing was started by any of them");
    let same = send(&s.app, "POST", "/api/v1/remote/pairing", HERE, &[("origin", "http://127.0.0.1:8420")], None).await;
    assert_eq!(same.status, StatusCode::OK, "the page's own origin is fine");
}

#[tokio::test]
async fn with_phones_off_another_machine_is_refused_whatever_it_carries() {
    let s = server_with(remote_with(Backing::Memory, Exposure::Loopback));
    for uri in ["/api/v1/devices", "/", "/api/v1/remote/pair"] {
        let answer = get(&s.app, uri, PHONE, &[]).await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(answer.body["error"]["code"], "remote_off");
    }
    let refused = send(&s.app, "POST", "/api/v1/remote/pairing", HERE, &[], None).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.body["error"]["code"], "phones_off");
}

#[tokio::test]
async fn the_setting_is_changed_from_this_machine_and_persists() {
    let path = std::env::temp_dir().join(format!("gazelle-remote-http-{}", std::process::id())).join("remote.json");
    let _ = std::fs::remove_file(&path);
    let s = server_with(remote_with(Backing::File(path.clone()), Exposure::Loopback));
    let on = send(&s.app, "PUT", "/api/v1/remote", HERE, &[], Some(json!({"allow_phones": true}))).await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(on.body["allow_phones"], true);
    let reloaded = remote_with(Backing::File(path.clone()), Exposure::Loopback);
    assert!(reloaded.allow_phones(), "the setting was written to remote.json");
    let bad = send(&s.app, "PUT", "/api/v1/remote", HERE, &[], Some(json!({"allow_phones": "yes"}))).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let _ = std::fs::remove_file(&path);

    let fixed = server_with(remote_with(Backing::Memory, Exposure::Everywhere("0.0.0.0:8420".parse().unwrap())));
    let refused = send(&fixed.app, "PUT", "/api/v1/remote", HERE, &[], Some(json!({"allow_phones": false}))).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.body["error"]["code"], "fixed_by_bind");
}

// ---------------------------------------------------------------------------------------------
// The WebSocket, over a real socket on 127.0.0.1
// ---------------------------------------------------------------------------------------------

/// Serve on 127.0.0.1, with every connection's peer rewritten to `peer` before the gate sees it.
async fn listen(s: &Server, peer: Option<SocketAddr>) -> SocketAddr {
    let app = s.app.clone().layer(axum::middleware::map_request(move |mut request: Request| async move {
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        request
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { http::serve(listener, app).await.unwrap() });
    address
}

fn ws_request(address: SocketAddr, headers: &[(&str, &str)]) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut request = format!("ws://{address}/api/v1/ws").into_client_request().unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(), value.parse().unwrap());
    }
    request
}

fn refused_with(result: Result<impl Sized, tokio_tungstenite::tungstenite::Error>) -> u16 {
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(other) => panic!("expected an HTTP refusal, got {other}"),
        Ok(_) => panic!("the upgrade should have been refused"),
    }
}

async fn hello<S>(ws: &mut S) -> Value
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match ws.next().await.expect("open").expect("no error") {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            _ => continue,
        }
    }
}

#[tokio::test]
async fn a_websocket_from_another_machine_needs_a_token_and_takes_a_header_or_the_cookie() {
    let s = server();
    let address = listen(&s, Some(PHONE.parse().unwrap())).await;
    assert_eq!(refused_with(tokio_tungstenite::connect_async(ws_request(address, &[])).await), 401);
    assert_eq!(refused_with(tokio_tungstenite::connect_async(ws_request(address, &[("authorization", "Bearer nope")])).await), 401);

    let (token, _, _) = pair(&s, "Pixel").await;
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_request(address, &[("authorization", &bearer(&token))])).await.expect("with a token");
    assert_eq!(hello(&mut ws).await["type"], "hello");
    let origin = format!("http://{address}");
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_request(address, &[("cookie", &format!("gazelle_token={token}")), ("origin", &origin)]))
        .await
        .expect("with the cookie, as a phone's browser sends it");
    assert_eq!(hello(&mut ws).await["type"], "hello");
}

#[tokio::test]
async fn revoking_a_phone_closes_its_open_websocket_and_no_other() {
    let s = server();
    let address = listen(&s, Some(PHONE.parse().unwrap())).await;
    let local = listen(&s, None).await;
    let (lose, lose_id, _) = pair(&s, "Lose").await;
    let (keep, _, _) = pair(&s, "Keep").await;
    let (mut lost, _) = tokio_tungstenite::connect_async(ws_request(address, &[("authorization", &bearer(&lose))])).await.unwrap();
    let (mut kept, _) = tokio_tungstenite::connect_async(ws_request(address, &[("authorization", &bearer(&keep))])).await.unwrap();
    let (mut here, _) = tokio_tungstenite::connect_async(ws_request(local, &[])).await.unwrap();
    // The hello says which connections are phones, so the web app on one skips what is refused.
    for (ws, phone) in [(&mut lost, true), (&mut kept, true), (&mut here, false)] {
        let said = hello(ws).await;
        assert_eq!(said["type"], "hello");
        assert_eq!(said["phone"], phone);
    }

    s.remote.revoke(&lose_id).unwrap();
    let closed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match lost.next().await {
                Some(Ok(Message::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                Some(Ok(_)) => continue,
                _ => return None,
            }
        }
    })
    .await
    .expect("the revoked phone's socket closed at once");
    assert_eq!(closed, Some(4401));

    // The others are still there: each still answers a call.
    for ws in [&mut kept, &mut here] {
        use futures_util::SinkExt;
        ws.send(Message::Text(json!({"id": 7, "device_id": "nope", "command": "get_mixer"}).to_string().into())).await.unwrap();
        let answer = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Message::Text(text) = ws.next().await.unwrap().unwrap() {
                    let frame: Value = serde_json::from_str(&text).unwrap();
                    if frame["id"] == 7 {
                        return frame;
                    }
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(answer["type"], "rpc_error");
    }
}

#[tokio::test]
async fn a_websocket_opened_by_another_sites_page_is_refused() {
    let s = server();
    let local = listen(&s, None).await;
    assert_eq!(refused_with(tokio_tungstenite::connect_async(ws_request(local, &[("origin", "http://evil.example")])).await), 403);
    let own = format!("http://{local}");
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_request(local, &[("origin", &own)])).await.expect("its own page is fine");
    assert_eq!(hello(&mut ws).await["type"], "hello");
}

/// **A paired phone is a remote for the recorder**: it reads the state and can press every transport
/// button, which is why `/api/v1/recording` is not among what stays on the computer. Presets are
/// another matter, since one names a folder on the computer: a phone's workspace save that changes
/// them is refused, and the computer's own is not.
#[tokio::test]
async fn a_phone_drives_the_recorder_and_leaves_its_presets_to_the_computer() {
    use gazelle_audio_server::aggregate::calibrate::Calibration;
    use gazelle_audio_server::recording::RecordingService;

    let remote = remote_with(Backing::Memory, Exposure::Everywhere("0.0.0.0:8420".parse().unwrap()));
    let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
    devices.attach_loopbacks(&[PID_QUADRO], 64);
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    let state = AppState {
        devices: devices.clone(),
        store: store.clone(),
        snapshots: Arc::new(MemorySnapshotStore::default()),
        force_dry_run: true,
        enable_recall: false,
        backend: "loopback".into(),
        themes_dir: None,
        show_window: None,
    };
    let recording = RecordingService::for_backend(true, Arc::new(Calibration::this_pc()), store.clone(), devices);
    let app = http::router(state).merge(http::recording::routes(recording.clone())).merge(http::remote::routes(remote.clone())).layer(axum::Extension(recording));
    let s = Server { app: guard::protect(app, remote.clone()), remote, stops: Arc::new(AtomicUsize::new(0)) };
    let (token, _, _) = pair(&s, "Phone").await;
    let auth = bearer(&token);
    let phone = [("authorization", auth.as_str())];

    assert!(!guard::LOCAL_ONLY.iter().any(|path| "/api/v1/recording".starts_with(path)), "the recorder is not kept on the computer");
    let status = get(&s.app, "/api/v1/recording", PHONE, &phone).await;
    assert_eq!(status.status, StatusCode::OK, "{}", status.body);
    assert_eq!(status.body["state"], "off");
    let stop = send(&s.app, "POST", "/api/v1/recording/stop", PHONE, &phone, Some(json!({}))).await;
    assert_eq!(stop.status, StatusCode::OK, "{}", stop.body);
    let record = send(&s.app, "POST", "/api/v1/recording/record", PHONE, &phone, Some(json!({}))).await;
    assert_eq!((record.status, record.body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("not_armed")), "answered, not forbidden");

    let presets = json!({"version": 1, "recording": {"presets": [{"id": "p", "name": "Band", "channels": [{"device": 0, "channel": 0}]}]}});
    let from_phone = send(&s.app, "PUT", "/api/v1/workspace", PHONE, &phone, Some(presets.clone())).await;
    assert_eq!((from_phone.status, from_phone.body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("not_local")), "{}", from_phone.body);
    let from_here = send(&s.app, "PUT", "/api/v1/workspace", HERE, &[], Some(presets)).await;
    assert_eq!(from_here.status, StatusCode::OK, "{}", from_here.body);
    let untouched = send(&s.app, "PUT", "/api/v1/workspace", PHONE, &phone, Some(from_here.body.clone())).await;
    assert_eq!(untouched.status, StatusCode::OK, "a phone's save that leaves the presets alone is taken: {}", untouched.body);
}
