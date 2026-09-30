//! The studio hub over HTTP: the recording widget and hub windows, and the recording settings.
//!
//! - `GET /api/v1/window/widget` and `GET /api/v1/window/hub`: whether each is open, and whether
//!   this Gazelle has windows at all: `{"available", "widget", "hub", "hub_full_screen"}`, with a
//!   `reason` when it has none.
//! - `POST /api/v1/window/widget` with `{"open": true}` or `false`: open or close the widget.
//! - `POST /api/v1/window/hub` with `{"open": ...}` and, or, `{"full_screen": ...}`: open the hub
//!   full screen on the monitor it was last on, close it, or take it in or out of full screen.
//!   Without windows (`--no-window`, `--no-tray`, a build without them) both answer 409 `no_window`.
//! - `GET /api/v1/recording/settings`: `{"auto_arm", "auto_arm_preset", "start_in_hub",
//!   "cubase_seed", "cubase_seed_check"}`. `cubase_seed_check` is `{"ok", "message"}` for the seed
//!   set, read again whenever the file has changed, or `null` with none set.
//! - `PUT /api/v1/recording/settings`: any of the first four; what is left out stays as it is.
//!   Turning auto-arm on needs a preset that is in the workspace (409 `no_preset` otherwise). A
//!   Cubase seed is a whole path to a track archive a take's archive can be made from, checked
//!   before it is kept (400 `bad_seed` otherwise); `null` or `""` sets none.
//!
//! **This machine only**, every one of them: the windows are on someone's desk, and the settings
//! decide whether this PC's audio drivers are held. The gate (`remote::guard::LOCAL_ONLY`) refuses
//! them to a phone already; each handler checks again, so a router put together without the gate
//! still does not hand them out. What auto-arm is doing is part of the recorder's own state
//! (`GET /api/v1/recording`, `auto_arm`), which a phone may read.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::handover;
use crate::recording::RecordingService;
use crate::remote::guard::refuse;
use crate::window::Viewports;

/// What a server without windows says.
pub const NO_WINDOW: &str = "This Gazelle is running without windows (--no-window, --no-tray, or a build without them), so there is no recording widget or hub to open. Open #/widget or #/hub in a browser instead.";

/// What the routes reach.
#[derive(Clone)]
struct Studio {
    recording: Arc<RecordingService>,
    viewports: Option<Arc<dyn Viewports>>,
}

pub fn routes(recording: Arc<RecordingService>, viewports: Option<Arc<dyn Viewports>>) -> Router {
    Router::new()
        .route("/api/v1/window/widget", get(windows).post(set_widget))
        .route("/api/v1/window/hub", get(windows).post(set_hub))
        .route("/api/v1/recording/settings", get(settings).put(set_settings))
        .layer(Extension(Studio { recording, viewports }))
}

fn refuse_unless_local(request: &Request) -> Option<Response> {
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(peer)| *peer);
    (!handover::allowed(peer)).then(|| refuse(StatusCode::FORBIDDEN, "not_local", "Only Gazelle on the computer itself can do that.".into()))
}

fn state(studio: &Studio) -> Value {
    match &studio.viewports {
        Some(viewports) => {
            let mut answer = serde_json::to_value(viewports.state()).unwrap_or_else(|_| json!({}));
            answer["available"] = json!(true);
            answer
        }
        None => json!({"available": false, "widget": false, "hub": false, "hub_full_screen": false, "reason": NO_WINDOW}),
    }
}

async fn windows(Extension(studio): Extension<Studio>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    Json(state(&studio)).into_response()
}

