//! The aggregate audio driver over HTTP: one answer that says whether this PC can run it, and
//! the fixes that answer implies.
//!
//! - `GET /api/v1/aggregate`: the drivers on this PC, whether ours is registered and what its
//!   class id points at, each configured device's live clock, rate, buffer and USB host
//!   controller, the driver's own live record and its event log, and a single ready or not ready
//!   with the reasons behind it. Every reason carries a code and, where Gazelle can put it right,
//!   the route and the value to send.
//! - `POST /api/v1/aggregate/match-buffers`: put every configured device on one buffer size,
//!   through the same write path and the same refusals as the per device driver route. Which
//!   interface each entry is comes from the one rule the answer uses
//!   (`crate::aggregate::service::match_device`), so a device the page can read is never one this
//!   route says it cannot find.
//! - `POST /api/v1/aggregate/register` and `/unregister`: run the registrar as an administrator,
//!   which is the one thing Gazelle does that asks for that. The answer always carries the
//!   command a person could run instead.
//! - `GET` and `POST /api/v1/aggregate/calibrate`, and `POST /api/v1/aggregate/calibrate/stop`:
//!   measure how far apart the interfaces really record, by playing a click and reading it back.
//!   One run at a time, on a thread of its own, followed while it goes and given up on when the
//!   person says so. It makes a noise in the room and holds both audio drivers while it runs, so
//!   everything that could stop it is refused before anything is opened
//!   (`crate::aggregate::calibrate`). The body may also name `witnesses`: extra input channels to
//!   record and report, which take no part in any trim.
//!
//! **Loopback peers only**, like the update and window routes. Registering a driver and changing
//! a DAW's buffer size are the machine's own business, not something a server reachable from a
//! network should offer.
//!
//! Merged into the router by `main.rs` with its service as an extension, as the driver and update
//! routes are, so `AppState` and every other caller of `http::router` are untouched. Reading a
//! driver loads a DLL and calls into it, so both the answer and the writes run on the blocking
//! pool.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::aggregate::calibrate::{Ask, Calibration};
use crate::aggregate::config::device_name;
use crate::aggregate::elevate::{arguments_for, command_for, DllSearch, REGISTRAR};
use crate::aggregate::service::{match_device, serial_of, AggregateService};
use crate::driver::{DriverChange, DriverWriteReport, WriteRefusal};
use crate::error::ServerError;
use crate::handover;
use crate::workspace::store::WorkspaceStore;

/// The server's `--dry-run`, carried to the write route as the driver route carries it.
#[derive(Clone, Copy)]
struct ForceDryRun(bool);

pub fn routes(service: Arc<AggregateService>, store: Arc<dyn WorkspaceStore>, force_dry_run: bool) -> Router {
    Router::new()
        .route("/api/v1/aggregate", get(read))
        .route("/api/v1/aggregate/match-buffers", post(match_buffers))
        .route("/api/v1/aggregate/register", post(register))
        .route("/api/v1/aggregate/unregister", post(unregister))
        .route("/api/v1/aggregate/calibrate", get(calibration).post(calibrate))
        .route("/api/v1/aggregate/calibrate/stop", post(calibrate_stop))
        .layer(Extension(service))
        .layer(Extension(store))
        .layer(Extension(Arc::new(Calibration::this_pc())))
        .layer(Extension(ForceDryRun(force_dry_run)))
}

/// An error body as every other route shapes one.
fn error(status: StatusCode, code: &str, message: String) -> Response {
    (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
}

/// Refuse anything that did not come from this machine. The peer is absent unless the server was
/// served with connect info, which `crate::http::serve` always does; an absent peer is refused
/// rather than trusted.
fn refuse_unless_local(request: &Request) -> Option<Response> {
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(peer)| *peer);
    (!handover::allowed(peer)).then(|| {
        error(
            StatusCode::FORBIDDEN,
            "not_local",
            "Only a program on this machine may read or change the aggregate driver's setup.".into(),
        )
    })
}

async fn read(
    Extension(service): Extension<Arc<AggregateService>>,
    Extension(store): Extension<Arc<dyn WorkspaceStore>>,
    request: Request,
) -> Result<Response, ServerError> {
    if let Some(refusal) = refuse_unless_local(&request) {
        return Ok(refusal);
    }
    let workspace = store.load()?;
    let answer = tokio::task::spawn_blocking(move || service.answer(&workspace))
        .await
        .map_err(|e| ServerError::Storage(format!("reading the aggregate's state stopped part way: {e}")))?;
    Ok(Json(answer).into_response())
}

/// Put every configured device on one buffer size.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MatchBuffers {
    buffer_size: u32,
    /// Change it even while a program is using a driver's interface, whose audio then restarts.
    #[serde(default)]
    force: bool,
}

/// What one device came to.
#[derive(Serialize)]
struct DeviceOutcome {
    /// The name the setup gives it.
    device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<DriverWriteReport>,
    /// Why nothing was sent to this device. Its codes are the driver route's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<WriteRefusal>,
}

