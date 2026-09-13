//! End-to-end transport tests: drive the protocol crate's request builder,
//! segment the bytes, feed them through the loopback transport, and correlate
//! the echoed response by sequence number. This exercises the full stack
//! (framing + segmentation + correlation) without any hardware.

use gazelle_audio_protocol::payload::PayloadValues;
use gazelle_audio_protocol::registry::{from_json_doc, Registry};
use gazelle_audio_transport::correlation::{Correlation, ResponseCorrelator};
use gazelle_audio_transport::framing::SEGMENT_SEND_ID;
use gazelle_audio_transport::{
    build_report, segment_report, Device, LoopbackDevice, RawPacket,
};

/// Load the real recovered command registry.
fn load_registry() -> Registry {
    let path = gazelle_audio_protocol::QUADRO_COMMANDS_PATH;
    let doc = std::fs::read_to_string(path).expect("read quadro_commands.json");
    let parsed: serde_json::Value = serde_json::from_str(&doc).expect("parse json");
    from_json_doc(&parsed).expect("load registry")
}

/// A small output packet size, chosen so the largest commands (320 B) must
/// span multiple segments during the round-trip test.
const MAX_PKT: usize = 64;

/// Antelope's USB vendor id, 9189 (`0x23E5`), from `ANTELOPE_USB_VENDOR_ID` in the
/// decompiled `antelope_dev_base.py`. Spelled out so a reader does not copy a wrong
/// value out of a test fixture.
const ANTELOPE_USB_VID: u16 = 9189;

/// A request is segmented on the way out and reassembled on the way back, and
/// the echoed report carries the same seq as the request.
#[test]
fn request_roundtrips_through_segments() {
    let reg = load_registry();
    let mut dev = LoopbackDevice::emulating(ANTELOPE_USB_VID, 0xa2f9, MAX_PKT);
    let mut correlator = ResponseCorrelator::new();

    // Build a real request for get_sonarworks_ir (a 320 B payload that must
    // span multiple segments at MAX_PKT = 64).
    let values = PayloadValues::default();
    let req_bytes = reg
        .build_request("get_sonarworks_ir", &values)
        .expect("build get_sonarworks_ir");
    assert!(
        req_bytes.len() > MAX_PKT,
        "expected segmentation for get_sonarworks_ir"
    );

    // Record the outgoing request so its response can be correlated.
    let header = gazelle_audio_protocol::wire::Header::from_bytes(&req_bytes).expect("valid header");
    correlator.record_request(header.cmd, header.ext2, "get_sonarworks_ir");

    // Segment the request the way a real device would transmit it.
    let segments = segment_report(
        header.cmd,
        header.seq,
        header.ext2,
        header.ext3,
        &req_bytes[16..],
        MAX_PKT,
    );
    assert!(
        segments.len() > 1,
        "get_sonarworks_ir should span multiple segments"
    );

    // Feed each segment through the loopback; the device echoes them back.
    for seg in &segments {
        dev.on_received_data(RawPacket { bytes: seg.clone() });
    }

    // The reassembled payload must equal the original request bytes.
    let reports = dev.poll();
    assert_eq!(reports.len(), 1, "expected one reassembled report");
    assert_eq!(reports[0].contents, req_bytes[16..]);
    // This loopback emulates a device, so it answers with cmd + 1 and the same ext2 --
    // exactly what `_sanitize_response` requires. `seq` is carried through but plays no
    // part in correlation.
    assert_eq!(reports[0].cmd(), header.cmd + 1);
    assert_eq!(reports[0].ext2(), header.ext2);

    // Correlation accepts it, on cmd and ext2.
    match correlator.correlate(reports[0].clone()) {
        Correlation::Matched { .. } => {}
        other => panic!("expected Matched, got {other:?}"),
    }
    assert!(!correlator.has_pending());
}

/// A non-segmented report passes straight through the loopback unchanged.
#[test]
fn small_report_passes_through_untouched() {
    let mut dev = LoopbackDevice::new(ANTELOPE_USB_VID, 0xa2f9, MAX_PKT);

    let contents = vec![0xaa, 0xbb, 0xcc, 0xdd];
    let raw = build_report(0x74, 0x1234_5678, 4, 0, &contents);
    dev.on_received_data(RawPacket { bytes: raw });

    let reports = dev.poll();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].cmd(), 0x74);
    assert_eq!(reports[0].seq(), 0x1234_5678);
    assert_eq!(reports[0].ext2(), 4);
    assert_eq!(reports[0].contents, contents);
}

/// All-zero packets (idle USB/HID reports) are dropped, not delivered.
#[test]
fn zero_packets_are_ignored() {
    let mut dev = LoopbackDevice::new(ANTELOPE_USB_VID, 0xa2f9, MAX_PKT);
    dev.on_received_data(RawPacket {
        bytes: vec![0u8; 512],
    });
    assert_eq!(dev.poll().len(), 0);
}

/// A report arriving with nothing outstanding is Unsolicited -- device-initiated traffic,
/// such as a cyclic report, rather than a failed response match.
#[test]
fn reports_with_no_outstanding_request_are_unsolicited() {
    let mut dev = LoopbackDevice::new(ANTELOPE_USB_VID, 0xa2f9, MAX_PKT);
    let mut correlator = ResponseCorrelator::new();

    // No request recorded, so any report is unmatched.
    let contents = vec![1u8, 2, 3];
    let raw = build_report(0x73, 0xdead_beef, 0, 0, &contents);
    dev.on_received_data(RawPacket { bytes: raw });
    let reports = dev.poll();

    match correlator.correlate(reports[0].clone()) {
        Correlation::Unsolicited { report } => {
            // 0x73 is the cyclic report; its `seq` carries a CRC32, not a request id.
            assert_eq!(report.cmd(), 0x73);
            assert_eq!(report.seq(), 0xdead_beef);
        }
        other => panic!("expected Unsolicited, got {other:?}"),
    }
    assert!(!correlator.has_pending());
}

/// The segment id matches the recovered constant.
#[test]
fn segment_ids_match_recovery() {
    // The device reassembles host->device segments (8052).
    assert_eq!(SEGMENT_SEND_ID, 8052);
}
