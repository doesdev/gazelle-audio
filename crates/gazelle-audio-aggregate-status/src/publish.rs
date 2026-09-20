//! The writing half: the driver's side of the record.
//!
//! Once a [`Publisher`] exists, writing the record is a compare and exchange, a counter bump, some
//! plain stores and another counter bump. No allocation, no lock that can be held by anyone slow,
//! no syscall and no file. That is what makes it safe to do from an audio callback, which is the
//! whole reason the live state is here rather than in a file.

use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};

use crate::map::Mapping;
use crate::read::StatusError;
use crate::record::{DriverArea, StatusRecord, FORMAT_VERSION, MAGIC, RECORD_BYTES};

/// The gate's two states. It is not a mutex: the audio path tries it once and walks away.
const FREE: u32 = 0;
const TAKEN: u32 = 1;

/// The driver's end of the shared record.
pub struct Publisher {
    base: *mut u8,
    /// Kept so the mapping stays mapped for as long as anything can write through it. Nothing
    /// calls into it on the audio path: the pointer was taken once, here.
    _mapping: Box<dyn Mapping>,
}

// The record is shared on purpose, and the seqlock plus the gate are what make that safe.
unsafe impl Send for Publisher {}
unsafe impl Sync for Publisher {}

impl std::fmt::Debug for Publisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Publisher at {:p}", self.base)
    }
}

impl Publisher {
    /// Take over a mapping. Whoever created the section writes the header; whoever found one
    /// already there checks it and refuses if it is not a record of the shape this build knows.
    ///
    /// Either way the driver's own area is put back to nothing, because a record left behind by a
    /// driver that has exited describes a session that is over.
    pub fn map(mapping: Box<dyn Mapping>) -> Result<Publisher, StatusError> {
        if mapping.bytes() < RECORD_BYTES {
            return Err(StatusError::TooSmall { found: mapping.bytes(), wanted: RECORD_BYTES });
        }
        let base = mapping.base();
        if base.is_null() {
            return Err(StatusError::NotMapped);
        }
        let record = base.cast::<StatusRecord>();
        if mapping.created() {
            // Safety: we made this section, nobody else has it yet, and it is big enough.
            unsafe {
                std::ptr::write_bytes(base, 0, RECORD_BYTES);
                std::ptr::addr_of_mut!((*record).magic).write(MAGIC);
                std::ptr::addr_of_mut!((*record).format_version).write(FORMAT_VERSION);
                std::ptr::addr_of_mut!((*record).record_size).write(RECORD_BYTES as u32);
            }
        } else {
            let magic = unsafe { std::ptr::addr_of!((*record).magic).read_volatile() };
            if magic != MAGIC {
                return Err(StatusError::NotOurs { found: magic });
            }
            let version = unsafe { std::ptr::addr_of!((*record).format_version).read_volatile() };
            if version != FORMAT_VERSION {
                return Err(StatusError::Version { found: version, understood: FORMAT_VERSION });
            }
        }
        let publisher = Publisher { base, _mapping: mapping };
        publisher.update(|area| *area = DriverArea::default());
        Ok(publisher)
    }

    fn record(&self) -> *mut StatusRecord {
        self.base.cast()
    }

    fn sequence(&self) -> &AtomicU64 {
        // Safety: the section is at least a record long and stays mapped for our lifetime.
        unsafe { &*std::ptr::addr_of!((*self.record()).sequence) }
    }

    fn gate(&self) -> &AtomicU32 {
        unsafe { &*std::ptr::addr_of!((*self.record()).gate) }
    }

    /// The generation Gazelle last asked for. Read only: this is Gazelle's field.
    pub fn generation(&self) -> u64 {
        unsafe { (*std::ptr::addr_of!((*self.record()).control.generation)).load(Ordering::Acquire) }
    }

    /// When Gazelle last asked, on the machine's clock.
    pub fn requested_nanos(&self) -> i64 {
        unsafe { (*std::ptr::addr_of!((*self.record()).control.requested_nanos)).load(Ordering::Acquire) }
    }

