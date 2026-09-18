//! The driver's ASIO instance information: the 196 bytes `TUSBAUDIO_GetASIOInstanceInfo` writes.
//!
//! The layout is not in any public header. It was read against both vendor control panels on
//! 2026-09-18 (`.agent/reference/driver-api.md`), so every value is checked before it is shown:
//! a driver whose layout has moved answers "could not be read", never a plausible wrong number.

use serde::Serialize;

/// How many bytes the driver writes. Both drivers seen write exactly this many.
pub const INFO_LEN: usize = 196;

/// Where the list of offered buffer sizes starts, and so how many can fit.
const SIZES_AT: usize = 36;
const MAX_SIZES: u32 = ((INFO_LEN - SIZES_AT) / 4) as u32;

/// The rates a driver of this kind can run at, loosely: anything outside is a layout that moved.
const RATES: std::ops::RangeInclusive<u32> = 8_000..=768_000;
/// No ASIO buffer is larger than this many samples.
const LARGEST_BUFFER: u32 = 65_536;

/// What was read, every value in samples except the rate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AsioInstance {
    /// The sample rate the instance runs at, in Hz (offset 0).
    pub sample_rate: u32,
    /// The preferred buffer size (offset 28).
    pub buffer_size: u32,
    /// Input latency as the driver reports it to an ASIO host (offset 20).
    pub input_latency: u32,
    /// Output latency as the driver reports it to an ASIO host (offset 24).
    pub output_latency: u32,
    /// The buffer sizes the driver offers, smallest first (count at 32, sizes from 36).
    pub buffer_sizes: Vec<u32>,
}

