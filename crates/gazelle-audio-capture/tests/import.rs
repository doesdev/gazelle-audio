use std::borrow::Cow;
use std::io::Cursor;
use std::time::Duration;

use gazelle_audio_capture::capture::decode::{usbmon, usbpcap, ByteOrder};
use gazelle_audio_capture::capture::event::{Direction, TransferType, UrbStage, UsbEvent};
use gazelle_audio_capture::capture::import::{epb_units_to_ns, open_frames, pcap_frames, pcapng_frames, ImportSource};
use gazelle_audio_capture::capture::writer::{write_pcap, CaptureWriter};
use gazelle_audio_capture::capture::{CaptureSource, RawFrame};
use pcap_file::pcapng::blocks::enhanced_packet::EnhancedPacketBlock;
use pcap_file::pcapng::blocks::interface_description::InterfaceDescriptionBlock;
use pcap_file::pcapng::PcapNgWriter;
use pcap_file::DataLink;

fn event(i: u64) -> UsbEvent {
    UsbEvent {
        ts_ns: 1_700_000_000_000_000_000 + i * 1_234_567,
        packet_index: i,
        bus: 1,
        device: 5,
        endpoint: 1,
        direction: Direction::In,
        transfer: TransferType::Interrupt,
        stage: UrbStage::Complete,
        urb_id: 100 + i,
        setup: None,
        status: 0,
        data_len: 2,
        data: vec![i as u8, 0x55],
        payload_dropped: false,
    }
}

fn frame(link_type: u32, i: u64, data: Vec<u8>) -> RawFrame {
    RawFrame { ts_ns: event(i).ts_ns, link_type, index: i, orig_len: data.len() as u32, byte_order: ByteOrder::Little, data }
}

#[test]
fn pcapng_round_trips_frames_with_nanosecond_timestamps() {
    let frames: Vec<RawFrame> = (0..3).map(|i| frame(249, i, usbpcap::encode(&event(i)))).collect();
    let mut w = CaptureWriter::new(Vec::new()).unwrap();
    for f in &frames {
        assert_eq!(w.write(f).unwrap(), f.index);
    }
    let bytes = w.finish().unwrap();
    let back: Vec<RawFrame> = pcapng_frames(Cursor::new(bytes)).unwrap().map(Result::unwrap).collect();
    assert_eq!(back, frames);
}

#[test]
fn pcapng_mixed_link_types_get_separate_interfaces() {
    let a = frame(249, 0, usbpcap::encode(&event(0)));
    let b = frame(220, 1, usbmon::encode(&event(1)));
    let mut w = CaptureWriter::new(Vec::new()).unwrap();
    w.write(&a).unwrap();
    w.write(&b).unwrap();
    let back: Vec<RawFrame> = pcapng_frames(Cursor::new(w.finish().unwrap())).unwrap().map(Result::unwrap).collect();
    assert_eq!(back.iter().map(|f| f.link_type).collect::<Vec<_>>(), vec![249, 220]);
}

#[test]
fn microsecond_pcapng_as_written_by_wireshark_is_rescaled() {
    // An interface without if_tsresol stores microseconds. pcap-file 2.0.0 hands the raw
    // value back as nanoseconds, so write the raw microsecond count the same way.
    let mut w = PcapNgWriter::new(Vec::new()).unwrap();
    w.write_pcapng_block(InterfaceDescriptionBlock::new(DataLink::USBPCAP, 0)).unwrap();
    let data = usbpcap::encode(&event(0));
    w.write_pcapng_block(EnhancedPacketBlock {
        interface_id: 0,
        timestamp: Duration::from_nanos(1_700_000_000_123_456),
        original_len: data.len() as u32,
        data: Cow::Borrowed(&data),
        options: vec![],
    })
    .unwrap();
    let f = pcapng_frames(Cursor::new(w.into_inner())).unwrap().next().unwrap().unwrap();
    assert_eq!(f.ts_ns, 1_700_000_000_123_456_000);
}

#[test]
fn tsresol_conversion_table() {
    assert_eq!(epb_units_to_ns(5, 6).unwrap(), 5_000);
    assert_eq!(epb_units_to_ns(5, 9).unwrap(), 5);
    assert_eq!(epb_units_to_ns(5_000, 12).unwrap(), 5);
    assert_eq!(epb_units_to_ns(1 << 10, 0x80 | 10).unwrap(), 1_000_000_000);
}

#[test]
fn tsresol_conversion_overflow_is_reported_not_panicked() {
    // Decimal exponent 48+ (10^39 scaling) overflows u64 arithmetic.
    assert!(epb_units_to_ns(5, 48).is_err());
    assert!(epb_units_to_ns(u64::MAX, 63).is_err());
    // Binary exponent 64 or more can't be a valid right-shift amount for a 64-bit value.
    assert!(epb_units_to_ns(5, 0x80 | 64).is_err());
    assert!(epb_units_to_ns(5, 0xFF).is_err());
}

