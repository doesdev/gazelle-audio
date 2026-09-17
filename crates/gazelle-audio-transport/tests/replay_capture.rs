//! Real device traffic through the host receive path: inbound packets extracted from live Studio+
//! captures (see each fixture's header) arrive as whole reports — state reports, replies, and the
//! one refusal the device sent. Neither capture held a segmented message, so segment reassembly is
//! covered by `host_receive.rs` instead.

use std::collections::BTreeMap;

use gazelle_audio_protocol::registry::from_json_doc;
use gazelle_audio_transport::correlation::{Correlation, ResponseCorrelator};
use gazelle_audio_transport::HostReceiver;

fn packets() -> Vec<Vec<u8>> {
    let text = include_str!("fixtures/studio-live-inbound.hex");
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| (0..l.len()).step_by(2).map(|i| u8::from_str_radix(&l[i..i + 2], 16).unwrap()).collect())
        .collect()
}

#[test]
fn live_inbound_packets_arrive_whole_and_the_state_reports_parse() {
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(gazelle_audio_protocol::STUDIO_COMMANDS_PATH).unwrap()).unwrap();
    let registry = from_json_doc(&doc).unwrap();
    let state = registry.cyclic(0x73).expect("the Studio+ schema declares its state report");

    let packets = packets();
    assert_eq!(packets.len(), 60);
    let mut host = HostReceiver::new();
    let mut counts = BTreeMap::<u32, usize>::new();
    for packet in &packets {
        assert_eq!(packet.len(), 320, "packets are kept exactly as read");
        let report = host.push(packet).expect("each inbound packet is a whole report");
        *counts.entry(report.cmd()).or_default() += 1;
        assert_eq!(report.contents.len(), 320 - 16, "a whole report keeps its padding");
        if report.cmd() == 0x73 {
            let fields = state.parse_contents(&report.contents).expect("the state report parses");
            assert!(fields.contains_key("monitor_vol"), "the parsed state carries the output levels");
        }
    }
    assert_eq!(counts, BTreeMap::from([(0x73, 30), (0x83, 30)]));
}

/// Replies from a live panel start-up (hardware session 2): each arrives whole, including the one
/// the device refused. A refusal keeps the header of the reply it stands for with the top bit set
/// in both `cmd` and `ext2` (`ext3` and the contents are zero), so it completes the one request
/// with that `cmd` and `ext2` as refused, and nothing else.
#[test]
fn live_replies_arrive_whole_and_a_refusal_fails_its_request() {
    let text = include_str!("fixtures/studio-live-replies.hex");
    let packets: Vec<Vec<u8>> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| (0..l.len()).step_by(2).map(|i| u8::from_str_radix(&l[i..i + 2], 16).unwrap()).collect())
        .collect();
    assert_eq!(packets.len(), 130);

    let mut host = HostReceiver::new();
    let mut counts = BTreeMap::<u32, usize>::new();
    let mut refusal = None;
    for packet in &packets {
        let report = host.push(packet).expect("each inbound packet is a whole report");
        assert_eq!(report.contents.len(), 320 - 16);
        *counts.entry(report.cmd()).or_default() += 1;
        if report.cmd() == 0x8000_0075 {
            refusal = Some(report);
        }
    }
    assert_eq!(counts, BTreeMap::from([(0x73, 50), (0x75, 29), (0x83, 50), (0x8000_0075, 1)]));

    // The device answers `get_*` (0x74) with 0x75; the refusal is that id with the top bit set.
    let refusal = refusal.expect("the fixture carries the refused reply");
    assert_eq!((refusal.ext2(), refusal.header.ext3), (0x8000_0011, 0), "ext2 is flagged too, and ext3 is not echoed");
    assert!(refusal.contents.iter().all(|&b| b == 0), "a refusal carries nothing");

    // ext2 17 is the licence group (`get_feature_mask` and the assignment reads on the Quadro).
    let mut correlator = ResponseCorrelator::new();
    correlator.record_request(0x74, 4, "get_mixer");
    assert!(matches!(correlator.correlate(refusal.clone()), Correlation::Unmatched { .. }), "not another selector's refusal");
    correlator.record_request(0x74, 0x11, "the refused read");
    assert!(matches!(correlator.correlate(refusal), Correlation::Refused { .. }));
    assert!(!correlator.has_pending(), "the refused request does not wait for its timeout");
}

