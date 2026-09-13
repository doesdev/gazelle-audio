//! usbmon decoder against hand-assembled 64-byte `struct usbmon_packet` headers.

use gazelle_audio_capture::capture::decode::{self, usbmon, ByteOrder, DecodeError};
use gazelle_audio_capture::capture::event::{Direction, SetupPacket, TransferType, UrbStage, UsbEvent};
use gazelle_audio_capture::capture::RawFrame;

fn frame(byte_order: ByteOrder, data: Vec<u8>) -> RawFrame {
    RawFrame {
        ts_ns: 1_700_000_000_123_456_000,
        link_type: 220,
        index: 7,
        orig_len: data.len() as u32,
        byte_order,
        data,
    }
}

/// struct usbmon_packet, 64 bytes, fields in `order`.
fn usbmon_header(order: ByteOrder, kind: u8, xfer: u8, ep: u8, dev: u8, flag_setup: u8, len_cap: u32) -> Vec<u8> {
    let mut h = vec![0u8; 64];
    let put16 = |v: u16| match order {
        ByteOrder::Little => v.to_le_bytes(),
        ByteOrder::Big => v.to_be_bytes(),
    };
    let put32 = |v: u32| match order {
        ByteOrder::Little => v.to_le_bytes(),
        ByteOrder::Big => v.to_be_bytes(),
    };
    let id = match order {
        ByteOrder::Little => 0x0102_0304_0506_0708u64.to_le_bytes(),
        ByteOrder::Big => 0x0102_0304_0506_0708u64.to_be_bytes(),
    };
    h[0..8].copy_from_slice(&id); // id
    h[8] = kind; // type
    h[9] = xfer; // xfer_type
    h[10] = ep; // epnum
    h[11] = dev; // devnum
    h[12..14].copy_from_slice(&put16(3)); // busnum
    h[14] = flag_setup;
    h[28..32].copy_from_slice(&put32((-32i32) as u32)); // status: -EPIPE
    h[32..36].copy_from_slice(&put32(len_cap)); // length
    h[36..40].copy_from_slice(&put32(len_cap)); // len_cap
    h[40..48].copy_from_slice(&[0xC0, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00]); // setup
    h
}

#[test]
fn control_submit_reads_setup_from_header() {
    let b = usbmon_header(ByteOrder::Little, b'S', 2, 0x80, 4, 0, 0);
    let ev = usbmon::decode(&frame(ByteOrder::Little, b)).unwrap().unwrap();
    assert_eq!(ev.stage, UrbStage::Submit);
    assert_eq!((ev.bus, ev.device, ev.endpoint, ev.direction), (3, 4, 0, Direction::In));
    assert_eq!(ev.urb_id, 0x0102_0304_0506_0708);
    assert_eq!(ev.status, -32);
    assert_eq!(ev.setup, Some(SetupPacket { request_type: 0xC0, request: 6, value: 0x0100, index: 0, length: 0x12 }));
}

#[test]
fn big_endian_completion_with_data() {
    let b = [usbmon_header(ByteOrder::Big, b'C', 1, 0x81, 4, b'-', 3), vec![5, 6, 7]].concat();
    let ev = usbmon::decode(&frame(ByteOrder::Big, b)).unwrap().unwrap();
    assert_eq!(ev.stage, UrbStage::Complete);
    assert_eq!(ev.bus, 3);
    assert_eq!(ev.urb_id, 0x0102_0304_0506_0708);
    assert_eq!(ev.setup, None);
    assert_eq!((ev.data_len, ev.data, ev.payload_dropped), (3, vec![5, 6, 7], false));
}

#[test]
fn iso_descriptors_precede_data() {
    let mut b = usbmon_header(ByteOrder::Little, b'C', 0, 0x81, 4, b'-', 2);
    b[60..64].copy_from_slice(&1u32.to_le_bytes()); // ndesc
    b.extend_from_slice(&[0; 16]);
    b.extend_from_slice(&[0xEE, 0xFF]);
    let ev = usbmon::decode(&frame(ByteOrder::Little, b)).unwrap().unwrap();
    assert_eq!(ev.data, vec![0xEE, 0xFF]);
}

#[test]
fn dispatch_routes_link_type_220() {
    let b = [usbmon_header(ByteOrder::Little, b'C', 1, 0x81, 4, b'-', 1), vec![9]].concat();
    let f = frame(ByteOrder::Little, b);
    assert_eq!(decode::decode(&f).unwrap().unwrap().data, vec![9]);
    assert_eq!(decode::header_len(&f).unwrap(), 64);
}

