//! The loopback answers every read like a device would: a reply that decodes against the command's
//! `returns` layout at exactly its length, with plausible values, and some of them following what
//! was set. Without this, pages that read device state fail or show unknown with no hardware (P74).

use gazelle_audio_protocol::field::Field;
use gazelle_audio_protocol::payload::PayloadValues;
use gazelle_audio_protocol::registry::Registry;
use gazelle_audio_protocol::wire::HEADER_SIZE;
use gazelle_audio_protocol::Command;
use gazelle_audio_server::device::manager::{loopback_stack, DeviceManager, ANTELOPE_USB_VID};
use gazelle_audio_server::snapshot::store::MemorySnapshotStore;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};
use gazelle_audio_transport::{segment_report, Device, LoopbackDevice, RawPacket, Report};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// Every request field zero, so reads that carry parameters can be built too.
fn zero_values(command: &Command) -> PayloadValues {
    let mut values = PayloadValues::default();
    for field in &command.params {
        values = match field {
            Field::Scalar { name, .. } => values.with_scalar(name, 0),
            other => values.with_bytes(other.name(), vec![0; other.size()]),
        };
    }
    values
}

/// Send one request through the loopback stack as the worker does, and collect what comes back.
fn exchange(device: &mut Box<dyn Device + Send>, request: &[u8]) -> Vec<Report> {
    let header = gazelle_audio_protocol::wire::Header::from_bytes(request).expect("header");
    for segment in segment_report(header.cmd, header.seq, header.ext2, header.ext3, &request[HEADER_SIZE..], device.max_packet_size()) {
        device.on_received_data(RawPacket { bytes: segment });
    }
    device.poll_reports()
}

fn models() -> Vec<(&'static str, u16, Arc<Registry>)> {
    let set = RegistrySet::builtin().expect("registries");
    [PID_QUADRO, PID_STUDIO].iter().map(|&pid| {
        let model = set.for_pid(pid).expect("model");
        (model.family, pid, model.registry.clone())
    }).collect()
}

fn device(pid: u16, registry: &Registry) -> Box<dyn Device + Send> {
    loopback_stack(Box::new(LoopbackDevice::emulating(ANTELOPE_USB_VID, pid, 64)), Some(registry))
}

/// The reply to `name` on a fresh device.
fn reply(pid: u16, registry: &Registry, name: &str) -> Report {
    let mut dev = device(pid, registry);
    let command = registry.get(name).expect("command");
    let request = command.build_request(&zero_values(command)).expect("request");
    let mut replies: Vec<Report> = exchange(&mut dev, &request)
        .into_iter()
        .filter(|r| r.header.cmd == command.report_id + 1 && r.header.ext2 == command.ext2)
        .collect();
    assert_eq!(replies.len(), 1, "{name}: one reply");
    replies.remove(0)
}

/// The table: every `get_*` in both registries gets a reply that decodes against its layout, and is
/// exactly as long as the layout (one byte fewer no longer decodes; no trailing bytes are allowed).
#[test]
fn every_read_in_both_registries_answers_a_reply_of_its_layout_length() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for (family, pid, registry) in models() {
        let mut names: Vec<&String> = registry.names().filter(|n| n.starts_with("get_")).collect();
        names.sort();
        for name in names {
            let command = registry.get(name).unwrap();
            assert!(!command.returns.is_empty(), "{family} {name} declares a reply");
            checked += 1;
            let report = reply(pid, &registry, name);
            let decodes = |contents: &[u8]| Command::parse_field_list(&command.returns, contents).is_ok();
            if !decodes(&report.contents) {
                failures.push(format!("{family} {name}: {} bytes do not decode", report.contents.len()));
            } else if report.contents.is_empty() || decodes(&report.contents[..report.contents.len() - 1]) {
                failures.push(format!("{family} {name}: {} bytes is longer than the layout", report.contents.len()));
            }
        }
    }
    // Quadro: 26 reads and 68 effect types' parameter reads; Studio+: 12 and 37.
    assert_eq!(checked, 26 + 68 + 12 + 37, "every read of both models");
    assert!(failures.is_empty(), "{failures:#?}");
}

/// P77: a zero mask would grey every microphone, so the loopback is licensed for everything.
#[test]
fn the_feature_mask_sets_every_bit() {
    for (family, pid, registry) in models() {
        if registry.get("get_feature_mask").is_none() {
            continue;
        }
        let report = reply(pid, &registry, "get_feature_mask");
        assert_eq!(report.contents, vec![0xFF; 290], "{family}");
    }
}