/// Live HID traffic from a Quadro (hardware session 2): the device wraps a report in an 8053
/// receive segment, which no USBPcap capture contained. `HostReceiver` reassembles those and passes
/// the unsegmented reports through, so a reader thread sees only whole reports either way.
#[test]
fn live_quadro_segments_reassemble_into_whole_reports() {
    let text = include_str!("fixtures/quadro-live-segments.hex");
    let packets: Vec<Vec<u8>> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| (0..l.len()).step_by(2).map(|i| u8::from_str_radix(&l[i..i + 2], 16).unwrap()).collect())
        .collect();
    assert_eq!(packets.len(), 120);
    assert!(
        packets.iter().any(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]) == gazelle_audio_transport::framing::SEGMENT_RECEIVE_ID),
        "the fixture carries real 8053 segments",
    );

    let mut host = HostReceiver::new();
    let mut counts = BTreeMap::<u32, usize>::new();
    let mut segmented_len = None;
    for packet in &packets {
        let segment = u32::from_le_bytes([packet[0], packet[1], packet[2], packet[3]]) == gazelle_audio_transport::framing::SEGMENT_RECEIVE_ID;
        let report = host.push(packet).expect("every packet completes a report");
        *counts.entry(report.cmd()).or_default() += 1;
        if segment {
            // The segment's `ext2` is the whole message length, so the contents end there rather
            // than running to the packet's padding.
            segmented_len = Some(report.contents.len());
        } else {
            assert_eq!(report.contents.len(), 320 - 16, "an unsegmented report keeps its padding");
        }
    }
    assert_eq!(counts, BTreeMap::from([(0x73, 60), (0x83, 60)]));
    assert_eq!(segmented_len, Some(4), "the Quadro wraps a 20-byte message: a header and four bytes");
}

/// The live Quadro's effect-meter report decodes, rather than being dropped. Its 0x83 arrives
/// wrapped in an 8053 segment whose `ext2` gives the true length, and with no effects loaded that
/// is four bytes: the mic emulation meters, idle at 96 dB below full scale. Before the schema
/// marked `afx_meters` as a variable tail, every one of these (about 125 a second) was discarded.
#[test]
fn the_live_quadro_effect_meter_report_decodes() {
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(gazelle_audio_protocol::QUADRO_COMMANDS_PATH).unwrap()).unwrap();
    let registry = from_json_doc(&doc).unwrap();
    let meters = registry.cyclic(0x83).expect("the Quadro schema declares its effect-meter report");

    let text = include_str!("fixtures/quadro-live-segments.hex");
    let mut host = HostReceiver::new();
    let mut decoded = 0;
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let packet: Vec<u8> = (0..line.len()).step_by(2).map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap()).collect();
        let report = host.push(&packet).expect("every packet completes a report");
        if report.cmd() != 0x83 {
            continue;
        }
        let fields = meters.parse_contents(&report.contents).expect("the effect-meter report decodes");
        let Some(gazelle_audio_protocol::payload::Value::List(items)) = fields.get("afx_meters") else { panic!("no afx_meters") };
        let [gazelle_audio_protocol::payload::Value::Struct(one)] = items.as_slice() else { panic!("afx_meters is not one struct") };
        let Some(gazelle_audio_protocol::payload::Value::Bytes(data)) = one.get("data") else { panic!("no data") };
        assert_eq!(data.as_slice(), &[96, 96, 96, 96], "no effects loaded, so only the four idle mic emulation meters");
        decoded += 1;
    }
    assert_eq!(decoded, 60, "every effect-meter report in the capture decodes");
}