#[test]
fn encode_round_trips_non_iso_transfers() {
    let base = UsbEvent {
        ts_ns: 1_700_000_000_000_001_000,
        packet_index: 7,
        bus: 1,
        device: 5,
        endpoint: 0,
        direction: Direction::Out,
        transfer: TransferType::Control,
        stage: UrbStage::Submit,
        urb_id: 42,
        setup: Some(SetupPacket { request_type: 0x21, request: 1, value: 0x0100, index: 0x0200, length: 2 }),
        status: 0,
        data_len: 2,
        data: vec![0xFE, 0xFF],
        payload_dropped: false,
    };
    let events = [
        base.clone(),
        UsbEvent { stage: UrbStage::Complete, setup: None, direction: Direction::In, data_len: 0, data: vec![], ..base.clone() },
        UsbEvent { transfer: TransferType::Interrupt, endpoint: 3, direction: Direction::In, setup: None, stage: UrbStage::Complete, data_len: 4, data: vec![1, 2, 3, 4], ..base.clone() },
        UsbEvent { transfer: TransferType::Bulk, endpoint: 2, setup: None, data_len: 3, data: vec![7, 7, 7], ..base },
    ];
    for ev in events {
        let back = usbmon::decode(&frame(ByteOrder::Little, usbmon::encode(&ev))).unwrap().unwrap();
        assert_eq!(UsbEvent { ts_ns: ev.ts_ns, packet_index: ev.packet_index, ..back }, ev);
    }
}

// --- Amendment (Task 3): data_len comes from `length` (offset 32), not `len_cap` (offset
// 36); payload_dropped = len_cap < length. These exercise that distinction directly, plus
// truncated-record / hostile-input paths that must error or cap safely, never panic.

#[test]
fn length_and_len_cap_diverge_data_len_uses_length() {
    // usbmon truncation: the kernel declares the full transfer was 20 bytes (`length`) but
    // only captured 3 of them (`len_cap`), and only those 3 bytes follow the header.
    let mut b = usbmon_header(ByteOrder::Little, b'C', 1, 0x81, 4, b'-', 0);
    b[32..36].copy_from_slice(&20u32.to_le_bytes()); // length
    b[36..40].copy_from_slice(&3u32.to_le_bytes()); // len_cap
    b.extend_from_slice(&[1, 2, 3]);
    let ev = usbmon::decode(&frame(ByteOrder::Little, b)).unwrap().unwrap();
    assert_eq!((ev.data_len, ev.data, ev.payload_dropped), (20, vec![1, 2, 3], true));
}

#[test]
fn len_cap_at_least_length_is_not_dropped() {
    let mut b = usbmon_header(ByteOrder::Little, b'C', 1, 0x81, 4, b'-', 0);
    b[32..36].copy_from_slice(&3u32.to_le_bytes()); // length
    b[36..40].copy_from_slice(&3u32.to_le_bytes()); // len_cap
    b.extend_from_slice(&[1, 2, 3]);
    let ev = usbmon::decode(&frame(ByteOrder::Little, b)).unwrap().unwrap();
    assert_eq!((ev.data_len, ev.data, ev.payload_dropped), (3, vec![1, 2, 3], false));
}

#[test]
fn frame_shorter_than_len_cap_is_capped_not_indexed_past() {
    // A hostile/corrupt record: len_cap and length both claim 50 bytes of payload, but the
    // frame itself only has 2 bytes after the header. Must cap to what is actually present,
    // never panic or read past the end of the frame.
    let mut b = usbmon_header(ByteOrder::Little, b'C', 1, 0x81, 4, b'-', 0);
    b[32..36].copy_from_slice(&50u32.to_le_bytes()); // length
    b[36..40].copy_from_slice(&50u32.to_le_bytes()); // len_cap
    b.extend_from_slice(&[9, 9]);
    let ev = usbmon::decode(&frame(ByteOrder::Little, b)).unwrap().unwrap();
    // Only 2 of the declared 50 bytes are present, so payload_dropped must be true even
    // though len_cap and length agree with each other (fix round 1: payload_dropped must
    // track data.len() < data_len, not just len_cap < length).
    assert_eq!((ev.data_len, ev.data, ev.payload_dropped), (50, vec![9, 9], true));
}

#[test]
fn header_shorter_than_64_bytes_is_truncated() {
    let b = vec![0u8; 40];
    assert_eq!(
        usbmon::decode(&frame(ByteOrder::Little, b.clone())),
        Err(DecodeError::Truncated { needed: 64, got: 40 })
    );
    assert_eq!(
        usbmon::header_len(&b, ByteOrder::Little),
        Err(DecodeError::Truncated { needed: 64, got: 40 })
    );
}

#[test]
fn huge_ndesc_beyond_frame_is_bad_header_len_not_a_panic() {
    // A hostile iso record claims far more descriptors than the frame could possibly hold.
    // The multiplication must not overflow/panic; it must simply fail validation.
    let mut b = usbmon_header(ByteOrder::Little, b'C', 0, 0x81, 4, b'-', 0);
    b[60..64].copy_from_slice(&u32::MAX.to_le_bytes()); // ndesc
    assert_eq!(
        usbmon::decode(&frame(ByteOrder::Little, b.clone())),
        Err(DecodeError::BadHeaderLen { declared: 64 + u32::MAX as usize * 16, frame: 64 })
    );
    assert_eq!(
        usbmon::header_len(&b, ByteOrder::Little),
        Err(DecodeError::BadHeaderLen { declared: 64 + u32::MAX as usize * 16, frame: 64 })
    );
}
