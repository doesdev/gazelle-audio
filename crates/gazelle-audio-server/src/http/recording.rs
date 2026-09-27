//! The recorder over HTTP.
//!
//! - `GET /api/v1/recording`: the state (`off`, `arming`, `armed`, `recording`, `disarming`), the
//!   preset it holds, the recorded channels with their levels, the pre-roll held, the take being
//!   recorded, what was lost, the disk, and the preset offered first.
//! - `POST /api/v1/recording/arm` with `{ "preset": id }`: open the aggregate and start holding a
//!   pre-roll. Answers once armed, or 409 with the reason.
//! - `POST /api/v1/recording/record` and `/stop`: start and end a take. Each takes effect at the
//!   next block of audio.
//! - `POST /api/v1/recording/disarm` with `{ "confirm": true }` when a take is being recorded, which
//!   it stops: without it that is 409 `confirm_disarm`.
//! - `GET /api/v1/recording/takes`: the takes recorded since Gazelle started, newest first, with
//!   their files.
//!
//! Presets are the workspace's `recording.presets`, saved with the rest of it.
//!
//! **A paired phone may use every one of these**: arming and recording from across the room is the
//! point. The live state also goes out on the WebSocket as `recording` frames (`crate::ws`).

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::error::ServerError;
use crate::recording::{Refusal, RecordingService};

pub fn routes(service: Arc<RecordingService>) -> Router {
    Router::new()
        .route("/api/v1/recording", get(status))
        .route("/api/v1/recording/arm", post(arm))
        .route("/api/v1/recording/record", post(record))
        .route("/api/v1/recording/stop", post(stop))
        .route("/api/v1/recording/disarm", post(disarm))
        .route("/api/v1/recording/takes", get(takes))
        .layer(Extension(service))
}

fn refused(refusal: Refusal) -> Response {
    let status = if refusal.code == "no_preset" { StatusCode::NOT_FOUND } else { StatusCode::CONFLICT };
    (status, Json(json!({ "error": { "code": refusal.code, "message": refusal.message } }))).into_response()
}

async fn status(Extension(service): Extension<Arc<RecordingService>>) -> Response {
    Json(service.answer()).into_response()
}

async fn takes(Extension(service): Extension<Arc<RecordingService>>) -> Response {
    Json(service.takes()).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arm {
    preset: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Disarm {
    #[serde(default)]
    confirm: bool,
}

async fn arm(Extension(service): Extension<Arc<RecordingService>>, request: Request) -> Result<Response, ServerError> {
    let body: Result<Json<Arm>, JsonRejection> = <Json<Arm> as axum::extract::FromRequest<()>>::from_request(request, &()).await;
    let Json(ask) = body.map_err(|e| ServerError::BadValue(e.body_text()))?;
    // Opening the drivers takes a moment and loads DLLs, so it is never done on a runtime worker.
    let armed = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        move || service.arm(&ask.preset)
    })
    .await
    .map_err(|e| ServerError::Storage(format!("arming stopped part way: {e}")))?;
    Ok(match armed {
        Ok(()) => Json(service.answer()).into_response(),
        Err(refusal) => refused(refusal),
    })
}

async fn record(Extension(service): Extension<Arc<RecordingService>>) -> Response {
    match service.record() {
        Ok(()) => Json(service.answer()).into_response(),
        Err(refusal) => refused(refusal),
    }
}

async fn stop(Extension(service): Extension<Arc<RecordingService>>) -> Response {
    let stopped = service.stop();
    let mut answer = service.answer();
    answer["stopped"] = json!(stopped);
    Json(answer).into_response()
}

