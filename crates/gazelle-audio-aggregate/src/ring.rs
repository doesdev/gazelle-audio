//! The one place audio crosses between two devices' callback threads.
//!
//! A ring of whole buffers, one producer and one consumer, and no lock anywhere: the producer only
//! ever moves the write index and the consumer only ever moves the read index, so neither can be
//! made to wait by the other. Both sides work in place, through a closure handed the slot itself,
//! so nothing is copied twice and nothing is allocated after [`Ring::new`].
//!
//! A full ring drops the newest buffer and a empty one reads as silence. Both are counted, because
//! either of them is a glitch a person can hear and the counts are what a status report shows.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub struct Ring {
    /// How many buffers the ring holds. One slot is always left empty, so the ring carries
    /// `slots - 1` buffers.
    slots: usize,
    /// How many samples one buffer holds: the device's selected channels times the block.
    len: usize,
    /// The samples themselves, taken out of a box so that neither side ever makes a reference to
    /// the whole of them, only to its own slot.
    data: *mut i32,
    read: AtomicUsize,
    write: AtomicUsize,
    /// Buffers the producer has published, ever. Only ever compared with zero, to tell a ring that
    /// has not started from one that is late.
    carried: AtomicU64,
    dropped: AtomicU64,
    starved: AtomicU64,
}

/// One thread writes and one thread reads, and the two indices they own are atomic. Nothing here
/// is shared any other way.
unsafe impl Sync for Ring {}
unsafe impl Send for Ring {}

impl Ring {
    /// A ring of `slots` buffers of `len` samples each, all silent.
    pub fn new(slots: usize, len: usize) -> Ring {
        let slots = slots.max(2);
        Ring {
            slots,
            len,
            data: Box::into_raw(vec![0i32; slots * len].into_boxed_slice()).cast(),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            carried: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            starved: AtomicU64::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.read.load(Ordering::Acquire) == self.write.load(Ordering::Acquire)
    }

    /// How many buffers are waiting to be read.
    pub fn waiting(&self) -> usize {
        let write = self.write.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Acquire);
        (write + self.slots - read) % self.slots
    }

    /// Buffers the producer had to throw away because the consumer was behind.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Buffers the consumer asked for and did not get, each of which was played as silence.
    pub fn starved(&self) -> u64 {
        self.starved.load(Ordering::Relaxed)
    }

    /// Fill the next slot and publish it. False when the ring was full, in which case `fill` was
    /// never called and nothing was written.
    ///
    /// Only one thread may call this.
    pub fn push_with(&self, fill: impl FnOnce(&mut [i32])) -> bool {
        let write = self.write.load(Ordering::Relaxed);
        let next = (write + 1) % self.slots;
        if next == self.read.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        // Safety: the consumer never touches the slot at the write index, because it stops at the
        // index this thread last published.
        let slot = unsafe { std::slice::from_raw_parts_mut(self.data.add(write * self.len), self.len) };
        fill(slot);
        self.write.store(next, Ordering::Release);
        self.carried.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Read the oldest slot and free it. False when the ring was empty, in which case `take` was
    /// never called and the caller must write silence itself.
    ///
    /// Only one thread may call this.
    pub fn pop_with(&self, take: impl FnOnce(&[i32])) -> bool {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) {
            // A ring the producer has not put anything in yet is a stream that has not begun, not
            // one that is late. The devices that follow are started before the one that drives the
            // callback, on purpose, so their first few callbacks ask for blocks that nobody was
            // ever going to have made: counting those would put a miss on the record of every
            // clean session and leave the count worth nothing.
            if self.carried.load(Ordering::Relaxed) > 0 {
                self.starved.fetch_add(1, Ordering::Relaxed);
            }
            return false;
        }
        // Safety: the producer never touches a slot it has published until the read index has
        // passed it, and this thread is the only one that moves the read index.
        let slot = unsafe { std::slice::from_raw_parts(self.data.add(read * self.len), self.len) };
        take(slot);
        self.read.store((read + 1) % self.slots, Ordering::Release);
        true
    }

