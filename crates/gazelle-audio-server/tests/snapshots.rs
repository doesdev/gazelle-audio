//! Snapshot capture, listing and comparison, over HTTP and against loopbacks.
//!
//! Nothing here touches hardware, and nothing here writes to a device: capture and compare only
//! read, and the loopback has answered every `get_*` since P95, so the whole of phase 5 is
//! exercisable without a device attached.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gazelle_audio_server::device::descriptor::DeviceId;
use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
use gazelle_audio_server::snapshot::store::{JsonDirStore, MemorySnapshotStore, SnapshotStore};
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};
use gazelle_audio_transport::{Device, LoopbackDevice};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

/// How devices are attached for a given test.
enum Backend {
    /// The full loopback stack, pushing cyclic reports: what `--loopback-cyclic-ms` gives.
    Cyclic,
    /// The full loopback stack with no cyclic traffic: every `get_*` answers, nothing else does.
    Quiet,
    /// A bare emulating loopback, with none of the layers that answer reads: every read fails,
    /// which is what a device refusing (P96) or timing out looks like to a capture.
    Silent,
}

struct Harness {
    app: axum::Router,
    #[allow(dead_code)]
    devices: Arc<DeviceManager>,
}

fn harness(backend: Backend, dry_run: bool, snapshots: Arc<dyn SnapshotStore>) -> Harness {
    let registries = RegistrySet::builtin().expect("registries");
    let devices = DeviceManager::new(registries);
    match backend {
        Backend::Cyclic => devices.attach_cyclic_loopbacks(&[PID_QUADRO, PID_STUDIO], 64, Duration::from_millis(10)),
        Backend::Quiet => devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64),
        Backend::Silent => {
            let bare: Box<dyn Device + Send> = Box::new(LoopbackDevice::emulating(9189, PID_QUADRO, 64));
            devices.attach(DeviceId::loopback(0), bare, "loopback", true);
        }
    }
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    let app = http::router(AppState {
        devices: devices.clone(),
        store,
        snapshots,
        force_dry_run: dry_run, enable_recall: false,
        backend: "loopback".into(),
        themes_dir: None,
        show_window: None,
    });
    Harness { app, devices }
}