/// Plausible values a fresh device would report, not zeros everywhere.
#[test]
fn fresh_reads_report_plausible_defaults() {
    let [(_, quadro, q), (_, studio, s)] = <[_; 2]>::try_from(models()).ok().unwrap();
    let decode = |pid: u16, registry: &Registry, name: &str| {
        let report = reply(pid, registry, name);
        Command::parse_field_list(&registry.get(name).unwrap().returns, &report.contents).expect("decodes")
    };
    use gazelle_audio_protocol::Value as V;
    let scalar = |v: &V| match v {
        V::U64(x) => *x as i64,
        V::I64(x) => *x,
        other => panic!("not a scalar: {other:?}"),
    };
    let entries = |fields: &std::collections::HashMap<String, V>| match &fields["entries"] {
        V::List(list) => list.iter().map(|e| match e { V::Struct(m) => m.clone(), _ => panic!() }).collect::<Vec<_>>(),
        _ => panic!("entries"),
    };

    // The reverb's density default is the schema's 100; it starts switched off.
    let reverb = decode(quadro, &q, "get_reverb_config");
    assert_eq!((scalar(&reverb["density"]), scalar(&reverb["on"])), (100, 0));
    // Reverb sends are centred, like mixer strips.
    for entry in entries(&decode(quadro, &q, "get_reverb_sends")) {
        assert_eq!((scalar(&entry["pan"]), scalar(&entry["mute"])), (32, 0));
    }
    // Thunderbolt latency Normal; the AFX2DAW split point the Quadro build fixes at 16.
    assert_eq!(scalar(&decode(studio, &s, "get_tb_latency")["mode"]), 1);
    let daw = decode(quadro, &q, "get_daw_mode");
    assert_eq!((scalar(&daw["enabled"]), scalar(&daw["split_point"])), (0, 16));
    // Effect type catalogues list their type ids in order.
    let types: Vec<i64> = entries(&decode(quadro, &q, "get_afx_available_instances")).iter().map(|e| scalar(&e["type_id"])).collect();
    assert_eq!(types, (0..90).collect::<Vec<_>>());
}

fn app(force_dry_run: bool) -> axum::Router {
    let devices = DeviceManager::new(RegistrySet::builtin().expect("registries"));
    devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64);
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    http::router(AppState { devices, store, snapshots: Arc::new(MemorySnapshotStore::default()), force_dry_run, enable_recall: false, backend: "loopback".into(), themes_dir: None, show_window: None })
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> Value {
    let res = app
        .clone()
        .oneshot(Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{uri}");
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["response_error"], Value::Null, "{uri}: {body}");
    body
}

/// Reads of state the pages change follow what was set, per device.
#[tokio::test]
async fn reads_follow_what_was_set() {
    let app = app(false);
    let q = "/api/v1/devices/loopback-0/command";
    let s = "/api/v1/devices/loopback-1/command";

    assert_eq!(post(&app, &format!("{q}/get_panning_law"), json!({})).await["response"]["panning"], 0, "0 dB, the first choice");
    post(&app, &format!("{q}/set_panning_law"), json!({"panning": 2})).await;
    assert_eq!(post(&app, &format!("{q}/get_panning_law"), json!({})).await["response"]["panning"], 2);

    let none = json!({"target": 0, "emu_model": 0, "ch_swap": 0, "pattern": 0});
    assert_eq!(post(&app, &format!("{q}/get_mic_emulations"), json!({})).await["response"]["entries"], json!([none, none]));
    post(&app, &format!("{q}/set_mic_emulation"), json!({"preamp_ch": 1, "target": 1, "emu_model": 3, "ch_swap": 1, "pattern": 2})).await;
    post(&app, &format!("{q}/set_mic_emulation"), json!({"preamp_ch": 3, "target": 2, "emu_model": 1, "ch_swap": 0, "pattern": 0})).await;
    assert_eq!(
        post(&app, &format!("{q}/get_mic_emulations"), json!({})).await["response"]["entries"],
        json!([none, {"target": 1, "emu_model": 3, "ch_swap": 1, "pattern": 2}]),
        "the reply holds preamps 1 and 2 only"
    );

    post(&app, &format!("{q}/set_reverb_return"), json!({"mixer_id": 2, "level": 50, "mute": 1})).await;
    let returns = post(&app, &format!("{q}/get_reverb_returns"), json!({})).await;
    assert_eq!(returns["response"]["entries"][2], json!({"level": 50, "mute": -1}), "mute is a signed bit");
    assert_eq!(returns["response"]["entries"][1], json!({"level": 0, "mute": 0}));

    let config = json!({"mixer_id": 1, "room_size": 5, "color": 6, "predelay": 7, "density": 80, "early_ref_gain": 9, "late_ref_delay": 10, "richness": 11, "reverb_time": 12, "reverb_level": 13, "on": 1});
    post(&app, &format!("{s}/set_reverb_config"), config.clone()).await;
    assert_eq!(post(&app, &format!("{s}/get_reverb_config"), json!({})).await["response"], config);
    assert_eq!(post(&app, &format!("{q}/get_reverb_config"), json!({})).await["response"]["density"], 100, "each device keeps its own");

    post(&app, &format!("{s}/set_tb_latency"), json!({"mode": 2, "buffer_adjust_in": 3, "buffer_adjust_out": 4, "latency_adjust_in": 0, "latency_adjust_out": 7})).await;
    assert_eq!(post(&app, &format!("{q}/get_panning_law"), json!({})).await["response"]["panning"], 2, "other sets leave it alone");
    assert_eq!(
        post(&app, &format!("{s}/get_tb_latency"), json!({})).await["response"],
        json!({"mode": 2, "buffer_adjust_in": 3, "buffer_adjust_out": 4, "latency_adjust_in": 0, "latency_adjust_out": 7})
    );
}

