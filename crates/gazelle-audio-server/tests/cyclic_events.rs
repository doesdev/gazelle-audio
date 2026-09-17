//! Cyclic reports must reach WebSocket clients as decoded fields, not opaque blobs.
//!
//! Until the schema carried cyclic layouts, `decode_cyclic` returned `None` unconditionally
//! and every event arrived as `undecoded` — no meters, no state sync. This drives a device
//! that pushes a real 0x73 report and asserts the event carries named values.

use gazelle_audio_protocol::wire::Header;
use gazelle_audio_server::device::descriptor::DeviceId;
use gazelle_audio_server::device::manager::{DeviceManager, ServerEvent};
use gazelle_audio_server::device::worker::DeviceEvent;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO};
use gazelle_audio_transport::{Device, RawPacket, Report};

/// A device that emits one canned report and nothing else.
struct CannedDevice {
    pending: Vec<Report>,
}

impl Device for CannedDevice {
    fn send(&mut self, _report: &Report) -> Result<bool, gazelle_audio_protocol::wire::WireError> {
        Ok(true)
    }
    fn on_received_data(&mut self, _packet: RawPacket) {}
    fn max_packet_size(&self) -> usize {
        304
    }
    fn vid(&self) -> u16 {
        9189
    }
    fn pid(&self) -> u16 {
        PID_QUADRO
    }
    fn poll_reports(&mut self) -> Vec<Report> {
        std::mem::take(&mut self.pending)
    }
}

#[tokio::test]
async fn cyclic_reports_arrive_decoded() {
    let registries = RegistrySet::builtin().expect("registries");
    let quadro = registries.for_pid(PID_QUADRO).expect("quadro registry");
    let layout = quadro
        .registry
        .cyclic(0x73)
        .expect("quadro declares a 0x73 layout");

    // A report long enough for the declared layout. Values are arbitrary; what matters is
    // that the server decodes them into named fields rather than reporting `undecoded`.
    let size: usize = layout.fields.iter().map(gazelle_audio_protocol::field::Field::size).sum();
    let contents = vec![0xA5u8; size.max(256)];
    let report = Report {
        header: Header::new(0x73, 0, 0, 0),
        contents,
    };

    let devices = DeviceManager::new(registries);
    let mut events = devices.subscribe();
    devices.attach(
        DeviceId::loopback(0),
        Box::new(CannedDevice { pending: vec![report] }),
        "canned",
        true,
    );

    // Drain events until the cyclic one appears (device_added comes first).
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    let mut decoded = None;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(tokio::time::Duration::from_millis(250), events.recv()).await {
            Ok(Ok(ServerEvent::Device(DeviceEvent::Cyclic { report_id, fields, .. }))) => {
                assert_eq!(report_id, 0x73);
                decoded = Some(fields);
                break;
            }
            Ok(Ok(ServerEvent::Device(DeviceEvent::Undecoded { report_id, .. }))) => {
                panic!("0x73 arrived undecoded; the schema layout was not applied (id 0x{report_id:X})");
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) | Err(_) => continue,
        }
    }

    let fields = decoded.expect("a decoded cyclic event within 5s");
    assert!(fields.len() > 50, "0x73 should decode to the full state field set");
    for expected in ["current_preset", "power_on", "usb_mode"] {
        assert!(
            fields.contains_key(expected),
            "decoded 0x73 should name '{expected}'; got {} fields",
            fields.len()
        );
    }
    devices.shutdown_all();
}

