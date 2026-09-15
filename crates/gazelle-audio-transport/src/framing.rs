//! Message framing: segmentation (send) and reassembly (receive).
//!
//! Recovered from `refs/decompiled/manager/antelope_dev_base.py` (`HWDevice._split_in_segments`
//! and `HWDevice._parse_segment`). Large reports are split into 8052-segmented messages
//! for transmission; the device reassembles those 8052 segments on receipt.
//!
//! Each segment carries a 16-byte header identical in layout to a normal report:
//!
//! ```text
//! cmd   : u32  = 8052 (send) / 8053 (receive)
//! seq   : u32  = chunk_len + header_size (bytes carried in this segment)
//! ext2  : u32  = total length of the reassembled message
//! ext3  : u32  = byte offset of this chunk within the reassembled message
//! ```
//!
//! The payload is everything after the header. On the send path the chunk is the next
//! slice of the message; on the receive path segments are concatenated in offset order
//! until the total length is reached.

use gazelle_audio_protocol::wire::{Header, WireError, HEADER_SIZE};

/// Report id used for outgoing segments (the device reassembles these).
pub const SEGMENT_SEND_ID: u32 = 8052;

/// Report id the device uses for its own segments (the host reassembles these).
pub const SEGMENT_RECEIVE_ID: u32 = 8053;

/// A single reassembly fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Total length of the message being reassembled.
    pub total_len: usize,
    /// Byte offset of this fragment within the message.
    pub offset: usize,
    /// The fragment payload (the message bytes for this segment).
    pub data: Vec<u8>,
}

/// The state of reassembling a message from incoming segments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Reassembly {
    /// No reassembly in progress (or a complete message was just produced).
    #[default]
    Idle,
    /// A message is being reassembled; holds the running buffer and target length.
    InProgress {
        /// Bytes collected so far.
        buf: Vec<u8>,
        /// Total length expected (from the first segment's `ext2`).
        total_len: usize,
    },
}

impl Reassembly {
    /// Feed one incoming segment into the reassembler.
    ///
    /// Returns `Ok(Some(msg))` when the final fragment completes the message,
    /// `Ok(None)` when more fragments are needed, and `Err` when the segment
    /// is not a valid 8053 segment.
    pub fn push(&mut self, seg: Segment) -> Result<Option<Vec<u8>>, WireError> {
        if seg.total_len == 0 {
            return Err(WireError::PayloadTooLarge);
        }
        match &mut *self {
            Reassembly::InProgress { buf, total_len } => {
                if *total_len != seg.total_len {
                    // A new message started before the previous one finished;
                    // drop the partial buffer and start fresh.
                    *buf = Vec::new();
                    *total_len = seg.total_len;
                }
                if seg.offset != buf.len() {
                    // Out-of-order or dropped segment. Reset to Idle rather than keeping
                    // the stale partial buffer: leaving it in place meant every later
                    // segment failed this same check forever, silently wedging the
                    // device (LoopbackDevice discards push errors).
                    //
                    // A segment at offset 0 is treated as the start of a new message.
                    let restart = seg.offset == 0;
                    *self = Reassembly::Idle;
                    if restart {
                        return self.push(seg);
                    }
                    return Err(WireError::TruncatedPayload);
                }
                buf.extend_from_slice(&seg.data);
                if buf.len() >= seg.total_len {
                    buf.truncate(seg.total_len);
                    Ok(Some(std::mem::take(buf)))
                } else {
                    Ok(None)
                }
            }
            Reassembly::Idle => {
                if seg.offset != 0 {
                    return Err(WireError::TruncatedPayload);
                }
                let mut buf = seg.data.clone();
                if buf.len() >= seg.total_len {
                    buf.truncate(seg.total_len);
                    Ok(Some(std::mem::take(&mut buf)))
                } else {
                    *self = Reassembly::InProgress {
                        buf,
                        total_len: seg.total_len,
                    };
                    Ok(None)
                }
            }
        }
    }

    /// Whether a reassembly is currently in progress.
    pub fn is_in_progress(&self) -> bool {
        !matches!(self, Reassembly::Idle)
    }
}

/// Split `data` into wire segments that each fit within `max_packet_size` bytes
/// (including the 16-byte header). Yields at least one segment for non-empty input.
///
/// Mirrors `HWDevice._split_in_segments`: each segment's `seq` is the number of
/// bytes it carries (chunk length + header size), `ext2` is the total message
/// length, and `ext3` is the byte offset of the chunk.
pub fn split_in_segments(data: &[u8], max_packet_size: usize) -> Vec<Segment> {
    let mut out = Vec::new();
    if data.is_empty() || max_packet_size <= HEADER_SIZE {
        return out;
    }
    let max_chunk = max_packet_size - HEADER_SIZE;
    let mut offset = 0usize;
    while offset < data.len() {
        let chunk_len = ((data.len() - offset).min(max_chunk)) as u32;
        let start = offset;
        let end = (offset + chunk_len as usize).min(data.len());
        out.push(Segment {
            total_len: data.len(),
            offset,
            data: data[start..end].to_vec(),
        });
        offset += chunk_len as usize;
    }
    out
}

