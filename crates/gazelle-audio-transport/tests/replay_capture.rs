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
/// the device refused by setting the top bit of the reply id. That refusal is also the case the
/// correlator does not match today, so a request answered this way waits for its timeout.
#[test]
fn live_replies_arrive_whole_and_a_refusal_is_not_correlated() {
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
    let mut correlator = ResponseCorrelator::new();
    correlator.record_request(0x74, refusal.ext2(), "the refused read");
    assert!(
        !matches!(correlator.correlate(refusal), Correlation::Matched { .. }),
        "a refusal does not answer its request today: the USB backend will have to handle it",
    );
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