    /// Write the driver's area, if the gate is free this instant. **This is the one the audio path
    /// calls**: it never waits, and a block whose update was skipped is simply written by the next
    /// one. Answers whether the write happened.
    pub fn try_update(&self, change: impl FnOnce(&mut DriverArea)) -> bool {
        if self.gate().compare_exchange(FREE, TAKEN, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return false;
        }
        self.write(change);
        self.gate().store(FREE, Ordering::Release);
        true
    }

    /// Write the driver's area, waiting for the gate. **Never call this from the audio path.** The
    /// only thing that can be holding the gate is one of our own threads, for the length of a few
    /// stores, so the wait is short, but it is a wait.
    pub fn update(&self, change: impl FnOnce(&mut DriverArea)) {
        let mut spins = 0u32;
        while self.gate().compare_exchange_weak(FREE, TAKEN, Ordering::Acquire, Ordering::Relaxed).is_err() {
            spins += 1;
            if spins < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
        self.write(change);
        self.gate().store(FREE, Ordering::Release);
    }

    /// The seqlock's writing half: odd, write, even. Only ever called with the gate held.
    fn write(&self, change: impl FnOnce(&mut DriverArea)) {
        self.sequence().fetch_add(1, Ordering::AcqRel);
        fence(Ordering::Release);
        // Safety: the gate is held, so this is the only writer, and a reader that catches us
        // half way through sees an odd sequence counter and reads again.
        let area = unsafe { &mut *std::ptr::addr_of_mut!((*self.record()).driver) };
        change(area);
        fence(Ordering::Release);
        self.sequence().fetch_add(1, Ordering::AcqRel);
    }

    /// What the driver's area says now, for the driver's own use. Reading its own record needs no
    /// seqlock: it is the writer.
    pub fn driver_area(&self) -> DriverArea {
        let mut copy = DriverArea::default();
        self.update(|area| copy = *area);
        copy
    }

    /// Where the record is, for a test that wants to look at the bytes.
    pub fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.base, RECORD_BYTES) }
    }

    /// The seqlock counter itself, so that a test can leave a write looking as though it never
    /// finished, which is what a driver killed part way through one looks like to a reader.
    #[cfg(test)]
    pub(crate) fn sequence_for_tests(&self) -> &AtomicU64 {
        self.sequence()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Scratch;
    use crate::read::Reader;
    use crate::record::{Line, Name};

    fn publisher() -> (Scratch, Publisher) {
        let scratch = Scratch::new();
        let publisher = Publisher::map(Box::new(scratch.clone())).expect("a fresh scratch mapping is a good section");
        (scratch, publisher)
    }

    #[test]
    fn creating_a_section_writes_the_header_once_and_leaves_the_counter_even() {
        let (scratch, publisher) = publisher();
        let record = scratch.base().cast::<StatusRecord>();
        assert_eq!(unsafe { (*record).magic }, MAGIC);
        assert_eq!(unsafe { (*record).format_version }, FORMAT_VERSION);
        assert_eq!(unsafe { (*record).record_size } as usize, RECORD_BYTES);
        assert_eq!(publisher.sequence().load(Ordering::Acquire) % 2, 0, "settled, so a reader may read");
    }

    #[test]
    fn a_mapping_too_small_to_hold_a_record_is_refused_rather_than_written_into() {
        let small = Scratch::of(64);
        let error = Publisher::map(Box::new(small.clone())).expect_err("64 bytes is not a record");
        assert!(matches!(error, StatusError::TooSmall { .. }), "{error:?}");
        let bytes = unsafe { std::slice::from_raw_parts(small.base(), small.bytes()) };
        assert!(bytes.iter().all(|&b| b == 0), "nothing was written into a section we refused");
    }

    #[test]
    fn a_section_somebody_else_made_of_another_version_is_refused_politely() {
        let scratch = Scratch::new();
        let record = scratch.base().cast::<StatusRecord>();
        unsafe {
            (*record).magic = MAGIC;
            (*record).format_version = FORMAT_VERSION + 1;
        }
        let error = Publisher::map(Box::new(scratch.opened())).expect_err("a later version is not ours to write");
        match error {
            StatusError::Version { found, understood } => {
                assert_eq!(found, FORMAT_VERSION + 1);
                assert_eq!(understood, FORMAT_VERSION);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_section_that_is_not_ours_at_all_is_refused() {
        let scratch = Scratch::new();
        unsafe { (*scratch.base().cast::<StatusRecord>()).magic = 1234 };
        let error = Publisher::map(Box::new(scratch.opened())).expect_err("that is somebody else's memory");
        assert!(matches!(error, StatusError::NotOurs { found: 1234 }), "{error:?}");
    }

    #[test]
    fn what_the_driver_writes_is_what_a_reader_reads() {
        let (scratch, publisher) = publisher();
        publisher.update(|area| {
            area.open = 1;
            area.streaming = 1;
            area.device_count = 2;
            area.master = 0;
            area.buffer_size = 512;
            area.sample_rate = 96_000.0;
            area.input_latency = 1148;
            area.output_latency = 1212;
            area.config_source = Line::from(r"C:\Users\someone\AppData\Roaming\gazelle\aggregate.json");
            area.devices[0].name = Name::from("Quadro");
            area.devices[0].is_master = 1;
            area.devices[1].name = Name::from("Studio+");
            area.devices[1].gap = -512;
        });
        let reader = Reader::map(Box::new(scratch.opened())).expect("the same section, opened");
        let seen = reader.read().expect("a settled record reads first time");
        assert_eq!(seen.driver.device_count, 2);
        assert_eq!(seen.driver.buffer_size, 512);
        assert_eq!(seen.driver.sample_rate, 96_000.0);
        assert_eq!(seen.driver.devices[0].name.get(), "Quadro");
        assert_eq!(seen.driver.devices[1].gap, -512);
        assert!(seen.driver.config_source.get().ends_with("aggregate.json"));
    }

    #[test]
    fn the_audio_path_walks_away_from_a_gate_it_cannot_have_rather_than_waiting() {
        let (_scratch, publisher) = publisher();
        publisher.update(|area| area.callbacks = 7);
        // Somebody else is in the middle of a write.
        publisher.gate().store(TAKEN, Ordering::Release);
        assert!(!publisher.try_update(|area| area.callbacks = 99), "it did not wait, and it did not write");
        publisher.gate().store(FREE, Ordering::Release);
        assert!(publisher.try_update(|area| area.callbacks = 99));
        assert_eq!(publisher.driver_area().callbacks, 99);
    }

    #[test]
    fn mapping_a_section_again_leaves_no_trace_of_the_session_that_ended() {
        let scratch = Scratch::new();
        {
            let publisher = Publisher::map(Box::new(scratch.clone())).expect("a good section");
            publisher.update(|area| {
                area.open = 1;
                area.callbacks = 4096;
                area.devices[0].name = Name::from("Quadro");
            });
        }
        // Gazelle is holding the section open, so the next driver finds it already there.
        let again = Publisher::map(Box::new(scratch.opened())).expect("the header is still ours");
        let area = again.driver_area();
        assert_eq!(area.open, 0, "a record left behind describes a session that is over");
        assert_eq!(area.callbacks, 0);
        assert_eq!(area.devices[0].name.get(), "");
    }

    #[test]
    fn gazelles_own_area_is_read_and_never_written_by_the_driver() {
        let (scratch, publisher) = publisher();
        let reader = Reader::map(Box::new(scratch.opened())).expect("the same section");
        assert_eq!(publisher.generation(), 0);
        let asked = reader.request(1_234);
        assert_eq!(asked, 1, "the first request is generation one");
        assert_eq!(publisher.generation(), 1);
        assert_eq!(publisher.requested_nanos(), 1_234);
        // The driver writing its own area does not disturb Gazelle's.
        publisher.update(|area| area.generation_in_force = 1);
        assert_eq!(publisher.generation(), 1);
        assert_eq!(reader.read().unwrap().control.generation, 1);
    }
}