async fn match_buffers(
    Extension(service): Extension<Arc<AggregateService>>,
    Extension(store): Extension<Arc<dyn WorkspaceStore>>,
    Extension(ForceDryRun(force_dry_run)): Extension<ForceDryRun>,
    request: Request,
) -> Result<Response, ServerError> {
    if let Some(refusal) = refuse_unless_local(&request) {
        return Ok(refusal);
    }
    let body: Result<Json<MatchBuffers>, JsonRejection> = <Json<MatchBuffers> as axum::extract::FromRequest<()>>::from_request(request, &()).await;
    let Json(ask) = body.map_err(|e| ServerError::BadValue(e.body_text()))?;
    let workspace = store.load()?;
    let Some(config) = workspace.aggregate.filter(|config| !config.devices.is_empty()) else {
        return Ok(error(
            StatusCode::CONFLICT,
            "not_configured",
            "No interfaces have been chosen for the aggregate yet, so there is nothing to match.".into(),
        ));
    };
    let wanted = config.devices.clone();

    let outcomes = tokio::task::spawn_blocking(move || {
        let change = DriverChange { buffer_size: Some(ask.buffer_size), safe_mode: None, force: ask.force };
        // Which interface each entry is, worked out exactly as the answer on the page worked it
        // out, so a device the page could read is never one this route says it cannot find.
        let entries = service.registry.entries().unwrap_or_default();
        let attached = service.devices.descriptors();
        wanted
            .iter()
            .map(|device| {
                let name = device_name(device);
                let found = match_device(device, &entries, &attached);
                let unavailable = |message: String, id: Option<String>| DeviceOutcome {
                    device: name.clone(),
                    device_id: id,
                    result: None,
                    error: Some(WriteRefusal { code: crate::driver::RefusalCode::Unavailable, message }),
                };
                let Some(descriptor) = found.descriptor else {
                    let why = found.note.unwrap_or_else(|| format!("{name} could not be matched to a connected interface, so its driver cannot be found."));
                    return unavailable(why, device.device_id.as_ref().map(|id| id.0.clone()));
                };
                let id = descriptor.id;
                let Some(serial) = serial_of(&id) else {
                    return unavailable(format!("{name} reports no serial, which is how its driver is found."), Some(id.0.clone()));
                };
                match service.driver.write(id.as_str(), serial, &change, force_dry_run) {
                    Ok(report) => DeviceOutcome { device: name, device_id: Some(id.0), result: Some(report), error: None },
                    Err(refusal) => DeviceOutcome { device: name, device_id: Some(id.0), result: None, error: Some(refusal) },
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| ServerError::Storage(format!("changing the drivers stopped part way: {e}")))?;

    let refused = outcomes.iter().filter(|outcome| outcome.error.is_some()).count();
    Ok(Json(json!({
        "buffer_size": ask.buffer_size,
        "changed": outcomes.len() - refused,
        "refused": refused,
        "devices": outcomes,
    }))
    .into_response())
}

/// How far the one measurement at a time has got. Idle is the ordinary answer.
async fn calibration(Extension(job): Extension<Arc<Calibration>>, request: Request) -> Result<Response, ServerError> {
    if let Some(refusal) = refuse_unless_local(&request) {
        return Ok(refusal);
    }
    Ok(Json(job.state()).into_response())
}

/// Start one. It plays a click out of a real output and holds both audio drivers while it goes,
/// so everything that could stop it is refused here or by the measuring itself, and nothing is
/// opened until all of it has passed.
async fn calibrate(
    Extension(job): Extension<Arc<Calibration>>,
    Extension(store): Extension<Arc<dyn WorkspaceStore>>,
    request: Request,
) -> Result<Response, ServerError> {
    if let Some(refusal) = refuse_unless_local(&request) {
        return Ok(refusal);
    }
    let body: Result<Json<Ask>, JsonRejection> = <Json<Ask> as axum::extract::FromRequest<()>>::from_request(request, &()).await;
    let Json(ask) = body.map_err(|e| ServerError::BadValue(e.body_text()))?;
    let workspace = store.load()?;
    if workspace.aggregate.is_none_or(|config| config.devices.len() < 2) {
        return Ok(error(
            StatusCode::CONFLICT,
            "not_configured",
            "Lining the interfaces up needs at least two of them in the aggregate, and there are not.".into(),
        ));
    }
    match job.start(&ask) {
        Ok(()) => Ok(Json(json!({ "started": true })).into_response()),
        Err(refusal) => Ok(error(StatusCode::CONFLICT, "not_started", refusal)),
    }
}

/// Give up on the run that is going. The click is in the room, so this is not something to make
/// somebody wait for: it is answered whether or not anything was running.
async fn calibrate_stop(Extension(job): Extension<Arc<Calibration>>, request: Request) -> Result<Response, ServerError> {
    if let Some(refusal) = refuse_unless_local(&request) {
        return Ok(refusal);
    }
    let running = job.is_running();
    job.stop();
    Ok(Json(json!({ "stopped": running })).into_response())
}

async fn register(Extension(service): Extension<Arc<AggregateService>>, request: Request) -> Result<Response, ServerError> {
    registration(service, request, false).await
}

async fn unregister(Extension(service): Extension<Arc<AggregateService>>, request: Request) -> Result<Response, ServerError> {
    registration(service, request, true).await
}

async fn registration(service: Arc<AggregateService>, request: Request, undo: bool) -> Result<Response, ServerError> {
    if let Some(refusal) = refuse_unless_local(&request) {
        return Ok(refusal);
    }
    let dll = match service.find_dll() {
        DllSearch::Found { dll } => std::path::PathBuf::from(dll),
        DllSearch::Missing { message, looked_in } => {
            return Ok((StatusCode::CONFLICT, Json(json!({ "error": { "code": "dll_not_found", "message": message, "looked_in": looked_in } }))).into_response())
        }
    };
    let command = command_for(&dll, undo);
    let arguments = arguments_for(&dll, undo);
    let elevator = service.elevator.clone();
    // The prompt is up while this runs, so it never runs on a runtime worker.
    let run = tokio::task::spawn_blocking(move || elevator.run(REGISTRAR, &arguments))
        .await
        .map_err(|e| ServerError::Storage(format!("asking for administrator rights stopped part way: {e}")))?;
    Ok(match run {
        Ok(run) => Json(json!({ "dll": dll.display().to_string(), "command": command, "run": run })).into_response(),
        // The command is given even then: it is the way out of whatever went wrong.
        Err(why) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": { "code": "not_elevated", "message": why }, "dll": dll.display().to_string(), "command": command })),
        )
            .into_response(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use gazelle_audio_transport::LoopbackDevice;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::aggregate::elevate::FakeElevator;
    use crate::aggregate::registry::{FakeRegistry, AGGREGATE_CLSID, AGGREGATE_NAME};
    use crate::aggregate::status::{AggregateStatus, DeviceStatus, FakeLink};
    use crate::aggregate::usb::FakeTopology;
    use crate::device::descriptor::DeviceId;
    use crate::device::manager::DeviceManager;
    use crate::driver::fake::{FakeDll, FakePc};
    use crate::driver::DriverService;
    use crate::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
    use crate::workspace::model::{Aggregate, AggregateDevice, Cable, CableEnd, Workspace};
    use crate::workspace::store::MemoryStore;

    const QUADRO: &str = "1000000000001";
    const STUDIO: &str = "2000000000002";
    const QUADRO_KEY: &str = "Zen Quadro Synergy Core";
    const STUDIO_KEY: &str = "ZenStudioTB ASIO Driver";

    struct Harness {
        app: Router,
        store: Arc<MemoryStore>,
        elevator: Arc<FakeElevator>,
        link: Arc<FakeLink>,
        quadro: Arc<FakeDll>,
        studio: Arc<FakeDll>,
    }

    /// Both interfaces attached over the loopback transport, both drivers registered, ours too,
    /// each interface on a controller of its own.
    fn harness() -> Harness {
        harness_with(FakeRegistry {
            entries: vec![
                FakeRegistry::entry(QUADRO_KEY, "{12217625-CB57-11EE-908D-7085C2FB2DD5}", r"C:\q.dll"),
                FakeRegistry::entry(STUDIO_KEY, "{AE4A4452-A316-11E5-A113-080027F6C1F4}", r"C:\s.dll"),
                FakeRegistry::entry(AGGREGATE_NAME, AGGREGATE_CLSID, r"C:\gazelle_aggregate.dll"),
            ],
            present: [r"C:\q.dll".into(), r"C:\s.dll".into(), r"C:\gazelle_aggregate.dll".into()].into_iter().collect(),
            failure: None,
        })
    }

    /// The same PC with both interfaces moved onto one USB host controller, which is what phase 0
    /// met as "not enough USB resources" at the second driver.
    fn one_controller() -> FakeTopology {
        FakeTopology::default()
            .on(0x23e5, PID_QUADRO, QUADRO, r"PCI\VEN_8086&DEV_15C0\3&11")
            .on(0x23e5, PID_STUDIO, STUDIO, r"PCI\VEN_8086&DEV_15C0\3&11")
    }

    fn two_controllers() -> FakeTopology {
        FakeTopology::default()
            .on(0x23e5, PID_QUADRO, QUADRO, r"PCI\VEN_8086&DEV_15C1\3&11")
            .on(0x23e5, PID_STUDIO, STUDIO, r"PCI\VEN_8086&DEV_15C0\3&11")
    }

    fn harness_with(registry: FakeRegistry) -> Harness {
        harness_full(registry, two_controllers())
    }

    fn harness_full(registry: FakeRegistry, tree: FakeTopology) -> Harness {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        devices.attach(DeviceId::from_serial(QUADRO), Box::new(LoopbackDevice::emulating(0x23e5, PID_QUADRO, 64)), "usb", true);
        devices.attach(DeviceId::from_serial(STUDIO), Box::new(LoopbackDevice::emulating(0x23e5, PID_STUDIO, 64)), "usb", true);
        let quadro = Arc::new(FakeDll::quadro(QUADRO));
        let studio = Arc::new(FakeDll::studio(STUDIO));
        let pc = Arc::new(FakePc::both(quadro.clone(), studio.clone()));
        let driver = Arc::new(DriverService::new(pc, Duration::from_millis(0)));
        let elevator = Arc::new(FakeElevator::default());
        let link = Arc::new(FakeLink::default());
        let service = Arc::new(AggregateService {
            devices: devices.clone(),
            driver,
            registry: Arc::new(registry),
            topology: Arc::new(tree),
            link: link.clone(),
            elevator: elevator.clone(),
            export_path: PathBuf::from(r"C:\appdata\gazelle\aggregate.json"),
            dll_candidates: vec![PathBuf::from(r"C:\nowhere\gazelle_aggregate.dll")],
        });
        let store = Arc::new(MemoryStore::default());
        Harness { app: routes(service, store.clone(), false), store, elevator, link, quadro, studio }
    }

    /// The setup phase 0 measured, with the Quadro driving the callback.
    fn pair() -> Aggregate {
        Aggregate {
            devices: vec![
                AggregateDevice { key: Some(QUADRO_KEY.into()), name: Some("Quadro".into()), device_id: Some(DeviceId::from_serial(QUADRO)), ..AggregateDevice::default() },
                AggregateDevice { key: Some("ZenStudioTB".into()), name: Some("Studio+".into()), device_id: Some(DeviceId::from_serial(STUDIO)), ..AggregateDevice::default() },
            ],
            callback_master: Some("Quadro".into()),
            alignment: Some("aligned".into()),
            rate: Some(96000),
            buffer_size: Some(512),
            extra: Default::default(),
        }
    }

    fn workspace_with(aggregate: Aggregate) -> Workspace {
        Workspace {
            aggregate: Some(aggregate),
            cables: vec![Cable {
                id: "c1".into(),
                from: CableEnd { device_id: DeviceId::from_serial(QUADRO), port: "SPDIF_OUT".into(), first: 0 },
                to: CableEnd { device_id: DeviceId::from_serial(STUDIO), port: "SPDIF_IN".into(), first: 0 },
                channels: 2,
            }],
            ..Workspace::default()
        }
    }

    /// A request from this machine, as the listener would report it.
    fn from_here(method: &str, uri: &str, body: Option<&str>) -> HttpRequest<Body> {
        let mut request = HttpRequest::builder().method(method).uri(uri);
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let mut request = request.body(body.map_or(Body::empty(), |b| Body::from(b.to_string()))).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5050))));
        request
    }

    async fn send(app: &Router, request: HttpRequest<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
        send(app, from_here("GET", uri, None)).await
    }

    async fn post(app: &Router, uri: &str, body: &str) -> (StatusCode, Value) {
        send(app, from_here("POST", uri, Some(body))).await
    }

    #[tokio::test]
    async fn nothing_has_been_measured_until_something_is_measured() {
        let h = harness();
        let (status, body) = get(&h.app, "/api/v1/aggregate/calibrate").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "idle");
        assert!(body.get("outcome").is_none() && body.get("refusal").is_none(), "{body}");
    }

    #[tokio::test]
    async fn one_interface_is_nothing_to_line_up_against_anything() {
        let h = harness();
        let mut alone = pair();
        alone.devices.truncate(1);
        h.store.save(&workspace_with(alone)).unwrap();
        let (status, body) = post(&h.app, "/api/v1/aggregate/calibrate", r#"{"direction":"inputs","outputs":[0],"inputs":[0]}"#).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], "not_configured");
    }

    #[tokio::test]
    async fn a_pass_that_is_not_one_is_refused_before_anything_is_opened() {
        let h = harness();
        h.store.save(&workspace_with(pair())).unwrap();
        let (status, body) = post(&h.app, "/api/v1/aggregate/calibrate", r#"{"direction":"sideways","outputs":[0,1],"inputs":[0,16]}"#).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], "not_started");
        assert!(body["error"]["message"].as_str().unwrap().contains("inputs or outputs"), "{body}");
        // And nothing was started by asking for something that is not a pass.
        let (_, state) = get(&h.app, "/api/v1/aggregate/calibrate").await;
        assert_eq!(state["state"], "idle");
    }

    /// A witness is an extra channel to listen in on, so one that is already being measured is not
    /// one, and the route says so in the same breath as every other cabling mistake.
    #[tokio::test]
    async fn a_witness_that_is_already_being_measured_is_refused_by_the_route() {
        let h = harness();
        h.store.save(&workspace_with(pair())).unwrap();
        let body = r#"{"direction":"inputs","outputs":[0,1],"inputs":[0,16],"witnesses":[16]}"#;
        let (status, body) = post(&h.app, "/api/v1/aggregate/calibrate", body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], "not_started");
        assert!(body["error"]["message"].as_str().unwrap().contains("already"), "{body}");
        let (_, state) = get(&h.app, "/api/v1/aggregate/calibrate").await;
        assert_eq!(state["state"], "idle");
    }

    /// Stopping is answered whether or not anything was going, because the point of it is the
    /// noise in the room, not the bookkeeping.
    #[tokio::test]
    async fn stopping_nothing_is_answered_and_changes_nothing() {
        let h = harness();
        let (status, body) = post(&h.app, "/api/v1/aggregate/calibrate/stop", "{}").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["stopped"], false);
    }

    #[tokio::test]
    async fn an_unconfigured_pc_is_not_ready_and_says_only_that() {
        let h = harness();
        let (status, body) = get(&h.app, "/api/v1/aggregate").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["configured"], false);
        assert_eq!(body["ready"], false);
        assert_eq!(body["reasons"][0]["code"], "not_configured");
        assert_eq!(body["reasons"].as_array().unwrap().len(), 1);
        assert_eq!(body["export_path"], r"C:\appdata\gazelle\aggregate.json");
        assert_eq!(body["drivers"].as_array().unwrap().len(), 3, "every driver on the PC is listed");
    }

    #[tokio::test]
    async fn a_configured_pc_reports_each_device_live_and_names_its_usb_controller() {
        let h = harness();
        h.store.save(&workspace_with(pair())).unwrap();
        let (status, body) = get(&h.app, "/api/v1/aggregate").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["configured"], true);
        let devices = body["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0]["name"], "Quadro");
        assert_eq!(devices[0]["is_master"], true);
        assert_eq!(devices[0]["registered"], true);
        assert_eq!(devices[0]["entry_key"], QUADRO_KEY);
        assert_eq!(devices[0]["attached"], true);
        assert_eq!(devices[0]["driver"]["buffer_size"], 512);
        assert_eq!(devices[0]["driver"]["asio_clients"], 0);
        assert_eq!(devices[0]["controller"]["instance_id"], r"PCI\VEN_8086&DEV_15C1\3&11");
        assert_eq!(devices[1]["controller"]["instance_id"], r"PCI\VEN_8086&DEV_15C0\3&11");
        assert_eq!(devices[1]["is_master"], false);
        assert_eq!(body["registration"]["registered"], true);
        assert!(body["registration"]["message"].as_str().unwrap().contains("is registered, pointing at"));
        // The devices the setup names are marked in the list of every driver on the PC.
        let drivers = body["drivers"].as_array().unwrap();
        assert_eq!(drivers.iter().filter(|d| d["configured"] == true).count(), 2);
        assert!(drivers.iter().any(|d| d["is_aggregate"] == true));
    }

    #[tokio::test]
    async fn two_devices_on_one_controller_are_the_reason_and_the_answer_says_which_two() {
        let apart = harness();
        apart.store.save(&workspace_with(pair())).unwrap();
        let (_, body) = get(&apart.app, "/api/v1/aggregate").await;
        assert!(
            body["reasons"].as_array().unwrap().iter().all(|r| r["code"] != "one_usb_controller"),
            "two controllers is what phase 0 needed: {body}"
        );

        let together = harness_full(
            FakeRegistry {
                entries: vec![
                    FakeRegistry::entry(QUADRO_KEY, "{1}", r"C:\q.dll"),
                    FakeRegistry::entry(STUDIO_KEY, "{2}", r"C:\s.dll"),
                    FakeRegistry::entry(AGGREGATE_NAME, AGGREGATE_CLSID, r"C:\a.dll"),
                ],
                present: [r"C:\q.dll".into(), r"C:\s.dll".into(), r"C:\a.dll".into()].into_iter().collect(),
                failure: None,
            },
            one_controller(),
        );
        together.store.save(&workspace_with(pair())).unwrap();
        let (_, body) = get(&together.app, "/api/v1/aggregate").await;
        let shared = body["reasons"].as_array().unwrap().iter().find(|r| r["code"] == "one_usb_controller").expect("{body}");
        assert_eq!(shared["device"], "Studio+");
        assert!(shared["message"].as_str().unwrap().contains("Quadro and Studio+ are both on"));
        assert_eq!(body["ready"], false);
    }

    #[tokio::test]
    async fn an_unregistered_driver_is_the_reason_and_the_fix_is_the_register_route() {
        let h = harness_with(FakeRegistry {
            entries: vec![FakeRegistry::entry(QUADRO_KEY, "{1}", r"C:\q.dll"), FakeRegistry::entry("ZenStudioTB", "{2}", r"C:\s.dll")],
            present: [r"C:\q.dll".into(), r"C:\s.dll".into()].into_iter().collect(),
            failure: None,
        });
        h.store.save(&workspace_with(pair())).unwrap();
        let (_, body) = get(&h.app, "/api/v1/aggregate").await;
        assert_eq!(body["registration"]["registered"], false);
        let reasons = body["reasons"].as_array().unwrap();
        let not_registered = reasons.iter().find(|r| r["code"] == "not_registered").expect("{body}");
        assert_eq!(not_registered["fix"]["route"], "aggregate/register");
        assert_eq!(not_registered["severity"], "blocking");
        assert_eq!(body["ready"], false);
    }

    #[tokio::test]
    async fn a_registration_pointing_at_a_copy_that_is_gone_says_so() {
        let h = harness_with(FakeRegistry {
            entries: vec![FakeRegistry::entry(AGGREGATE_NAME, AGGREGATE_CLSID, r"C:\moved\gazelle_aggregate.dll")],
            present: Default::default(),
            failure: None,
        });
        let (_, body) = get(&h.app, "/api/v1/aggregate").await;
        assert_eq!(body["registration"]["registered"], true);
        assert_eq!(body["registration"]["dll_present"], false);
        assert_eq!(body["registration"]["dll"], r"C:\moved\gazelle_aggregate.dll");
        assert!(body["registration"]["message"].as_str().unwrap().contains("is not there any more"));
    }

    #[tokio::test]
    async fn a_device_the_pc_does_not_register_is_named() {
        let h = harness_with(FakeRegistry {
            entries: vec![FakeRegistry::entry(QUADRO_KEY, "{1}", r"C:\q.dll"), FakeRegistry::entry(AGGREGATE_NAME, AGGREGATE_CLSID, r"C:\a.dll")],
            present: [r"C:\q.dll".into(), r"C:\a.dll".into()].into_iter().collect(),
            failure: None,
        });
        h.store.save(&workspace_with(pair())).unwrap();
        let (_, body) = get(&h.app, "/api/v1/aggregate").await;
        let missing = body["reasons"].as_array().unwrap().iter().find(|r| r["code"] == "device_missing").expect("{body}");
        assert_eq!(missing["device"], "Studio+");
        assert_eq!(body["devices"][1]["registered"], false);
    }

    #[tokio::test]
    async fn the_drivers_live_record_and_its_event_log_come_back_with_the_answer() {
        let h = harness();
        *h.link.status.lock().unwrap() = crate::aggregate::status::StatusReading::Read(AggregateStatus {
            open: true,
            streaming: true,
            generation: 3,
            generation_in_force: 3,
            up_to_date: true,
            plan: Some(crate::aggregate::status::AggregatePlan { master: "Quadro".into(), rate: 96000, buffer_size: 512, inputs: 40, outputs: 40, alignment: "aligned".into(), input_latency: 1148, output_latency: 1212 }),
            devices: vec![DeviceStatus { name: "Studio+".into(), streaming: true, sample_gap: 512, callbacks: 900, ..DeviceStatus::default() }],
            last_refusal: Some("the Studio+ would not take 44100".into()),
            config_source: Some(r"C:ppdata\gazelleggregate.json".into()),
        });
        h.link.events.lock().unwrap().push(crate::aggregate::status::AggregateEvent { at: "2026-09-20 21:14:07".into(), kind: "refused".into(), message: "not enough USB resources".into() });
        let (_, body) = get(&h.app, "/api/v1/aggregate").await;
        assert_eq!(body["status"]["state"], "read");
        assert_eq!(body["status"]["open"], true);
        assert_eq!(body["status"]["generation"], 3);
        assert_eq!(body["status"]["up_to_date"], true);
        assert_eq!(body["status"]["plan"]["input_latency"], 1148);
        assert_eq!(body["status"]["devices"][0]["sample_gap"], 512);
        assert_eq!(body["status"]["last_refusal"], "the Studio+ would not take 44100");
        assert_eq!(body["events"][0]["message"], "not enough USB resources");
    }

    #[tokio::test]
    async fn a_pc_where_the_driver_has_published_nothing_says_so_rather_than_failing() {
        let h = harness();
        let (status, body) = get(&h.app, "/api/v1/aggregate").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"]["state"], "silent");
        assert_eq!(body["events"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn matching_buffers_changes_every_configured_device() {
        let h = harness();
        h.store.save(&workspace_with(pair())).unwrap();
        let (status, body) = post(&h.app, "/api/v1/aggregate/match-buffers", r#"{"buffer_size":256}"#).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["changed"], 2);
        assert_eq!(body["refused"], 0);
        assert_eq!(body["devices"][0]["device"], "Quadro");
        assert_eq!(body["devices"][0]["result"]["outcome"], "applied");
        assert_eq!(body["devices"][0]["result"]["read_back"]["asio"]["value"]["buffer_size"], 256);
        assert_eq!(h.quadro.sets().len(), 1);
        assert_eq!(h.studio.sets().len(), 1);
    }

    #[tokio::test]
    async fn a_device_whose_interface_is_in_use_is_refused_by_name_until_it_is_forced() {
        let h = harness();
        // The Studio+ has a DAW on it; the Quadro does not.
        let studio = Arc::new(FakeDll::studio(STUDIO).with_asio(crate::driver::asio::tests::quadro_in_use(2)));
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        devices.attach(DeviceId::from_serial(QUADRO), Box::new(LoopbackDevice::emulating(0x23e5, PID_QUADRO, 64)), "usb", true);
        devices.attach(DeviceId::from_serial(STUDIO), Box::new(LoopbackDevice::emulating(0x23e5, PID_STUDIO, 64)), "usb", true);
        let quadro = Arc::new(FakeDll::quadro(QUADRO));
        let service = Arc::new(AggregateService {
            devices: devices.clone(),
            driver: Arc::new(DriverService::new(Arc::new(FakePc::both(quadro.clone(), studio.clone())), Duration::from_millis(0))),
            registry: Arc::new(FakeRegistry::default()),
            topology: Arc::new(FakeTopology::default()),
            link: Arc::new(FakeLink::default()),
            elevator: Arc::new(FakeElevator::default()),
            export_path: PathBuf::from("aggregate.json"),
            dll_candidates: Vec::new(),
        });
        let store = Arc::new(MemoryStore::default());
        store.save(&workspace_with(pair())).unwrap();
        let app = routes(service, store, false);

        let (status, body) = post(&app, "/api/v1/aggregate/match-buffers", r#"{"buffer_size":256}"#).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["changed"], 1);
        assert_eq!(body["refused"], 1);
        assert_eq!(body["devices"][1]["error"]["code"], "asio_in_use");
        assert!(studio.sets().is_empty(), "nothing was sent to the one that refused");
        assert_eq!(quadro.sets().len(), 1, "and the other one was still changed");

        let (_, body) = post(&app, "/api/v1/aggregate/match-buffers", r#"{"buffer_size":256,"force":true}"#).await;
        assert_eq!(body["refused"], 0, "{body}");
        assert_eq!(studio.sets().len(), 1);
        let _ = h;
    }

    #[tokio::test]
    async fn matching_buffers_refuses_a_size_no_driver_offers_and_an_unknown_field() {
        let h = harness();
        h.store.save(&workspace_with(pair())).unwrap();
        let (status, body) = post(&h.app, "/api/v1/aggregate/match-buffers", r#"{"buffer_size":500}"#).await;
        assert_eq!(status, StatusCode::OK, "each device says why on its own: {body}");
        assert_eq!(body["refused"], 2);
        assert_eq!(body["devices"][0]["error"]["code"], "not_offered");
        assert!(h.quadro.sets().is_empty());
        let (status, body) = post(&h.app, "/api/v1/aggregate/match-buffers", r#"{"buffer":256}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_value", "{body}");
    }

    #[tokio::test]
    async fn matching_buffers_with_nothing_configured_is_refused_and_nothing_is_sent() {
        let h = harness();
        let (status, body) = post(&h.app, "/api/v1/aggregate/match-buffers", r#"{"buffer_size":256}"#).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "not_configured");
        assert!(h.quadro.sets().is_empty());
    }

    /// The bug this route had: a setup that never said which interface each entry is read fine on
    /// the page, because the answer works the model out, and then refused every write.
    #[tokio::test]
    async fn a_setup_that_names_no_interface_is_still_written_to_because_the_model_settles_it() {
        let h = harness();
        let mut config = pair();
        for device in &mut config.devices {
            device.device_id = None;
        }
        config.devices[1].key = Some(STUDIO_KEY.into());
        h.store.save(&workspace_with(config)).unwrap();

        let (_, body) = get(&h.app, "/api/v1/aggregate").await;
        assert_eq!(body["devices"][0]["matched_by"], "worked_out");
        assert_eq!(body["devices"][0]["device_id"], format!("serial:{QUADRO}"));
        assert!(body["devices"][0].get("match_note").is_none(), "nothing to explain when it was worked out");
        assert_eq!(body["devices"][0]["channels"]["source"], "gazelle");
        assert_eq!(body["devices"][0]["channels"]["inputs"][0], "Preamp 1");
        assert_eq!(body["devices"][0]["channels"]["outputs"][0], "Monitor L");
        assert!(
            body["reasons"].as_array().unwrap().iter().all(|r| r["code"] != "device_not_matched"),
            "one of each model is plugged in, so there is nothing to ask: {body}"
        );

        let (status, body) = post(&h.app, "/api/v1/aggregate/match-buffers", r#"{"buffer_size":256}"#).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["changed"], 2, "{body}");
        assert_eq!(body["devices"][0]["device_id"], format!("serial:{QUADRO}"), "and it says which interface it wrote to");
        assert_eq!(h.quadro.sets().len(), 1);
        assert_eq!(h.studio.sets().len(), 1);
    }

    /// The one case where it genuinely cannot be worked out. The refusal says what would settle it,
    /// and the answer carries the same words against the device itself.
    #[tokio::test]
    async fn a_device_gazelle_cannot_tell_apart_is_refused_by_name_and_the_answer_says_why() {
        let h = harness_with(FakeRegistry {
            entries: vec![
                FakeRegistry::entry(QUADRO_KEY, "{1}", r"C:\q.dll"),
                FakeRegistry::entry("Focusrite USB", "{2}", r"C:\f.dll"),
                FakeRegistry::entry(AGGREGATE_NAME, AGGREGATE_CLSID, r"C:\a.dll"),
            ],
            present: [r"C:\q.dll".into(), r"C:\f.dll".into(), r"C:\a.dll".into()].into_iter().collect(),
            failure: None,
        });
        let mut config = pair();
        config.devices[0].device_id = None;
        config.devices[1] = AggregateDevice { key: Some("Focusrite USB".into()), name: Some("Focusrite".into()), ..AggregateDevice::default() };
        h.store.save(&workspace_with(config)).unwrap();

        let (_, body) = get(&h.app, "/api/v1/aggregate").await;
        assert_eq!(body["devices"][1]["matched_by"], "none");
        assert_eq!(body["devices"][1]["attached"], false);
        assert_eq!(body["devices"][1]["channels"]["source"], "none");
        assert!(body["devices"][1]["match_note"].as_str().unwrap().contains("does not say which model it is"), "{body}");
        let unmatched = body["reasons"].as_array().unwrap().iter().find(|r| r["code"] == "device_not_matched").expect("{body}");
        assert_eq!(unmatched["device"], "Focusrite");
        assert_eq!(unmatched["severity"], "warning");

        let (_, body) = post(&h.app, "/api/v1/aggregate/match-buffers", r#"{"buffer_size":256}"#).await;
        assert_eq!(body["changed"], 1, "the one it could work out was still changed: {body}");
        assert_eq!(body["devices"][1]["error"]["code"], "unavailable");
        assert!(body["devices"][1]["error"]["message"].as_str().unwrap().contains("Choose it on its card"));
        assert_eq!(h.quadro.sets().len(), 1);
        assert!(h.studio.sets().is_empty());
    }

    #[tokio::test]
    async fn a_device_that_is_not_connected_is_said_to_be_rather_than_reached_for() {
        let h = harness();
        let mut config = pair();
        config.devices[1].device_id = Some(DeviceId::from_serial("not here"));
        h.store.save(&workspace_with(config)).unwrap();
        let (_, body) = post(&h.app, "/api/v1/aggregate/match-buffers", r#"{"buffer_size":256}"#).await;
        assert_eq!(body["devices"][1]["error"]["code"], "unavailable");
        assert!(body["devices"][1]["error"]["message"].as_str().unwrap().contains("is not connected to Gazelle"));
        assert!(h.studio.sets().is_empty());
    }

    #[tokio::test]
    async fn registering_runs_the_registrar_elevated_and_gives_back_the_command_as_well() {
        let dll = std::env::temp_dir().join(format!("gazelle-agg-{}-gazelle_aggregate.dll", std::process::id()));
        std::fs::write(&dll, b"not really a dll").unwrap();
        let mut h = harness();
        let service = Arc::new(AggregateService {
            devices: DeviceManager::new(RegistrySet::builtin().unwrap()),
            driver: Arc::new(DriverService::new(Arc::new(FakePc::both(h.quadro.clone(), h.studio.clone())), Duration::from_millis(0))),
            registry: Arc::new(FakeRegistry::default()),
            topology: Arc::new(FakeTopology::default()),
            link: h.link.clone(),
            elevator: h.elevator.clone(),
            export_path: PathBuf::from("aggregate.json"),
            dll_candidates: vec![dll.clone()],
        });
        h.app = routes(service, h.store.clone(), false);

        let (status, body) = post(&h.app, "/api/v1/aggregate/register", "{}").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["run"]["started"], true);
        assert_eq!(body["run"]["exit_code"], 0);
        assert_eq!(body["command"], format!("regsvr32 \"{}\"", dll.display()));
        assert_eq!(h.elevator.calls(), vec![(REGISTRAR.to_string(), format!("/s \"{}\"", dll.display()))]);

        let (_, body) = post(&h.app, "/api/v1/aggregate/unregister", "{}").await;
        assert_eq!(body["command"], format!("regsvr32 /u \"{}\"", dll.display()));
        assert_eq!(h.elevator.calls()[1].1, format!("/u /s \"{}\"", dll.display()));
        let _ = std::fs::remove_file(&dll);
    }

    #[tokio::test]
    async fn registering_with_no_dll_says_where_it_looked_and_elevates_nothing() {
        let h = harness();
        let (status, body) = post(&h.app, "/api/v1/aggregate/register", "{}").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "dll_not_found");
        assert_eq!(body["error"]["looked_in"][0], r"C:\nowhere\gazelle_aggregate.dll");
        assert!(h.elevator.calls().is_empty());
    }

    #[tokio::test]
    async fn a_request_from_the_network_is_refused_and_nothing_is_read_or_run() {
        let h = harness();
        h.store.save(&workspace_with(pair())).unwrap();
        for (method, uri, body) in [
            ("GET", "/api/v1/aggregate", None),
            ("POST", "/api/v1/aggregate/match-buffers", Some(r#"{"buffer_size":256}"#)),
            ("POST", "/api/v1/aggregate/register", Some("{}")),
            ("POST", "/api/v1/aggregate/unregister", Some("{}")),
        ] {
            let mut request = HttpRequest::builder().method(method).uri(uri).header("content-type", "application/json");
            let _ = &mut request;
            let mut request = request.body(body.map_or(Body::empty(), |b| Body::from(b.to_string()))).unwrap();
            request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 5], 5050))));
            let (status, answer) = send(&h.app, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {answer}");
            assert_eq!(answer["error"]["code"], "not_local");
        }
        assert!(h.elevator.calls().is_empty());
        assert!(h.quadro.sets().is_empty());
    }
}
