//! **Broadcast WAV, one mono file per channel**, and RF64 once a file passes 4 GB.
//!
//! Written by hand rather than with a crate: the two things a recorder needs beyond a plain WAV,
//! the `bext` chunk (EBU Tech 3285) and the switch to RF64 (EBU Tech 3306), are a few fixed layouts,
//! and the crates that write WAV do one or the other, or neither. Everything here is laid out the
//! way the specifications lay it out, and the tests below read the bytes back field by field.
//!
//! # The layout
//!
//! ```text
//!  0  "RIFF"  size            "WAVE"            (becomes "RF64", 0xFFFFFFFF)
//! 12  "JUNK"  28              28 zero bytes     (becomes "ds64" with the 64-bit sizes)
//! 48  "fmt "  16 or 18        the format
//!     "fact"  4               frames            (32-bit float only)
//!     "bext"  602 + history   description, originator, date, time, TimeReference, version 1
//!     "data"  size            the samples       (size becomes 0xFFFFFFFF in RF64)
//!     "cue "  28              one cue point     (only a take after a count-in: the downbeat)
//!     "LIST"  "adtl"          its label, "labl"
//! ```
//!
//! The cue and its label follow the audio and are written once, when the file is finished, so a DAW
//! that reads cue points puts a marker on the downbeat after a count-in.
//!
//! The `JUNK` chunk is the room EBU Tech 3306 asks for, right after `WAVE`, so that a file that grows
//! past 4 GB can become RF64 in place, without moving a byte of audio.
//!
//! # The samples
//!
//! **The aggregate hands every channel over as 32-bit integers** (`ASIOSTInt32LSB`, the one sample
//! type it presents: `gazelle_aggregate::aggregate::Aggregate::channel_info`), with the converter's
//! 24 bits at the top. So:
//!
//! - **24-bit integer** keeps the top three bytes of each sample: an arithmetic shift right by 8,
//!   written little endian. Nothing is rounded or dithered, because the low byte carries nothing
//!   from a 24-bit converter; a lower byte that did carry something is dropped, which is the same
//!   as what a DAW recording 24-bit does with these drivers.
//! - **32-bit float** is the sample divided by 2^31, so full scale is 1.0. For anything a 24-bit
//!   converter produces this is exact, since a float carries 24 bits of mantissa.
//!
//! # Crashes
//!
//! **The sizes are written at intervals** ([`WavWriter::with_sizes_every`], two seconds of audio by
//! default), not only at the end. A file cut short by a crash or a pulled cable then says how much
//! audio it holds up to the last time they were written, and any reader opens it; the audio after
//! that point is in the file as well, and most editors will take it when told to read to the end.

use std::io::{self, Seek, SeekFrom, Write};

/// What a take's files hold, as a preset chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// 24-bit integer PCM, the default: what the converters produce, in three bytes a sample.
    Int24,
    /// 32-bit IEEE float, full scale 1.0.
    Float32,
}

impl SampleFormat {
    /// Bytes one sample takes in the file.
    pub fn bytes(self) -> u16 {
        match self {
            SampleFormat::Int24 => 3,
            SampleFormat::Float32 => 4,
        }
    }

    pub fn bits(self) -> u16 {
        self.bytes() * 8
    }

    /// `WAVE_FORMAT_PCM` or `WAVE_FORMAT_IEEE_FLOAT`.
    pub fn tag(self) -> u16 {
        match self {
            SampleFormat::Int24 => 1,
            SampleFormat::Float32 => 3,
        }
    }

    /// How a person reads it: "24-bit", "32-bit float".
    pub fn words(self) -> &'static str {
        match self {
            SampleFormat::Int24 => "24-bit",
            SampleFormat::Float32 => "32-bit float",
        }
    }

    /// Append the file's bytes for these samples, straight from what the driver handed over.
    pub fn encode(self, samples: &[i32], out: &mut Vec<u8>) {
        match self {
            SampleFormat::Int24 => {
                for &sample in samples {
                    // The top three bytes, little endian: the same as `sample >> 8`, sign and all.
                    let bytes = sample.to_le_bytes();
                    out.extend_from_slice(&bytes[1..4]);
                }
            }
            SampleFormat::Float32 => {
                const SCALE: f32 = 1.0 / 2_147_483_648.0;
                for &sample in samples {
                    out.extend_from_slice(&((sample as f32) * SCALE).to_le_bytes());
                }
            }
        }
    }
}