/// Parse a raw 16-byte-header buffer into a [`Segment`] if it is an 8052 segment
/// (host -> device, which the device reassembles).
///
/// Returns `Err(WireError::PayloadTooLarge)` when the buffer is too short to hold
/// the header, and `Err(WireError::FieldOverflow)` when the command id is not
/// [`SEGMENT_SEND_ID`] (the caller should treat it as a non-segmented report).
pub fn parse_send_segment(buf: &[u8]) -> Result<Segment, WireError> {
    parse_segment(buf, SEGMENT_SEND_ID)
}

/// Parse a device-to-host 8053 segment, with the same rules and errors as [`parse_send_segment`].
pub fn parse_receive_segment(buf: &[u8]) -> Result<Segment, WireError> {
    parse_segment(buf, SEGMENT_RECEIVE_ID)
}

fn parse_segment(buf: &[u8], id: u32) -> Result<Segment, WireError> {
    if buf.len() < HEADER_SIZE {
        return Err(WireError::TruncatedHeader);
    }
    let header = Header::from_bytes(buf)?;
    if header.cmd != id {
        return Err(WireError::FieldOverflow);
    }
    // `seq` carries chunk_len + HEADER_SIZE, so anything below HEADER_SIZE is malformed.
    // Subtracting first panicked in debug builds and wrapped in release, which meant the
    // two profiles disagreed on device-controlled input.
    let chunk_len = (header.seq as usize)
        .checked_sub(HEADER_SIZE)
        .ok_or(WireError::TruncatedPayload)?;
    if chunk_len > buf.len() - HEADER_SIZE {
        return Err(WireError::TruncatedPayload);
    }
    Ok(Segment {
        total_len: header.ext2 as usize,
        offset: header.ext3 as usize,
        data: buf[HEADER_SIZE..HEADER_SIZE + chunk_len].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_segment_for_small_message() {
        let data = vec![1u8, 2, 3, 4, 5];
        let segs = split_in_segments(&data, 64);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].data, data);
        assert_eq!(segs[0].offset, 0);
        assert_eq!(segs[0].total_len, 5);
    }

    #[test]
    fn empty_message_yields_no_segments() {
        assert!(split_in_segments(&[], 64).is_empty());
    }

    #[test]
    fn large_message_splits_into_segments() {
        // 30-byte message, 16-byte header => max 16 payload bytes per segment.
        let data = vec![7u8; 30];
        let segs = split_in_segments(&data, 32);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].offset, 0);
        assert_eq!(segs[1].offset, 16);
        assert_eq!(segs[0].data.len(), 16);
        assert_eq!(segs[1].data.len(), 14);
        // seq carries chunk_len + header_size.
        assert_eq!(segs[0].total_len, 30);
        let mut reassembled = Vec::new();
        for s in &segs {
            reassembled.extend_from_slice(&s.data);
        }
        assert_eq!(reassembled, data);
    }

    #[test]
    fn reassembly_across_segments() {
        let data = vec![9u8; 40];
        let segs = split_in_segments(&data, 24); // 8 payload bytes/segment
        assert_eq!(segs.len(), 5);
        let mut reasm = Reassembly::default();
        let mut collected: Option<Vec<u8>> = None;
        for s in segs {
            // Only the final segment completes the message; earlier pushes
            // return Ok(None) and must not overwrite the running buffer.
            collected = reasm.push(s).expect("push");
        }
        assert_eq!(collected, Some(data));
    }

    #[test]
    fn reassembly_is_incremental() {
        let data = vec![3u8; 20];
        let segs = split_in_segments(&data, 24);
        let mut reasm = Reassembly::default();
        // First segment alone should not complete.
        let first = segs[0].clone();
        assert!(reasm.push(first).expect("push").is_none());
        assert!(reasm.is_in_progress());
        // Remaining segments complete it.
        let mut out = None;
        for s in &segs[1..] {
            out = reasm.push(s.clone()).expect("push");
        }
        assert_eq!(out, Some(data));
    }

    #[test]
    fn parse_send_segment_rejects_non_segment() {
        let header = Header::new(0x70, 20, 0, 0).to_bytes().to_vec();
        assert_eq!(parse_send_segment(&header), Err(WireError::FieldOverflow));
    }

    #[test]
    fn parse_send_segment_roundtrips() {
        let data = vec![5u8; 25];
        let segs = split_in_segments(&data, 32);
        for s in &segs {
            let parsed = parse_send_segment(&segment_to_buffer(s.clone()));
            assert_eq!(parsed, Ok(s.clone()));
        }
    }

    /// Build a raw buffer for a segment using the send id (8052).
    fn segment_to_buffer(seg: Segment) -> Vec<u8> {
        let chunk_len = seg.data.len();
        let header = Header::new(
            SEGMENT_SEND_ID,
            chunk_len as u32 + HEADER_SIZE as u32,
            seg.total_len as u32,
            seg.offset as u32,
        );
        let mut buf = header.to_bytes().to_vec();
        buf.extend_from_slice(&seg.data);
        buf
    }
}