/// The body, or the refusal in the usual shape.
async fn body<T: serde::de::DeserializeOwned>(request: Request) -> Result<T, Box<Response>> {
    <Json<T> as axum::extract::FromRequest<()>>::from_request(request, &())
        .await
        .map(|Json(body)| body)
        .map_err(|e| Box::new(refuse(StatusCode::BAD_REQUEST, "bad_value", e.body_text())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    open: bool,
}

async fn set_widget(Extension(studio): Extension<Studio>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    let ask: Open = match body(request).await {
        Ok(ask) => ask,
        Err(refusal) => return *refusal,
    };
    let Some(viewports) = &studio.viewports else {
        return refuse(StatusCode::CONFLICT, "no_window", NO_WINDOW.into());
    };
    tracing::info!("recording widget {}, from the app", if ask.open { "opened" } else { "closed" });
    viewports.set_widget(ask.open);
    Json(state(&studio)).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hub {
    #[serde(default)]
    open: Option<bool>,
    #[serde(default)]
    full_screen: Option<bool>,
}

async fn set_hub(Extension(studio): Extension<Studio>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    let ask: Hub = match body(request).await {
        Ok(ask) => ask,
        Err(refusal) => return *refusal,
    };
    if ask.open.is_none() && ask.full_screen.is_none() {
        return refuse(StatusCode::BAD_REQUEST, "bad_value", "say open, full_screen, or both".into());
    }
    let Some(viewports) = &studio.viewports else {
        return refuse(StatusCode::CONFLICT, "no_window", NO_WINDOW.into());
    };
    if let Some(open) = ask.open {
        tracing::info!("recording hub {}, from the app", if open { "opened" } else { "closed" });
        viewports.set_hub(open);
    }
    if let Some(full) = ask.full_screen {
        viewports.set_hub_full_screen(full);
    }
    Json(state(&studio)).into_response()
}

async fn settings(Extension(studio): Extension<Studio>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    Json(settings_answer(&studio.recording, studio.recording.studio().get())).into_response()
}

/// The settings, and what the Cubase seed set is found to be.
fn settings_answer(recording: &RecordingService, settings: crate::studio::StudioSettings) -> Value {
    let mut answer = serde_json::to_value(settings).unwrap_or_else(|_| json!({}));
    answer["cubase_seed_check"] = json!(recording.cubase_seed_check());
    answer
}

/// A path as a person pastes it: trimmed, and without the quotes Explorer's "Copy as path" adds.
fn pasted_path(text: &str) -> String {
    let trimmed = text.trim();
    trimmed.strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(trimmed).trim().to_string()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    #[serde(default)]
    auto_arm: Option<bool>,
    /// `null` forgets the preset; left out, it stays.
    #[serde(default, deserialize_with = "some")]
    auto_arm_preset: Option<Option<String>>,
    #[serde(default)]
    start_in_hub: Option<bool>,
    /// `null` or `""` sets none; left out, it stays.
    #[serde(default, deserialize_with = "some")]
    cubase_seed: Option<Option<String>>,
}

/// A field that is there, even as `null`, is `Some`.
fn some<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

async fn set_settings(Extension(studio): Extension<Studio>, request: Request) -> Response {
    if let Some(refusal) = refuse_unless_local(&request) {
        return refusal;
    }
    let change: Change = match body(request).await {
        Ok(change) => change,
        Err(refusal) => return *refusal,
    };
    let current = studio.recording.studio().get();
    let preset = change.auto_arm_preset.clone().unwrap_or(current.auto_arm_preset.clone()).filter(|id| !id.trim().is_empty());
    if change.auto_arm.unwrap_or(current.auto_arm) {
        let Some(id) = preset.as_deref() else {
            return refuse(StatusCode::BAD_REQUEST, "bad_value", "Choose the preset auto-arm arms with.".into());
        };
        if let Err(refusal) = studio.recording.preset_exists(id) {
            return refuse(StatusCode::CONFLICT, refusal.code, refusal.message);
        }
    }
    let seed = change.cubase_seed.as_ref().map(|seed| seed.as_deref().map(pasted_path).filter(|path| !path.is_empty()));
    if let Some(Some(path)) = &seed {
        if !std::path::Path::new(path).is_absolute() {
            return refuse(StatusCode::BAD_REQUEST, "bad_seed", r"Give the seed's whole path, such as C:\Cubase\Recorded.xml.".into());
        }
        if let Err(why) = crate::recording::check_seed_file(std::path::Path::new(path)) {
            return refuse(StatusCode::BAD_REQUEST, "bad_seed", why);
        }
    }
    let result = studio.recording.change_settings(|settings| {
        if let Some(on) = change.auto_arm {
            settings.auto_arm = on;
        }
        if let Some(preset) = change.auto_arm_preset {
            settings.auto_arm_preset = preset;
        }
        if let Some(hub) = change.start_in_hub {
            settings.start_in_hub = hub;
        }
        if let Some(seed) = seed {
            settings.cubase_seed = seed;
        }
    });
    match result {
        Ok(settings) => Json(settings_answer(&studio.recording, settings)).into_response(),
        Err(why) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", why),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::aggregate::calibrate::Calibration;
    use crate::device::manager::DeviceManager;
    use crate::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
    use crate::window::ViewportsState;
    use crate::workspace::model::{Recording, RecordingChannel, RecordingPreset, Workspace};
    use crate::workspace::store::{MemoryStore, WorkspaceStore};

    /// Windows made of data: what was asked, and what is open.
    #[derive(Default)]
    struct Desk(Mutex<ViewportsState>);

    impl Viewports for Desk {
        fn state(&self) -> ViewportsState {
            *self.0.lock().unwrap()
        }
        fn set_widget(&self, open: bool) {
            self.0.lock().unwrap().widget = open;
        }
        fn set_hub(&self, open: bool) {
            let mut state = self.0.lock().unwrap();
            state.hub = open;
            state.hub_full_screen = open;
        }
        fn set_hub_full_screen(&self, full: bool) {
            self.0.lock().unwrap().hub_full_screen = full;
        }
    }

    fn app(viewports: Option<Arc<dyn Viewports>>) -> Router {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64);
        let store = Arc::new(MemoryStore::default());
        let preset = RecordingPreset { id: "band".into(), name: "Band".into(), channels: vec![RecordingChannel { device: 0, channel: 0 }], ..RecordingPreset::default() };
        store.save(&Workspace { recording: Some(Recording { presets: vec![preset], ..Recording::default() }), ..Workspace::default() }).unwrap();
        let recording = RecordingService::for_backend(true, Arc::new(Calibration::this_pc()), store, devices);
        routes(recording, viewports)
    }

    async fn send(app: &Router, method: &str, uri: &str, body: Option<&str>, from: [u8; 4]) -> (StatusCode, Value) {
        let mut request = HttpRequest::builder().method(method).uri(uri);
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let mut request = request.body(body.map_or(Body::empty(), |b| Body::from(b.to_string()))).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from((from, 5050))));
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    const HERE: [u8; 4] = [127, 0, 0, 1];
    const PHONE: [u8; 4] = [192, 168, 1, 50];

    #[tokio::test]
    async fn the_widget_and_the_hub_open_and_close_and_say_so() {
        let desk = Arc::new(Desk::default());
        let app = app(Some(desk.clone()));
        let (status, body) = send(&app, "GET", "/api/v1/window/widget", None, HERE).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"available": true, "widget": false, "hub": false, "hub_full_screen": false}));

        let (_, body) = send(&app, "POST", "/api/v1/window/widget", Some(r#"{"open":true}"#), HERE).await;
        assert_eq!(body["widget"], true);
        let (_, body) = send(&app, "POST", "/api/v1/window/hub", Some(r#"{"open":true}"#), HERE).await;
        assert_eq!((body["hub"].as_bool(), body["hub_full_screen"].as_bool()), (Some(true), Some(true)));
        let (_, body) = send(&app, "POST", "/api/v1/window/hub", Some(r#"{"full_screen":false}"#), HERE).await;
        assert_eq!((body["hub"].as_bool(), body["hub_full_screen"].as_bool()), (Some(true), Some(false)), "Esc leaves full screen, not the hub");
        let (_, body) = send(&app, "GET", "/api/v1/window/hub", None, HERE).await;
        assert_eq!(body["widget"], true, "one answer covers both");
        assert_eq!(desk.state(), ViewportsState { widget: true, hub: true, hub_full_screen: false });

        let (status, body) = send(&app, "POST", "/api/v1/window/hub", Some("{}"), HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_value")));
        let (status, _) = send(&app, "POST", "/api/v1/window/widget", Some(r#"{"open":"yes"}"#), HERE).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn without_windows_the_routes_say_why_and_open_nothing() {
        let app = app(None);
        let (status, body) = send(&app, "GET", "/api/v1/window/widget", None, HERE).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["available"], false);
        assert!(body["reason"].as_str().unwrap().contains("--no-window"));
        let (status, body) = send(&app, "POST", "/api/v1/window/hub", Some(r#"{"open":true}"#), HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("no_window")));
    }

    #[tokio::test]
    async fn the_settings_change_in_part_and_auto_arm_needs_a_preset_that_is_there() {
        let app = app(None);
        let (status, body) = send(&app, "GET", "/api/v1/recording/settings", None, HERE).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"auto_arm": false, "auto_arm_preset": null, "start_in_hub": false, "cubase_seed": null, "cubase_seed_check": null}), "off by default");

        let (status, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(r#"{"auto_arm":true}"#), HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_value")), "on, with nothing to arm with");
        let (status, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(r#"{"auto_arm":true,"auto_arm_preset":"gone"}"#), HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("no_preset")));

        let (status, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(r#"{"auto_arm":true,"auto_arm_preset":"band"}"#), HERE).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!({"auto_arm": true, "auto_arm_preset": "band", "start_in_hub": false, "cubase_seed": null, "cubase_seed_check": null}));
        let (_, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(r#"{"start_in_hub":true}"#), HERE).await;
        assert_eq!(body, json!({"auto_arm": true, "auto_arm_preset": "band", "start_in_hub": true, "cubase_seed": null, "cubase_seed_check": null}), "what is left out stays");
        let (_, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(r#"{"auto_arm":false}"#), HERE).await;
        assert_eq!(body["auto_arm_preset"], "band", "off keeps the preset for next time");
        let (_, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(r#"{"auto_arm_preset":null}"#), HERE).await;
        assert_eq!(body["auto_arm_preset"], Value::Null);
        let (status, _) = send(&app, "PUT", "/api/v1/recording/settings", Some(r#"{"auto_arm":1}"#), HERE).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_cubase_seed_is_checked_before_it_is_kept_and_again_when_it_changes() {
        let app = app(None);
        let dir = std::env::temp_dir().join(format!("gazelle-cubase-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("Recorded.xml");
        std::fs::write(&good, include_str!("../../../gazelle-audio-record/testdata/cubase-seed.xml")).unwrap();
        let bad = dir.join("Other.xml");
        std::fs::write(&bad, r#"<tracklist2><list name="track" type="obj"/></tracklist2>"#).unwrap();
        let put = |path: &std::path::Path| json!({ "cubase_seed": format!("\"{}\"", path.display()) }).to_string();

        let (status, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(&put(&good)), HERE).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["cubase_seed"], json!(good.display().to_string()), "Explorer's quotes are taken off");
        assert_eq!(body["cubase_seed_check"]["ok"], true);
        assert!(body["cubase_seed_check"]["message"].as_str().unwrap().contains("\"Recorded\" with its group"), "{body}");

        let (status, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(&put(&bad)), HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_seed")));
        assert!(body["error"]["message"].as_str().unwrap().contains("no folder track"), "{body}");
        let (status, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(r#"{"cubase_seed":"Recorded.xml"}"#), HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_seed")), "a whole path, or nothing");
        let (_, body) = send(&app, "GET", "/api/v1/recording/settings", None, HERE).await;
        assert_eq!(body["cubase_seed"], json!(good.display().to_string()), "a refused seed changes nothing");

        // The file changes under it: the check says so, and the setting stays for when it is put right.
        std::fs::write(&good, "not a track archive at all").unwrap();
        let (_, body) = send(&app, "GET", "/api/v1/recording/settings", None, HERE).await;
        assert_eq!(body["cubase_seed_check"]["ok"], false, "{body}");
        assert_eq!(body["cubase_seed"], json!(good.display().to_string()));

        let (_, body) = send(&app, "PUT", "/api/v1/recording/settings", Some(r#"{"cubase_seed":null}"#), HERE).await;
        assert_eq!((body["cubase_seed"].clone(), body["cubase_seed_check"].clone()), (Value::Null, Value::Null));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_phone_is_refused_every_one_of_them_even_without_the_gate() {
        let app = app(Some(Arc::new(Desk::default())));
        for (method, uri, body) in [
            ("GET", "/api/v1/window/widget", None),
            ("POST", "/api/v1/window/widget", Some(r#"{"open":true}"#)),
            ("POST", "/api/v1/window/hub", Some(r#"{"open":true}"#)),
            ("GET", "/api/v1/recording/settings", None),
            ("PUT", "/api/v1/recording/settings", Some(r#"{"start_in_hub":true}"#)),
        ] {
            let (status, answer) = send(&app, method, uri, body, PHONE).await;
            assert_eq!((status, answer["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("not_local")), "{method} {uri}");
        }
    }
}