/// What the `bext` chunk says about a take. Every file of a take carries the same one, apart from
/// its description, so a DAW puts them all at the same place on its timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bext {
    /// Up to 256 characters: what this file is.
    pub description: String,
    /// Up to 32: what made it.
    pub originator: String,
    /// Up to 32: a reference of the originator's own.
    pub originator_reference: String,
    /// `yyyy-mm-dd`, the local date the take's first sample was recorded on.
    pub origination_date: String,
    /// `hh:mm:ss`, the local time of that sample.
    pub origination_time: String,
    /// The take's first sample, counted in samples since midnight at the file's rate.
    pub time_reference: u64,
    /// How the audio came to be, one line per step, each ending `\r\n`.
    pub coding_history: String,
}

/// The fixed part of `bext`: 256 + 32 + 32 + 10 + 8 + 4 + 4 + 2 + 64 + 190.
pub const BEXT_FIXED: usize = 602;
/// The room kept for `ds64`: three 64-bit sizes and an empty table's length.
pub const DS64_SIZE: u32 = 28;
/// Where the `JUNK` chunk that becomes `ds64` starts.
const JUNK_AT: u64 = 12;
/// The largest RIFF size a plain WAV can say.
pub const RIFF_LIMIT: u64 = u32::MAX as u64;

/// One file being written.
pub struct WavWriter<W: Write + Seek> {
    out: W,
    format: SampleFormat,
    /// Where the data chunk's size field is.
    data_size_at: u64,
    /// Where the audio starts.
    data_start: u64,
    /// Where the `fact` chunk's frame count is, for a float file.
    fact_at: Option<u64>,
    data_bytes: u64,
    frames: u64,
    rf64: bool,
    /// The RIFF size past which the file becomes RF64. [`RIFF_LIMIT`] except in a test.
    limit: u64,
    /// Audio written since the sizes were last written.
    since_sizes: u64,
    sizes_every: u64,
    scratch: Vec<u8>,
    /// Bytes of chunks after the audio: the cue and its label, once finished.
    trailer: u64,
}

impl<W: Write + Seek> WavWriter<W> {
    /// Write the headers of a new mono file. `out` should be empty and positioned at its start.
    pub fn create(mut out: W, format: SampleFormat, rate: u32, bext: &Bext) -> io::Result<WavWriter<W>> {
        let mut head: Vec<u8> = Vec::with_capacity(1024);
        head.extend_from_slice(b"RIFF");
        head.extend_from_slice(&0u32.to_le_bytes());
        head.extend_from_slice(b"WAVE");
        // The room RF64 needs, right after WAVE (EBU Tech 3306).
        head.extend_from_slice(b"JUNK");
        head.extend_from_slice(&DS64_SIZE.to_le_bytes());
        head.extend_from_slice(&[0u8; DS64_SIZE as usize]);

        let bytes = format.bytes();
        let float = format == SampleFormat::Float32;
        head.extend_from_slice(b"fmt ");
        head.extend_from_slice(&(if float { 18u32 } else { 16u32 }).to_le_bytes());
        head.extend_from_slice(&format.tag().to_le_bytes());
        head.extend_from_slice(&1u16.to_le_bytes());
        head.extend_from_slice(&rate.to_le_bytes());
        head.extend_from_slice(&(rate.saturating_mul(u32::from(bytes))).to_le_bytes());
        head.extend_from_slice(&bytes.to_le_bytes());
        head.extend_from_slice(&format.bits().to_le_bytes());
        if float {
            // cbSize: nothing follows.
            head.extend_from_slice(&0u16.to_le_bytes());
        }

        let mut fact_at = None;
        if float {
            // A format other than PCM carries its length in frames here (and in ds64 once RF64).
            head.extend_from_slice(b"fact");
            head.extend_from_slice(&4u32.to_le_bytes());
            fact_at = Some(head.len() as u64);
            head.extend_from_slice(&0u32.to_le_bytes());
        }

        let history = ascii(&bext.coding_history);
        let bext_size = BEXT_FIXED + history.len();
        head.extend_from_slice(b"bext");
        head.extend_from_slice(&(bext_size as u32).to_le_bytes());
        field(&mut head, &bext.description, 256);
        field(&mut head, &bext.originator, 32);
        field(&mut head, &bext.originator_reference, 32);
        field(&mut head, &bext.origination_date, 10);
        field(&mut head, &bext.origination_time, 8);
        head.extend_from_slice(&(bext.time_reference as u32).to_le_bytes());
        head.extend_from_slice(&((bext.time_reference >> 32) as u32).to_le_bytes());
        // Version 1: a UMID may follow, and here none does, so it is all zero, as are the 190
        // reserved bytes. Version 2's loudness fields are not measured, so the file does not claim
        // them.
        head.extend_from_slice(&1u16.to_le_bytes());
        head.extend_from_slice(&[0u8; 64]);
        head.extend_from_slice(&[0u8; 190]);
        head.extend_from_slice(&history);
        if bext_size % 2 == 1 {
            head.push(0);
        }

        head.extend_from_slice(b"data");
        let data_size_at = head.len() as u64;
        head.extend_from_slice(&0u32.to_le_bytes());
        let data_start = head.len() as u64;

        out.write_all(&head)?;
        let sizes_every = u64::from(rate) * u64::from(bytes) * 2;
        let mut writer = WavWriter {
            out,
            format,
            data_size_at,
            data_start,
            fact_at,
            data_bytes: 0,
            frames: 0,
            rf64: false,
            limit: RIFF_LIMIT,
            since_sizes: 0,
            sizes_every: sizes_every.max(1),
            scratch: Vec::new(),
            trailer: 0,
        };
        writer.write_sizes(false)?;
        Ok(writer)
    }

