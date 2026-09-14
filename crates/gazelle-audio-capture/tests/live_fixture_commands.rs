//! Second real USBPcap capture (Studio+ probe on the new PC, session s5): every panel command
//! for Channel 8 gain and phase decodes to the expected id, channel and value, and the device's
//! 0x73 status report confirms each one shortly after.

use std::path::PathBuf;

use gazelle_audio_capture::capture::decode::decode;
use gazelle_audio_capture::capture::event::{Direction, UsbEvent};
use gazelle_audio_capture::capture::import::open_frames;
use gazelle_audio_capture::capture::pipeline::DeviceMap;
use gazelle_audio_capture::session::store::SessionInfo;

const REPORT_LEN: usize = 320;
const CMD: u8 = 0x70;
const STATUS: u8 = 0x73;
const SET_PRE_GAIN: u8 = 0x4f;
const SET_PRE_PHASEINV: u8 = 0x51;
/// Channel 8, zero-based.
const CHANNEL: u8 = 7;
/// In the 0x73 report, gain sits at 33 + channel and phase invert at bit 0x40 of 45 + channel.
const GAIN_BASE: usize = 33;
const PHASE_BASE: usize = 45;
const PHASE_BIT: u8 = 0x40;
const CONFIRM_WITHIN_NS: u64 = 100_000_000;

fn fixture_events() -> (SessionInfo, Vec<UsbEvent>) {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let info: SessionInfo =
        serde_json::from_slice(&std::fs::read(fixtures.join("usbpcap-live-2.session.json")).unwrap()).unwrap();
    let mut map = DeviceMap::default();
    let mut events = Vec::new();
    for frame in open_frames(&fixtures.join("usbpcap-live-2.pcapng")).unwrap() {
        if let Some(ev) = decode(&frame.unwrap()).unwrap() {
            map.observe(&ev);
            events.push(ev);
        }
    }
    let (bus, device) = map.find(info.vid, info.pid).expect("target descriptor stored in the capture");
    let hex: String = map.get(bus, device).unwrap().descriptor.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(Some(hex), info.device_descriptor_hex);
    events.retain(|ev| ev.bus == bus && ev.device == device && ev.urb_id != 0);
    (info, events)
}

fn commands(events: &[UsbEvent]) -> Vec<&UsbEvent> {
    events.iter().filter(|ev| ev.direction == Direction::Out && ev.endpoint == 1 && !ev.data.is_empty()).collect()
}

fn status_reports(events: &[UsbEvent]) -> impl Iterator<Item = &UsbEvent> {
    events
        .iter()
        .filter(|ev| ev.direction == Direction::In && ev.endpoint == 2 && ev.data.len() == REPORT_LEN && ev.data[0] == STATUS)
}

#[test]
fn channel_8_commands_decode_to_the_probed_values() {
    let (_, events) = fixture_events();
    let decoded: Vec<(u8, u8, i8)> = commands(&events)
        .iter()
        .map(|ev| {
            assert_eq!(ev.data.len(), REPORT_LEN);
            assert_eq!((ev.data[0], ev.data[4]), (CMD, 0x13));
            (ev.data[16], ev.data[17], ev.data[18] as i8)
        })
        .collect();

    // Step 2: dragged 10 -> -6 -> 0 dB. Step 4: wheel 0 -> 10 dB, one command per dB.
    // Step 5: typed 0 dB, one absolute command. Step 6: phase invert on.
    let mut expected: Vec<(u8, u8, i8)> = [9, -6, -5, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0]
        .into_iter()
        .map(|db| (SET_PRE_GAIN, CHANNEL, db))
        .collect();
    expected.push((SET_PRE_PHASEINV, CHANNEL, 1));
    assert_eq!(decoded, expected);
}

#[test]
fn every_command_is_confirmed_by_the_status_report() {
    let (_, events) = fixture_events();
    let reports: Vec<&UsbEvent> = status_reports(&events).collect();
    assert!(!reports.is_empty());
    assert_eq!(reports[0].data[GAIN_BASE + CHANNEL as usize] as i8, 10, "Channel 8 starts at 10 dB");
    assert_eq!(reports[0].data[PHASE_BASE + CHANNEL as usize] & PHASE_BIT, 0, "phase starts off");

    for cmd in commands(&events) {
        let (id, value) = (cmd.data[16], cmd.data[18]);
        let confirmed = reports.iter().filter(|r| r.ts_ns > cmd.ts_ns && r.ts_ns - cmd.ts_ns <= CONFIRM_WITHIN_NS).any(|r| match id {
            SET_PRE_GAIN => r.data[GAIN_BASE + CHANNEL as usize] == value,
            SET_PRE_PHASEINV => (r.data[PHASE_BASE + CHANNEL as usize] & PHASE_BIT != 0) == (value != 0),
            other => panic!("unexpected payload id {other:#04x}"),
        });
        assert!(confirmed, "command id {id:#04x} value {} at {} not confirmed within 100 ms", value as i8, cmd.ts_ns);
    }
}