/// `--loopback-cyclic-ms` loopbacks push every declared cyclic report on a timer, decoded,
/// with values that move, while still answering commands like a plain emulating loopback.
/// Clients and the web UI need cyclic traffic without hardware.
#[tokio::test]
async fn cyclic_loopbacks_emit_decoded_reports_and_still_answer() {
    use gazelle_audio_protocol::payload::PayloadValues;
    use std::time::Duration;

    let registries = RegistrySet::builtin().expect("registries");
    let devices = DeviceManager::new(registries);
    let mut events = devices.subscribe();
    devices.attach_cyclic_loopbacks(&[PID_QUADRO], 64, Duration::from_millis(20));

    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    let mut seen: Vec<std::collections::HashMap<String, gazelle_audio_protocol::payload::Value>> = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    while tokio::time::Instant::now() < deadline && (seen.len() < 2 || ids.len() < 2) {
        match tokio::time::timeout(tokio::time::Duration::from_millis(250), events.recv()).await {
            Ok(Ok(ServerEvent::Device(DeviceEvent::Cyclic { report_id, fields, .. }))) => {
                ids.insert(report_id);
                if report_id == 0x73 {
                    seen.push(fields);
                }
            }
            Ok(Ok(ServerEvent::Device(DeviceEvent::Undecoded { report_id, len, .. }))) => {
                panic!("cyclic loopback report 0x{report_id:X} ({len} bytes) arrived undecoded");
            }
            _ => continue,
        }
    }
    assert!(seen.len() >= 2, "at least two decoded 0x73 reports within 5 s, got {}", seen.len());
    assert_eq!(ids.into_iter().collect::<Vec<_>>(), vec![0x73, 0x83], "every Quadro cyclic layout is emitted");
    let bank_sources: Vec<String> = seen.iter().map(|fields| format!("{:?}", fields.get("pm_bank_src"))).collect();
    assert!(bank_sources.windows(2).any(|w| w[0] != w[1]), "the reported values move between reports: {bank_sources:?}");

    let id = DeviceId::loopback(0);
    let outcome = devices
        .handle(&id)
        .expect("handle")
        .request("get_adats_links", PayloadValues::default(), None, false)
        .await
        .expect("a live command is still answered alongside cyclic traffic");
    assert!(!outcome.dry_run);
    devices.shutdown_all();
}

/// The effect-meter report the cyclic loopback emits is shaped like each model's own, so the web UI
/// can be exercised without hardware.
///
/// The Quadro's is as long as its loaded effects make it -- two bytes per effect, chain by chain,
/// then its four mic emulation meters -- and the chains are the ones `get_afx_strip_order` answers,
/// so the two agree. The Studio+'s is its fixed 16 chains by 8 slots, peaks then gain reductions.
#[tokio::test]
async fn the_cyclic_loopback_emits_an_effect_meter_report_shaped_like_the_device_s() {
    use gazelle_audio_protocol::payload::Value;
    use gazelle_audio_server::device::read_loopback::LOOPBACK_CHAIN;
    use gazelle_audio_server::registry_set::PID_STUDIO;
    use std::time::Duration;

    for (pid, chains) in [(PID_QUADRO, 6usize), (PID_STUDIO, 16usize)] {
        let registries = RegistrySet::builtin().expect("registries");
        let devices = DeviceManager::new(registries);
        let mut events = devices.subscribe();
        devices.attach_cyclic_loopbacks(&[pid], 64, Duration::from_millis(20));

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        let mut meters = None;
        while tokio::time::Instant::now() < deadline && meters.is_none() {
            match tokio::time::timeout(tokio::time::Duration::from_millis(250), events.recv()).await {
                Ok(Ok(ServerEvent::Device(DeviceEvent::Cyclic { report_id: 0x83, fields, .. }))) => meters = Some(fields),
                Ok(Ok(ServerEvent::Device(DeviceEvent::Undecoded { report_id, len, .. }))) => {
                    panic!("cyclic loopback report 0x{report_id:X} ({len} bytes) arrived undecoded");
                }
                _ => continue,
            }
        }
        let fields = meters.expect("an effect-meter report within 5 s");
        if pid == PID_QUADRO {
            let Some(Value::List(items)) = fields.get("afx_meters") else { panic!("no afx_meters: {fields:?}") };
            let [Value::Struct(one)] = items.as_slice() else { panic!("afx_meters is not one struct") };
            let Some(Value::Bytes(data)) = one.get("data") else { panic!("no data") };
            // The four mic emulation meters follow the effect meters, as the device sends them.
            assert_eq!(data.len(), chains * LOOPBACK_CHAIN.len() * 2 + 4, "two bytes per loaded effect, then the mic emulation meters");
            assert!(data[..data.len() - 4].iter().any(|&b| b < 60), "the effect meters carry signal, not silence");
        } else {
            let Some(Value::List(peaks)) = fields.get("channel_peaks") else { panic!("no channel_peaks: {fields:?}") };
            assert_eq!(peaks.len(), chains, "one entry per chain");
            let Some(Value::Struct(first)) = peaks.first() else { panic!("no first chain") };
            let Some(Value::Bytes(slots)) = first.get("effect_peaks") else { panic!("no effect_peaks") };
            assert_eq!(slots.len(), 8, "eight slots a chain");
            assert!(slots[..LOOPBACK_CHAIN.len()].iter().any(|&b| b < 60), "the loaded slots carry signal");
            assert!(slots[LOOPBACK_CHAIN.len()..].iter().all(|&b| b == 96), "an empty slot meters silence");
            assert!(fields.contains_key("channel_gain_reductions"), "gain reductions travel with the peaks");
        }
        devices.shutdown_all();
    }
}
