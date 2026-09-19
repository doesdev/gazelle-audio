//! The driver's ASIO instance information: the 196 bytes `TUSBAUDIO_GetASIOInstanceInfo` writes.
//!
//! The layout is not in any public header. It was read against both vendor control panels on
//! 2026-09-18, so every value is checked before it is shown:
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

/// Offset 16's Safe Mode bit, and the same bit of `SetASIOBufferPreferredSize`'s `options`.
pub const SAFE_MODE_FLAG: u32 = 0x10000;

/// More programs than this holding one driver's ASIO interface is a layout that moved.
pub const MAX_ASIO_CLIENTS: u32 = 64;

/// `SetASIOBufferPreferredSize`'s `options` for a Safe Mode setting. The driver replaces its
/// options with these rather than merging, and Safe Mode is the only bit it was seen to carry.
pub fn options_for(safe_mode: bool) -> u32 {
    if safe_mode {
        SAFE_MODE_FLAG
    } else {
        0
    }
}

/// What was read, every value in samples except the rate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AsioInstance {
    /// The sample rate the instance runs at, in Hz (offset 0).
    pub sample_rate: u32,
    /// The rate the preferred buffer applies at, in Hz (offset 4): what `SetASIOBufferPreferredSize`
    /// is given as `referenceSampleRate`.
    pub reference_rate: u32,
    /// The preferred buffer size (offset 28).
    pub buffer_size: u32,
    /// Input latency as the driver reports it to an ASIO host (offset 20).
    pub input_latency: u32,
    /// Output latency as the driver reports it to an ASIO host (offset 24).
    pub output_latency: u32,
    /// The buffer sizes the driver offers, smallest first (count at 32, sizes from 36).
    pub buffer_sizes: Vec<u32>,
    /// Safe Mode, as the driver runs it: bit 16 of offset 16 (0x10000 on, 0 off; flipped in the
    /// vendor panel and read both ways on 2026-09-18). Output latency grows with it.
    pub safe_mode: bool,
    /// The driver's count of ASIO clients (offset 8): nonzero while a program is using its ASIO
    /// interface. Both kernel drivers fill it from the same counter `GetClientInfo` answers as its
    /// ASIO client count, and the vendor panel's "ASIO active" / "ASIO not active" comes from this
    /// structure. Seen live 2026-09-18: one DAW recording on the Quadro counted 4, so it counts
    /// connections, not programs.
    pub asio_clients: u32,
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
        // Above nothing and under a second. Not "at least one buffer": with Safe Mode off the
        // Quadro reports an output latency of 191 samples on a 256 sample buffer.
        if latency == 0 || latency > sample_rate {
            return Err(format!("its {name} latency ({latency} samples) is not plausible for a {buffer_size} sample buffer"));
        }
    }
    let safe_mode = word(bytes, 16) & SAFE_MODE_FLAG != 0;
    let asio_clients = word(bytes, 8);
    if asio_clients > MAX_ASIO_CLIENTS {
        return Err(format!("its count of programs using ASIO ({asio_clients}) is not plausible"));
    }
    let reference_rate = word(bytes, 4);
    Ok(AsioInstance { sample_rate, reference_rate, buffer_size, input_latency, output_latency, buffer_sizes, safe_mode, asio_clients })
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

    /// The Quadro's bytes as read on 2026-09-18.
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
            AsioInstance { sample_rate: 44100, reference_rate: 44100, buffer_size: 512, input_latency: 571, output_latency: 632, buffer_sizes: OFFERED.to_vec(), safe_mode: true, asio_clients: 0 }
        );
    }

    #[test]
    fn the_studios_bytes_read_as_its_panel_showed_them() {
        assert_eq!(
            parse(&studio()).unwrap(),
            AsioInstance { sample_rate: 44100, reference_rate: 44100, buffer_size: 512, input_latency: 568, output_latency: 585, buffer_sizes: OFFERED.to_vec(), safe_mode: true, asio_clients: 0 }
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

    /// The Quadro's bytes with Safe Mode off at 256 samples (the user, 2026-09-18): the output
    /// latency is *less* than one buffer, and the flag at offset 16 is clear.
    pub(crate) fn quadro_safe_mode_off() -> Vec<u8> {
        let mut words = vec![44100, 44100, 0, 0, 0, 315, 191, 256, 9];
        words.extend(OFFERED);
        from_words(&words)
    }

    #[test]
    fn a_latency_shorter_than_the_buffer_is_read_as_the_panel_showed_it() {
        let read = parse(&quadro_safe_mode_off()).unwrap();
        assert_eq!((read.buffer_size, read.input_latency, read.output_latency), (256, 315, 191));
        assert!(!read.safe_mode);
    }

    #[test]
    fn safe_mode_is_the_flag_at_offset_16() {
        assert!(parse(&quadro()).unwrap().safe_mode, "0x10000 with Safe Mode on, as read live");
        assert!(parse(&studio()).unwrap().safe_mode);
        assert!(!parse(&quadro_safe_mode_off()).unwrap().safe_mode, "0 with it off");
    }

    #[test]
    fn a_latency_that_is_not_a_latency_is_refused() {
        assert!(parse(&with(quadro(), 20, 0)).unwrap_err().contains("input latency"));
        assert!(parse(&with(quadro(), 24, 50_000)).unwrap_err().contains("output latency"));
        assert!(parse(&with(quadro(), 24, 1)).is_ok(), "any latency above nothing and under a second is plausible");
    }

    /// The Quadro's bytes with a DAW holding the driver's ASIO interface: offset 8 counts it.
    pub(crate) fn quadro_in_use(clients: u32) -> Vec<u8> {
        with(quadro(), 8, clients)
    }

    #[test]
    fn offset_8_counts_the_programs_using_asio() {
        assert_eq!(parse(&quadro()).unwrap().asio_clients, 0, "no DAW open when the reference was read");
        assert_eq!(parse(&quadro_in_use(1)).unwrap().asio_clients, 1);
        assert_eq!(parse(&quadro_in_use(3)).unwrap().asio_clients, 3);
        assert!(parse(&quadro_in_use(MAX_ASIO_CLIENTS + 1)).unwrap_err().contains("programs using ASIO"));
        assert!(parse(&quadro_in_use(MAX_ASIO_CLIENTS)).is_ok());
    }

    #[test]
    fn offset_4_is_the_rate_the_buffer_applies_at() {
        assert_eq!(parse(&with(quadro(), 4, 48000)).unwrap().reference_rate, 48000);
        assert_eq!(parse(&quadro()).unwrap().reference_rate, 44100);
    }

    #[test]
    fn the_safe_mode_option_is_the_flag_read_at_offset_16() {
        assert_eq!(options_for(true), 0x10000);
        assert_eq!(options_for(false), 0);
        assert_eq!(options_for(true), word(&quadro(), 16), "what is sent is what is read with it on");
        assert_eq!(options_for(false), word(&quadro_safe_mode_off(), 16));
    }
}