    /// Become RF64 once the RIFF size would pass `limit` bytes rather than 4 GB. For a test.
    pub fn with_limit(mut self, limit: u64) -> WavWriter<W> {
        self.limit = limit;
        self
    }

    /// Write the sizes into the headers every `bytes` of audio rather than every two seconds.
    pub fn with_sizes_every(mut self, bytes: u64) -> WavWriter<W> {
        self.sizes_every = bytes.max(1);
        self
    }

    /// The samples of one channel, as the driver handed them over.
    pub fn write(&mut self, samples: &[i32]) -> io::Result<()> {
        let mut bytes = std::mem::take(&mut self.scratch);
        bytes.clear();
        self.format.encode(samples, &mut bytes);
        let wrote = self.write_bytes(&bytes, samples.len() as u64);
        self.scratch = bytes;
        wrote
    }

    /// Silence, for audio that never arrived: the file keeps its length, so the take stays lined up
    /// with the other files and with the clock.
    pub fn write_silence(&mut self, frames: u64) -> io::Result<()> {
        let chunk = [0i32; 1024];
        let mut left = frames;
        while left > 0 {
            let now = left.min(chunk.len() as u64) as usize;
            self.write(&chunk[..now])?;
            left -= now as u64;
        }
        Ok(())
    }

    fn write_bytes(&mut self, bytes: &[u8], frames: u64) -> io::Result<()> {
        let riff_after = self.data_start + self.data_bytes + bytes.len() as u64 + 1 - 8;
        if !self.rf64 && riff_after > self.limit {
            self.rf64 = true;
            self.write_sizes(false)?;
        }
        self.out.write_all(bytes)?;
        self.data_bytes += bytes.len() as u64;
        self.frames += frames;
        self.since_sizes += bytes.len() as u64;
        if self.since_sizes >= self.sizes_every {
            self.write_sizes(false)?;
        }
        Ok(())
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Bytes of audio written so far.
    pub fn data_bytes(&self) -> u64 {
        self.data_bytes
    }

    /// Whether this file has become RF64.
    pub fn is_rf64(&self) -> bool {
        self.rf64
    }

    /// Write every size field for what is in the file now, and go back to its end.
    ///
    /// `padded` is true only at the end, when the pad byte an odd data chunk needs has been written.
    fn write_sizes(&mut self, padded: bool) -> io::Result<()> {
        self.since_sizes = 0;
        let pad = u64::from(padded && self.data_bytes % 2 == 1);
        let riff = self.data_start + self.data_bytes + pad + self.trailer - 8;
        let end = self.data_start + self.data_bytes + pad + self.trailer;
        if self.rf64 {
            self.at(0, b"RF64")?;
            self.at(4, &u32::MAX.to_le_bytes())?;
            let mut ds64 = Vec::with_capacity(36);
            ds64.extend_from_slice(b"ds64");
            ds64.extend_from_slice(&DS64_SIZE.to_le_bytes());
            ds64.extend_from_slice(&riff.to_le_bytes());
            ds64.extend_from_slice(&self.data_bytes.to_le_bytes());
            ds64.extend_from_slice(&self.frames.to_le_bytes());
            ds64.extend_from_slice(&0u32.to_le_bytes());
            self.at(JUNK_AT, &ds64)?;
            self.at(self.data_size_at, &u32::MAX.to_le_bytes())?;
            if let Some(fact) = self.fact_at {
                self.at(fact, &u32::MAX.to_le_bytes())?;
            }
        } else {
            self.at(4, &(riff as u32).to_le_bytes())?;
            self.at(self.data_size_at, &(self.data_bytes as u32).to_le_bytes())?;
            if let Some(fact) = self.fact_at {
                self.at(fact, &(self.frames.min(RIFF_LIMIT) as u32).to_le_bytes())?;
            }
        }
        self.out.seek(SeekFrom::Start(end))?;
        Ok(())
    }

    fn at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        self.out.seek(SeekFrom::Start(offset))?;
        self.out.write_all(bytes)
    }

