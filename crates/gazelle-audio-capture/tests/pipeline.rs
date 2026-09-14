use gazelle_audio_capture::capture::decode::{usbpcap, ByteOrder, DecodeError};
use gazelle_audio_capture::capture::event::{Direction, SetupPacket, TransferType, UrbStage, UsbEvent};
use gazelle_audio_capture::capture::pipeline::{DeviceFilter, DeviceMap, PayloadPolicy, Pipeline};
use gazelle_audio_capture::capture::rate::RateMeter;
use gazelle_audio_capture::capture::RawFrame;

fn ev(device: u16, transfer: TransferType, stage: UrbStage, direction: Direction, setup: Option<SetupPacket>, data: Vec<u8>) -> UsbEvent {
    UsbEvent {
        ts_ns: 1_000,
        packet_index: 0,
        bus: 1,
        device,
        endpoint: if transfer == TransferType::Control { 0 } else { 2 },
        direction,
        transfer,
        stage,
        urb_id: 0,
        setup,
        status: 0,
        data_len: data.len() as u32,
        data,
        payload_dropped: false,
    }
}

fn raw(e: &UsbEvent) -> RawFrame {
    let data = usbpcap::encode(e);
    RawFrame { ts_ns: e.ts_ns, link_type: 249, index: 0, orig_len: data.len() as u32, byte_order: ByteOrder::Little, data }
}

const GET_DEVICE_DESCRIPTOR: SetupPacket = SetupPacket { request_type: 0x80, request: 6, value: 0x0100, index: 0, length: 18 };

fn device_descriptor(vid: u16, pid: u16) -> Vec<u8> {
    let v = vid.to_le_bytes();
    let p = pid.to_le_bytes();
    vec![18, 1, 0x00, 0x02, 0xEF, 0x02, 0x01, 64, v[0], v[1], p[0], p[1], 0x00, 0x01, 1, 2, 3, 1]
}

/// Injected descriptors for devices 3 (other) and 5 (target), then traffic from both.
fn hub_stream() -> Vec<UsbEvent> {
    use Direction::*;
    use TransferType::*;
    use UrbStage::*;
    vec![
        ev(3, Control, Submit, In, Some(GET_DEVICE_DESCRIPTOR), vec![]),
        ev(3, Control, Complete, In, None, device_descriptor(0x046D, 0xC52B)),
        ev(5, Control, Submit, In, Some(GET_DEVICE_DESCRIPTOR), vec![]),
        ev(5, Control, Complete, In, None, device_descriptor(0x1234, 0xABCD)),
        ev(3, Interrupt, Complete, In, None, vec![1, 2, 3]),
        ev(5, Interrupt, Complete, In, None, vec![4, 5, 6]),
        ev(5, Bulk, Submit, Out, None, vec![9; 64]),
        ev(5, Isochronous, Complete, In, None, vec![8; 32]),
    ]
}

fn run(filter: DeviceFilter, policy: PayloadPolicy) -> Vec<(RawFrame, UsbEvent)> {
    let mut p = Pipeline::new(filter, policy);
    hub_stream().iter().flat_map(|e| p.process(raw(e)).unwrap()).collect()
}

#[test]
fn device_map_learns_vid_pid_from_descriptor_exchange() {
    let mut map = DeviceMap::default();
    for e in hub_stream() {
        map.observe(&e);
    }
    assert_eq!(map.get(1, 5).map(|d| (d.vid, d.pid)), Some((0x1234, 0xABCD)));
    assert_eq!(map.get(1, 5).unwrap().descriptor.len(), 18);
    assert_eq!(map.find(0x046D, 0xC52B), Some((1, 3)));
    assert_eq!(map.find(0xFFFF, 0xFFFF), None);
}

#[test]
fn target_filter_keeps_only_the_target_including_its_descriptor_request() {
    let out = run(DeviceFilter::Target { vid: 0x1234, pid: 0xABCD }, PayloadPolicy::default());
    let devices: Vec<(u16, TransferType, UrbStage)> = out.iter().map(|(_, e)| (e.device, e.transfer, e.stage)).collect();
    assert_eq!(
        devices,
        vec![
            (5, TransferType::Control, UrbStage::Submit),
            (5, TransferType::Control, UrbStage::Complete),
            (5, TransferType::Interrupt, UrbStage::Complete),
            (5, TransferType::Bulk, UrbStage::Submit),
            (5, TransferType::Isochronous, UrbStage::Complete),
        ]
    );
}

#[test]
fn address_and_all_filters() {
    assert_eq!(run(DeviceFilter::Address { bus: 1, device: 3 }, PayloadPolicy::default()).len(), 3);
    assert_eq!(run(DeviceFilter::All, PayloadPolicy::default()).len(), 8);
}

