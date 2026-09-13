//! Writes the per-probe `.pcapng` (and classic `.pcap` fixtures).

use std::borrow::Cow;
use std::io::Write;
use std::time::Duration;

use pcap_file::pcap::{PcapHeader, PcapPacket, PcapWriter};
use pcap_file::pcapng::blocks::enhanced_packet::EnhancedPacketBlock;
use pcap_file::pcapng::blocks::interface_description::{InterfaceDescriptionBlock, InterfaceDescriptionOption};
use pcap_file::pcapng::PcapNgWriter;
use pcap_file::{DataLink, Endianness, TsResolution};

use super::decode::usbmon::LINKTYPE_USB_LINUX_MMAPPED;
use super::decode::ByteOrder;
use super::{CaptureError, RawFrame};

fn format_err(e: pcap_file::PcapError) -> CaptureError {
    CaptureError::Format(e.to_string())
}

/// Little-endian pcapng with one interface per link type, nanosecond `if_tsresol`.
pub struct CaptureWriter<W: Write> {
    inner: PcapNgWriter<W>,
    interfaces: Vec<u32>,
    written: u64,
}

impl<W: Write> CaptureWriter<W> {
    pub fn new(writer: W) -> Result<Self, CaptureError> {
        let inner = PcapNgWriter::with_endianness(writer, Endianness::Little).map_err(format_err)?;
        Ok(Self { inner, interfaces: Vec::new(), written: 0 })
    }

    /// Appends a frame; returns its zero-based index in the file.
    pub fn write(&mut self, frame: &RawFrame) -> Result<u64, CaptureError> {
        if frame.link_type == LINKTYPE_USB_LINUX_MMAPPED && frame.byte_order == ByteOrder::Big {
            return Err(CaptureError::Format("big-endian usbmon frames cannot be stored little-endian".into()));
        }
        let interface_id = match self.interfaces.iter().position(|l| *l == frame.link_type) {
            Some(i) => i as u32,
            None => {
                let mut idb = InterfaceDescriptionBlock::new(DataLink::from(frame.link_type), 0);
                idb.options.push(InterfaceDescriptionOption::IfTsResol(9));
                self.inner.write_pcapng_block(idb).map_err(format_err)?;
                self.interfaces.push(frame.link_type);
                (self.interfaces.len() - 1) as u32
            }
        };
        let epb = EnhancedPacketBlock {
            interface_id,
            timestamp: Duration::from_nanos(frame.ts_ns),
            original_len: frame.orig_len,
            data: Cow::Borrowed(&frame.data),
            options: Vec::new(),
        };
        self.inner.write_pcapng_block(epb).map_err(format_err)?;
        self.written += 1;
        Ok(self.written - 1)
    }

    pub fn finish(self) -> Result<W, CaptureError> {
        let mut w = self.inner.into_inner();
        w.flush()?;
        Ok(w)
    }
}

/// Writes frames of one link type as classic microsecond pcap, as `USBPcapCMD` produces.
pub fn write_pcap<W: Write>(writer: W, link_type: u32, order: ByteOrder, frames: &[RawFrame]) -> Result<W, CaptureError> {
    let header = PcapHeader {
        datalink: DataLink::from(link_type),
        endianness: match order {
            ByteOrder::Little => Endianness::Little,
            ByteOrder::Big => Endianness::Big,
        },
        ts_resolution: TsResolution::MicroSecond,
        snaplen: 65535,
        ..PcapHeader::default()
    };
    let mut w = PcapWriter::with_header(writer, header).map_err(format_err)?;
    for f in frames {
        let packet = PcapPacket::new(Duration::from_nanos(f.ts_ns), f.orig_len, &f.data);
        w.write_packet(&packet).map_err(format_err)?;
    }
    Ok(w.into_writer())
}
