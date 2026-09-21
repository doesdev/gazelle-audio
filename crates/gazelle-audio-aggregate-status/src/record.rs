//! The record itself: fixed size, no pointers, no allocation, and one writer per area.
//!
//! Everything here is `#[repr(C)]` with its size asserted by a test, because the whole point of
//! this crate is that two programs built at different times agree on the bytes. A field is never
//! removed or moved; a new one goes in a `reserved` slot, and anything that cannot be done that way
//! is a new [`FORMAT_VERSION`] and a reader that politely says so.
//!
//! Every field is an integer, a float or a byte array, so every bit pattern is a valid value of its
//! type. That is what lets a reader copy the bytes out from under a writer and decide afterwards,
//! from the sequence counter, whether to keep them.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64};

/// "GZAGGST1" read as bytes. A section that does not start with this is not ours.
pub const MAGIC: u64 = u64::from_le_bytes(*b"GZAGGST1");

/// The version of the layout below. A reader that understands this number and finds a different
/// one refuses and says what it found, rather than reading rubbish out of a record it does not
/// know the shape of.
///
/// **2** added what a session measured of each interface's capture phase, which is three fields
/// per device and so a record of a different size. The shared section's name carries this number,
/// so a driver and a Gazelle built either side of the change do not meet at all rather than
/// reading each other's bytes as the wrong fields.
pub const FORMAT_VERSION: u32 = 2;

/// How many devices a record carries. The same limit the driver holds, and raising it is a new
/// format version, because it changes the size of the record.
pub const MAX_DEVICES: usize = 8;

/// The longest device or driver name the record carries, in bytes.
pub const NAME_BYTES: usize = 32;

/// The longest line of prose the record carries: a refusal, or where the configuration came from.
pub const TEXT_BYTES: usize = 256;

/// A string of at most `N` bytes, with its length, so a reader never looks for a terminator and
/// never reads past the end.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Text<const N: usize> {
    pub bytes: [u8; N],
    pub len: u32,
    pub reserved: u32,
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Text { bytes: [0; N], len: 0, reserved: 0 }
    }
}

impl<const N: usize> Text<N> {
    /// Put a string in, cut to fit. The cut lands on a character boundary, so what comes back out
    /// is always the beginning of what went in rather than half of a letter.
    pub fn set(&mut self, text: &str) {
        let mut take = text.len().min(N);
        while take > 0 && !text.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[..take].copy_from_slice(&text.as_bytes()[..take]);
        self.bytes[take..].fill(0);
        self.len = take as u32;
    }

    /// What it holds. A length past the end of the array, or bytes that are not text, read as
    /// nothing: a reader of a record it cannot trust says less rather than guessing.
    pub fn get(&self) -> &str {
        let len = (self.len as usize).min(N);
        std::str::from_utf8(&self.bytes[..len]).unwrap_or("")
    }

    pub fn from(text: &str) -> Text<N> {
        let mut made = Text::default();
        made.set(text);
        made
    }
}

impl<const N: usize> std::fmt::Debug for Text<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.get(), f)
    }
}

impl<const N: usize> PartialEq for Text<N> {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

/// A device name, as the configuration file calls it.
pub type Name = Text<NAME_BYTES>;
/// A line of prose.
pub type Line = Text<TEXT_BYTES>;

/// How the plan lines its devices up. The same two words the configuration file uses.
pub mod alignment {
    pub const ALIGNED: u32 = 0;
    pub const LOWEST_LATENCY: u32 = 1;

    pub fn name(code: u32) -> &'static str {
        match code {
            LOWEST_LATENCY => "lowest_latency",
            _ => "aligned",
        }
    }
}