#[test]
fn stream_payloads_are_dropped_by_default_and_the_frame_truncated() {
    let out = run(DeviceFilter::Address { bus: 1, device: 5 }, PayloadPolicy::default());
    let (bulk_frame, bulk) = &out[3];
    assert_eq!(bulk.transfer, TransferType::Bulk);
    assert_eq!((bulk.data_len, bulk.data.len(), bulk.payload_dropped), (64, 0, true));
    assert_eq!(bulk_frame.data.len(), usbpcap::BASE_HEADER_LEN);
    assert_eq!(bulk_frame.orig_len as usize, usbpcap::BASE_HEADER_LEN + 64);
    let reread = usbpcap::decode(bulk_frame).unwrap().unwrap();
    assert_eq!(&reread, bulk, "stored frame decodes to the emitted event");
    let (_, iso) = &out[4];
    assert!(iso.payload_dropped && iso.data.is_empty());
    let (_, interrupt) = &out[2];
    assert_eq!(interrupt.data, vec![4, 5, 6], "interrupt payloads are kept");
}

#[test]
fn session_flag_keeps_stream_payloads() {
    let out = run(DeviceFilter::Address { bus: 1, device: 5 }, PayloadPolicy { keep_stream_payloads: true });
    assert_eq!(out[3].1.data, vec![9; 64]);
    assert!(!out[3].1.payload_dropped);
}

#[test]
fn short_descriptor_completion_does_not_panic_and_learns_nothing() {
    let mut map = DeviceMap::default();
    let submit = ev(5, TransferType::Control, UrbStage::Submit, Direction::In, Some(GET_DEVICE_DESCRIPTOR), vec![]);
    map.observe(&submit);
    // A truncated completion: right descriptor type byte, but far fewer than the 12 bytes
    // needed to reach the VID/PID fields.
    let short_complete = ev(5, TransferType::Control, UrbStage::Complete, Direction::In, None, vec![18, 1, 0, 0, 0]);
    map.observe(&short_complete);
    assert_eq!(map.get(1, 5), None, "a short completion must not be learned as a device");
}

#[test]
fn descriptor_completion_without_a_held_submit_learns_nothing() {
    let mut map = DeviceMap::default();
    // No prior GET_DESCRIPTOR submit was observed for device 5.
    let complete = ev(5, TransferType::Control, UrbStage::Complete, Direction::In, None, device_descriptor(0x1234, 0xABCD));
    map.observe(&complete);
    assert_eq!(map.get(1, 5), None, "a completion with no matching held submit must not be learned");
}

#[test]
fn process_propagates_a_decode_error_for_a_malformed_frame() {
    // Header length (u16 at offset 0) declared far larger than the frame itself.
    let mut data = vec![0u8; usbpcap::BASE_HEADER_LEN];
    data[0..2].copy_from_slice(&5000u16.to_le_bytes());
    let frame = RawFrame { ts_ns: 0, link_type: 249, index: 0, orig_len: data.len() as u32, byte_order: ByteOrder::Little, data };
    let mut p = Pipeline::new(DeviceFilter::All, PayloadPolicy::default());
    let result = p.process(frame);
    match result {
        Err(DecodeError::BadHeaderLen { declared, frame }) => {
            assert_eq!((declared, frame), (5000, usbpcap::BASE_HEADER_LEN));
        }
        other => panic!("expected Err(DecodeError::BadHeaderLen {{ .. }}), got {other:?}"),
    }
}

#[test]
fn rate_meter_counts_a_sliding_second() {
    let mut m = RateMeter::new(1_000_000_000);
    for i in 0..50 {
        m.record(i * 20_000_000);
    }
    assert_eq!(m.per_second(980_000_000), 50.0);
    assert_eq!(m.per_second(1_490_000_000), 25.0, "records at 500..=980 ms remain");
    assert_eq!(m.per_second(5_000_000_000), 0.0);
}

#[test]
fn rate_meter_survives_a_clock_step_backwards() {
    let mut m = RateMeter::new(1_000_000_000);
    m.record(10_000_000_000);
    // The wall clock stepped back 5 s; the old record must still age out on time, rather than
    // sitting at the front and blocking eviction of everything recorded after it.
    for i in 0..10 {
        m.record(5_000_000_000 + i * 10_000_000);
    }
    assert_eq!(m.per_second(5_100_000_000), 10.0, "the future-dated record is outside (now - 1 s, now]");
    assert_eq!(m.per_second(6_200_000_000), 0.0);
    assert_eq!(m.per_second(10_500_000_000), 1.0, "the future-dated record counts once its time comes");
}
