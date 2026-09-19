//! Recall's plan over HTTP, against loopbacks.
//!
//! **Nothing here writes to a device**, and nothing here is a device: the loopback answers every
//! `get_*` and pushes its cyclic report, and the bytes every step carries come from the server's own
//! dry run, which stops before the wire. Applying a plan waits for a hardware session; the
//! route that will do it refuses here.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
use gazelle_audio_server::snapshot::model::Snapshot;
use gazelle_audio_server::snapshot::plan::leaf_paths;
use gazelle_audio_server::snapshot::store::{MemorySnapshotStore, SnapshotStore};
use gazelle_audio_server::snapshot::writers::{writer_for, Target, Writer};
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

fn harness(dry_run: bool, enable_recall: bool) -> axum::Router {
    let registries = RegistrySet::builtin().expect("registries");
    let devices = DeviceManager::new(registries);
    devices.attach_cyclic_loopbacks(&[PID_QUADRO, PID_STUDIO], 64, Duration::from_millis(10));
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    let snapshots: Arc<dyn SnapshotStore> = Arc::new(MemorySnapshotStore::default());
    http::router(AppState {
        devices,
        store,
        snapshots,
        force_dry_run: dry_run,
        enable_recall,
        backend: "loopback".into(),
        themes_dir: None,
        show_window: None,
    })
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(body) => request.header("content-type", "application/json").body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .expect("request");
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Take a snapshot once both devices have pushed a state report, so it holds every section.
async fn take(app: &axum::Router) -> String {
    for _ in 0..200 {
        let (status, body) = call(app, "POST", "/api/v1/snapshots", Some(json!({ "name": "Drum tracking" }))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["id"].as_str().expect("an id").to_string();
        let (_, snapshot) = call(app, "GET", &format!("/api/v1/snapshots/{id}"), None).await;
        let ready = ["loopback-0", "loopback-1"]
            .iter()
            .all(|device| snapshot["devices"][device]["unreadable"].as_array().is_some_and(Vec::is_empty) && snapshot["devices"][device]["sections"]["clock"].is_object());
        if ready {
            return id;
        }
        call(app, "DELETE", &format!("/api/v1/snapshots/{id}"), None).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the loopbacks never reported their state");
}

/// Every leaf path a capture of both models can hold maps to a writer, a named reason, or a
/// withheld command — and never to nothing.
///
/// This is the test that fails the day capture learns a new field: the snapshot comes from the
/// server's own capture, so the table cannot drift away from the read plan without saying so.
#[tokio::test]
async fn a_writer_covers_every_path_a_capture_can_record() {
    let app = harness(false, false);
    let id = take(&app).await;
    let (_, body) = call(&app, "GET", &format!("/api/v1/snapshots/{id}"), None).await;
    let snapshot: Snapshot = serde_json::from_value(body).expect("a snapshot");

    let mut unmapped: Vec<String> = Vec::new();
    let mut commands: BTreeSet<String> = BTreeSet::new();
    let mut named: usize = 0;
    let mut checked: usize = 0;
    for (device_id, device) in &snapshot.devices {
        assert!(!device.family.is_empty(), "{device_id} has no family");
        for (section, path) in leaf_paths(&device.sections) {
            checked += 1;
            match writer_for(&device.family, &section, &path) {
                Writer::Unmapped => unmapped.push(format!("{device_id} {section}.{path}")),
                Writer::NoWriter { reason } | Writer::Withheld { reason, .. } => {
                    assert!(reason.len() > 30, "{section}.{path} needs a reason a person can act on");
                    named += 1;
                }
                Writer::Command(target) => {
                    // Every mapped target must build from this very snapshot: a writer that cannot
                    // produce its command is not a writer. A gain array is one captured value and
                    // one command per channel, so it stands in for its first channel here.
                    let target = match target {
                        Target::Gains { kind } => Target::Gain { kind, id: 0 },
                        other => other,
                    };
                    let write = target.build(&device.family, &device.sections).unwrap_or_else(|e| panic!("{device_id} {section}.{path}: {e}"));
                    commands.insert(write.command);
                }
            }
        }
    }
    assert!(unmapped.is_empty(), "capture records values no writer is mapped for:\n{}", unmapped.join("\n"));
    assert!(checked > 2_000, "only {checked} paths were checked — the capture was not full");
    assert!(named > 10, "only {named} paths are explicitly named as having no writer");
    // The whole write surface recall uses, so a command silently dropped from the table shows up.
    let expected: BTreeSet<String> = [
        // The Studio+ panel sets its digital input gains; the Quadro's never does, so its own are
        // withheld and these two are the Studio+'s alone.
        "set_adat_gain",
        "set_spdif_gain",
        "set_brightness",
        "set_dc_coupled",
        "set_dim",
        "set_hard_mute",
        "set_line_gain",
        "set_mic_emulation",
        "set_mixer",
        "set_mixer_cfg",
        "set_mute",
        "set_panning_law",
        "set_pre_gain",
        "set_pre_phantom",
        "set_pre_phase_inv",
        "set_pre_phaseinv",
        "set_pre_type",
        "set_routing",
        "set_samp_rate",
        "set_spdif_src",
        "set_sync_source",
        "set_tbk_enable",
        "set_tbk_vol",
        "set_trim",
        "set_trim_config",
        "set_volume",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    assert_eq!(commands, expected, "the commands recall would send have changed");
}

/// A plan is a description: it says it sent nothing, it carries the parts in running order, and
/// every step it does list knows its bytes.
#[tokio::test]
async fn a_plan_describes_what_would_be_sent_and_says_it_sent_nothing() {
    let app = harness(false, false);
    let id = take(&app).await;
    let (status, plan) = call(&app, "POST", &format!("/api/v1/snapshots/{id}/recall/plan"), Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["sent"], json!(false));
    assert_eq!(plan["current_state_read"], json!(true));
    assert_eq!(plan["raise_threshold_db"], json!(6));
    assert!(plan["note"].as_str().unwrap_or_default().contains("Nothing has been sent"));
    // The loopback's cyclic pattern moves, so the reads either match or differ; what must never
    // happen is a step for a device that was not read, or a step with no bytes.
    for step in plan["steps"].as_array().expect("steps") {
        assert!(step["bytes"].as_str().is_some_and(|b| !b.is_empty()), "{step}");
        assert!(step["ext3"].is_null(), "no writer takes a per-request selector: {step}");
    }
    let parts: Vec<&str> = plan["parts"].as_array().expect("parts").iter().map(|p| p["name"].as_str().unwrap_or_default()).collect();
    assert_eq!(parts, ["silence", "clock", "settings", "dc_coupling", "inputs", "phantom", "routing", "mixer", "outputs", "restore"]);
}

/// Every step's bytes are the server's own: the same command, the same arguments and the same dry
/// run a client could ask for by hand.
#[tokio::test]
async fn each_steps_bytes_are_what_the_servers_dry_run_reports() {
    let app = harness(false, false);
    let id = take(&app).await;
    // Move something on each device, so there is something to plan.
    for device in ["loopback-0", "loopback-1"] {
        let (status, body) = call(&app, "POST", &format!("/api/v1/devices/{device}/command/set_volume"), Some(json!({"id": 1, "volume": 44}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, plan) = call(&app, "POST", &format!("/api/v1/snapshots/{id}/recall/plan"), None).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let steps = plan["steps"].as_array().expect("steps").clone();
    assert!(!steps.is_empty(), "something was changed, so something is planned");
    for step in steps.iter().take(40) {
        let uri = format!("/api/v1/devices/{}/command/{}?dry_run=true", step["device_id"].as_str().expect("a device"), step["command"].as_str().expect("a command"));
        let (status, sent) = call(&app, "POST", &uri, Some(step["args"].clone())).await;
        assert_eq!(status, StatusCode::OK, "{sent}");
        assert_eq!(sent["dry_run"], json!(true));
        assert_eq!(sent["sent_hex"], step["bytes"], "the plan's bytes are not the server's for {}", step["label"]);
        assert_eq!(sent["sent_len"], step["bytes_len"]);
    }
}

/// A snapshot naming a device that is not attached plans nothing for it and says why.
#[tokio::test]
async fn a_device_that_is_not_attached_is_named_and_gets_no_steps() {
    let app = harness(false, false);
    let id = take(&app).await;
    let (_, body) = call(&app, "GET", &format!("/api/v1/snapshots/{id}"), None).await;
    // Re-import the snapshot under a new id with a device that was never attached.
    let mut snapshot = body.clone();
    snapshot["id"] = json!("snap-absent");
    let ghost = snapshot["devices"]["loopback-0"].clone();
    snapshot["devices"] = json!({ "loopback-7": ghost });
    let (status, added) = call(&app, "POST", "/api/v1/snapshots/import", Some(json!([snapshot]))).await;
    assert_eq!(status, StatusCode::OK, "{added}");

    let (status, plan) = call(&app, "POST", "/api/v1/snapshots/snap-absent/recall/plan", None).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert!(plan["steps"].as_array().is_some_and(Vec::is_empty), "nothing can be sent to a device that is not there: {plan}");
    let devices = plan["devices"].as_array().expect("devices");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["device_id"], json!("loopback-7"));
    assert_eq!(devices[0]["missing"], json!(true));
    let excluded = plan["excluded"].as_array().expect("excluded");
    assert_eq!(excluded[0]["kind"], json!("device_missing"));
    assert!(excluded[0]["reason"].as_str().unwrap_or_default().contains("not attached now"));
}

/// In dry run the server answers no reads, so a plan says the present is unknown rather than
/// pretending the devices agree with the snapshot.
#[tokio::test]
async fn a_plan_in_dry_run_says_the_present_was_not_read() {
    let taking = harness(false, false);
    let id = take(&taking).await;
    let (_, snapshot) = call(&taking, "GET", &format!("/api/v1/snapshots/{id}"), None).await;

    let app = harness(true, false);
    let (status, added) = call(&app, "POST", "/api/v1/snapshots/import", Some(json!([snapshot]))).await;
    assert_eq!(status, StatusCode::OK, "{added}");
    // A capture is refused in dry run; a plan is not, because it is a description.
    let (status, refused) = call(&app, "POST", "/api/v1/snapshots", Some(json!({"name": "x"}))).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{refused}");

    let (status, plan) = call(&app, "POST", &format!("/api/v1/snapshots/{id}/recall/plan"), None).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["current_state_read"], json!(false));
    assert!(!plan["steps"].as_array().expect("steps").is_empty(), "with nothing known to be right, everything recallable is listed");
    assert_eq!(plan["sent"], json!(false));
}

/// The apply route is closed: the server flag and the request body must both say otherwise, and
/// even then nothing is sent.
#[tokio::test]
async fn the_apply_route_refuses_while_recall_is_not_enabled() {
    let app = harness(false, false);
    let id = take(&app).await;
    for body in [None, Some(json!({"enable_recall": true}))] {
        let (status, refused) = call(&app, "POST", &format!("/api/v1/snapshots/{id}/recall"), body).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{refused}");
        assert_eq!(refused["error"]["code"], json!("unsupported"));
        assert!(refused["error"]["message"].as_str().unwrap_or_default().contains("recall is not enabled"), "{refused}");
        assert!(refused["error"]["message"].as_str().unwrap_or_default().contains("--enable-recall"), "{refused}");
    }
}

#[tokio::test]
async fn the_flag_alone_is_not_enough_and_a_run_that_is_not_dry_still_refuses() {
    let app = harness(false, true);
    let id = take(&app).await;
    let (status, refused) = call(&app, "POST", &format!("/api/v1/snapshots/{id}/recall"), Some(json!({}))).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or_default().contains("recall is not enabled"), "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or_default().contains("\"enable_recall\": true"), "{refused}");

    // Both switches thrown, and the server is not in dry run: this is where applying attaches, and
    // it is not built. Nothing reaches a device.
    let (status, refused) = call(&app, "POST", &format!("/api/v1/snapshots/{id}/recall"), Some(json!({"enable_recall": true}))).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or_default().contains("is not built"), "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or_default().contains("hardware session"), "{refused}");
    // The message says what the hardware session must confirm, not where some document says so.
    assert!(refused["error"]["message"].as_str().unwrap_or_default().contains("hard mute"), "{refused}");
    assert!(!refused["error"]["message"].as_str().unwrap_or_default().contains('§'), "{refused}");
}

#[tokio::test]
async fn with_both_switches_and_dry_run_the_apply_route_reports_the_bytes_and_sends_nothing() {
    let taking = harness(false, false);
    let id = take(&taking).await;
    let (_, snapshot) = call(&taking, "GET", &format!("/api/v1/snapshots/{id}"), None).await;

    let app = harness(true, true);
    call(&app, "POST", "/api/v1/snapshots/import", Some(json!([snapshot]))).await;
    let (status, body) = call(&app, "POST", &format!("/api/v1/snapshots/{id}/recall"), Some(json!({"enable_recall": true}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dry_run"], json!(true));
    assert_eq!(body["sent"], json!(false));
    assert!(body["note"].as_str().unwrap_or_default().contains("not one of them was sent"));
    let steps = body["steps"].as_array().expect("steps");
    assert!(!steps.is_empty());
    assert!(steps.iter().all(|s| s["bytes"].as_str().is_some_and(|b| !b.is_empty())), "every step reports what it would send");
}

/// The dangerous parts are off, and the plan carries the guards rather than a client re-deriving
/// them.
#[tokio::test]
async fn the_guards_ride_on_the_plan() {
    let app = harness(false, false);
    let id = take(&app).await;
    // Switch 48V on for a preamp the snapshot has off, and change the clock, so both guards bite.
    let (_, snapshot) = call(&app, "GET", &format!("/api/v1/snapshots/{id}"), None).await;
    let phantom = snapshot["devices"]["loopback-1"]["sections"]["inputs"]["preamps"][0]["phantom"].as_i64().unwrap_or(0);
    call(&app, "POST", "/api/v1/devices/loopback-1/command/set_pre_phantom", Some(json!({"id": 0, "phantom": 1 - phantom}))).await;
    call(&app, "POST", "/api/v1/devices/loopback-1/command/set_sync_source", Some(json!({"src_index": 3}))).await;

    let (status, plan) = call(&app, "POST", &format!("/api/v1/snapshots/{id}/recall/plan"), None).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let part = |name: &str| plan["parts"].as_array().expect("parts").iter().find(|p| p["name"] == json!(name)).cloned().expect("a part");
    for name in ["phantom", "clock", "dc_coupling", "settings"] {
        assert_eq!(part(name)["chosen"], json!(false), "{name} is off by default");
        assert_eq!(part(name)["default_on"], json!(false));
    }
    for name in ["mixer", "routing", "inputs", "outputs"] {
        assert_eq!(part(name)["chosen"], json!(true), "{name} is on by default");
    }
    for step in plan["steps"].as_array().expect("steps") {
        let part = step["part"].as_str().unwrap_or_default();
        if matches!(part, "phantom" | "clock" | "dc_coupling" | "settings") {
            assert!(!step["blocked_by"].as_array().expect("blocked_by").is_empty(), "{step}");
        }
    }
    // Ticked and chosen, the same plan lets them through.
    let asked = json!({"parts": {"phantom": true, "clock": true}, "confirm": {"phantom": true, "clock": true}});
    let (status, ticked) = call(&app, "POST", &format!("/api/v1/snapshots/{id}/recall/plan"), Some(asked)).await;
    assert_eq!(status, StatusCode::OK, "{ticked}");
    for step in ticked["steps"].as_array().expect("steps") {
        if matches!(step["part"].as_str().unwrap_or_default(), "phantom" | "clock") {
            assert!(step["blocked_by"].as_array().expect("blocked_by").is_empty(), "{step}");
        }
    }
    assert!(ticked["ready"].as_u64().unwrap_or(0) >= plan["ready"].as_u64().unwrap_or(0));
}
