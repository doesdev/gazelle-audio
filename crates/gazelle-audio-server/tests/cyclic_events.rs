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