async fn disarm(Extension(service): Extension<Arc<RecordingService>>, request: Request) -> Result<Response, ServerError> {
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024).await.map_err(|e| ServerError::BadValue(e.to_string()))?;
    let ask: Disarm = if bytes.iter().all(u8::is_ascii_whitespace) {
        Disarm::default()
    } else {
        serde_json::from_slice(&bytes).map_err(|e| ServerError::BadValue(format!("not a disarm: {e}")))?
    };
    let done = tokio::task::spawn_blocking({
        let service = Arc::clone(&service);
        move || service.disarm(ask.confirm)
    })
    .await
    .map_err(|e| ServerError::Storage(format!("disarming stopped part way: {e}")))?;
    Ok(match done {
        Ok(disarmed) => {
            let mut answer = service.answer();
            answer["disarmed"] = json!(disarmed);
            Json(answer).into_response()
        }
        Err(refusal) => refused(refusal),
    })
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::{Duration, Instant};

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::aggregate::calibrate::Calibration;
    use crate::device::manager::DeviceManager;
    use crate::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
    use crate::workspace::model::{Recording, RecordingChannel, RecordingPreset, Workspace};
    use crate::workspace::store::{MemoryStore, WorkspaceStore};

    fn app() -> (Router, Arc<MemoryStore>) {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64);
        let store = Arc::new(MemoryStore::default());
        store
            .save(&Workspace {
                recording: Some(Recording {
                    presets: vec![RecordingPreset {
                        id: "band".into(),
                        name: "Band".into(),
                        channels: vec![RecordingChannel { device: 0, channel: 0 }, RecordingChannel { device: 1, channel: 0 }],
                        preroll_max_seconds: Some(5.0),
                        ..RecordingPreset::default()
                    }],
                    ..Recording::default()
                }),
                ..Workspace::default()
            })
            .unwrap();
        let service = RecordingService::for_backend(true, Arc::new(Calibration::this_pc()), store.clone(), devices);
        (routes(service), store)
    }

    async fn send(app: &Router, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
        let mut request = HttpRequest::builder().method(method).uri(uri);
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let mut request = request.body(body.map_or(Body::empty(), |b| Body::from(b.to_string()))).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5050))));
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test(flavor = "multi_thread")]
    // Held across awaits on purpose: it keeps every test that hosts the aggregate apart, and
    // nothing else ever waits on it.
    #[allow(clippy::await_holding_lock)]
    async fn arm_record_stop_and_disarm_on_the_loopback_leave_a_take_with_its_files() {
        let _one = crate::recording::hosting();
        let (app, _) = app();
        let (status, body) = send(&app, "GET", "/api/v1/recording", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"], "off");
        assert_eq!(body["loopback"], true);

        let (status, body) = send(&app, "POST", "/api/v1/recording/record", Some("{}")).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("not_armed")));
        let (status, body) = send(&app, "POST", "/api/v1/recording/arm", Some(r#"{"preset":"nope"}"#)).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("no_preset")));

        let (status, body) = send(&app, "POST", "/api/v1/recording/arm", Some(r#"{"preset":"band"}"#)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "armed");
        assert_eq!(body["preset"]["name"], "Band");
        assert_eq!(body["channels"].as_array().unwrap().len(), 2);
        assert!((body["preroll"]["preroll_seconds"].as_f64().unwrap() - 5.0).abs() < 0.1, "{body}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (status, body) = send(&app, "POST", "/api/v1/recording/record", Some("{}")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "recording");
        let (status, body) = send(&app, "POST", "/api/v1/recording/disarm", Some("{}")).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("confirm_disarm")));
        tokio::time::sleep(Duration::from_millis(400)).await;
        let (_, body) = send(&app, "POST", "/api/v1/recording/stop", None).await;
        assert_eq!(body["stopped"], true);
        assert_eq!(body["state"], "armed");
        let since = Instant::now();
        let takes = loop {
            let (_, body) = send(&app, "GET", "/api/v1/recording/takes", None).await;
            if !body["takes"].as_array().unwrap().is_empty() {
                break body["takes"].clone();
            }
            assert!(since.elapsed() < Duration::from_secs(10), "the take was never written");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let files = takes[0]["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| std::path::Path::new(f.as_str().unwrap()).starts_with(crate::config::loopback_recordings_dir())));
        let (status, body) = send(&app, "POST", "/api/v1/recording/disarm", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!((body["disarmed"].as_bool(), body["state"].as_str()), (Some(true), Some("off")));
        for file in files.iter().chain(std::iter::once(&takes[0]["log"])) {
            let _ = std::fs::remove_file(file.as_str().unwrap());
        }
    }

    #[tokio::test]
    async fn a_request_that_is_not_one_is_refused_in_the_usual_shape() {
        let (app, _) = app();
        let (status, body) = send(&app, "POST", "/api/v1/recording/arm", Some(r#"{"preset":"band","channels":[0]}"#)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_value");
        let (status, _) = send(&app, "POST", "/api/v1/recording/disarm", Some(r#"{"confirm":"yes"}"#)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