/// Routing starts from an identity-like routing rather than all MUTE, so the Routing and Mixer pages
/// have something to show: outputs play the computer, recordings take the preamps, mixer inputs take
/// the preamps (after the Quadro's six effect returns) and the first computer pair.
#[tokio::test]
async fn routing_starts_from_an_identity_like_routing() {
    let app = app(false);
    let slots = |body: Value| -> Vec<(u64, u64)> {
        body["response"]["bank_configs"].as_array().unwrap().iter().map(|p| (p["in_periph_id"].as_u64().unwrap(), p["in_chann"].as_u64().unwrap())).collect()
    };
    let read = |device: &'static str, group: u32| {
        let app = app.clone();
        async move { slots(post(&app, &format!("/api/v1/devices/{device}/command/get_routing?ext3={group}"), json!({})).await) }
    };

    // Quadro sources: PREAMP 0, USB 1 PLAY 1, AFX OUT 5, LOOPBACK HP1 6, MUTE 10.
    let line_out = read("loopback-0", 0).await;
    assert_eq!(&line_out[..3], &[(1, 0), (1, 1), (10, 0)]);
    assert_eq!(&read("loopback-0", 1).await[..2], &[(6, 0), (6, 1)], "HP1 plays its mix");
    let usb_rec = read("loopback-0", 4).await;
    assert_eq!(&usb_rec[..5], &[(0, 0), (0, 1), (0, 2), (0, 3), (10, 0)]);
    let mix = read("loopback-0", 8).await;
    assert_eq!(&mix[..13], &[(5, 0), (5, 1), (5, 2), (5, 3), (5, 4), (5, 5), (0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1), (10, 0)]);
    assert_eq!(mix.len(), 64);
    assert_eq!(read("loopback-0", 7).await[0], (10, 0), "nothing feeds the effects");

    // Studio+ sources: PREAMP 0, USB PLAY 3, MUTE 11; MIX CH1 is destination 10.
    assert_eq!(&read("loopback-1", 0).await[..3], &[(3, 0), (3, 1), (3, 2)], "eight line outputs");
    assert_eq!(&read("loopback-1", 7).await[..2], &[(3, 8), (3, 9)], "ADAT OUT plays USB channels 9 onwards");
    let studio_mix = read("loopback-1", 10).await;
    assert_eq!(&studio_mix[..7], &[(0, 0), (0, 1), (0, 2), (0, 3), (3, 0), (3, 1), (11, 0)]);
    assert_eq!(studio_mix.len(), 32);
}

/// An effect's parameter read answers the panel's starting values (`afx_parameters.json`, carried as
/// the reply fields' defaults): one instance on the Quadro, every instance on the Studio+.
#[tokio::test]
async fn effect_parameter_reads_answer_the_panels_starting_values() {
    let app = app(false);
    let q = "/api/v1/devices/loopback-0/command";
    let s = "/api/v1/devices/loopback-1/command";

    let gate = json!({"enabled": 1, "threshold": 90, "range": 0, "attack": 100, "decay": 50, "hold": 0, "gain": 0});
    assert_eq!(post(&app, &format!("{q}/get_powergate_conf"), json!({"id": 3})).await["response"]["entries"], json!([gate]));

    let studio = post(&app, &format!("{s}/get_powergate_configs"), json!({})).await;
    let entries = studio["response"]["entries"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 16, "every instance");
    let mut with_link = gate.clone();
    with_link["linked"] = json!(0);
    assert!(entries.iter().all(|e| *e == with_link), "{entries:?}");

    // The Studio+ starts an X903 where its panel resets one (antelope.ui.afx.defaults), not at its widgets' values.
    assert_eq!(
        post(&app, &format!("{s}/get_dbx_903_configs"), json!({})).await["response"]["entries"][15],
        json!({"enabled": 1, "threshold": 100, "ratio": 0, "output": 50, "linked": 0})
    );
    // The Quadro's own starting values for the same effect differ.
    assert_eq!(
        post(&app, &format!("{q}/get_X903_conf"), json!({"id": 0})).await["response"]["entries"][0],
        json!({"enabled": 1, "threshold": 82, "ratio": 50, "output": 50})
    );
    // A sidechain source has no starting value: it reads 0.
    let brainiac = post(&app, &format!("{q}/get_Brainiac_conf"), json!({"id": 1})).await;
    assert_eq!((brainiac["response"]["entries"][0]["ratio"].clone(), brainiac["response"]["entries"][0]["sideSource"].clone()), (json!(8), json!(0)));
}

/// Decision 0012: a dry run sends nothing and so reads nothing, whatever the loopback would answer.
#[tokio::test]
async fn a_dry_run_read_still_answers_nothing() {
    let app = app(true);
    let body = post(&app, "/api/v1/devices/loopback-0/command/get_panning_law", json!({})).await;
    assert_eq!((body["dry_run"].clone(), body["response"].clone()), (json!(true), Value::Null));
}
