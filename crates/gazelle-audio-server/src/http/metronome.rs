//! The metronome over HTTP.
//!
//! - `GET /api/v1/metronome`: its settings (`settings`), whether it is running, who started it
//!   (`started_by`: `hand`, `count_in`, `follow` or `preview`), the beat and bar last played, how long
//!   a beat is, how long ago the last one was (`since_beat_seconds`, as of `at_ms`), a count-in under
//!   way, and the outputs it plays to, named as the aggregate names them, while it is open.
//! - `POST /api/v1/metronome/start`: start the click. With nothing armed this opens the interfaces
//!   and holds the audio drivers, as arming does. 409 `metronome_refused` with the reason (no outputs
//!   chosen, the interfaces would not open), or `measuring`.
//! - `POST /api/v1/metronome/stop`: stop it. A count-in under way is cancelled.
//! - `POST /api/v1/metronome/preview`: one bar, 12 dB under the volume, only while armed.
//! - `GET /api/v1/metronome/settings` and `PUT` with any of `tempo`, `numerator`, `denominator`,
//!   `accent`, `subdivision`, `sound`, `volume_db`, `outputs`, `count_in_bars`, `follow_record`; what
//!   is left out stays. 400 `bad_value` for nonsense, 409 `outputs_fixed` for new outputs while the
//!   interfaces are open.
//!
//! Tap tempo is worked out by the page that is tapped: the taps are its own, and a round trip per
//! tap would only add the network's jitter to them.
//!
//! **A paired phone may start, stop and preview it and change its tempo and volume**, from across the
//! room. Everything else it may read and not change: a phone's `PUT` naming any other field is 403
//! `not_local`. The outputs above all are this computer's wiring.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use gazelle_calibrate::Pick;
use gazelle_record::metronome::Subdivision;
use gazelle_record::sounds::Sound;
use serde::Deserialize;
use serde_json::json;

use crate::handover;
use crate::recording::{RecordingService, Refusal};
use crate::remote::guard::refuse;

pub fn routes(service: Arc<RecordingService>) -> Router {
    Router::new()
        .route("/api/v1/metronome", get(status))
        .route("/api/v1/metronome/start", post(start))
        .route("/api/v1/metronome/stop", post(stop))
        .route("/api/v1/metronome/preview", post(preview))
        .route("/api/v1/metronome/settings", get(settings).put(set_settings))
        .layer(Extension(service))
}

fn refused(refusal: Refusal) -> Response {
    let status = match refusal.code {
        "bad_value" => StatusCode::BAD_REQUEST,
        "storage_error" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
    };
    refuse(status, refusal.code, refusal.message)
}

async fn status(Extension(service): Extension<Arc<RecordingService>>) -> Response {
    Json(service.metronome_answer()).into_response()
}

async fn start(Extension(service): Extension<Arc<RecordingService>>) -> Response {
    // Opening the drivers takes a moment and loads DLLs, so it is never done on a runtime worker.
    let started = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        move || service.metronome_start()
    })
    .await;
    match started {
        Ok(Ok(())) => Json(service.metronome_answer()).into_response(),
        Ok(Err(refusal)) => refused(refusal),
        Err(why) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", format!("starting the metronome stopped part way: {why}")),
    }
}

async fn stop(Extension(service): Extension<Arc<RecordingService>>) -> Response {
    // The last click rings out and the drivers may be let go: off the runtime.
    let stopped = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        move || service.metronome_stop()
    })
    .await
    .unwrap_or(false);
    let mut answer = service.metronome_answer();
    answer["stopped"] = json!(stopped);
    Json(answer).into_response()
}

async fn preview(Extension(service): Extension<Arc<RecordingService>>) -> Response {
    match service.metronome_preview() {
        Ok(()) => Json(service.metronome_answer()).into_response(),
        Err(refusal) => refused(refusal),
    }
}

async fn settings(Extension(service): Extension<Arc<RecordingService>>) -> Response {
    Json(service.metronome_settings()).into_response()
}

/// A change: what is left out stays.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    tempo: Option<f64>,
    volume_db: Option<f64>,
    numerator: Option<u32>,
    denominator: Option<u32>,
    accent: Option<bool>,
    subdivision: Option<Subdivision>,
    sound: Option<Sound>,
    outputs: Option<Vec<Pick>>,
    count_in_bars: Option<u32>,
    follow_record: Option<bool>,
}

impl Change {
    /// Whether it changes anything but the tempo and the volume, which is all a phone may change.
    fn beyond_a_phone(&self) -> bool {
        self.numerator.is_some()
            || self.denominator.is_some()
            || self.accent.is_some()
            || self.subdivision.is_some()
            || self.sound.is_some()
            || self.outputs.is_some()
            || self.count_in_bars.is_some()
            || self.follow_record.is_some()
    }
}

