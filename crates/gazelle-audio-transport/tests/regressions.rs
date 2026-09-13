//! Regression tests for defects found in the 2026-09-12 review of the transport crate.
//!
//! These cover the receive path, which is device-controlled input: a malformed or
//! out-of-order segment must be rejected, never panic and never wedge the reassembler.

use gazelle_audio_protocol::wire::Header;
use gazelle_audio_transport::framing::{parse_send_segment, Reassembly, Segment, SEGMENT_SEND_ID};
use gazelle_audio_transport::{Device, LoopbackDevice, RawPacket};

/// A segment whose `seq` is below the header size must be rejected, not underflow.
///
/// Previously this panicked in debug builds and wrapped in release, so the two profiles
/// disagreed on attacker- or fault-controlled input.
#[test]
fn segment_with_undersized_seq_is_rejected() {
    let mut buf = Header::new(SEGMENT_SEND_ID, 4, 4, 0).to_bytes().to_vec();
    buf.extend_from_slice(&[0u8; 4]);
    assert!(
        parse_send_segment(&buf).is_err(),
        "seq below HEADER_SIZE must be an error, not an underflow"
    );
}

/// A reassembled message shorter than a header must be dropped, not sliced.
#[test]
fn short_reassembled_message_does_not_panic() {
    let mut dev = LoopbackDevice::new(9189, 0xa2f9, 64);
    // total_len = 4, below the 16-byte header.
    let mut buf = Header::new(SEGMENT_SEND_ID, 4 + 16, 4, 0).to_bytes().to_vec();
    buf.extend_from_slice(&[1, 2, 3, 4]);
    dev.on_received_data(RawPacket { bytes: buf });
    assert!(dev.poll_reports().is_empty(), "a malformed short message yields no report");
}

/// One dropped segment must not wedge the reassembler forever.
#[test]
fn reassembler_recovers_after_a_dropped_segment() {
    let mut r = Reassembly::default();

    // Start a 12-byte message, then skip straight to offset 8 (segment 2 lost).
    assert!(r.push(Segment { total_len: 12, offset: 0, data: vec![0xAA; 4] }).unwrap().is_none());
    assert!(
        r.push(Segment { total_len: 12, offset: 8, data: vec![0xCC; 4] }).is_err(),
        "the out-of-order segment is rejected"
    );

    // A fresh message of the SAME total_len must now succeed. Previously the stale
    // partial buffer survived the error and failed every subsequent offset check.
    assert!(r.push(Segment { total_len: 12, offset: 0, data: vec![1; 4] }).unwrap().is_none());
    assert!(r.push(Segment { total_len: 12, offset: 4, data: vec![2; 4] }).unwrap().is_none());
    let done = r
        .push(Segment { total_len: 12, offset: 8, data: vec![3; 4] })
        .unwrap()
        .expect("final segment completes the message");
    assert_eq!(done, [vec![1; 4], vec![2; 4], vec![3; 4]].concat());
}

/// `pending()` must not consume the reports it counts.
#[test]
fn pending_does_not_destroy_reports() {
    let mut dev = LoopbackDevice::new(9189, 0xa2f9, 64);

    let contents = [0xEFu8; 8];
    let mut msg = Header::new(0x70, 8, 0, 0).to_bytes().to_vec();
    msg.extend_from_slice(&contents);
    let mut seg = Header::new(SEGMENT_SEND_ID, (msg.len() + 16) as u32, msg.len() as u32, 0)
        .to_bytes()
        .to_vec();
    seg.extend_from_slice(&msg);
    dev.on_received_data(RawPacket { bytes: seg });

    assert_eq!(dev.pending(), 1, "one report is waiting");
    let reports = dev.poll_reports();
    assert_eq!(reports.len(), 1, "pending() must not have consumed it");
    assert_eq!(reports[0].contents, contents);
}

/// Correlation follows the device's own rule, recovered from `_sanitize_response`.
///
/// Previously this matched on `seq`, which the project notes asserted but nothing verified.
/// Reading the bytecode showed the device matches on `cmd` and `ext2` instead.
#[test]
fn correlation_uses_cmd_and_ext2_not_seq() {
    use gazelle_audio_protocol::wire::Header;
    use gazelle_audio_transport::correlation::{Correlation, ResponseCorrelator};
    use gazelle_audio_transport::Report;

    let mut c = ResponseCorrelator::new();
    c.record_request(0x74, 4, "get_mixer");

    // Same seq as the request but the wrong cmd: must NOT match, though the old
    // seq-based correlator would have accepted it.
    let wrong = Report { header: Header::new(0x73, 0, 4, 0), contents: vec![0; 4] };
    assert!(matches!(c.correlate(wrong), Correlation::Unmatched { .. }));

    // Correct response: report_id + 1, ext2 echoed.
    let right = Report { header: Header::new(0x75, 999, 4, 0), contents: vec![0; 4] };
    assert!(
        matches!(c.correlate(right), Correlation::Matched { .. }),
        "seq is irrelevant; cmd+1 and ext2 are what matter"
    );
}