    /// Throw away everything waiting. Only the consumer may call this: it is how a device that
    /// went away is stopped from playing what it missed when it comes back.
    pub fn drain(&self) {
        self.read.store(self.write.load(Ordering::Acquire), Ordering::Release);
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        // Nothing can be reading or writing by now: the devices were stopped before this.
        let whole = std::ptr::slice_from_raw_parts_mut(self.data, self.slots * self.len);
        drop(unsafe { Box::from_raw(whole) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(ring: &Ring, value: i32) -> bool {
        ring.push_with(|slot| slot.fill(value))
    }

    fn pop(ring: &Ring) -> Option<i32> {
        let mut seen = None;
        ring.pop_with(|slot| seen = Some(slot[0]));
        seen
    }

    #[test]
    fn a_buffer_comes_out_the_way_it_went_in() {
        let ring = Ring::new(4, 8);
        assert!(ring.is_empty());
        assert!(push(&ring, 7));
        assert_eq!(ring.waiting(), 1);
        assert_eq!(pop(&ring), Some(7));
        assert!(ring.is_empty());
    }

    #[test]
    fn the_indices_wrap_round_without_losing_order() {
        let ring = Ring::new(3, 4);
        // Three slots carry two buffers, so this goes round the end several times.
        for round in 0..20i32 {
            assert!(push(&ring, round), "round {round}");
            assert_eq!(pop(&ring), Some(round));
        }
        assert_eq!(ring.dropped(), 0);
        assert_eq!(ring.starved(), 0);
    }

    #[test]
    fn a_full_ring_drops_the_newest_buffer_and_counts_it() {
        let ring = Ring::new(3, 2);
        assert!(push(&ring, 1));
        assert!(push(&ring, 2));
        assert!(!push(&ring, 3), "a ring of three slots carries two buffers");
        assert_eq!(ring.dropped(), 1);
        // What was already in it is untouched, and in order.
        assert_eq!(pop(&ring), Some(1));
        assert_eq!(pop(&ring), Some(2));
        assert_eq!(pop(&ring), None);
        assert_eq!(ring.starved(), 1);
    }

    #[test]
    fn an_empty_ring_never_hands_out_a_stale_buffer() {
        let ring = Ring::new(4, 2);
        assert!(push(&ring, 9));
        assert_eq!(pop(&ring), Some(9));
        // The samples are still in the slot, but the reader is told no rather than shown them.
        let mut touched = false;
        assert!(!ring.pop_with(|_| touched = true));
        assert!(!touched, "the closure must not run when there is nothing to read");
    }

    #[test]
    fn a_ring_nobody_has_filled_yet_is_not_a_device_that_is_late() {
        // The devices that follow start before the one that drives the callback, so they ask for
        // blocks before there are any. That is the stream beginning, not a click.
        let ring = Ring::new(4, 2);
        assert_eq!(pop(&ring), None);
        assert_eq!(pop(&ring), None);
        assert_eq!(ring.starved(), 0, "nothing was late, because nothing had been made yet");
        // Once a block has come through, an empty ring is a block that did not arrive in time.
        push(&ring, 1);
        assert_eq!(pop(&ring), Some(1));
        assert_eq!(pop(&ring), None);
        assert_eq!(ring.starved(), 1);
    }

    #[test]
    fn draining_throws_away_what_a_device_missed() {
        let ring = Ring::new(5, 2);
        push(&ring, 1);
        push(&ring, 2);
        assert_eq!(ring.waiting(), 2);
        ring.drain();
        assert_eq!(ring.waiting(), 0);
        assert_eq!(pop(&ring), None);
        // And it works again afterwards.
        push(&ring, 3);
        assert_eq!(pop(&ring), Some(3));
    }

    #[test]
    fn two_real_threads_keep_every_buffer_in_order() {
        use std::sync::Arc;
        let ring = Arc::new(Ring::new(4, 16));
        let producer = {
            let ring = Arc::clone(&ring);
            std::thread::spawn(move || {
                let mut sent = 0i32;
                while sent < 5_000 {
                    if ring.push_with(|slot| slot.fill(sent)) {
                        sent += 1;
                    }
                    std::hint::spin_loop();
                }
            })
        };
        let mut expected = 0i32;
        while expected < 5_000 {
            let mut got = None;
            if ring.pop_with(|slot| got = Some((slot[0], slot[15]))) {
                let (first, last) = got.expect("the closure ran");
                assert_eq!(first, expected, "buffers arrived out of order");
                assert_eq!(last, expected, "a buffer was read while it was half written");
                expected += 1;
            }
            std::hint::spin_loop();
        }
        producer.join().expect("the producer finished");
    }
}