#[test]
fn classic_pcap_little_and_big_endian() {
    let frames: Vec<RawFrame> = (0..2).map(|i| frame(220, i, usbmon::encode(&event(i)))).collect();
    let le = write_pcap(Vec::new(), 220, ByteOrder::Little, &frames).unwrap();
    let back: Vec<RawFrame> = pcap_frames(Cursor::new(le)).unwrap().map(Result::unwrap).collect();
    // Classic pcap here is microsecond resolution.
    assert_eq!(back[1].ts_ns, frames[1].ts_ns / 1_000 * 1_000);
    assert_eq!(back[1].data, frames[1].data);
    assert_eq!(back[1].byte_order, ByteOrder::Little);
    let be = write_pcap(Vec::new(), 220, ByteOrder::Big, &frames).unwrap();
    let f = pcap_frames(Cursor::new(be)).unwrap().next().unwrap().unwrap();
    assert_eq!(f.byte_order, ByteOrder::Big);
    assert_eq!(f.link_type, 220);
}

#[test]
fn import_source_detects_format_by_magic() {
    let dir = tempfile::tempdir().unwrap();
    let frames: Vec<RawFrame> = (0..4).map(|i| frame(249, i, usbpcap::encode(&event(i)))).collect();
    let ng = dir.path().join("a.pcapng");
    let mut w = CaptureWriter::new(std::fs::File::create(&ng).unwrap()).unwrap();
    for f in &frames {
        w.write(f).unwrap();
    }
    w.finish().unwrap();
    let pcap = dir.path().join("b.pcap");
    write_pcap(std::fs::File::create(&pcap).unwrap(), 249, ByteOrder::Little, &frames).unwrap();
    assert_eq!(open_frames(&ng).unwrap().count(), 4);
    assert_eq!(open_frames(&pcap).unwrap().count(), 4);
    let mut src = ImportSource::new(&ng);
    assert!(src.describe().ends_with("a.pcapng"));
    assert_eq!(src.start().unwrap().frames.count(), 4);
}

#[test]
fn big_endian_usbmon_frames_cannot_be_stored() {
    let mut f = frame(220, 0, usbmon::encode(&event(0)));
    f.byte_order = ByteOrder::Big;
    let mut w = CaptureWriter::new(Vec::new()).unwrap();
    assert!(w.write(&f).is_err());
}

#[test]
fn pcapng_unknown_interface_id_does_not_panic() {
    // A hostile pcapng that references an interface id which was never declared. pcap-file's
    // own `PcapNgWriter` refuses to write this (`InvalidInterfaceId`), so the bytes are built
    // by hand here to exercise the reader's own guard against a malicious/corrupt file.
    fn block(block_type: u32, body: &[u8]) -> Vec<u8> {
        let total_len = (4 + 4 + body.len() + 4) as u32;
        let mut out = Vec::with_capacity(total_len as usize);
        out.extend_from_slice(&block_type.to_le_bytes());
        out.extend_from_slice(&total_len.to_le_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(&total_len.to_le_bytes());
        out
    }

    let mut bytes = Vec::new();
    // Section Header Block: byte-order magic + version 1.0 + section_length -1, no options.
    let mut shb_body = Vec::new();
    shb_body.extend_from_slice(&0x1A2B_3C4Du32.to_le_bytes());
    shb_body.extend_from_slice(&1u16.to_le_bytes());
    shb_body.extend_from_slice(&0u16.to_le_bytes());
    shb_body.extend_from_slice(&(-1i64).to_le_bytes());
    bytes.extend(block(0x0A0D_0D0A, &shb_body));

    // Interface Description Block: only interface 0 is declared.
    let mut idb_body = Vec::new();
    idb_body.extend_from_slice(&249u16.to_le_bytes()); // LINKTYPE_USBPCAP
    idb_body.extend_from_slice(&0u16.to_le_bytes()); // reserved
    idb_body.extend_from_slice(&0u32.to_le_bytes()); // snaplen
    bytes.extend(block(1, &idb_body));

    // Enhanced Packet Block referencing interface 7, which was never declared.
    let data = [1u8, 2, 3, 4];
    let mut epb_body = Vec::new();
    epb_body.extend_from_slice(&7u32.to_le_bytes()); // interface_id
    epb_body.extend_from_slice(&0u32.to_le_bytes()); // timestamp high
    epb_body.extend_from_slice(&0u32.to_le_bytes()); // timestamp low
    epb_body.extend_from_slice(&(data.len() as u32).to_le_bytes()); // captured_len
    epb_body.extend_from_slice(&(data.len() as u32).to_le_bytes()); // original_len
    epb_body.extend_from_slice(&data);
    bytes.extend(block(6, &epb_body));

    let mut it = pcapng_frames(Cursor::new(bytes)).unwrap();
    assert!(it.next().unwrap().is_err());
}

#[test]
fn truncated_pcap_header_is_a_format_error_not_a_panic() {
    let bytes = vec![0xD4, 0xC3, 0xB2, 0xA1, 0x02, 0x00];
    assert!(pcap_frames(Cursor::new(bytes)).is_err());
}

#[test]
fn truncated_pcapng_header_is_a_format_error_not_a_panic() {
    let bytes = vec![0x0A, 0x0D, 0x0D, 0x0A, 0x01, 0x02];
    assert!(pcapng_frames(Cursor::new(bytes)).is_err());
}

#[test]
fn open_frames_on_empty_file_is_a_format_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.pcap");
    std::fs::File::create(&path).unwrap();
    assert!(open_frames(&path).is_err());
}

#[test]
fn open_frames_missing_file_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.pcap");
    assert!(matches!(open_frames(&path), Err(gazelle_audio_capture::capture::CaptureError::Io(_))));
}
