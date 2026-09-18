//! `GET /api/v1/devices/{id}/driver`: the audio driver's settings for one device, read only.
//!
//! Merged into the router by `main.rs`, carrying the device list and the driver reader as
//! extensions, as the update routes do, so `AppState` and every other caller of `http::router`
//! are untouched. An answer is kept a few seconds (`driver::CACHE_FOR`); `?refresh=true` asks the
//! driver again. The read may load a DLL and call into it, so it runs on the blocking pool.

use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;

use crate::device::descriptor::DeviceId;
use crate::device::manager::DeviceManager;
use crate::driver::{now_ms, DriverAnswer, DriverReport, DriverService};
use crate::error::ServerError;

pub fn routes(devices: Arc<DeviceManager>, driver: Arc<DriverService>) -> Router {
    Router::new()
        .route("/api/v1/devices/{id}/driver", get(read))
        .layer(Extension(devices))
        .layer(Extension(driver))
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
        (routes(devices, driver), pc)
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
