//! Link type 220: Linux usbmon `struct usbmon_packet`, 64-byte memory-mapped variant.
//!
//! Offsets: id u64 @0, type u8 @8 ('S'/'C'/'E'), xfer_type u8 @9, epnum u8 @10, devnum u8 @11,
//! busnum u16 @12, flag_setup i8 @14 (0 = setup present), flag_data i8 @15, ts_sec i64 @16,
//! ts_usec i32 @24, status i32 @28, length u32 @32, len_cap u32 @36, setup/iso [8] @40,
//! interval i32 @48, start_frame i32 @52, xfer_flags u32 @56, ndesc u32 @60.
//! ISO transfers carry `ndesc` 16-byte descriptors before the data.
//! Fields are in the capturing host's byte order, which the file's byte order records.
//!
//! `length` (@32) is the full transfer size the kernel reports; `len_cap` (@36) is how much
//! of it usbmon actually captured into this record. `data_len` is taken from `length`.
//! `data` never holds more than `min(len_cap, length)` bytes, and is further capped to
//! whatever is physically present in the frame so decoding never reads past its end.
//! `payload_dropped` is `data.len() < data_len`, matching [`UsbEvent::payload_dropped`]'s
//! contract and USBPcap's use of its own declared length. This holds even for a
//! doubly-corrupted frame where `len_cap`/`length` both overstate what is present.

use super::{ByteOrder, DecodeError};
use crate::capture::event::{Direction, SetupPacket, TransferType, UrbStage, UsbEvent};
use crate::capture::RawFrame;

pub const LINKTYPE_USB_LINUX_MMAPPED: u32 = 220;
pub const HEADER_LEN: usize = 64;
pub const ISO_DESCRIPTOR_LEN: usize = 16;

fn u16_at(b: &[u8], at: usize, order: ByteOrder) -> u16 {
    let x = [b[at], b[at + 1]];
    match order {
        ByteOrder::Little => u16::from_le_bytes(x),
        ByteOrder::Big => u16::from_be_bytes(x),
    }
}

fn u32_at(b: &[u8], at: usize, order: ByteOrder) -> u32 {
    let x: [u8; 4] = b[at..at + 4].try_into().expect("4 bytes");
    match order {
        ByteOrder::Little => u32::from_le_bytes(x),
        ByteOrder::Big => u32::from_be_bytes(x),
    }
}

fn u64_at(b: &[u8], at: usize, order: ByteOrder) -> u64 {
    let x: [u8; 8] = b[at..at + 8].try_into().expect("8 bytes");
    match order {
        ByteOrder::Little => u64::from_le_bytes(x),
        ByteOrder::Big => u64::from_be_bytes(x),
    }
}

/// Offset of the data: 64 plus ISO descriptors when present. Uses u64 arithmetic so a
/// hostile `ndesc` (up to `u32::MAX`) cannot overflow `usize` on any target.
pub fn header_len(data: &[u8], order: ByteOrder) -> Result<usize, DecodeError> {
    if data.len() < HEADER_LEN {
        return Err(DecodeError::Truncated { needed: HEADER_LEN, got: data.len() });
    }
    let ndesc = if data[9] == 0 { u32_at(data, 60, order) } else { 0 };
    let len = HEADER_LEN as u64 + ndesc as u64 * ISO_DESCRIPTOR_LEN as u64;
    if len > data.len() as u64 {
        let declared = usize::try_from(len).unwrap_or(usize::MAX);
        return Err(DecodeError::BadHeaderLen { declared, frame: data.len() });
    }
    Ok(len as usize)
}

/// Decodes one frame; `Ok(None)` for unknown transfer or event types.
pub fn decode(frame: &RawFrame) -> Result<Option<UsbEvent>, DecodeError> {
    let b = &frame.data;
    let order = frame.byte_order;
    let hlen = header_len(b, order)?;
    let Some(transfer) = TransferType::from_wire(b[9]) else {
        return Ok(None);
    };
    let stage = match b[8] {
        b'S' => UrbStage::Submit,
        b'C' | b'E' => UrbStage::Complete,
        _ => return Ok(None),
    };
    let setup = if transfer == TransferType::Control && b[14] == 0 {
        SetupPacket::parse(&b[40..48])
    } else {
        None
    };
    let length = u32_at(b, 32, order);
    let len_cap = u32_at(b, 36, order);
    let payload = &b[hlen..];
    let captured = (payload.len() as u64).min(len_cap as u64).min(length as u64) as usize;
    Ok(Some(UsbEvent {
        ts_ns: frame.ts_ns,
        packet_index: frame.index,
        bus: u16_at(b, 12, order),
        device: b[11] as u16,
        endpoint: b[10] & 0x7F,
        direction: if b[10] & 0x80 != 0 { Direction::In } else { Direction::Out },
        transfer,
        stage,
        urb_id: u64_at(b, 0, order),
        setup,
        status: u32_at(b, 28, order) as i32,
        data_len: length,
        data: payload[..captured].to_vec(),
        // Fix round 1: must reflect what was actually decoded into `data`, not just
        // len_cap vs length: a doubly-corrupted frame where both declared fields
        // overstate what's physically present must still report a drop.
        payload_dropped: (captured as u64) < length as u64,
    }))
}

/// Encodes an event as a little-endian usbmon frame (synthetic fixtures only).
pub fn encode(ev: &UsbEvent) -> Vec<u8> {
    let mut out = vec![0u8; HEADER_LEN];
    out[0..8].copy_from_slice(&ev.urb_id.to_le_bytes());
    out[8] = if ev.stage == UrbStage::Submit { b'S' } else { b'C' };
    out[9] = ev.transfer.to_wire();
    out[10] = ev.endpoint & 0x7F | if ev.direction == Direction::In { 0x80 } else { 0 };
    out[11] = ev.device as u8;
    out[12..14].copy_from_slice(&ev.bus.to_le_bytes());
    out[14] = if ev.setup.is_some() { 0 } else { b'-' };
    out[15] = if ev.data.is_empty() { b'<' } else { 0 };
    out[16..24].copy_from_slice(&((ev.ts_ns / 1_000_000_000) as i64).to_le_bytes());
    out[24..28].copy_from_slice(&(((ev.ts_ns % 1_000_000_000) / 1_000) as i32).to_le_bytes());
    out[28..32].copy_from_slice(&ev.status.to_le_bytes());
    out[32..36].copy_from_slice(&ev.data_len.to_le_bytes()); // length
    out[36..40].copy_from_slice(&(ev.data.len() as u32).to_le_bytes()); // len_cap
    if let Some(s) = ev.setup {
        out[40..48].copy_from_slice(&s.to_bytes());
    }
    out.extend_from_slice(&ev.data);
    out
}
