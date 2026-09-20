//! Where the record actually lives, behind a trait, so that every rule in this crate is decided
//! against memory a test owns rather than against a real shared section.
//!
//! The real one is in [`crate::windows`]. There is nothing Windows specific above this line.

use std::sync::Arc;

/// A run of bytes two programs can both see. The only thing anything here needs of it is where it
/// starts and how long it is.
///
/// # Safety
///
/// An implementation promises that `base` points at `bytes` bytes that stay at that address, and stay
/// mapped, for as long as the implementation is alive, and that the address is aligned for
/// [`crate::record::StatusRecord`].
pub unsafe trait Mapping: Send + Sync {
    fn base(&self) -> *mut u8;
    fn bytes(&self) -> usize;
    /// Whether this mapping was created by us, rather than opened after someone else made it. Only
    /// the creator writes the header.
    fn created(&self) -> bool;
}

/// A mapping made of ordinary memory: what the tests use, and what a program with nowhere to put a
/// real section can fall back to without changing a line of the code above.
///
/// Cloning one shares the same bytes, which is how a test puts a publisher and a reader on two
/// ends of one record.
#[derive(Clone)]
pub struct Scratch {
    bytes: Arc<Aligned>,
    created: bool,
}

/// A buffer of `u64`, which is aligned for anything in the record.
struct Aligned {
    words: Box<[u64]>,
}

// The whole point of a shared section is that two threads look at it at once; the seqlock in
// `publish` and `read` is what makes that safe, and it is the same discipline here.
unsafe impl Send for Aligned {}
unsafe impl Sync for Aligned {}

impl Scratch {
    /// A record's worth of zeroed memory, as if we had just created the section.
    pub fn new() -> Scratch {
        Scratch::of(crate::record::RECORD_BYTES)
    }

    /// A run of exactly this many bytes, for a test that wants one too small to hold a record.
    pub fn of(bytes: usize) -> Scratch {
        let words = bytes.div_ceil(8);
        Scratch { bytes: Arc::new(Aligned { words: vec![0u64; words].into_boxed_slice() }), created: true }
    }

    /// The same bytes, seen as a reader who did not create them sees them.
    pub fn opened(&self) -> Scratch {
        Scratch { bytes: Arc::clone(&self.bytes), created: false }
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Scratch::new()
    }
}

unsafe impl Mapping for Scratch {
    fn base(&self) -> *mut u8 {
        self.bytes.words.as_ptr() as *mut u8
    }

    fn bytes(&self) -> usize {
        self.bytes.words.len() * 8
    }

    fn created(&self) -> bool {
        self.created
    }
}

/// Something to wait on and something to poke: the reload event, behind a trait for the same
/// reason.
pub trait Notifier: Send + Sync {
    /// Wake whoever is waiting. Gazelle calls this after it has bumped the generation.
    fn signal(&self);
}

/// How a wait ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    /// Somebody signalled.
    Signalled,
    /// Nobody did, and the time ran out. A watcher still looks around on one of these.
    TimedOut,
    /// The thing being waited on has gone. Stop waiting.
    Closed,
}

/// The waiting half of the same event.
pub trait Waiter: Send {
    fn wait(&self, millis: u32) -> Wake;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scratch_mapping_is_a_records_worth_of_aligned_zeroes() {
        let scratch = Scratch::new();
        assert!(scratch.bytes() >= crate::record::RECORD_BYTES);
        assert_eq!(scratch.base() as usize % std::mem::align_of::<crate::record::StatusRecord>(), 0);
        assert!(scratch.created(), "whoever made it writes the header");
        let bytes = unsafe { std::slice::from_raw_parts(scratch.base(), scratch.bytes()) };
        assert!(bytes.iter().all(|&b| b == 0));
    }

    #[test]
    fn opening_a_scratch_mapping_shares_its_bytes_without_claiming_to_have_made_them() {
        let made = Scratch::new();
        let opened = made.opened();
        assert_eq!(made.base(), opened.base());
        assert!(!opened.created());
    }
}
