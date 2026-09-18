//! `GET /api/v1/devices/{id}/driver`: the audio driver's settings for one device.
//! `PUT /api/v1/devices/{id}/driver`: change its buffer size and/or Safe Mode.
//!
//! Merged into the router by `main.rs`, carrying the device list and the driver reader as
//! extensions, as the update routes do, so `AppState` and every other caller of `http::router`
//! are untouched. An answer is kept a few seconds (`driver::CACHE_FOR`); `?refresh=true` asks the
//! driver again. The read may load a DLL and call into it, so it runs on the blocking pool.
//!
//! A `PUT` takes `{"buffer_size"?, "safe_mode"?, "force"?}`. **An error status means nothing was
//! sent to the driver**: 400 for a request that cannot be built (nothing to change, a size the
//! driver does not offer, a malformed body), 409 when the driver cannot be changed now (no driver,
//! unreadable settings, or a program using its ASIO interface without `force`). Once a call is
//! sent the answer is 200 with its `outcome` (`applied`, `mismatch`, `unconfirmed`, `failed`), the
//! call as sent, and the driver's read-back. `?dry_run=true`, or a server run with `--dry-run`,
//! builds the call and sends nothing.

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;

use crate::device::descriptor::DeviceId;
use crate::device::manager::DeviceManager;
use crate::driver::{now_ms, DriverAnswer, DriverChange, DriverReport, DriverService, RefusalCode, WriteRefusal};
use crate::error::ServerError;

/// The server's `--dry-run`, carried to the write route.
#[derive(Clone, Copy)]
struct ForceDryRun(bool);

pub fn routes(devices: Arc<DeviceManager>, driver: Arc<DriverService>, force_dry_run: bool) -> Router {
    Router::new()
        .route("/api/v1/devices/{id}/driver", get(read).put(write))
        .layer(Extension(devices))
        .layer(Extension(driver))
        .layer(Extension(ForceDryRun(force_dry_run)))
}

#[derive(Deserialize, Default)]
struct WriteQuery {
    #[serde(default)]
    dry_run: bool,
}

/// A refusal as the other routes' errors look: `{"error": {"code", "message"}}`.
fn refused(refusal: WriteRefusal) -> Response {
    let status = match refusal.code {
        RefusalCode::NothingToChange | RefusalCode::NotOffered => StatusCode::BAD_REQUEST,
        RefusalCode::Unavailable | RefusalCode::Unreadable | RefusalCode::AsioInUse => StatusCode::CONFLICT,
    };
    (status, Json(serde_json::json!({ "error": refusal }))).into_response()
}

async fn write(
    Extension(devices): Extension<Arc<DeviceManager>>,
    Extension(driver): Extension<Arc<DriverService>>,
    Extension(ForceDryRun(force_dry_run)): Extension<ForceDryRun>,
    Path(id): Path<String>,
    Query(query): Query<WriteQuery>,
    body: Result<Json<DriverChange>, JsonRejection>,
) -> Result<Response, ServerError> {
    let descriptor = devices.descriptor(&DeviceId(id.clone()))?;
    let Json(change) = body.map_err(|e| ServerError::BadValue(e.body_text()))?;
    if descriptor.backend != "usb" {
        let message = format!("The {} backend has no audio driver on this PC.", descriptor.backend);
        return Ok(refused(WriteRefusal { code: RefusalCode::Unavailable, message }));
    }
    let Some(serial) = id.strip_prefix("serial:").map(str::to_string) else {
        let message = "This device reports no serial, which is how its driver is found.".to_string();
        return Ok(refused(WriteRefusal { code: RefusalCode::Unavailable, message }));
    };
    let dry_run = query.dry_run || force_dry_run;
    let written = tokio::task::spawn_blocking(move || driver.write(&id, &serial, &change, dry_run))
        .await
        .map_err(|e| ServerError::Storage(format!("changing the driver stopped part way: {e}")))?;
    Ok(match written {
        Ok(report) => Json(report).into_response(),
        Err(refusal) => refused(refusal),
    })
}

#[derive(Deserialize, Default)]
struct Ask {
    #[serde(default)]
    refresh: bool,
}