async fn set_settings(Extension(service): Extension<Arc<RecordingService>>, request: Request) -> Response {
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(peer)| *peer);
    let change: Change = match <Json<Change> as axum::extract::FromRequest<()>>::from_request(request, &()).await {
        Ok(Json(change)) => change,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, "bad_value", e.body_text()),
    };
    if change.beyond_a_phone() && !handover::allowed(peer) {
        return refuse(StatusCode::FORBIDDEN, "not_local", "A phone may change the metronome's tempo and volume; everything else about it is changed on the computer.".into());
    }
    let result = service.change_metronome(|settings| {
        let Change { tempo, volume_db, numerator, denominator, accent, subdivision, sound, outputs, count_in_bars, follow_record } = change;
        if let Some(value) = tempo {
            settings.tempo = value;
        }
        if let Some(value) = volume_db {
            settings.volume_db = value;
        }
        if let Some(value) = numerator {
            settings.numerator = value;
        }
        if let Some(value) = denominator {
            settings.denominator = value;
        }
        if let Some(value) = accent {
            settings.accent = value;
        }
        if let Some(value) = subdivision {
            settings.subdivision = value;
        }
        if let Some(value) = sound {
            settings.sound = value;
        }
        if let Some(value) = outputs {
            settings.outputs = value;
        }
        if let Some(value) = count_in_bars {
            settings.count_in_bars = value;
        }
        if let Some(value) = follow_record {
            settings.follow_record = value;
        }
    });
    match result {
        Ok(settings) => Json(settings).into_response(),
        Err(refusal) => refused(refusal),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::aggregate::calibrate::Calibration;
    use crate::device::manager::DeviceManager;
    use crate::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
    use crate::workspace::store::MemoryStore;

    fn app() -> (Router, Arc<RecordingService>) {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64);
        let service = RecordingService::for_backend(true, Arc::new(Calibration::this_pc()), Arc::new(MemoryStore::default()), devices);
        (routes(Arc::clone(&service)), service)
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
    async fn the_settings_change_in_part_are_checked_and_a_phone_changes_only_tempo_and_volume() {
        let (app, _) = app();
        let (status, body) = send(&app, "GET", "/api/v1/metronome/settings", None, HERE).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tempo"], 120.0);
        assert_eq!(body["volume_db"], -18.0);
        assert_eq!(body["outputs"], json!([]));

        let (status, body) = send(&app, "PUT", "/api/v1/metronome/settings", Some(r#"{"tempo":97.34,"sound":"woodblock","outputs":[{"device":0,"channel":6},{"device":0,"channel":7}],"count_in_bars":2}"#), HERE).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!((body["tempo"].as_f64(), body["sound"].as_str(), body["count_in_bars"].as_u64()), (Some(97.3), Some("woodblock"), Some(2)), "tempo in steps of 0.1");
        assert_eq!(body["outputs"][1], json!({"device": 0, "channel": 7}));

        let (status, body) = send(&app, "PUT", "/api/v1/metronome/settings", Some(r#"{"tempo":401}"#), HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_value")));
        let (status, _) = send(&app, "PUT", "/api/v1/metronome/settings", Some(r#"{"sound":"gong"}"#), HERE).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = send(&app, "PUT", "/api/v1/metronome/settings", Some(r#"{"swing":0.5}"#), HERE).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, body) = send(&app, "PUT", "/api/v1/metronome/settings", Some(r#"{"tempo":140,"volume_db":-24}"#), PHONE).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!((body["tempo"].as_f64(), body["volume_db"].as_f64()), (Some(140.0), Some(-24.0)));
        for change in [r#"{"outputs":[]}"#, r#"{"tempo":100,"sound":"click"}"#, r#"{"count_in_bars":1}"#, r#"{"follow_record":true}"#] {
            let (status, body) = send(&app, "PUT", "/api/v1/metronome/settings", Some(change), PHONE).await;
            assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("not_local")), "{change}");
        }
        let (_, body) = send(&app, "GET", "/api/v1/metronome", None, PHONE).await;
        assert_eq!(body["settings"]["tempo"], 140.0, "a phone reads everything");
        assert_eq!(body["running"], false);
    }

    #[tokio::test(flavor = "multi_thread")]
    // Held across awaits on purpose: it keeps every test that hosts the aggregate apart.
    #[allow(clippy::await_holding_lock)]
    async fn start_and_stop_on_the_loopback_open_and_close_the_interfaces() {
        let _one = crate::recording::hosting();
        let (app, service) = app();
        let (status, body) = send(&app, "POST", "/api/v1/metronome/start", None, HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("metronome_refused")));
        assert!(body["error"]["message"].as_str().unwrap().contains("no outputs"), "{body}");

        send(&app, "PUT", "/api/v1/metronome/settings", Some(r#"{"outputs":[{"device":0,"channel":6},{"device":0,"channel":7}]}"#), HERE).await;
        let (status, body) = send(&app, "POST", "/api/v1/metronome/start", None, PHONE).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["running"], true);
        assert_eq!(body["started_by"], "hand");
        assert_eq!(body["outputs"].as_array().unwrap().len(), 2, "{body}");
        assert!(body["at_ms"].as_u64().is_some());
        assert!(service.is_active(), "it holds the interfaces, as arming does");
        let (status, body) = send(&app, "PUT", "/api/v1/metronome/settings", Some(r#"{"outputs":[{"device":1,"channel":0}]}"#), HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("outputs_fixed")));
        tokio::time::sleep(Duration::from_millis(700)).await;
        let (_, body) = send(&app, "GET", "/api/v1/metronome", None, HERE).await;
        assert!(body["bar"].as_u64().unwrap() >= 1 && body["since_beat_seconds"].as_f64().is_some(), "{body}");
        let (status, body) = send(&app, "POST", "/api/v1/metronome/preview", None, HERE).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("metronome_refused")), "only while armed");
        let (_, body) = send(&app, "POST", "/api/v1/metronome/stop", None, PHONE).await;
        assert_eq!((body["stopped"].as_bool(), body["running"].as_bool(), body["open"].as_bool()), (Some(true), Some(false), Some(false)));
        assert!(!service.is_active());
        assert_eq!(service.answer()["metronome"]["running"], false, "the recorder's live state carries it");
    }
}