/// Whether a rate is one a driver of this kind could be running at.
pub fn plausible_rate(rate: u32) -> bool {
    RATES.contains(&rate)
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Read the structure, or say which check it failed.
pub fn parse(bytes: &[u8]) -> Result<AsioInstance, String> {
    if bytes.len() < INFO_LEN {
        return Err(format!("the driver gave {} bytes where {INFO_LEN} were expected", bytes.len()));
    }
    let sample_rate = word(bytes, 0);
    if !plausible_rate(sample_rate) {
        return Err(format!("its sample rate ({sample_rate}) is not a plausible rate"));
    }
    let count = word(bytes, 32);
    if count == 0 || count > MAX_SIZES {
        return Err(format!("it offers {count} buffer sizes, where 1 to {MAX_SIZES} would fit"));
    }
    let buffer_sizes: Vec<u32> = (0..count as usize).map(|i| word(bytes, SIZES_AT + 4 * i)).collect();
    if buffer_sizes.iter().any(|&size| size == 0 || size > LARGEST_BUFFER) {
        return Err(format!("the buffer sizes it offers ({buffer_sizes:?}) are not all plausible"));
    }
    if buffer_sizes.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(format!("the buffer sizes it offers ({buffer_sizes:?}) are not in order"));
    }
    let buffer_size = word(bytes, 28);
    if !buffer_sizes.contains(&buffer_size) {
        return Err(format!("its buffer size ({buffer_size}) is not among the sizes it offers"));
    }
    let (input_latency, output_latency) = (word(bytes, 20), word(bytes, 24));
    for (name, latency) in [("input", input_latency), ("output", output_latency)] {
        // A latency is at least one buffer and well under a second; anything else is not a latency.
        if latency < buffer_size || latency > sample_rate {
            return Err(format!("its {name} latency ({latency} samples) is not plausible for a {buffer_size} sample buffer"));
        }
    }
    Ok(AsioInstance { sample_rate, buffer_size, input_latency, output_latency, buffer_sizes })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The structure as 49 little-endian words, from the reference's table.
    pub(crate) fn from_words(words: &[u32]) -> Vec<u8> {
        let mut bytes = vec![0u8; INFO_LEN];
        for (i, w) in words.iter().enumerate() {
            bytes[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
        bytes
    }

    const OFFERED: [u32; 9] = [8, 16, 32, 64, 128, 256, 512, 1024, 2048];

    /// The Quadro's bytes as read on 2026-09-18 (`reference/driver-api.md`).
    pub(crate) fn quadro() -> Vec<u8> {
        let mut words = vec![44100, 44100, 0, 0, 0x10000, 571, 632, 512, 9];
        words.extend(OFFERED);
        from_words(&words)
    }

    /// The Studio+'s bytes as read on 2026-09-18.
    pub(crate) fn studio() -> Vec<u8> {
        let mut words = vec![44100, 44100, 0, 0, 0x10000, 568, 585, 512, 9];
        words.extend(OFFERED);
        from_words(&words)
    }

    fn with(mut bytes: Vec<u8>, at: usize, value: u32) -> Vec<u8> {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        bytes
    }

    #[test]
    fn the_quadros_bytes_read_as_its_panel_showed_them() {
        assert_eq!(
            parse(&quadro()).unwrap(),
            AsioInstance { sample_rate: 44100, buffer_size: 512, input_latency: 571, output_latency: 632, buffer_sizes: OFFERED.to_vec() }
        );
    }

    #[test]
    fn the_studios_bytes_read_as_its_panel_showed_them() {
        assert_eq!(
            parse(&studio()).unwrap(),
            AsioInstance { sample_rate: 44100, buffer_size: 512, input_latency: 568, output_latency: 585, buffer_sizes: OFFERED.to_vec() }
        );
    }

    #[test]
    fn exact_raw_bytes_of_the_first_words_are_little_endian() {
        // 44100 = 0x0000AC44, 571 = 0x023B, 512 = 0x0200: the first bytes as a hex dump shows them.
        let bytes = quadro();
        assert_eq!(&bytes[0..4], &[0x44, 0xac, 0x00, 0x00]);
        assert_eq!(&bytes[20..24], &[0x3b, 0x02, 0x00, 0x00]);
        assert_eq!(&bytes[28..36], &[0x00, 0x02, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn a_short_answer_is_refused() {
        assert!(parse(&quadro()[..195]).unwrap_err().contains("195 bytes"));
    }

    #[test]
    fn a_rate_that_is_not_a_rate_is_refused() {
        assert!(parse(&with(quadro(), 0, 0)).unwrap_err().contains("sample rate"));
        assert!(parse(&with(quadro(), 0, 7_999)).unwrap_err().contains("sample rate"));
        assert!(parse(&with(quadro(), 0, 1_000_000)).unwrap_err().contains("sample rate"));
        assert!(parse(&with(quadro(), 0, 768_000)).is_ok(), "768 kHz is the top of the range");
        assert!(parse(&with(quadro(), 0, 8_000)).is_ok(), "8 kHz is the bottom");
    }

    #[test]
    fn a_count_out_of_bounds_is_refused() {
        assert!(parse(&with(quadro(), 32, 0)).unwrap_err().contains("0 buffer sizes"));
        assert!(parse(&with(quadro(), 32, 41)).unwrap_err().contains("41 buffer sizes"));
    }

    #[test]
    fn a_buffer_size_not_offered_is_refused() {
        assert!(parse(&with(quadro(), 28, 500)).unwrap_err().contains("not among"));
    }

    #[test]
    fn offered_sizes_out_of_order_or_empty_are_refused() {
        assert!(parse(&with(quadro(), 40, 4)).unwrap_err().contains("not in order"));
        assert!(parse(&with(quadro(), 36, 0)).unwrap_err().contains("not all plausible"));
        // A count larger than the list: the zeros after it are read as sizes.
        assert!(parse(&with(quadro(), 32, 10)).unwrap_err().contains("not all plausible"));
    }

    #[test]
    fn a_latency_that_is_not_a_latency_is_refused() {
        assert!(parse(&with(quadro(), 20, 100)).unwrap_err().contains("input latency"));
        assert!(parse(&with(quadro(), 24, 50_000)).unwrap_err().contains("output latency"));
        assert!(parse(&with(quadro(), 24, 512)).is_ok(), "a latency of exactly one buffer is plausible");
    }
}