async fn read(
    Extension(devices): Extension<Arc<DeviceManager>>,
    Extension(driver): Extension<Arc<DriverService>>,
    Path(id): Path<String>,
    Query(ask): Query<Ask>,
) -> Result<Json<DriverReport>, ServerError> {
    let descriptor = devices.descriptor(&DeviceId(id.clone()))?;
    let answer = |answer| DriverReport { device_id: id.clone(), read_at_ms: now_ms(), cached: false, answer };
    if descriptor.backend != "usb" {
        return Ok(Json(answer(DriverAnswer::NoDriver {
            message: format!("The {} backend has no audio driver on this PC.", descriptor.backend),
        })));
    }
    let Some(serial) = id.strip_prefix("serial:").map(str::to_string) else {
        return Ok(Json(answer(DriverAnswer::NotFound {
            message: "This device reports no serial, which is how its driver is found.".into(),
        })));
    };
    let report = tokio::task::spawn_blocking(move || driver.read(&id, &serial, ask.refresh)).await;
    Ok(Json(report.unwrap_or_else(|e| DriverReport {
        device_id: descriptor.id.to_string(),
        read_at_ms: now_ms(),
        cached: false,
        answer: DriverAnswer::Failed { message: format!("Reading the driver stopped part way: {e}.") },
    })))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gazelle_audio_transport::LoopbackDevice;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::driver::fake::{FakeDll, FakePc};
    use crate::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};

    const QUADRO: &str = "1000000000001";

    /// A loopback, a device that looks attached over USB with a serial, and one without.
    fn app() -> (Router, Arc<FakePc>) {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        devices.attach_loopbacks(&[PID_QUADRO], 64);
        devices.attach(DeviceId::from_serial(QUADRO), Box::new(LoopbackDevice::emulating(0x23e5, PID_QUADRO, 64)), "usb", true);
        devices.attach(DeviceId::from_topology(0x23e5, PID_STUDIO, 1, 7), Box::new(LoopbackDevice::emulating(0x23e5, PID_STUDIO, 64)), "usb", false);
        let pc = Arc::new(FakePc::both(Arc::new(FakeDll::quadro(QUADRO)), Arc::new(FakeDll::studio("other"))));
        let driver = Arc::new(DriverService::new(pc.clone(), Duration::from_secs(60)));
        (routes(devices, driver, false), pc)
    }

    /// The same, with the Quadro's driver given to the test and the server's dry run chosen.
    fn app_with(quadro: Arc<FakeDll>, force_dry_run: bool) -> Router {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        devices.attach_loopbacks(&[PID_QUADRO], 64);
        devices.attach(DeviceId::from_serial(QUADRO), Box::new(LoopbackDevice::emulating(0x23e5, PID_QUADRO, 64)), "usb", true);
        let pc = Arc::new(FakePc::both(quadro, Arc::new(FakeDll::studio("other"))));
        routes(devices, Arc::new(DriverService::new(pc, Duration::from_secs(60))), force_dry_run)
    }

    async fn put(app: &Router, uri: &str, body: &str) -> (StatusCode, Value) {
        let request = Request::builder().method("PUT").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    const QUADRO_DRIVER: &str = "/api/v1/devices/serial:1000000000001/driver";

    #[tokio::test]
    async fn a_put_changes_the_buffer_and_answers_the_read_back() {
        let quadro = Arc::new(FakeDll::quadro(QUADRO));
        let app = app_with(quadro.clone(), false);
        let (status, body) = put(&app, QUADRO_DRIVER, r#"{"buffer_size":256}"#).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["outcome"], "applied");
        assert_eq!(body["call"]["options"], 0x10000, "Safe Mode kept as it was");
        assert_eq!(body["read_back"]["asio"]["value"]["buffer_size"], 256);
        assert_eq!(body["read_back"]["asio"]["value"]["input_latency"], 315);
        assert_eq!(quadro.sets().len(), 1);
        // A read afterwards is the read-back, not the answer cached before it.
        let (_, read) = get(&app, QUADRO_DRIVER).await;
        assert_eq!(read["asio"]["value"]["buffer_size"], 256);
    }

    #[tokio::test]
    async fn a_put_changes_safe_mode_alone() {
        let quadro = Arc::new(FakeDll::quadro(QUADRO));
        let app = app_with(quadro.clone(), false);
        let (status, body) = put(&app, QUADRO_DRIVER, r#"{"safe_mode":false}"#).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["call"], serde_json::json!({ "asio_instance": 0, "reference_sample_rate": 44100, "preferred_size": 512, "options": 0 }));
        assert_eq!(body["read_back"]["safe_mode"]["value"], false);
    }

    #[tokio::test]
    async fn an_unoffered_size_is_a_400_and_nothing_is_sent() {
        let quadro = Arc::new(FakeDll::quadro(QUADRO));
        let app = app_with(quadro.clone(), false);
        let (status, body) = put(&app, QUADRO_DRIVER, r#"{"buffer_size":500}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "not_offered", "{body}");
        let (status, body) = put(&app, QUADRO_DRIVER, r#"{}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "nothing_to_change");
        let (status, body) = put(&app, QUADRO_DRIVER, r#"{"buffer":256}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "an unknown field is not ignored");
        assert_eq!(body["error"]["code"], "bad_value", "{body}");
        assert!(quadro.sets().is_empty());
    }

    #[tokio::test]
    async fn asio_in_use_is_a_409_until_forced() {
        let quadro = Arc::new(FakeDll::quadro(QUADRO).with_asio(crate::driver::asio::tests::quadro_in_use(1)));
        let app = app_with(quadro.clone(), false);
        let (status, body) = put(&app, QUADRO_DRIVER, r#"{"buffer_size":256}"#).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "asio_in_use", "{body}");
        assert!(quadro.sets().is_empty());
        let (status, body) = put(&app, QUADRO_DRIVER, r#"{"buffer_size":256,"force":true}"#).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(quadro.sets().len(), 1);
    }

    #[tokio::test]
    async fn a_mismatch_is_answered_as_one() {
        let quadro = Arc::new(FakeDll { ignores_set: true, ..FakeDll::quadro(QUADRO) });
        let app = app_with(quadro, false);
        let (status, body) = put(&app, QUADRO_DRIVER, r#"{"buffer_size":256}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["outcome"], "mismatch");
        assert_eq!(body["read_back"]["asio"]["value"]["buffer_size"], 512);
    }

    #[tokio::test]
    async fn a_dry_run_server_or_request_sends_nothing() {
        let quadro = Arc::new(FakeDll::quadro(QUADRO));
        let (_, body) = put(&app_with(quadro.clone(), true), QUADRO_DRIVER, r#"{"buffer_size":256}"#).await;
        assert_eq!(body["outcome"], "dry_run");
        let (_, body) = put(&app_with(quadro.clone(), false), &format!("{QUADRO_DRIVER}?dry_run=true"), r#"{"buffer_size":256}"#).await;
        assert_eq!(body["outcome"], "dry_run");
        assert!(quadro.sets().is_empty());
    }

    #[tokio::test]
    async fn the_loopback_cannot_be_changed_and_nothing_is_loaded() {
        let (app, pc) = app();
        let (status, body) = put(&app, "/api/v1/devices/loopback-0/driver", r#"{"buffer_size":256}"#).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "unavailable");
        assert_eq!(body["error"]["message"], "The loopback backend has no audio driver on this PC.");
        assert_eq!(pc.loads(), 0);
        let (status, _) = put(&app, "/api/v1/devices/serial:nobody/driver", r#"{"buffer_size":256}"#).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
        let response = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn a_usb_device_reads_its_driver_settings() {
        let (app, _) = app();
        let (status, body) = get(&app, "/api/v1/devices/serial:1000000000001/driver").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["device_id"], "serial:1000000000001");
        assert_eq!(body["state"], "read");
        assert_eq!(body["asio"]["value"]["buffer_size"], 512);
        assert_eq!(body["asio"]["value"]["output_latency"], 632);
        assert_eq!(body["safe_mode"]["value"], true);
        assert_eq!(body["cached"], false);
    }

    #[tokio::test]
    async fn an_answer_is_cached_until_a_refresh_is_asked_for() {
        let (app, pc) = app();
        get(&app, "/api/v1/devices/serial:1000000000001/driver").await;
        let (_, body) = get(&app, "/api/v1/devices/serial:1000000000001/driver").await;
        assert_eq!(body["cached"], true);
        assert_eq!(pc.loads(), 1);
        let (_, body) = get(&app, "/api/v1/devices/serial:1000000000001/driver?refresh=true").await;
        assert_eq!(body["cached"], false);
        assert_eq!(pc.loads(), 2);
    }

    #[tokio::test]
    async fn the_loopback_has_no_driver_and_nothing_is_loaded_for_it() {
        let (app, pc) = app();
        let (status, body) = get(&app, "/api/v1/devices/loopback-0/driver").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"], "no_driver");
        assert_eq!(body["message"], "The loopback backend has no audio driver on this PC.");
        assert_eq!(pc.loads(), 0);
    }

    #[tokio::test]
    async fn a_device_with_no_serial_cannot_be_matched() {
        let (app, pc) = app();
        let (_, body) = get(&app, "/api/v1/devices/usb:23e5:a100:1:7/driver").await;
        assert_eq!(body["state"], "not_found");
        assert!(body["message"].as_str().unwrap().contains("no serial"));
        assert_eq!(pc.loads(), 0);
    }

    #[tokio::test]
    async fn an_unknown_device_is_a_404() {
        let (app, _) = app();
        let (status, body) = get(&app, "/api/v1/devices/serial:nobody/driver").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "unknown_device", "{body}");
    }
}
