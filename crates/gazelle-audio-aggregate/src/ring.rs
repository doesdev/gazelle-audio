//! The one place audio crosses between two devices' callback threads.
//!
//! A ring of whole buffers, one producer and one consumer, and no lock anywhere: the producer only
//! ever moves the write index and the consumer only ever moves the read index, so neither can be
//! made to wait by the other. Both sides work in place, through a closure handed the slot itself,
//! so nothing is copied twice and nothing is allocated after [`Ring::new`].
//!
//! A full ring drops the newest buffer and an empty one reads as silence. Both are counted once the
//! ring has settled, because either of them is then a glitch a person can hear and the counts are
//! what a status report shows. What "settled" means, and why the first few buffers are outside it,
//! is at [`SETTLING_BUFFERS`].

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// How many buffers the producer has to have published before an empty ring counts against it.
///
/// **Why the start of a stream is different.** The two devices' callbacks are woken by two
/// converters, and where the consumer's lands inside the producer's block is not settled when a
/// stream starts. A consumer that lands just before its producer finds nothing waiting and takes a
/// block of silence, and that is what moves it: from then on it reads the block the producer
/// published in the previous callback, a whole buffer in hand, and it keeps that hand for the rest
/// of the session. **The silence is the two of them arriving at that arrangement**, and the buffer
/// in hand is exactly the one buffer a device that crosses a ring is given in the plan's
/// arithmetic. Nothing is missing from the recording: nobody is recording a few milliseconds into
/// a session, and what follows the silence is every sample the device captured.
///
/// **Why a silence later is a loss.** Once they are settled, the consumer has that whole buffer of
/// room, and an empty ring means the producer did not make its block in time to fill it. That is a
/// block of silence dropped into the middle of a take, and it costs the device another buffer of
/// path for the rest of the session on top. It is counted, reported and left as the silence it is.
///
/// **Why this many.** Measured against both interfaces on 2026-09-20, at 256 samples: a follower
/// took one or two of these in the instant a session started, one in a five second run, none in a
/// thirteen second run, one in a twenty five second one. It does not grow with the length of a
/// session because it is not a rate: it happens once, while the callbacks find each other. Eight
/// buffers is several times what was ever seen and still only a few milliseconds.
const SETTLING_BUFFERS: u64 = 8;

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
    /// Buffers the producer has published, ever. Compared with `filling_from` to tell a ring that
    /// is still settling from one that is late.
    carried: AtomicU64,
    /// What `carried` stood at when this ring last began filling: zero, or whatever it had carried
    /// when a drain threw its contents away. The consumer owns it, and a drain is the only thing
    /// that moves it, because a drained ring is one whose two ends have to find each other again.
    filling_from: AtomicU64,
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
            filling_from: AtomicU64::new(0),
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

    /// Buffers the consumer asked for and did not get, each of which was played as silence. Only
    /// the ones a settled ring lost: see [`SETTLING_BUFFERS`].
    pub fn starved(&self) -> u64 {
        self.starved.load(Ordering::Relaxed)
    }

    /// Whether this ring's two ends have found each other yet, which is what decides whether an
    /// empty one is a block that went missing or the stream still starting.
    pub fn is_settled(&self) -> bool {
        self.carried.load(Ordering::Relaxed) >= self.filling_from.load(Ordering::Relaxed).saturating_add(SETTLING_BUFFERS)
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
            // Silence either way, and whether it is a loss depends on whether this ring had
            // settled. A ring that is still settling has two ends that have not found each other
            // yet, and the block of silence is what moves them into the arrangement they keep for
            // the rest of the session: nothing was made too late, because the producer was never
            // going to have made it by then. A settled ring's consumer has a whole buffer of room,
            // so an empty one is a block that was not ready in time, which is a click a person can
            // hear. Counting the first kind would put a loss on the record of every clean session
            // and leave the count worth nothing; not counting the second would hide the only fault
            // this driver has to report.
            if self.is_settled() {
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
        // Whatever comes next is this ring starting over, and its two ends have to find each other
        // again exactly as they did when the stream began. A device coming back from a stall is
        // already reported as what it is; counting the blocks it takes to get going again would be
        // reporting the same fault a second time under another name.
        self.filling_from.store(self.carried.load(Ordering::Relaxed), Ordering::Relaxed);
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

    /// Run the ring through its settling, one buffer in and the same one out, so that a test of
    /// what a running session does starts where a running session is.
    fn settle(ring: &Ring) {
        for value in 0..SETTLING_BUFFERS as i32 {
            assert!(push(ring, value), "the ring holds one at a time here");
            assert_eq!(pop(ring), Some(value));
        }
        assert!(ring.is_settled());
        assert_eq!(ring.starved(), 0, "nothing was lost getting here");
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
    fn a_session_that_starts_with_the_follower_behind_has_lost_nothing() {
        // What a session looks like at the hardware: the consumer is there first and finds
        // nothing, then the two of them come in and out of step for a few blocks while they find
        // each other, and then they stay found. Not one sample of any of it is a block that went
        // missing, and a session that logged a loss for it would be every clean session.
        let ring = Ring::new(4, 2);
        assert_eq!(pop(&ring), None, "the consumer arrived before the producer had started");
        assert_eq!(pop(&ring), None);
        for value in 0..SETTLING_BUFFERS as i32 - 1 {
            assert!(push(&ring, value));
            assert_eq!(pop(&ring), Some(value));
            // And the consumer coming round again before the producer's next block.
            assert_eq!(pop(&ring), None);
        }
        assert_eq!(ring.starved(), 0, "none of that was a block that went missing");
        assert_eq!(ring.dropped(), 0);
        assert!(!ring.is_settled());
        // The block that settles it. From here the consumer has a whole buffer of room, so the
        // next empty ring is the producer failing to fill it and is counted.
        assert!(push(&ring, 100));
        assert_eq!(pop(&ring), Some(100));
        assert!(ring.is_settled());
        assert_eq!(pop(&ring), None);
        assert_eq!(ring.starved(), 1);
    }

    #[test]
    fn a_block_that_goes_missing_once_the_ring_has_settled_is_counted_and_read_as_silence() {
        let ring = Ring::new(4, 2);
        settle(&ring);
        // The producer does not make this one in time. The consumer is told no, so that it writes
        // silence itself, and it is never shown the stale samples still sitting in the slot.
        let mut touched = false;
        assert!(!ring.pop_with(|_| touched = true), "there is nothing to hand over");
        assert!(!touched, "and nothing stale is handed over in its place");
        assert_eq!(ring.starved(), 1, "a block that was not ready in time is a block a person hears");
        // And the ring carries on: the next block through is the next block, in order.
        assert!(push(&ring, 99));
        assert_eq!(pop(&ring), Some(99));
        assert_eq!(ring.starved(), 1, "one block missing is one block missing, counted once");
        // Every one after it is counted too: a settled ring never stops answering for itself.
        assert_eq!(pop(&ring), None);
        assert_eq!(pop(&ring), None);
        assert_eq!(ring.starved(), 3);
    }

    #[test]
    fn a_producer_that_never_produces_at_all_is_a_stall_and_not_a_run_of_losses() {
        // A device that has stopped calling back is found by the master watching its callbacks,
        // and it is reported as gone. Counting a loss for every block of a device that is not
        // there would bury the one line that says what actually happened.
        let ring = Ring::new(4, 2);
        for _ in 0..500 {
            assert_eq!(pop(&ring), None);
        }
        assert_eq!(ring.starved(), 0);
        assert!(!ring.is_settled(), "a ring nothing was ever put in never settles");
    }

    #[test]
    fn a_ring_that_was_thrown_away_settles_again_rather_than_counting_the_refill() {
        let ring = Ring::new(4, 2);
        settle(&ring);
        push(&ring, 1);
        ring.drain();
        assert!(!ring.is_settled(), "what a device missed is gone, and it starts again from here");
        assert_eq!(pop(&ring), None);
        assert_eq!(ring.starved(), 0, "the blocks a recovering device takes to get going are its stall, not losses");
        settle(&ring);
        assert_eq!(pop(&ring), None);
        assert_eq!(ring.starved(), 1, "and once it is going again it answers for itself as before");
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