    /// Write the sizes now, whatever the interval says: a take that is stopping, or a check.
    pub fn sync_sizes(&mut self) -> io::Result<()> {
        self.write_sizes(false)?;
        self.out.flush()
    }

    /// Finish the file: the pad byte an odd data chunk needs, and every size, for the last time.
    pub fn finish(self) -> io::Result<W> {
        self.finish_with_cue(None)
    }

    /// Finish the file with one cue point `at` samples into the audio, labelled `label`, which a DAW
    /// shows as a marker. A cue past what a cue point can say (4 billion samples) is left out.
    pub fn finish_with_cue(mut self, cue: Option<(u64, &str)>) -> io::Result<W> {
        let padded = self.data_bytes % 2 == 1;
        if padded {
            let riff_after = self.data_start + self.data_bytes + 1 - 8;
            if !self.rf64 && riff_after > self.limit {
                self.rf64 = true;
            }
            self.out.write_all(&[0])?;
        }
        if let Some((at, label)) = cue.filter(|(at, _)| *at <= u64::from(u32::MAX)) {
            let trailer = cue_chunks(at as u32, label);
            self.out.write_all(&trailer)?;
            self.trailer = trailer.len() as u64;
        }
        self.write_sizes(true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// A `cue ` chunk with one point at `at` samples into the data, and a `LIST` `adtl` chunk with its
/// label.
fn cue_chunks(at: u32, label: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(96);
    out.extend_from_slice(b"cue ");
    out.extend_from_slice(&28u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    // dwName, dwPosition, fccChunk, dwChunkStart, dwBlockStart, dwSampleOffset.
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    let mut text = ascii(label);
    text.push(0);
    let labl = 4 + text.len();
    out.extend_from_slice(b"LIST");
    out.extend_from_slice(&((4 + 8 + labl + labl % 2) as u32).to_le_bytes());
    out.extend_from_slice(b"adtl");
    out.extend_from_slice(b"labl");
    out.extend_from_slice(&(labl as u32).to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&text);
    if labl % 2 == 1 {
        out.push(0);
    }
    out
}

/// Text for a `bext` field: ASCII, as the chunk is, with anything else as `?`.
fn ascii(text: &str) -> Vec<u8> {
    text.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() || c == '\r' || c == '\n' { c as u8 } else { b'?' }).collect()
}

/// A fixed-width `bext` field: the text, cut to fit, and zeros after it.
fn field(into: &mut Vec<u8>, text: &str, width: usize) {
    let mut bytes = ascii(text);
    bytes.truncate(width);
    bytes.resize(width, 0);
    into.extend_from_slice(&bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn bext() -> Bext {
        Bext {
            description: "Vocal mic (Quadro 1)".into(),
            originator: "Gazelle".into(),
            originator_reference: "T007".into(),
            origination_date: "2026-09-27".into(),
            origination_time: "14:03:21".into(),
            // Past 2^32, so the high word is exercised: 14:03:21 at 96 kHz is 4 856 016 000.
            time_reference: 4_856_016_000,
            coding_history: "A=PCM,F=96000,W=24,M=mono,T=Gazelle\r\n".into(),
        }
    }

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn u64_at(bytes: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    /// Every chunk in a file, as `(id, where its body starts, the size its header says)`, walked
    /// as a reader walks them, pad bytes and all.
    fn chunks(bytes: &[u8]) -> Vec<(String, usize, u32)> {
        let mut found = Vec::new();
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let id = String::from_utf8_lossy(&bytes[at..at + 4]).to_string();
            let size = u32_at(bytes, at + 4);
            found.push((id.clone(), at + 8, size));
            if size == u32::MAX {
                break;
            }
            at += 8 + size as usize + (size as usize % 2);
        }
        found
    }

    fn written(format: SampleFormat, samples: &[i32]) -> Vec<u8> {
        let mut writer = WavWriter::create(Cursor::new(Vec::new()), format, 96_000, &bext()).unwrap();
        writer.write(samples).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn a_24_bit_file_is_riff_wave_with_junk_fmt_bext_and_data_in_that_order() {
        let bytes = written(SampleFormat::Int24, &[0x7FFF_FF00u32 as i32, -256, 0x0012_3400]);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32_at(&bytes, 4) as usize, bytes.len() - 8, "the RIFF size is the file less its first eight bytes");
        assert_eq!(&bytes[8..12], b"WAVE");
        let ids: Vec<String> = chunks(&bytes).into_iter().map(|(id, _, _)| id).collect();
        assert_eq!(ids, vec!["JUNK", "fmt ", "bext", "data"]);
        let found = chunks(&bytes);
        assert_eq!(found[0], ("JUNK".into(), 20, 28), "the room for ds64, right after WAVE");
        let fmt = found[1].1;
        assert_eq!(found[1].2, 16);
        assert_eq!(u16_at(&bytes, fmt), 1, "WAVE_FORMAT_PCM");
        assert_eq!(u16_at(&bytes, fmt + 2), 1, "mono");
        assert_eq!(u32_at(&bytes, fmt + 4), 96_000);
        assert_eq!(u32_at(&bytes, fmt + 8), 96_000 * 3, "bytes a second");
        assert_eq!(u16_at(&bytes, fmt + 12), 3, "block align");
        assert_eq!(u16_at(&bytes, fmt + 14), 24);
        let (_, data, size) = found[3].clone();
        assert_eq!(size, 9, "three samples of three bytes");
        assert_eq!(&bytes[data..data + 9], &[0xFF, 0xFF, 0x7F, 0xFF, 0xFF, 0xFF, 0x34, 0x12, 0x00], "the top three bytes of each, little endian");
        assert_eq!(bytes.len(), data + 10, "and the pad byte an odd chunk takes");
    }

    #[test]
    fn the_bext_chunk_is_laid_out_as_ebu_tech_3285_says() {
        let bytes = written(SampleFormat::Int24, &[0; 4]);
        let (_, at, size) = chunks(&bytes).into_iter().find(|(id, _, _)| id == "bext").unwrap();
        let history = "A=PCM,F=96000,W=24,M=mono,T=Gazelle\r\n";
        assert_eq!(size as usize, BEXT_FIXED + history.len());
        assert_eq!(&bytes[at..at + 20], b"Vocal mic (Quadro 1)");
        assert!(bytes[at + 20..at + 256].iter().all(|&b| b == 0), "the rest of the description is zero");
        assert_eq!(&bytes[at + 256..at + 263], b"Gazelle");
        assert_eq!(&bytes[at + 288..at + 292], b"T007");
        assert_eq!(&bytes[at + 320..at + 330], b"2026-09-27", "OriginationDate at 320");
        assert_eq!(&bytes[at + 330..at + 338], b"14:03:21", "OriginationTime at 330");
        let low = u32_at(&bytes, at + 338);
        let high = u32_at(&bytes, at + 342);
        assert_eq!((u64::from(high) << 32) | u64::from(low), 4_856_016_000, "TimeReference, low word first");
        assert_eq!(high, 1);
        assert_eq!(u16_at(&bytes, at + 346), 1, "version 1");
        assert!(bytes[at + 348..at + 602].iter().all(|&b| b == 0), "no UMID, and the reserved bytes are zero");
        assert_eq!(&bytes[at + 602..at + 602 + history.len()], history.as_bytes());
    }

    #[test]
    fn a_float_file_says_ieee_float_and_carries_a_fact_chunk_with_its_length() {
        let bytes = written(SampleFormat::Float32, &[i32::MIN, 0, 1 << 30, -(1 << 30), 0x7FFF_FF00]);
        let found = chunks(&bytes);
        let ids: Vec<&str> = found.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["JUNK", "fmt ", "fact", "bext", "data"]);
        let fmt = found[1].1;
        assert_eq!(found[1].2, 18, "the float format carries cbSize");
        assert_eq!(u16_at(&bytes, fmt), 3, "WAVE_FORMAT_IEEE_FLOAT");
        assert_eq!(u16_at(&bytes, fmt + 14), 32);
        assert_eq!(u16_at(&bytes, fmt + 16), 0, "cbSize");
        assert_eq!(u32_at(&bytes, found[2].1), 5, "fact holds the frame count");
        let data = found[4].1;
        let sample = |i: usize| f32::from_le_bytes(bytes[data + 4 * i..data + 4 * i + 4].try_into().unwrap());
        assert_eq!(sample(0), -1.0);
        assert_eq!(sample(1), 0.0);
        assert_eq!(sample(2), 0.5);
        assert_eq!(sample(3), -0.5);
        assert_eq!(sample(4), 8_388_607.0 / 8_388_608.0, "a 24-bit full scale sample is exact");
    }

    #[test]
    fn every_24_bit_value_goes_to_float_exactly() {
        for top in [-8_388_608i32, -8_388_607, -1, 0, 1, 12_345, 8_388_607] {
            let mut out = Vec::new();
            SampleFormat::Float32.encode(&[top << 8], &mut out);
            let value = f32::from_le_bytes(out[..4].try_into().unwrap());
            assert_eq!(f64::from(value) * 8_388_608.0, f64::from(top), "{top}");
        }
    }

    #[test]
    fn a_file_that_passes_the_limit_becomes_rf64_in_place() {
        // A limit of a few hundred bytes stands in for 4 GB.
        let mut writer = WavWriter::create(Cursor::new(Vec::new()), SampleFormat::Int24, 48_000, &bext()).unwrap().with_limit(1_000);
        let before = writer.data_start;
        writer.write(&[1 << 8; 10]).unwrap();
        assert!(!writer.is_rf64(), "still small enough");
        writer.write(&vec![2 << 8; 200]).unwrap();
        assert!(writer.is_rf64(), "past the limit");
        writer.write(&[3 << 8; 7]).unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        assert_eq!(&bytes[0..4], b"RF64");
        assert_eq!(u32_at(&bytes, 4), u32::MAX);
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"ds64", "the JUNK chunk became ds64, where it was");
        assert_eq!(u32_at(&bytes, 16), 28);
        let data_bytes = (10 + 200 + 7) * 3;
        assert_eq!(u64_at(&bytes, 20) as usize, bytes.len() - 8, "the 64-bit RIFF size");
        assert_eq!(u64_at(&bytes, 28), data_bytes as u64, "the 64-bit data size");
        assert_eq!(u64_at(&bytes, 36), 217, "the sample count");
        assert_eq!(u32_at(&bytes, 44), 0, "no table");
        let data = chunks(&bytes).into_iter().find(|(id, _, _)| id == "data").unwrap();
        assert_eq!(data.2, u32::MAX, "the data chunk defers to ds64");
        assert_eq!(data.1 as u64, before, "and the audio did not move");
        assert_eq!(&bytes[data.1..data.1 + 3], &[1, 0, 0]);
        assert_eq!(&bytes[data.1 + 30..data.1 + 33], &[2, 0, 0], "the first sample written after the switch");
        assert_eq!(bytes.len(), data.1 + data_bytes + 1, "with its pad byte");
    }

    #[test]
    fn a_float_file_that_becomes_rf64_hands_its_length_to_ds64() {
        let mut writer = WavWriter::create(Cursor::new(Vec::new()), SampleFormat::Float32, 48_000, &bext()).unwrap().with_limit(900);
        writer.write(&vec![0; 100]).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert_eq!(&bytes[0..4], b"RF64");
        let fact = chunks(&bytes).into_iter().find(|(id, _, _)| id == "fact").unwrap();
        assert_eq!(u32_at(&bytes, fact.1), u32::MAX);
        assert_eq!(u64_at(&bytes, 36), 100);
    }

    #[test]
    fn the_sizes_are_written_while_the_file_grows_so_a_crash_leaves_a_readable_file() {
        let mut writer = WavWriter::create(Cursor::new(Vec::new()), SampleFormat::Int24, 48_000, &bext()).unwrap().with_sizes_every(30);
        writer.write(&[5 << 8; 12]).unwrap();
        // Never finished: this is what a crash leaves.
        let bytes = writer.out.clone().into_inner();
        let data = chunks(&bytes).into_iter().find(|(id, _, _)| id == "data").unwrap();
        assert_eq!(data.2, 36, "the data size was written at the interval");
        assert_eq!(u32_at(&bytes, 4) as usize, bytes.len() - 8, "and the RIFF size with it");
    }

    #[test]
    fn silence_keeps_a_file_the_length_it_should_be() {
        let mut writer = WavWriter::create(Cursor::new(Vec::new()), SampleFormat::Int24, 48_000, &bext()).unwrap();
        writer.write(&[9 << 8; 3]).unwrap();
        writer.write_silence(2_500).unwrap();
        assert_eq!(writer.frames(), 2_503);
        let bytes = writer.finish().unwrap().into_inner();
        let data = chunks(&bytes).into_iter().find(|(id, _, _)| id == "data").unwrap();
        assert_eq!(data.2 as usize, 2_503 * 3);
        assert!(bytes[data.1 + 9..data.1 + 2_503 * 3].iter().all(|&b| b == 0));
    }

    #[test]
    fn a_cue_after_the_audio_marks_the_downbeat_and_the_sizes_count_it() {
        let mut writer = WavWriter::create(Cursor::new(Vec::new()), SampleFormat::Int24, 48_000, &bext()).unwrap();
        writer.write(&[7 << 8; 5]).unwrap();
        let bytes = writer.finish_with_cue(Some((3, "Downbeat"))).unwrap().into_inner();
        assert_eq!(u32_at(&bytes, 4) as usize, bytes.len() - 8, "the RIFF size takes the cue in");
        let found = chunks(&bytes);
        let ids: Vec<&str> = found.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["JUNK", "fmt ", "bext", "data", "cue ", "LIST"]);
        let (_, cue, size) = found[4].clone();
        assert_eq!(size, 28);
        assert_eq!(u32_at(&bytes, cue), 1, "one cue point");
        assert_eq!(u32_at(&bytes, cue + 8), 3, "at sample 3");
        assert_eq!(&bytes[cue + 12..cue + 16], b"data");
        assert_eq!(u32_at(&bytes, cue + 24), 3, "and its sample offset");
        let (_, list, list_size) = found[5].clone();
        assert_eq!(&bytes[list..list + 8], b"adtllabl");
        assert_eq!(u32_at(&bytes, list + 12), 1, "the label names cue point 1");
        assert_eq!(&bytes[list + 16..list + 25], b"Downbeat\0");
        assert_eq!(list + list_size as usize + (list_size as usize % 2), bytes.len(), "and it is the last thing in the file");
        let (_, data, data_size) = found[3].clone();
        assert_eq!(data_size, 15, "the audio is untouched");
        assert_eq!(&bytes[data..data + 3], &[7, 0, 0]);
    }

    #[test]
    fn text_that_is_not_ascii_is_kept_readable_rather_than_broken() {
        let mut text = Vec::new();
        field(&mut text, "Café 1", 8);
        assert_eq!(&text, b"Caf? 1\0\0");
        let mut long = Vec::new();
        field(&mut long, "a name far too long for its field", 10);
        assert_eq!(&long, b"a name far");
    }
}
