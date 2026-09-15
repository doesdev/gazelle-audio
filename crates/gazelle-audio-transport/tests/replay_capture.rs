//! Real device traffic through the host receive path: inbound packets extracted from a live
//! Studio+ capture (see the fixture's header) each arrive as one whole report, and the state
//! reports parse with the Studio+ schema. The traffic is cyclic only; segment reassembly is
//! covered by `host_receive.rs`.

use std::collections::BTreeMap;

use gazelle_audio_protocol::registry::from_json_doc;
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