/// What became of one interface's phase measurement.
///
/// **A phase is not a trim.** The trim is the constant a person measured once with a cable and a
/// click, and it lives in the configuration file beside the reference: the phase that was measured
/// in the session the trim was measured in. The phase is what the interface's capture pipeline
/// settles on when its stream starts, which is a different number every session, and it is
/// measured at the start of each one. Each session is lined up by the reference minus the phase,
/// and then the trim applies.
pub mod phase {
    /// Nothing in the configuration file says how to measure this interface, so nothing was.
    pub const NOT_CONFIGURED: u32 = 0;
    /// The measurement is in flight: the signal has gone out and nothing has come back yet.
    pub const MEASURING: u32 = 1;
    /// It was measured and the interface was lined up to the phase its trim was measured at.
    pub const APPLIED: u32 = 2;
    /// Nothing arrived on the measurement channel, which is a refusal to correct rather than a
    /// correction of zero.
    pub const NOT_HEARD: u32 = 3;
    /// Something arrived, and how far it had moved from the reference was not near a whole
    /// number of 32 sample steps, which is the only way the hardware moves. A change that is not
    /// is a measurement of something else.
    pub const OFF_THE_GRID: u32 = 4;
    /// Lining it up would have taken more room than the driver keeps for it.
    pub const TOO_FAR: u32 = 5;
    /// It was measured, and there is nothing to line it up to: no phase was measured in the
    /// session its trim was measured in. Nothing is moved, and measuring the interfaces once
    /// supplies one. Not a refusal: the measurement was fine.
    pub const NO_REFERENCE: u32 = 6;
    /// It was measured and deliberately not applied, because this session was measuring the trim,
    /// and a trim is measured on the drivers' own figures with nothing moved underneath it.
    pub const MEASURED_ONLY: u32 = 7;

    /// Whether a code means the interface was lined up by a measurement.
    pub fn is_applied(code: u32) -> bool {
        code == APPLIED
    }

    /// Whether a code means a measurement was asked for and turned down.
    pub fn is_refused(code: u32) -> bool {
        matches!(code, NOT_HEARD | OFF_THE_GRID | TOO_FAR)
    }

    /// Whether a code means the measurement has come to something, whatever it came to: the
    /// moment a session has a line of the log to write about it.
    pub fn is_settled(code: u32) -> bool {
        is_applied(code) || is_refused(code) || matches!(code, NO_REFERENCE | MEASURED_ONLY)
    }

    /// The one word this state goes by. A code this version does not know reads as nothing having
    /// been set up, which is what a reader can say least wrongly about it.
    pub fn name(code: u32) -> &'static str {
        match code {
            MEASURING => "measuring",
            APPLIED => "applied",
            NOT_HEARD => "not_heard",
            OFF_THE_GRID => "off_the_grid",
            TOO_FAR => "too_far",
            NO_REFERENCE => "no_reference",
            MEASURED_ONLY => "measured_only",
            _ => "not_configured",
        }
    }
}

/// One sub-device, as the driver sees it now.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeviceStatus {
    /// How many times this device has called back in this session.
    pub callbacks: u64,
    /// Blocks thrown away because this device's ring was full.
    pub dropped: u64,
    /// Blocks that were not there when they were wanted, which is what a person hears as a click.
    pub starved: u64,
    /// **The number the person most wants**: this device's sample count minus the master's. Zero
    /// while the two are locked, and growing in one direction when they are not. It is in samples,
    /// and it is only ever meaningful while [`DeviceStatus::streaming`] is true on both.
    pub gap: i64,
    /// What the configuration file calls this device.
    pub name: Name,
    /// What its own driver calls itself.
    pub driver_name: Name,
    pub inputs: u32,
    pub outputs: u32,
    /// Whether this device is calling back at all.
    pub streaming: u32,
    /// Whether the driver has given up on it for the moment: its inputs read as silence and its
    /// outputs are muted until it comes back.
    pub stalled: u32,
    /// Whether this is the device whose callback drives the DAW.
    pub is_master: u32,
    /// What this device's own driver reports, plus the trim from the configuration file.
    pub latency_in: i32,
    pub latency_out: i32,
    /// Samples this device is held back by so that every device lines up.
    pub pad_in: i32,
    pub pad_out: i32,
    /// One of [`phase`]: what became of this interface's phase measurement this session.
    pub phase_state: u32,
    /// What the measurement came to, in samples, exactly as measured.
    /// Positive means this interface's capture arrived later than the reported figures said it
    /// would. Only worth reading when [`DeviceStatus::phase_state`] says something was measured.
    pub phase_measured: i32,
    /// What was actually added to this interface's input path because of it, in samples: what was
    /// measured minus the reference, so the interface is held back by the reference minus what was
    /// measured. Zero unless the state is [`phase::APPLIED`], because a measurement that is not
    /// used is never a correction of zero.
    pub phase_applied: i32,
    pub reserved: u32,
}

