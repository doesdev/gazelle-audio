//! The reading half: Gazelle's side of the record.
//!
//! A reader never blocks the driver and the driver never waits for a reader. All a read does is
//! take the sequence counter, copy the driver's area out, take the counter again, and keep what it
//! copied only if the counter was settled and did not move. Anything else is another try.

use std::sync::atomic::{fence, AtomicI64, AtomicU64, Ordering};

use crate::map::Mapping;
use crate::record::{ControlSnapshot, DriverArea, StatusRecord, FORMAT_VERSION, MAGIC, RECORD_BYTES};

/// How many times a read tries before it says the writer is busy. A write is a few hundred plain
/// stores, so in practice the first or second attempt always wins; this number exists so that a
/// reader facing a driver that died mid-write gives an answer rather than spinning for ever.
pub const ATTEMPTS: u32 = 64;

/// Why a record could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusError {
    /// There is nothing mapped: the driver is not running, or never published.
    NotMapped,
    /// Something is there, but it does not start with our magic number.
    NotOurs { found: u64 },
    /// It is ours, and it is a version this build does not know the shape of. This is the polite
    /// refusal: an older Gazelle meeting a newer driver says so rather than reading rubbish.
    Version { found: u32, understood: u32 },
    /// The section is smaller than a record, so the rest of it is not there to read.
    TooSmall { found: usize, wanted: usize },
    /// The writer was in the middle of a write every time we looked.
    Busy,
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatusError::NotMapped => write!(f, "the aggregate driver is not publishing anything"),
            StatusError::NotOurs { found } => {
                write!(f, "that shared section is not the aggregate driver's (it starts {found:#018x})")
            }
            StatusError::Version { found, understood } => write!(
                f,
                "the aggregate driver publishes format version {found} and this build reads version {understood}"
            ),
            StatusError::TooSmall { found, wanted } => {
                write!(f, "the shared section is {found} bytes and a record is {wanted}")
            }
            StatusError::Busy => write!(f, "the aggregate driver was writing its status every time it was read"),
        }
    }
}

impl std::error::Error for StatusError {}

/// One consistent look at the whole record.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// What the driver published.
    pub driver: DriverArea,
    /// What Gazelle last asked for, read back so that a caller can see whether the driver has
    /// caught up: [`DriverArea::generation_in_force`] equal to this generation means it has.
    pub control: ControlSnapshot,
}

impl Snapshot {
    /// Whether the driver is running the configuration Gazelle last asked for.
    pub fn is_up_to_date(&self) -> bool {
        self.driver.generation_in_force == self.control.generation
    }

    /// The devices the record actually describes, rather than the whole fixed array.
    pub fn devices(&self) -> &[crate::record::DeviceStatus] {
        let count = (self.driver.device_count as usize).min(crate::record::MAX_DEVICES);
        &self.driver.devices[..count]
    }
}

/// Gazelle's end of the shared record.
pub struct Reader {
    base: *mut u8,
    _mapping: Box<dyn Mapping>,
}

// Shared on purpose; the seqlock is what makes it safe.
unsafe impl Send for Reader {}
unsafe impl Sync for Reader {}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reader at {:p}", self.base)
    }
}

impl Reader {
    /// Check a mapping is a record of a shape this build knows, and take it.
    pub fn map(mapping: Box<dyn Mapping>) -> Result<Reader, StatusError> {
        if mapping.base().is_null() {
            return Err(StatusError::NotMapped);
        }
        if mapping.bytes() < RECORD_BYTES {
            return Err(StatusError::TooSmall { found: mapping.bytes(), wanted: RECORD_BYTES });
        }
        let base = mapping.base();
        let reader = Reader { base, _mapping: mapping };
        reader.check_header()?;
        Ok(reader)
    }

    fn record(&self) -> *mut StatusRecord {
        self.base.cast()
    }

    fn check_header(&self) -> Result<(), StatusError> {
        let record = self.record();
        // Safety: the mapping is at least a record long and stays mapped for our lifetime.
        let magic = unsafe { std::ptr::addr_of!((*record).magic).read_volatile() };
        if magic != MAGIC {
            return Err(StatusError::NotOurs { found: magic });
        }
        let version = unsafe { std::ptr::addr_of!((*record).format_version).read_volatile() };
        if version != FORMAT_VERSION {
            return Err(StatusError::Version { found: version, understood: FORMAT_VERSION });
        }
        let size = unsafe { std::ptr::addr_of!((*record).record_size).read_volatile() } as usize;
        if size < RECORD_BYTES {
            return Err(StatusError::TooSmall { found: size, wanted: RECORD_BYTES });
        }
        Ok(())
    }

