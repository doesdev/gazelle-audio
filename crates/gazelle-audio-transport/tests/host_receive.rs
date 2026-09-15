//! The host side of the receive path: what a real adapter's reader feeds in from the device.
//!
//! The device sends large messages as 8053 segments (the host sends 8052; see `framing.rs`),
//! and every other report whole, zero-padded to the packet size. `LoopbackDevice` stands in for
//! the device and reassembles 8052, so the host needs its own reassembler for 8053.

use gazelle_audio_protocol::wire::{Header, HEADER_SIZE};
use gazelle_audio_transport::framing::{split_in_segments, SEGMENT_RECEIVE_ID};
use gazelle_audio_transport::{build_report, HostReceiver};

const PACKET: usize = 64;

/// The device's segments for one message: 8053 headers, each packet zero-padded.
fn device_segments(message: &[u8]) -> Vec<Vec<u8>> {
    split_in_segments(message, PACKET)
        .into_iter()
        .map(|seg| {
            let mut buf = Header::new(SEGMENT_RECEIVE_ID, (seg.data.len() + HEADER_SIZE) as u32, seg.total_len as u32, seg.offset as u32)
                .to_bytes()
                .to_vec();
            buf.extend_from_slice(&seg.data);
            buf.resize(PACKET, 0);
            buf
        })
        .collect()
}

fn padded(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.resize(PACKET, 0);
    bytes
}

#[test]
fn device_segments_reassemble_into_one_report_with_its_exact_contents() {
    let contents: Vec<u8> = (0..99u8).collect(); // a 99-byte reply, like a Studio+ get_mixer
    let message = build_report(0x75, 0, 12, 2, &contents);
    let segments = device_segments(&message);
    assert!(segments.len() > 1, "the test message must span segments");

    let mut host = HostReceiver::new();
    let mut reports = Vec::new();
    for seg in &segments {
        reports.extend(host.push(seg));
    }
    assert_eq!(reports.len(), 1);
    assert_eq!((reports[0].cmd(), reports[0].ext2(), reports[0].ext3()), (0x75, 12, 2));
    assert_eq!(reports[0].contents, contents, "segment padding is not part of the message");
}

#[test]
fn whole_reports_pass_through_and_a_cyclic_report_between_segments_does_not_break_reassembly() {
    let reply = build_report(0x75, 0, 12, 0, &[7u8; 80]);
    let segments = device_segments(&reply);
    let cyclic = padded(build_report(0x73, 0x1234, 0, 0, &[1, 2, 3]));

    let mut host = HostReceiver::new();
    let mut cmds = Vec::new();
    for packet in [segments[0].clone(), cyclic.clone(), vec![0u8; PACKET]].iter().chain(segments[1..].iter()) {
        cmds.extend(host.push(packet).map(|r| r.cmd()));
    }
    assert_eq!(cmds, vec![0x73, 0x75], "the cyclic report arrives whole; idle all-zero packets are skipped");

    let whole = host.push(&cyclic).expect("a whole report");
    assert_eq!(whole.seq(), 0x1234);
    assert_eq!(&whole.contents[..3], &[1, 2, 3]);
}

#[test]
fn malformed_device_segments_are_dropped_without_wedging() {
    let mut host = HostReceiver::new();
    // seq below the header size.
    let bad = padded(Header::new(SEGMENT_RECEIVE_ID, 4, 40, 0).to_bytes().to_vec());
    assert!(host.push(&bad).is_none());
    // A message shorter than a header.
    let mut short = Header::new(SEGMENT_RECEIVE_ID, (HEADER_SIZE + 4) as u32, 4, 0).to_bytes().to_vec();
    short.extend_from_slice(&[1, 2, 3, 4]);
    assert!(host.push(&padded(short)).is_none());
    // A packet too short for a header.
    assert!(host.push(&[0x73, 0, 0]).is_none());

    let reply = build_report(0x71, 0, 5, 0, &[9u8; 70]);
    let reports: Vec<_> = device_segments(&reply).iter().filter_map(|s| host.push(s)).collect();
    assert_eq!(reports.len(), 1, "a good message still reassembles afterwards");
}