/// Everything the driver publishes. **Only the driver writes this.**
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DriverArea {
    /// When the current session started, on the machine's own clock in nanoseconds. Zero when
    /// there is no session.
    pub session_nanos: i64,
    /// When the last block went through, on the same clock. A figure that has stopped moving while
    /// [`DriverArea::streaming`] is true is a driver that has stopped.
    pub last_block_nanos: i64,
    /// Blocks the master has driven in this session.
    pub callbacks: u64,
    /// Samples handed to the DAW in this session.
    pub position: u64,
    /// The generation of the configuration the driver is actually running. When this matches
    /// [`ControlArea::generation`], what Gazelle asked for is what is in force.
    pub generation_in_force: u64,
    /// The generation that was refused, if one was. With [`DriverArea::refusal`] it says which
    /// request went wrong and why.
    pub refused_generation: u64,
    pub sample_rate: f64,
    /// Whether a DAW has the driver open with its buffers made.
    pub open: u32,
    /// Whether audio is running.
    pub streaming: u32,
    pub device_count: u32,
    /// Which device in [`DriverArea::devices`] drives the callback.
    pub master: u32,
    pub buffer_size: i32,
    /// One of [`alignment`].
    pub alignment: u32,
    /// The one figure each way the aggregate reports for the whole of itself.
    pub input_latency: i32,
    pub output_latency: i32,
    /// How many channels the DAW actually asked for.
    pub daw_inputs: u32,
    pub daw_outputs: u32,
    /// The last thing the driver refused to do, in the same words the DAW was given. Empty when
    /// nothing has been refused.
    pub refusal: Line,
    /// Where the configuration in force was read from.
    pub config_source: Line,
    pub devices: [DeviceStatus; MAX_DEVICES],
}

/// Everything Gazelle writes. **Only Gazelle writes this**, and the driver only ever reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControlSnapshot {
    /// Bumped by Gazelle after it has written the configuration file. The driver's watcher thread
    /// notices a number it has not seen and re-reads the file.
    pub generation: u64,
    /// When that request was made, on the machine's clock.
    pub requested_nanos: i64,
    pub reserved: [u64; 6],
}

/// The live form of [`ControlSnapshot`], which is what actually sits in the section.
#[repr(C)]
pub struct ControlArea {
    pub generation: AtomicU64,
    pub requested_nanos: AtomicI64,
    pub reserved: [u64; 6],
}

/// The whole of the shared section.
///
/// The header is written once, when the section is created, and never again. The sequence counter
/// and the gate are the only fields anything spins on.
#[repr(C)]
pub struct StatusRecord {
    /// [`MAGIC`], so that a reader that mapped the wrong thing knows at once.
    pub magic: u64,
    /// The seqlock. Odd while the driver is writing its area, even when it is settled.
    pub sequence: AtomicU64,
    pub format_version: u32,
    /// `size_of::<StatusRecord>()`, so a reader can refuse a record smaller than the one it knows
    /// even at a version it thinks it understands.
    pub record_size: u32,
    /// One word, taken by whichever of the driver's own threads is writing. The audio thread never
    /// waits for it.
    pub gate: AtomicU32,
    pub reserved: u32,
    pub control: ControlArea,
    pub driver: DriverArea,
}

