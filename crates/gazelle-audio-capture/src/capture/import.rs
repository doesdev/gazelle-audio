//! `.pcap` / `.pcapng` reading for import and for live tools that stream pcap on stdout.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use pcap_file::pcap::PcapReader;
use pcap_file::pcapng::blocks::interface_description::InterfaceDescriptionOption;
use pcap_file::pcapng::{Block, PcapNgReader};
use pcap_file::Endianness;

use super::decode::ByteOrder;
use super::{CaptureError, CaptureSource, CaptureStream, FrameIter, RawFrame, StopHandle};

const PCAPNG_MAGIC: [u8; 4] = [0x0A, 0x0D, 0x0D, 0x0A];

fn byte_order(e: Endianness) -> ByteOrder {
    match e {
        Endianness::Little => ByteOrder::Little,
        Endianness::Big => ByteOrder::Big,
    }
}

fn format_err(e: pcap_file::PcapError) -> CaptureError {
    CaptureError::Format(e.to_string())
}

/// Converts a pcapng EPB timestamp to nanoseconds.
///
/// pcap-file 2.0.0 returns the raw 64-bit EPB timestamp as `Duration::from_nanos(raw)` whatever
/// the interface's `if_tsresol`, so `raw` is in interface units: `10^-v` s when bit 7 of `v` is
/// clear, `2^-v` s when set. Absent `if_tsresol` means 6 (microseconds).
///
/// `tsresol` comes straight from the capture file, so a hostile or corrupt value must be
/// reported, not panic the import. The intermediate scaling (`raw * 10^k` or `raw * 1e9`) is
/// done in `u128`, which has enough headroom that it cannot overflow for any `u64` `raw` and
/// any 7-bit exponent; only the exponent-to-scale step (decimal `10^k` for a large `k`) and the
/// final narrowing back to `u64` can fail, and both are checked. Fix round 1: an earlier version
/// multiplied by `1_000_000_000` in `u64` *before* shifting, which overflowed for realistic
/// epoch-scale timestamps at ordinary binary exponents (4, 10, 20, 30, ...) even though the
/// true nanosecond value fit easily in a `u64`. Computing in `u128` and narrowing only once,
/// at the end, avoids that.
pub fn epb_units_to_ns(raw: u64, tsresol: u8) -> Result<u64, CaptureError> {
    let bad = || CaptureError::Format(format!("if_tsresol {tsresol} out of range for raw timestamp {raw}"));
    let exp = (tsresol & 0x7F) as u32;
    let ns: u128 = if tsresol & 0x80 == 0 {
        if exp <= 9 {
            let scale = 10u128.checked_pow(9 - exp).ok_or_else(bad)?;
            (raw as u128).checked_mul(scale).ok_or_else(bad)?
        } else {
            let scale = 10u128.checked_pow(exp - 9).ok_or_else(bad)?;
            (raw as u128).checked_div(scale).ok_or_else(bad)?
        }
    } else {
        let scaled = (raw as u128).checked_mul(1_000_000_000).ok_or_else(bad)?;
        scaled.checked_shr(exp).ok_or_else(bad)?
    };
    u64::try_from(ns).map_err(|_| bad())
}

/// Frames from a classic pcap stream (file or a tool's stdout).
pub fn pcap_frames<R: Read + Send + 'static>(reader: R) -> Result<FrameIter, CaptureError> {
    let mut reader = PcapReader::new(reader).map_err(format_err)?;
    let header = reader.header();
    let link_type: u32 = header.datalink.into();
    let order = byte_order(header.endianness);
    let mut index = 0u64;
    Ok(Box::new(std::iter::from_fn(move || {
        let packet = match reader.next_packet()? {
            Ok(p) => p,
            Err(e) => return Some(Err(format_err(e))),
        };
        let frame = RawFrame {
            ts_ns: packet.timestamp.as_nanos() as u64,
            link_type,
            index,
            orig_len: packet.orig_len,
            byte_order: order,
            data: packet.data.into_owned(),
        };
        index += 1;
        Some(Ok(frame))
    })))
}

/// Frames from a pcapng stream; Enhanced Packet Blocks only, timestamps honour `if_tsresol`.
pub fn pcapng_frames<R: Read + Send + 'static>(reader: R) -> Result<FrameIter, CaptureError> {
    let mut reader = PcapNgReader::new(reader).map_err(format_err)?;
    let mut index = 0u64;
    Ok(Box::new(std::iter::from_fn(move || loop {
        let (interface_id, raw_ts, orig_len, data) = match reader.next_block()? {
            Err(e) => return Some(Err(format_err(e))),
            Ok(Block::EnhancedPacket(epb)) => (
                epb.interface_id,
                epb.timestamp.as_nanos() as u64,
                epb.original_len,
                epb.data.into_owned(),
            ),
            Ok(_) => continue,
        };
        let Some(idb) = reader.interfaces().get(interface_id as usize) else {
            return Some(Err(CaptureError::Format(format!(
                "packet references unknown interface {interface_id}"
            ))));
        };
        let tsresol = idb
            .options
            .iter()
            .find_map(|o| match o {
                InterfaceDescriptionOption::IfTsResol(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(6);
        let ts_ns = match epb_units_to_ns(raw_ts, tsresol) {
            Ok(ns) => ns,
            Err(e) => return Some(Err(e)),
        };
        let frame = RawFrame {
            ts_ns,
            link_type: idb.linktype.into(),
            index,
            orig_len,
            byte_order: byte_order(reader.section().endianness),
            data,
        };
        index += 1;
        return Some(Ok(frame));
    })))
}

/// Opens a capture file, choosing pcap or pcapng by magic number.
pub fn open_frames(path: &Path) -> Result<FrameIter, CaptureError> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;
    let reader = BufReader::new(file);
    if magic == PCAPNG_MAGIC {
        pcapng_frames(reader)
    } else {
        pcap_frames(reader)
    }
}

/// [`CaptureSource`] over frames already in memory (tests, demo sessions).
pub struct MemorySource {
    label: String,
    frames: Vec<RawFrame>,
}

impl MemorySource {
    pub fn new(label: impl Into<String>, frames: Vec<RawFrame>) -> Self {
        Self { label: label.into(), frames }
    }
}

impl CaptureSource for MemorySource {
    fn describe(&self) -> String {
        self.label.clone()
    }

    fn start(&mut self) -> Result<CaptureStream, CaptureError> {
        let frames = self.frames.clone().into_iter().map(Ok);
        Ok(CaptureStream { frames: Box::new(frames), stop: StopHandle::noop() })
    }
}

/// [`CaptureSource`] over a `.pcap` / `.pcapng` file.
pub struct ImportSource {
    path: PathBuf,
}

impl ImportSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl CaptureSource for ImportSource {
    fn describe(&self) -> String {
        format!("import {}", self.path.display())
    }

    fn start(&mut self) -> Result<CaptureStream, CaptureError> {
        Ok(CaptureStream { frames: open_frames(&self.path)?, stop: StopHandle::noop() })
    }
}