fn plain(backend: Backend) -> Harness {
    harness(backend, false, Arc::new(MemorySnapshotStore::default()))
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

/// Take a snapshot once both devices have pushed a state report, and return its id.
///
/// A capture the instant the server starts is legitimately half unread — the devices have said
/// nothing yet — so a test about what a full snapshot holds waits for one.
async fn take_when_ready(app: &axum::Router, name: &str) -> String {
    for _ in 0..200 {
        let (status, body) = call(app, "POST", "/api/v1/snapshots", Some(json!({ "name": name }))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["id"].as_str().expect("an id").to_string();
        let (_, snapshot) = call(app, "GET", &format!("/api/v1/snapshots/{id}"), None).await;
        let ready = ["loopback-0", "loopback-1"]
            .iter()
            .all(|device| snapshot["devices"][device]["sections"]["clock"]["sync_source"].is_number());
        if ready {
            return id;
        }
        call(app, "DELETE", &format!("/api/v1/snapshots/{id}"), None).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no state report arrived within two seconds");
}

/// How many changes of each kind a diff holds, over every device and section.
fn kinds(diff: &Value) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    let changes = diff["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|d| d["sections"].as_array().into_iter().flatten())
        .flat_map(|s| s["changes"].as_array().into_iter().flatten())
        .chain(diff["workspace"].as_array().into_iter().flatten());
    for change in changes {
        *counts.entry(change["kind"].as_str().unwrap_or("?").to_string()).or_insert(0) += 1;
    }
    counts
}

#[tokio::test]
async fn a_snapshot_records_every_section_of_both_models_it_can_read() {
    let h = plain(Backend::Cyclic);
    let id = take_when_ready(&h.app, "Drum tracking").await;

    let (status, snapshot) = call(&h.app, "GET", &format!("/api/v1/snapshots/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{snapshot}");
    assert_eq!(snapshot["version"], 1, "a snapshot carries its schema version from the first one written");
    assert_eq!(snapshot["name"], "Drum tracking");
    assert!(snapshot["created"].as_str().expect("created").ends_with('Z'));

    for (device, family, model) in [("loopback-0", "quadro", "Zen Quadro Synergy Core"), ("loopback-1", "studio", "Zen Studio+")] {
        let d = &snapshot["devices"][device];
        assert_eq!(d["family"], family, "{device}: {d}");
        assert_eq!(d["model"], model);
        assert!(d["current_preset"].is_number(), "the device's preset slot is recorded: {d}");

        // The mixer: four mixes of a master and 32 strips, and the link flags.
        let mixer = &d["sections"]["mixer"];
        for mix in 0..4 {
            // A reply of one field is that field: `get_mixer` answers 33 entries, not a wrapper.
            let entries = mixer[format!("mixes[{mix}]")].as_array().unwrap_or_else(|| panic!("{device} mix {mix}: {mixer}"));
            assert_eq!(entries.len(), 33, "{device} mix {mix} is a master and 32 strips");
            assert!(entries[0]["level"].is_number() && entries[0]["mute"].is_number());
        }
        assert_eq!(mixer["links"].as_array().expect("link flags").len(), 64, "{mixer}");

        // Routing, keyed by topology group id rather than wire position.
        let routing = &d["sections"]["routing"];
        assert!(routing["MIXER_IN0"]["bank_configs"].as_array().expect("slots").len() >= 32);
        assert!(routing["SPDIF_OUT0"].is_object(), "{routing}");

        // Inputs, outputs, clock and settings.
        let inputs = &d["sections"]["inputs"];
        assert!(inputs["preamps"].is_array() || inputs["preamps"].is_string(), "{inputs}");
        assert!(inputs["links"]["preamp"].is_array(), "{inputs}");
        assert!(d["sections"]["clock"]["sync_source"].is_number(), "{d}");
        assert!(d["sections"]["clock"]["rate_index"].is_number());
        assert!(d["sections"]["settings"]["brightness"].is_number());
        assert!(d["sections"]["outputs"]["trims"]["monitor"].is_number(), "{}", d["sections"]["outputs"]);

        // Nothing whose meaning is unknown, and nothing momentary.
        let text = d.to_string();
        for never in ["osc_level", "power_on", "usb_mode", "talkback_on", "peaks_", "reserved"] {
            assert!(!text.contains(never), "{device} captured '{never}'");
        }
    }
    // Per model.
    assert!(snapshot["devices"]["loopback-0"]["sections"]["settings"]["panning_law"].is_number());
    assert!(snapshot["devices"]["loopback-0"]["sections"]["outputs"]["hard_mute"].is_number());
    assert!(snapshot["devices"]["loopback-1"]["sections"]["clock"]["spdif_src"].is_number());
    assert!(snapshot["devices"]["loopback-1"]["sections"]["outputs"]["hp1"]["volume"].is_number());
    assert!(snapshot["devices"]["loopback-1"]["sections"]["outputs"]["talkback"]["to_monitor"].is_number());
    assert!(snapshot["devices"]["loopback-1"]["sections"]["inputs"]["line_gains"].is_string() || snapshot["devices"]["loopback-1"]["sections"]["inputs"]["line_gains"].is_array());
}

/// Whatever could not be read is named and explained, not filled in.
#[tokio::test]
async fn a_read_that_fails_is_recorded_as_unknown_rather_than_invented() {
    // No cyclic traffic: every value that only the state report carries is unknown, while the
    // `get_*` reads still answer.
    let h = plain(Backend::Quiet);
    let (status, created) = call(&h.app, "POST", "/api/v1/snapshots", Some(json!({"name": "Quiet"}))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let (_, snapshot) = call(&h.app, "GET", &format!("/api/v1/snapshots/{}", created["id"].as_str().expect("id")), None).await;
    let device = &snapshot["devices"]["loopback-0"];
    let unreadable = device["unreadable"].as_array().expect("unreadable");
    let paths: Vec<&str> = unreadable.iter().filter_map(|u| u["path"].as_str()).collect();
    assert!(paths.contains(&"clock.sync_source"), "{paths:?}");
    assert!(paths.contains(&"inputs.preamps"), "{paths:?}");
    assert!(unreadable[0]["reason"].as_str().expect("a reason").contains("has not reported its state"), "{unreadable:?}");
    assert!(device["sections"]["clock"].get("sync_source").is_none(), "an unread value is absent, not zero");
    assert!(device["current_preset"].is_null(), "the preset slot is not guessed either");
    // What could be read still is.
    assert!(device["sections"]["mixer"]["mixes[0]"].is_array());
    // And the list says how much of it is unknown, without carrying the values.
    let (_, listed) = call(&h.app, "GET", "/api/v1/snapshots", None).await;
    assert_eq!(listed["snapshots"][0]["devices"][0]["unreadable"], unreadable.len());

    // A device that answers no reads at all: a snapshot of nothing, said plainly.
    let silent = plain(Backend::Silent);
    let (status, created) = call(&silent.app, "POST", "/api/v1/snapshots", Some(json!({"name": "Silent"}))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let (_, snapshot) = call(&silent.app, "GET", &format!("/api/v1/snapshots/{}", created["id"].as_str().expect("id")), None).await;
    let device = &snapshot["devices"]["loopback-0"];
    assert!(device["sections"].as_object().expect("sections").is_empty(), "nothing was readable: {device}");
    let reasons: Vec<&str> = device["unreadable"].as_array().expect("unreadable").iter().filter_map(|u| u["reason"].as_str()).collect();
    assert!(reasons.iter().any(|r| r.contains("get_mixer")), "the failing command is named: {reasons:?}");
}

/// The list, rename and delete: what the Snapshots section is made of.
#[tokio::test]
async fn snapshots_are_listed_newest_first_renamed_and_deleted() {
    let h = plain(Backend::Quiet);
    let (_, first) = call(&h.app, "POST", "/api/v1/snapshots", Some(json!({"name": "Before the take"}))).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (_, second) = call(&h.app, "POST", "/api/v1/snapshots", Some(json!({"name": "After the take", "note": "kick sounded better"}))).await;
    let (first, second) = (first["id"].as_str().expect("id").to_string(), second["id"].as_str().expect("id").to_string());

    let (status, listed) = call(&h.app, "GET", "/api/v1/snapshots", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["count"], 2);
    assert_eq!(listed["snapshots"][0]["name"], "After the take", "newest first: {listed}");
    assert_eq!(listed["snapshots"][0]["note"], "kick sounded better");
    assert_eq!(listed["snapshots"][0]["devices"].as_array().expect("devices").len(), 2, "the list says which devices it came from");
    assert_eq!(listed["snapshots"][0]["devices"][0]["model"], "Zen Quadro Synergy Core");
    assert!(listed["snapshots"][0].get("workspace").is_none(), "the list is a list, not every value: {listed}");

    // Rename. Only the name and note can change; nothing recorded can.
    let (status, renamed) = call(&h.app, "PATCH", &format!("/api/v1/snapshots/{first}"), Some(json!({"name": "  Take 1  "}))).await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Take 1", "a name is trimmed");
    let (status, refused) = call(&h.app, "PATCH", &format!("/api/v1/snapshots/{first}"), Some(json!({"name": "  "}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"]["code"], "bad_value");
    let (_, unchanged) = call(&h.app, "GET", &format!("/api/v1/snapshots/{first}"), None).await;
    assert_eq!(unchanged["name"], "Take 1");

    // Delete.
    let (status, deleted) = call(&h.app, "DELETE", &format!("/api/v1/snapshots/{second}"), None).await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    let (status, gone) = call(&h.app, "GET", &format!("/api/v1/snapshots/{second}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(gone["error"]["code"], "unknown_snapshot");
    assert_eq!(call(&h.app, "DELETE", &format!("/api/v1/snapshots/{second}"), None).await.0, StatusCode::NOT_FOUND);
    let (_, listed) = call(&h.app, "GET", "/api/v1/snapshots", None).await;
    assert_eq!(listed["count"], 1);

    // A snapshot needs a name, and an id that would escape the store is refused.
    assert_eq!(call(&h.app, "POST", "/api/v1/snapshots", Some(json!({"name": ""}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&h.app, "GET", "/api/v1/snapshots/..%2Fworkspace", None).await.0, StatusCode::BAD_REQUEST);
}

/// Comparing with now: nothing when nothing moved, and the change in the section it belongs to.
#[tokio::test]
async fn comparing_with_now_finds_what_changed_and_nothing_when_it_has_not() {
    let h = plain(Backend::Quiet);
    let (_, created) = call(&h.app, "POST", "/api/v1/snapshots", Some(json!({"name": "Take 1"}))).await;
    let id = created["id"].as_str().expect("id").to_string();

    let (status, same) = call(&h.app, "GET", &format!("/api/v1/snapshots/{id}/compare"), None).await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_eq!(same["snapshot"]["name"], "Take 1", "the diff names what it is comparing with");
    assert!(same["workspace"].as_array().expect("workspace").is_empty());
    // Nothing was touched, so nothing that could be read differs. What this loopback never pushed
    // is listed as unknown, which is not agreement (P96) and is the only thing here.
    let kinds = kinds(&same);
    assert_eq!(kinds.get("changed"), None, "{}", serde_json::to_string_pretty(&same).unwrap_or_default());
    assert_eq!(kinds.get("only_in_snapshot"), None);
    assert_eq!(kinds.get("only_now"), None);
    assert!(kinds.get("unknown").copied().unwrap_or(0) > 0, "the values this loopback never reported are named: {kinds:?}");

    // Change one thing in each section the loopback keeps state for, then compare again.
    let set = |command: &str, args: Value| {
        let app = h.app.clone();
        let uri = format!("/api/v1/devices/loopback-0/command/{command}");
        async move { call(&app, "POST", &uri, Some(args)).await }
    };
    assert_eq!(set("set_panning_law", json!({"panning": 2})).await.0, StatusCode::OK);
    assert_eq!(set("set_mixer", json!({"channel": 1, "level": 23})).await.0, StatusCode::OK);
    assert_eq!(set("set_mic_emulation", json!({"preamp_ch": 0, "target": 1, "emu_model": 3, "ch_swap": 0, "pattern": 0})).await.0, StatusCode::OK);
    let workspace = json!({"version": 1, "aliases": {"loopback-0": "Desk Quadro"}});
    assert_eq!(call(&h.app, "PUT", "/api/v1/workspace", Some(workspace)).await.0, StatusCode::OK);

    let (status, changed) = call(&h.app, "GET", &format!("/api/v1/snapshots/{id}/compare"), None).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["same"], false);
    let device = changed["devices"].as_array().expect("devices").iter().find(|d| d["device_id"] == "loopback-0").expect("the Quadro");
    let section = |name: &str| device["sections"].as_array().expect("sections").iter().find(|s| s["section"] == name).cloned().unwrap_or(Value::Null);

    let change = |section: Value, path: &str| {
        section["changes"]
            .as_array()
            .unwrap_or_else(|| panic!("no changes in {section}"))
            .iter()
            .find(|c| c["path"] == path)
            .cloned()
            .unwrap_or_else(|| panic!("no {path} among {}", section["changes"]))
    };

    let settings = section("settings");
    assert_eq!(settings["title"], "Settings");
    let panning = change(settings, "panning_law");
    assert_eq!(panning["label"], "Settings · panning law");
    assert_eq!((panning["from"].clone(), panning["to"].clone()), (json!(0), json!(2)));
    assert_eq!(panning["kind"], "changed");

    let level = change(section("mixer"), "mixes[0][1].level");
    assert_eq!(level["to"], 23);
    assert_eq!(level["label"], "Mixer · Mix 1 · strip 1 · level");

    let emulation = change(section("inputs"), "emulations[0].emu_model");
    assert_eq!(emulation["label"], "Inputs · preamp 1 emulation · emulation");
    assert_eq!(emulation["to"], 3);

    assert_eq!(changed["workspace"][0]["label"], "Workspace · aliases · loopback-0");
    assert_eq!(changed["workspace"][0]["to"], "Desk Quadro");
    assert!(changed["changes"].as_u64().expect("a count") >= 4);

    // The snapshot itself is untouched by a comparison: it is a record, not a working copy.
    let (_, snapshot) = call(&h.app, "GET", &format!("/api/v1/snapshots/{id}"), None).await;
    assert_eq!(snapshot["devices"]["loopback-0"]["sections"]["settings"]["panning_law"], 0);
}

/// A device in the snapshot but not attached now is named as missing, not diffed away.
#[tokio::test]
async fn a_device_that_is_no_longer_attached_is_named_rather_than_compared() {
    let h = plain(Backend::Quiet);
    let (_, created) = call(&h.app, "POST", "/api/v1/snapshots", Some(json!({"name": "Both"}))).await;
    let id = created["id"].as_str().expect("id").to_string();
    h.devices.detach(&DeviceId::loopback(1)).expect("detach the Studio+");

    let (_, diff) = call(&h.app, "GET", &format!("/api/v1/snapshots/{id}/compare"), None).await;
    let studio = diff["devices"].as_array().expect("devices").iter().find(|d| d["device_id"] == "loopback-1").expect("the Studio+");
    assert_eq!(studio["missing"], true);
    assert_eq!(studio["model"], "Zen Studio+", "it is still named, so the page can say which device");
    assert!(studio["sections"].as_array().expect("sections").is_empty());
    assert_eq!(diff["same"], false);
}

/// The store is a directory of documents, so snapshots outlive the server that took them.
#[tokio::test]
async fn snapshots_survive_a_server_restart() {
    let mut dir = std::env::temp_dir();
    dir.push(format!("gazelle-snap-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let first = harness(Backend::Quiet, false, Arc::new(JsonDirStore::new(&dir)));
    let (_, created) = call(&first.app, "POST", "/api/v1/snapshots", Some(json!({"name": "Before the reboot"}))).await;
    let id = created["id"].as_str().expect("id").to_string();
    first.devices.shutdown_all();
    drop(first);

    // A second server over the same directory is what the user has after a restart.
    let second = harness(Backend::Quiet, false, Arc::new(JsonDirStore::new(&dir)));
    let (status, listed) = call(&second.app, "GET", "/api/v1/snapshots", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["count"], 1, "{listed}");
    assert_eq!(listed["snapshots"][0]["name"], "Before the reboot");
    let (_, snapshot) = call(&second.app, "GET", &format!("/api/v1/snapshots/{id}"), None).await;
    assert!(snapshot["devices"]["loopback-0"]["sections"]["mixer"]["mixes[0]"].is_array(), "{snapshot}");
    // And it can still be compared with the devices as they are now.
    assert_eq!(call(&second.app, "GET", &format!("/api/v1/snapshots/{id}/compare"), None).await.0, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A snapshot from a newer app is refused whole, never half-read.
#[tokio::test]
async fn a_schema_version_this_server_does_not_read_is_refused() {
    let h = plain(Backend::Quiet);
    let newer = json!([{
        "version": 2, "id": "snap-newer", "name": "From a newer app",
        "created": "2026-10-01T00:00:00Z", "workspace": {"version": 1}, "devices": {}
    }]);
    let (status, body) = call(&h.app, "POST", "/api/v1/snapshots/import", Some(newer)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "bad_value");
    assert!(body["error"]["message"].as_str().expect("message").contains("version 2"), "{body}");
    assert_eq!(call(&h.app, "GET", "/api/v1/snapshots", None).await.1["count"], 0, "nothing was stored");
}

/// Import is add-only: a snapshot already here keeps the moment it recorded.
#[tokio::test]
async fn importing_adds_snapshots_and_keeps_the_ones_already_stored() {
    let h = plain(Backend::Quiet);
    let (_, mine) = call(&h.app, "POST", "/api/v1/snapshots", Some(json!({"name": "Mine"}))).await;
    let id = mine["id"].as_str().expect("id").to_string();

    let backup = json!([
        {"version": 1, "id": id, "name": "Theirs, same id", "created": "2020-01-01T00:00:00Z", "workspace": {"version": 1}, "devices": {}},
        {"version": 1, "id": "snap-theirs", "name": "Theirs", "created": "2026-09-16T12:00:00Z", "workspace": {"version": 1}, "devices": {}}
    ]);
    let (status, result) = call(&h.app, "POST", "/api/v1/snapshots/import", Some(backup)).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["added"], json!(["snap-theirs"]));
    assert_eq!(result["skipped"], json!([id]));
    let (_, kept) = call(&h.app, "GET", &format!("/api/v1/snapshots/{id}"), None).await;
    assert_eq!(kept["name"], "Mine", "the one already here was not replaced");
    assert_eq!(call(&h.app, "GET", "/api/v1/snapshots", None).await.1["count"], 2);

    // A file that is not a list of snapshots says so rather than storing half of it.
    let (status, body) = call(&h.app, "POST", "/api/v1/snapshots/import", Some(json!({"snapshots": []}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// In dry run the request path stops before the device, so there is nothing to record (0012).
#[tokio::test]
async fn a_snapshot_is_refused_in_dry_run_rather_than_recording_nothing() {
    let h = harness(Backend::Quiet, true, Arc::new(MemorySnapshotStore::default()));
    let (status, body) = call(&h.app, "POST", "/api/v1/snapshots", Some(json!({"name": "Dry"}))).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["error"]["code"], "unsupported");
    assert!(body["error"]["message"].as_str().expect("message").contains("dry run"), "{body}");
    assert_eq!(call(&h.app, "GET", "/api/v1/snapshots", None).await.1["count"], 0);
}