/// How many bytes a section has to be.
pub const RECORD_BYTES: usize = std::mem::size_of::<StatusRecord>();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_is_the_one_written_down() {
        // A change to any of these is a change to what a Gazelle built yesterday will read out of
        // a driver built today. If one of these fails, the format version has to move with it.
        assert_eq!(std::mem::size_of::<Name>(), 40);
        assert_eq!(std::mem::size_of::<Line>(), 264);
        assert_eq!(std::mem::size_of::<DeviceStatus>(), 168);
        assert_eq!(std::mem::size_of::<DriverArea>(), 1968);
        assert_eq!(std::mem::size_of::<ControlArea>(), 64);
        assert_eq!(std::mem::size_of::<ControlArea>(), std::mem::size_of::<ControlSnapshot>());
        assert_eq!(std::mem::size_of::<StatusRecord>(), 2064);
        assert_eq!(std::mem::align_of::<StatusRecord>(), 8);
        assert_eq!(MAGIC.to_le_bytes(), *b"GZAGGST1");
    }

    #[test]
    fn a_name_longer_than_the_record_carries_is_cut_rather_than_lost() {
        let name = Name::from("a device with a very long name indeed, far past the room there is");
        assert_eq!(name.get().len(), NAME_BYTES);
        assert!(name.get().starts_with("a device with a very long name"));
    }

    #[test]
    fn a_cut_lands_on_a_letter_and_never_in_the_middle_of_one() {
        // Three bytes each, so the limit falls inside a character rather than between two.
        let mut name = Name::default();
        name.set(&"\u{20ac}".repeat(20));
        assert_eq!(name.get().chars().count(), 10, "ten whole characters and no half of an eleventh");
        assert_eq!(name.get().len(), 30);
    }

    #[test]
    fn setting_a_shorter_name_leaves_nothing_of_the_longer_one_behind() {
        let mut name = Name::from("Zen Studio Plus, the long way");
        name.set("Quadro");
        assert_eq!(name.get(), "Quadro");
        assert!(name.bytes[6..].iter().all(|&b| b == 0), "the tail of the old name is gone");
    }

    #[test]
    fn a_length_that_is_not_true_reads_as_nothing_rather_than_as_rubbish() {
        let mut name = Name::from("Quadro");
        name.len = 9_999;
        // Clamped to the array, so the read stays inside the record whatever the writer did.
        assert_eq!(name.get().len(), NAME_BYTES);
        let mut broken = Name::default();
        broken.bytes[0] = 0xFF;
        broken.len = 1;
        assert_eq!(broken.get(), "", "bytes that are not text are not guessed at");
    }

    #[test]
    fn a_phase_state_says_which_of_the_three_things_happened_and_an_unknown_one_says_nothing() {
        assert!(phase::is_applied(phase::APPLIED));
        assert!(!phase::is_applied(phase::MEASURING));
        // Every refusal is a refusal, and neither measuring nor having nothing set up is one.
        for code in [phase::NOT_HEARD, phase::OFF_THE_GRID, phase::TOO_FAR] {
            assert!(phase::is_refused(code), "{code}");
            assert!(!phase::is_applied(code));
        }
        assert!(!phase::is_refused(phase::MEASURING));
        assert!(!phase::is_refused(phase::NOT_CONFIGURED));
        assert_eq!(phase::name(phase::OFF_THE_GRID), "off_the_grid");
        // Measured with nothing to line up to, and measured while a trim was: neither is a
        // refusal and neither moved anything, and both have come to something.
        for code in [phase::NO_REFERENCE, phase::MEASURED_ONLY] {
            assert!(!phase::is_refused(code) && !phase::is_applied(code), "{code}");
            assert!(phase::is_settled(code), "{code}");
        }
        assert_eq!(phase::name(phase::NO_REFERENCE), "no_reference");
        assert_eq!(phase::name(phase::MEASURED_ONLY), "measured_only");
        assert!(!phase::is_settled(phase::MEASURING) && !phase::is_settled(phase::NOT_CONFIGURED));
        assert_eq!(phase::name(88), "not_configured", "a code from a later driver is not guessed at");
    }

    #[test]
    fn an_alignment_code_this_version_does_not_know_reads_as_the_default() {
        assert_eq!(alignment::name(alignment::ALIGNED), "aligned");
        assert_eq!(alignment::name(alignment::LOWEST_LATENCY), "lowest_latency");
        assert_eq!(alignment::name(77), "aligned");
    }
}