    fn sequence(&self) -> &AtomicU64 {
        unsafe { &*std::ptr::addr_of!((*self.record()).sequence) }
    }

    fn generation_field(&self) -> &AtomicU64 {
        unsafe { &*std::ptr::addr_of!((*self.record()).control.generation) }
    }

    fn requested_field(&self) -> &AtomicI64 {
        unsafe { &*std::ptr::addr_of!((*self.record()).control.requested_nanos) }
    }

    /// One consistent look at the record, or why there is not one.
    pub fn read(&self) -> Result<Snapshot, StatusError> {
        self.read_watching(|_| {})
    }

    /// The same read, with something happening in the middle of each attempt. This exists so that
    /// a test can be the writer that tears a read, which is the one thing about a seqlock that has
    /// to be proven rather than reasoned about.
    pub fn read_watching(&self, mut between: impl FnMut(u32)) -> Result<Snapshot, StatusError> {
        self.check_header()?;
        for attempt in 0..ATTEMPTS {
            let before = self.sequence().load(Ordering::Acquire);
            if !before.is_multiple_of(2) {
                // A write is in flight. Nothing copied now could be trusted.
                between(attempt);
                continue;
            }
            fence(Ordering::Acquire);
            let driver = self.copy_driver_area();
            fence(Ordering::Acquire);
            between(attempt);
            let after = self.sequence().load(Ordering::Acquire);
            if before == after {
                let control = ControlSnapshot {
                    generation: self.generation_field().load(Ordering::Acquire),
                    requested_nanos: self.requested_field().load(Ordering::Acquire),
                    reserved: [0; 6],
                };
                return Ok(Snapshot { driver, control });
            }
        }
        Err(StatusError::Busy)
    }

    /// Copy the driver's area out whether or not it is being written. Every field of it is an
    /// integer, a float or a byte, so every bit pattern is a value of its type; whether the value
    /// means anything is what the sequence counter decides, afterwards.
    fn copy_driver_area(&self) -> DriverArea {
        let mut copy = DriverArea::default();
        // Safety: both are a DriverArea's worth of bytes, and they do not overlap.
        unsafe {
            std::ptr::copy_nonoverlapping(
                std::ptr::addr_of!((*self.record()).driver).cast::<u8>(),
                (&mut copy as *mut DriverArea).cast::<u8>(),
                std::mem::size_of::<DriverArea>(),
            );
        }
        copy
    }

    /// **Ask the driver to read its configuration again.** Gazelle writes the configuration file
    /// first, then calls this, then signals the reload event. Answers the generation the driver is
    /// now expected to reach.
    pub fn request(&self, nanos: i64) -> u64 {
        self.requested_field().store(nanos, Ordering::Release);
        self.generation_field().fetch_add(1, Ordering::AcqRel) + 1
    }

