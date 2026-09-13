//! USBPcap decoder against hand-assembled frames whose offsets come from the published
//! `USBPCAP_BUFFER_PACKET_HEADER` layout, not from the crate's encoder.

use gazelle_audio_capture::capture::decode::{self, usbpcap, ByteOrder, DecodeError};
use gazelle_audio_capture::capture::event::{Direction, SetupPacket, TransferType, UrbStage, UsbEvent};
use gazelle_audio_capture::capture::RawFrame;

fn frame(data: Vec<u8>) -> RawFrame {
    RawFrame {
        ts_ns: 1_700_000_000_123_456_000,
        link_type: 249,
        index: 7,
        orig_len: data.len() as u32,
        byte_order: ByteOrder::Little,
        data,
    }
}

/// USBPCAP_BUFFER_PACKET_HEADER, little-endian, packed.
#[allow(clippy::too_many_arguments)]
fn usbpcap_header(
    header_len: u16,
    irp: u64,
    status: u32,
    info: u8,
    bus: u16,
    device: u16,
    endpoint: u8,
    transfer: u8,
    data_len: u32,
) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend_from_slice(&header_len.to_le_bytes()); // 0
    h.extend_from_slice(&irp.to_le_bytes()); // 2
    h.extend_from_slice(&status.to_le_bytes()); // 10
    h.extend_from_slice(&0x0009u16.to_le_bytes()); // 14 function
    h.push(info); // 16
    h.extend_from_slice(&bus.to_le_bytes()); // 17
    h.extend_from_slice(&device.to_le_bytes()); // 19
    h.push(endpoint); // 21
    h.push(transfer); // 22
    h.extend_from_slice(&data_len.to_le_bytes()); // 23
    assert_eq!(h.len(), 27);
    h
}

#[test]
fn control_setup_submit_carries_setup_and_out_data() {
    let mut b = usbpcap_header(28, 0xFFFF_A001, 0, 0x00, 1, 5, 0x00, 2, 8 + 2);
    b.push(0); // stage: setup
    b.extend_from_slice(&[0x40, 0x01, 0x34, 0x12, 0x02, 0x00, 0x02, 0x00]);
    b.extend_from_slice(&[0xAA, 0xBB]);
    let ev = usbpcap::decode(&frame(b)).unwrap().unwrap();
    assert_eq!(
        ev,
        UsbEvent {
            ts_ns: 1_700_000_000_123_456_000,
            packet_index: 7,
            bus: 1,
            device: 5,
            endpoint: 0,
            direction: Direction::Out,
            transfer: TransferType::Control,
            stage: UrbStage::Submit,
            urb_id: 0xFFFF_A001,
            setup: Some(SetupPacket { request_type: 0x40, request: 1, value: 0x1234, index: 2, length: 2 }),
            status: 0,
            data_len: 2,
            data: vec![0xAA, 0xBB],
            payload_dropped: false,
        }
    );
}

#[test]
fn control_complete_in_has_no_setup() {
    let mut b = usbpcap_header(28, 9, 0, 0x01, 1, 5, 0x80, 2, 3);
    b.push(3); // stage: complete
    b.extend_from_slice(&[1, 2, 3]);
    let ev = usbpcap::decode(&frame(b)).unwrap().unwrap();
    assert_eq!(ev.stage, UrbStage::Complete);
    assert_eq!(ev.direction, Direction::In);
    assert_eq!(ev.setup, None);
    assert_eq!((ev.data_len, ev.data), (3, vec![1, 2, 3]));
}

#[test]
fn interrupt_in_and_negative_status() {
    let b = [usbpcap_header(27, 1, 0xC000_0004, 0x01, 2, 9, 0x83, 1, 4), vec![9, 8, 7, 6]].concat();
    let ev = usbpcap::decode(&frame(b)).unwrap().unwrap();
    assert_eq!((ev.bus, ev.device, ev.endpoint), (2, 9, 3));
    assert_eq!(ev.transfer, TransferType::Interrupt);
    assert_eq!(ev.status, 0xC000_0004u32 as i32);
    assert_eq!(ev.data, vec![9, 8, 7, 6]);
}

#[test]
fn isochronous_skips_iso_packet_table() {
    let mut b = usbpcap_header(27 + 12 + 12, 1, 0, 0x00, 1, 5, 0x01, 0, 4);
    b.extend_from_slice(&[0; 12]); // startFrame, numberOfPackets, errorCount
    b.extend_from_slice(&[0; 12]); // one USBPCAP_BUFFER_ISO_PACKET
    b.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
    let ev = usbpcap::decode(&frame(b)).unwrap().unwrap();
    assert_eq!(ev.transfer, TransferType::Isochronous);
    assert_eq!(ev.data, vec![0x11, 0x22, 0x33, 0x44]);
}

#[test]
fn truncated_payload_is_flagged() {
    let b = [usbpcap_header(27, 1, 0, 0x00, 1, 5, 0x02, 3, 512), vec![1, 2]].concat();
    let ev = usbpcap::decode(&frame(b)).unwrap().unwrap();
    assert_eq!((ev.data_len, ev.data.len(), ev.payload_dropped), (512, 2, true));
}

#[test]
fn irp_info_is_not_an_event_and_bad_lengths_are_errors() {
    let b = usbpcap_header(27, 1, 0, 0, 1, 5, 0, 0xFE, 0);
    assert_eq!(usbpcap::decode(&frame(b)).unwrap(), None);
    let b = usbpcap_header(60, 1, 0, 0, 1, 5, 0, 1, 0);
    assert_eq!(usbpcap::decode(&frame(b)), Err(DecodeError::BadHeaderLen { declared: 60, frame: 27 }));
    assert_eq!(usbpcap::decode(&frame(vec![0; 10])), Err(DecodeError::Truncated { needed: 27, got: 10 }));
}

#[test]
fn excess_payload_is_capped_to_data_len() {
    let b = [usbpcap_header(27, 1, 0, 0x00, 1, 5, 0x02, 1, 4), vec![1, 2, 3, 4, 5, 6]].concat();
    let ev = usbpcap::decode(&frame(b)).unwrap().unwrap();
    assert_eq!((ev.data_len, ev.data.len(), ev.payload_dropped), (4, 4, false));
    assert_eq!(ev.data, vec![1, 2, 3, 4]);
}

#[test]
fn control_header_too_short_is_an_error() {
    // header_len 27 is a valid base header, but the control minimum is 28 (base + stage byte).
    let b = usbpcap_header(27, 1, 0, 0, 1, 5, 0, 2, 0);
    assert_eq!(usbpcap::decode(&frame(b)), Err(DecodeError::BadHeaderLen { declared: 27, frame: 27 }));
}

#[test]
fn dispatch_rejects_unknown_link_types() {
    let mut f = frame(vec![0; 64]);
    f.link_type = 1;
    assert_eq!(decode::decode(&f), Err(DecodeError::UnsupportedLinkType(1)));
}

#[test]
fn encode_round_trips_every_transfer_type() {
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
        UsbEvent { transfer: TransferType::Bulk, endpoint: 2, setup: None, data_len: 3, data: vec![7, 7, 7], ..base.clone() },
        UsbEvent { transfer: TransferType::Isochronous, endpoint: 1, setup: None, data_len: 1, data: vec![9], ..base },
    ];
    for ev in events {
        let back = usbpcap::decode(&frame(usbpcap::encode(&ev))).unwrap().unwrap();
        assert_eq!(UsbEvent { ts_ns: ev.ts_ns, packet_index: ev.packet_index, ..back }, ev);
    }
}
