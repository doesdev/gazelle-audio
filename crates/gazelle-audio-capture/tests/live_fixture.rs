//! First real USBPcap capture (Task 15): every frame decodes, and the stored target's
//! descriptor matches the session it came from.

use std::path::PathBuf;

use gazelle_audio_capture::capture::decode::decode;
use gazelle_audio_capture::capture::import::open_frames;
use gazelle_audio_capture::capture::pipeline::DeviceMap;
use gazelle_audio_capture::session::store::SessionInfo;

#[test]
fn live_usbpcap_capture_decodes_cleanly() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let info: SessionInfo =
        serde_json::from_slice(&std::fs::read(fixtures.join("usbpcap-live-1.session.json")).unwrap()).unwrap();
    let mut map = DeviceMap::default();
    let mut events = 0usize;
    let mut last_ts = 0u64;
    for frame in open_frames(&fixtures.join("usbpcap-live-1.pcapng")).unwrap() {
        let frame = frame.unwrap();
        assert_eq!(frame.link_type, 249);
        if let Some(ev) = decode(&frame).unwrap() {
            assert!(ev.ts_ns >= last_ts, "timestamps are monotonic at frame {}", frame.index);
            last_ts = ev.ts_ns;
            map.observe(&ev);
            events += 1;
        }
    }
    assert!(events > 0);
    let (bus, device) = map.find(info.vid, info.pid).expect("target descriptor stored in the capture");
    let hex: String = map.get(bus, device).unwrap().descriptor.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(Some(hex), info.device_descriptor_hex);
}