    /// What Gazelle has asked for, without a whole read.
    pub fn generation(&self) -> u64 {
        self.generation_field().load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Scratch;
    use crate::publish::Publisher;
    use crate::record::Name;

    fn pair() -> (Publisher, Reader) {
        let scratch = Scratch::new();
        let publisher = Publisher::map(Box::new(scratch.clone())).expect("a good section");
        let reader = Reader::map(Box::new(scratch.opened())).expect("the same one");
        (publisher, reader)
    }

    #[test]
    fn a_settled_record_reads_on_the_first_attempt() {
        let (publisher, reader) = pair();
        publisher.update(|area| area.callbacks = 12);
        let mut attempts = 0;
        let seen = reader.read_watching(|_| attempts += 1).expect("nothing is writing");
        assert_eq!(seen.driver.callbacks, 12);
        assert_eq!(attempts, 1, "one look was enough");
    }

    #[test]
    fn a_reader_that_catches_a_torn_write_tries_again_and_gets_a_whole_one() {
        let (publisher, reader) = pair();
        publisher.update(|area| {
            area.callbacks = 100;
            area.position = 100 * 512;
            area.devices[0].name = Name::from("before");
        });
        // The first attempt is torn on purpose: the record changes underneath the copy, between
        // the two readings of the sequence counter. The second is left alone.
        let mut torn = 0;
        let seen = reader
            .read_watching(|attempt| {
                if attempt == 0 {
                    torn += 1;
                    publisher.update(|area| {
                        area.callbacks = 200;
                        area.position = 200 * 512;
                        area.devices[0].name = Name::from("after");
                    });
                }
            })
            .expect("the second attempt is clean");
        assert_eq!(torn, 1, "the test tore exactly one attempt");
        // Not a mixture of the two: the counters and the name are all from the same write.
        assert_eq!(seen.driver.callbacks, 200);
        assert_eq!(seen.driver.position, 200 * 512);
        assert_eq!(seen.driver.devices[0].name.get(), "after");
    }

    #[test]
    fn a_writer_that_never_finishes_is_given_up_on_rather_than_half_read() {
        let (publisher, reader) = pair();
        publisher.update(|area| area.callbacks = 5);
        // A write that started and never ended leaves the counter odd, which is exactly what a
        // driver killed mid-write looks like.
        let mut attempts = 0;
        let error = reader
            .read_watching(|_| {
                attempts += 1;
                if attempts == 1 {
                    publisher.sequence_for_tests().fetch_add(1, Ordering::AcqRel);
                }
            })
            .expect_err("there is nothing consistent to read");
        assert_eq!(error, StatusError::Busy);
        assert_eq!(attempts, ATTEMPTS, "it tried, a bounded number of times, and stopped");
    }

    #[test]
    fn a_reader_of_a_version_it_does_not_know_refuses_rather_than_reading_rubbish() {
        let scratch = Scratch::new();
        let _publisher = Publisher::map(Box::new(scratch.clone())).expect("a good section");
        let reader = Reader::map(Box::new(scratch.opened())).expect("the same one");
        assert!(reader.read().is_ok());
        // A driver built later republishes the same section in a shape we do not know.
        unsafe { (*scratch.base().cast::<StatusRecord>()).format_version = FORMAT_VERSION + 3 };
        let error = reader.read().expect_err("not a record this build knows");
        assert_eq!(error, StatusError::Version { found: FORMAT_VERSION + 3, understood: FORMAT_VERSION });
        assert!(error.to_string().contains("format version"), "{error}");
    }

    #[test]
    fn nothing_mapped_is_an_answer_of_its_own() {
        let error = Reader::map(Box::new(Scratch::of(16))).expect_err("that is not a record");
        assert!(matches!(error, StatusError::TooSmall { .. }), "{error:?}");
        assert!(error.to_string().contains("shared section"), "{error}");
    }

    #[test]
    fn a_section_nobody_has_published_into_is_not_ours() {
        let scratch = Scratch::new();
        let error = Reader::map(Box::new(scratch.opened())).expect_err("all zeroes is not our magic number");
        assert_eq!(error, StatusError::NotOurs { found: 0 });
    }

    #[test]
    fn a_snapshot_says_whether_the_driver_has_caught_up() {
        let (publisher, reader) = pair();
        assert!(reader.read().unwrap().is_up_to_date(), "nobody has asked for anything");
        let asked = reader.request(9);
        assert!(!reader.read().unwrap().is_up_to_date(), "asked for, not yet adopted");
        publisher.update(|area| area.generation_in_force = asked);
        assert!(reader.read().unwrap().is_up_to_date());
    }

    #[test]
    fn a_snapshot_shows_only_the_devices_the_record_describes() {
        let (publisher, reader) = pair();
        publisher.update(|area| {
            area.device_count = 2;
            area.devices[0].name = Name::from("Quadro");
            area.devices[1].name = Name::from("Studio+");
        });
        let seen = reader.read().unwrap();
        assert_eq!(seen.devices().len(), 2);
        assert_eq!(seen.devices()[1].name.get(), "Studio+");
        // A count out of its senses cannot walk a reader off the end of the array.
        publisher.update(|area| area.device_count = 900);
        assert_eq!(reader.read().unwrap().devices().len(), crate::record::MAX_DEVICES);
    }
}
