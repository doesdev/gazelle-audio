//! **The capture ring**: where the audio callback puts every block, the pre-roll lives while armed,
//! and the writer drains a take from.
//!
//! # One ring, block-planar
//!
//! The ring is one allocation of slots, each slot one callback's block, and within a slot each
//! channel's run of samples one after another, exactly as the aggregate hands them over. That is
//! chosen over one ring per channel, and over interleaved frames, because:
//!
//! - **one position counts for every channel.** A sample position names the same instant on every
//!   channel by construction, so the channels of a take cannot drift apart or be cut at different
//!   places, and the callback publishes one number a block rather than one per channel;
//! - **the callback copies, and does nothing else.** Each channel is one `memcpy` of a contiguous run
//!   into a contiguous run: no interleaving on the audio thread;
//! - **the writer reads what it writes.** Each file gets a contiguous run per block.
//!
//! # Who writes what
//!
//! There is one writer of the audio, the callback, and one reader, the writer thread. Everything they
//! share is an atomic, and the rules below are what make reading a slot safe:
//!
//! - `written` is the callback's: the position of the next block. Published after the block.
//! - Each slot's **tag** is the position of the block it holds, stored after its samples are. A
//!   reader that finds a tag that is not the position it wants knows that block never arrived.
//! - `read` is the reader's: the next position it will take. [`PARKED`] when there is no take. The
//!   callback sets it once, at the start of a take, and only while it is parked.
//! - `keep_until` is the callback's: while a take is running nothing at or after `read` may be
//!   overwritten; once it is stopping, only what is before its end.
//!
//! # The pre-roll and a take
//!
//! While armed and not recording the callback simply goes round, overwriting the oldest block, and
//! the ring holds the last [`Capture::preroll_frames`] of audio. **Record** asks the callback for a
//! take; at the start of its next block it decides where the take begins: the oldest block still
//! held, but never before the end of the previous take (audio already in a file is not put in the
//! next one, so back-to-back takes tile the timeline with no gap and no overlap). From then on the
//! blocks from that position are protected, so the pre-roll and the live audio are one run of
//! positions and there is nothing between them. **Stop** is the same, the other way: the take ends
//! at the start of the next block, and the reader drains to there.
//!
//! # When the writer falls behind
//!
//! A block that would overwrite audio the writer has not taken yet is **not written**: what is
//! waiting is never lost to make room. The new block is the one lost, it is counted
//! ([`Snapshot::overruns`]) with its position, and its slot keeps its old tag, so when the writer
//! gets there it sees exactly which block never arrived, writes silence of the same length (the
//! take stays lined up with the clock and the other files) and says where in the take it was.
//!
//! # The audio thread
//!
//! [`Capture::push_with`] allocates nothing, takes no lock, makes no call into the system and never
//! waits. The memory it writes into was allocated, committed and touched page by page at Arm.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// `read` while there is no take, and `keep_until` while one is running.
pub const PARKED: u64 = u64::MAX;
/// A slot's tag before anything was written into it.
const EMPTY: u64 = u64::MAX;
/// Bytes in a page, for touching every one of them.
const PAGE: usize = 4096;

/// What a take is, once the callback has started it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// Its first sample, as a position in the stream.
    pub start: u64,
    /// Where the stream was when Record took effect: everything before this is pre-roll. After a
    /// count-in, where it was when Record was pressed.
    pub pressed_at: u64,
    /// The downbeat after a count-in, when the take started from one: a position in the stream.
    pub cue: Option<u64>,
}

/// What the ring says about itself, read off the audio thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Frames the callback has handed over since Arm.
    pub written: u64,
    /// Whether the callback has a take running.
    pub recording: bool,
    /// The take running, or the last one.
    pub window: Option<Window>,
    /// How much pre-roll is held now, in frames: while armed, what Record would reach back to; while
    /// recording, what the take started with.
    pub held_frames: u64,
    /// Blocks lost because the writer was behind, since Arm.
    pub overruns: u64,
    /// The position of the last of them.
    pub last_overrun_at: Option<u64>,
}

/// The ring. Made at Arm, shared by the callback and the writer, dropped at Disarm.
pub struct Capture {
    channels: usize,
    block: usize,
    slots: usize,
    capacity: u64,
    preroll: u64,
    data: *mut i32,
    len: usize,
    tags: Box<[AtomicU64]>,
    written: AtomicU64,
    read: AtomicU64,
    keep_until: AtomicU64,
    /// The earliest position a take may start at: the end of the previous one.
    floor: AtomicU64,
    /// What the person asked for: a take or none. Read by the callback each block.
    want: AtomicBool,
    /// What the callback is doing: a take or none.
    recording: AtomicBool,
    take_start: AtomicU64,
    pressed_at: AtomicU64,
    /// A take the callback starts after a count-in: when Record was pressed, and the downbeat. Taken
    /// by the take that starts next.
    pending_pressed: AtomicU64,
    pending_cue: AtomicU64,
    take_cue: AtomicU64,
    started_any: AtomicBool,
    overruns: AtomicU64,
    last_overrun: AtomicU64,
    /// Per channel, the loudest sample since the last look, as a magnitude.
    peaks: Box<[AtomicU32]>,
}

// The samples behind `data` are shared by exactly the protocol described above: the callback writes
// a slot only while no reader can want it, and a reader reads a slot only while the callback may
// not write it. Everything else is atomic.
unsafe impl Send for Capture {}
unsafe impl Sync for Capture {}

impl Drop for Capture {
    fn drop(&mut self) {
        // Safety: made by `Box::into_raw` of a slice this long, and dropped once.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(self.data, self.len)) });
    }
}

impl Capture {
    /// Reserve, commit and touch the whole ring now, so that nothing about it can fail, or wait on
    /// the memory manager, once the audio is running. An allocation that fails is a refusal to arm,
    /// in words.
    ///
    /// `capacity_frames` and `preroll_frames` are whole blocks; the pre-roll leaves at least one
    /// block of the capacity free (`crate::sizing` leaves a good deal more).
    pub fn allocate(channels: usize, block: usize, capacity_frames: u64, preroll_frames: u64) -> Result<Capture, String> {
        if channels == 0 || block == 0 {
            return Err("there are no channels to record".into());
        }
        let slots = (capacity_frames / block as u64) as usize;
        if slots < 2 {
            return Err("the pre-roll is smaller than two blocks of audio".into());
        }
        let len = slots
            .checked_mul(channels)
            .and_then(|n| n.checked_mul(block))
            .ok_or_else(|| "the pre-roll asked for is larger than this PC can address".to_string())?;
        let mut samples: Vec<i32> = Vec::new();
        samples.try_reserve_exact(len).map_err(|why| {
            format!("{} MB could not be reserved for the pre-roll ({why}): choose a smaller percentage, or close something", (len as u64 * 4) >> 20)
        })?;
        // Write every sample, then every page again through a volatile write that cannot be left
        // out: memory that has been written is memory the system has committed and put in place.
        // Safety: the capacity was reserved above, and every element is written before `set_len`.
        unsafe {
            std::ptr::write_bytes(samples.as_mut_ptr(), 0, len);
            let step = PAGE / std::mem::size_of::<i32>();
            let mut at = 0;
            while at < len {
                std::ptr::write_volatile(samples.as_mut_ptr().add(at), 0);
                at += step;
            }
            samples.set_len(len);
        }
        let data = Box::into_raw(samples.into_boxed_slice()).cast::<i32>();
        let capacity = (slots * block) as u64;
        let preroll = (preroll_frames.min(capacity - block as u64) / block as u64) * block as u64;
        Ok(Capture {
            channels,
            block,
            slots,
            capacity,
            preroll,
            data,
            len,
            tags: (0..slots).map(|_| AtomicU64::new(EMPTY)).collect(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(PARKED),
            keep_until: AtomicU64::new(0),
            floor: AtomicU64::new(0),
            want: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            take_start: AtomicU64::new(0),
            pressed_at: AtomicU64::new(0),
            pending_pressed: AtomicU64::new(PARKED),
            pending_cue: AtomicU64::new(PARKED),
            take_cue: AtomicU64::new(PARKED),
            started_any: AtomicBool::new(false),
            overruns: AtomicU64::new(0),
            last_overrun: AtomicU64::new(PARKED),
            peaks: (0..channels).map(|_| AtomicU32::new(0)).collect(),
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn block(&self) -> usize {
        self.block
    }

    pub fn capacity_frames(&self) -> u64 {
        self.capacity
    }

    pub fn preroll_frames(&self) -> u64 {
        self.preroll
    }

    /// Bytes the ring holds on to.
    pub fn bytes(&self) -> u64 {
        self.len as u64 * 4
    }

    // ---------------------------------------------------------------------------------------------
    // The audio thread.
    // ---------------------------------------------------------------------------------------------

    /// **One block, on the callback thread.** `fill` is handed each channel's run in the slot, to
    /// copy that channel's samples into; it is the only thing that touches them.
    ///
    /// Nothing here allocates, locks, logs or waits.
    pub fn push_with(&self, mut fill: impl FnMut(usize, &mut [i32])) {
        let at = self.written.load(Ordering::Relaxed);

        // What the person asked for, taken at a block boundary, which is where a take begins and ends.
        let want = self.want.load(Ordering::Acquire);
        let recording = self.recording.load(Ordering::Relaxed);
        if !want && recording {
            self.keep_until.store(at, Ordering::Release);
            self.floor.store(at, Ordering::Relaxed);
            self.recording.store(false, Ordering::Release);
        } else if want && !recording && self.read.load(Ordering::Acquire) == PARKED {
            // The oldest block still held, but nothing the last take already has.
            let start = self.floor.load(Ordering::Relaxed).max(at.saturating_sub(self.preroll));
            let pressed = self.pending_pressed.swap(PARKED, Ordering::Relaxed);
            self.take_start.store(start, Ordering::Relaxed);
            self.pressed_at.store(if pressed == PARKED { at } else { pressed.clamp(start, at) }, Ordering::Relaxed);
            self.take_cue.store(self.pending_cue.swap(PARKED, Ordering::Relaxed), Ordering::Relaxed);
            self.keep_until.store(PARKED, Ordering::Relaxed);
            self.started_any.store(true, Ordering::Relaxed);
            self.recording.store(true, Ordering::Relaxed);
            // Published last: a reader that sees the take sees everything above.
            self.read.store(start, Ordering::Release);
        }

        let slot = ((at / self.block as u64) % self.slots as u64) as usize;
        // What this slot holds now, if the ring has been round: the block from one lap ago.
        let needed = at.checked_sub(self.capacity).is_some_and(|old| {
            let read = self.read.load(Ordering::Acquire);
            read != PARKED && old >= read && old < self.keep_until.load(Ordering::Relaxed)
        });
        if needed {
            // The writer has not taken that block yet: this one is lost instead, and said so.
            self.overruns.fetch_add(1, Ordering::Relaxed);
            self.last_overrun.store(at, Ordering::Relaxed);
        } else {
            let base = slot * self.channels * self.block;
            for channel in 0..self.channels {
                // Safety: this slot is the callback's alone right now (see the module), and the run
                // is inside the allocation.
                let run = unsafe { std::slice::from_raw_parts_mut(self.data.add(base + channel * self.block), self.block) };
                fill(channel, run);
                let peak = run.iter().fold(0u32, |loudest, &sample| loudest.max(sample.unsigned_abs()));
                self.peaks[channel].fetch_max(peak, Ordering::Relaxed);
            }
            self.tags[slot].store(at, Ordering::Release);
        }
        self.written.store(at + self.block as u64, Ordering::Release);
    }

    // ---------------------------------------------------------------------------------------------
    // Any thread: what the person asks for.
    // ---------------------------------------------------------------------------------------------

    /// Ask for a take, or for the one running to stop. It happens at the start of the callback's next
    /// block. Asking twice is asking once; a stop and a record in the same block leave the take
    /// running, which loses nothing.
    pub fn want_recording(&self, on: bool) {
        if !on {
            self.pending_pressed.store(PARKED, Ordering::Relaxed);
            self.pending_cue.store(PARKED, Ordering::Relaxed);
        }
        self.want.store(on, Ordering::Release);
    }

    /// **On the callback, before its block is pushed**: a take after a count-in. It starts at this
    /// block as any take does, reaching back into the pre-roll, and remembers `pressed` (when Record
    /// was pressed, which is where its clock counts from) and `downbeat`, both stream positions.
    pub fn start_take_counted(&self, pressed: u64, downbeat: u64) {
        self.pending_pressed.store(pressed, Ordering::Relaxed);
        self.pending_cue.store(downbeat, Ordering::Relaxed);
        self.want.store(true, Ordering::Release);
    }

    pub fn wants_recording(&self) -> bool {
        self.want.load(Ordering::Acquire)
    }

    /// **Only once the stream has stopped**, when no callback can run again: end any take where the
    /// audio ended, as the callback would have, so the writer can finish it.
    pub fn close_after_stream_stopped(&self) {
        self.want.store(false, Ordering::Release);
        if self.recording.load(Ordering::Acquire) {
            let at = self.written.load(Ordering::Acquire);
            self.keep_until.store(at, Ordering::Release);
            self.floor.store(at, Ordering::Relaxed);
            self.recording.store(false, Ordering::Release);
        }
    }

    /// Where things stand, for the page.
    pub fn snapshot(&self) -> Snapshot {
        let written = self.written.load(Ordering::Acquire);
        let recording = self.recording.load(Ordering::Acquire);
        let window = self.started_any.load(Ordering::Acquire).then(|| Window {
            start: self.take_start.load(Ordering::Relaxed),
            pressed_at: self.pressed_at.load(Ordering::Relaxed),
            cue: Some(self.take_cue.load(Ordering::Relaxed)).filter(|&cue| cue != PARKED),
        });
        let held_frames = match (recording, window) {
            (true, Some(window)) => window.pressed_at - window.start,
            _ => written.saturating_sub(self.floor.load(Ordering::Relaxed)).min(self.preroll),
        };
        let last = self.last_overrun.load(Ordering::Relaxed);
        Snapshot {
            written,
            recording,
            window,
            held_frames,
            overruns: self.overruns.load(Ordering::Relaxed),
            last_overrun_at: (last != PARKED).then_some(last),
        }
    }

    /// Each channel's loudest sample since the last look, as a magnitude out of 2^31, and start
    /// again. Only one thing should look: the status.
    pub fn take_peaks(&self) -> Vec<u32> {
        self.peaks.iter().map(|peak| peak.swap(0, Ordering::Relaxed)).collect()
    }

    // ---------------------------------------------------------------------------------------------
    // The writer thread.
    // ---------------------------------------------------------------------------------------------

    /// The take the writer is to drain, if there is one.
    pub fn take(&self) -> Option<Window> {
        (self.read.load(Ordering::Acquire) != PARKED).then(|| Window {
            start: self.take_start.load(Ordering::Relaxed),
            pressed_at: self.pressed_at.load(Ordering::Relaxed),
            cue: Some(self.take_cue.load(Ordering::Relaxed)).filter(|&cue| cue != PARKED),
        })
    }

    /// The next position the writer takes.
    pub fn read_position(&self) -> u64 {
        self.read.load(Ordering::Acquire)
    }

    /// Where the running take ends, or [`PARKED`] while it is still running.
    pub fn take_end(&self) -> u64 {
        self.keep_until.load(Ordering::Acquire)
    }

    /// The position of the next block the callback will write.
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// The block at `position`, channel runs one after another, or `None` when that block never
    /// arrived (an overrun). Only for a position the writer has not passed and the callback has.
    pub fn block_at(&self, position: u64) -> Option<&[i32]> {
        let slot = ((position / self.block as u64) % self.slots as u64) as usize;
        if self.tags[slot].load(Ordering::Acquire) != position {
            return None;
        }
        let size = self.channels * self.block;
        // Safety: the tag says the block is there, and the callback may not write this slot again
        // until the writer has moved past it.
        Some(unsafe { std::slice::from_raw_parts(self.data.add(slot * size), size) })
    }

    /// The writer is done with everything before `position`.
    pub fn consumed(&self, position: u64) {
        self.read.store(position, Ordering::Release);
    }

    /// The writer has finished the take; the callback may start another.
    pub fn park(&self) {
        self.read.store(PARKED, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: usize = 16;
    const CHANNELS: usize = 3;

    /// A counting ramp with every channel apart: sample `n` of channel `c`.
    fn ramp(n: u64, c: usize) -> i32 {
        (n as i32).wrapping_mul(CHANNELS as i32).wrapping_add(c as i32)
    }

    /// A source that feeds the ring a ramp, one block a call.
    struct Source {
        next: u64,
    }

    impl Source {
        fn push(&mut self, capture: &Capture) {
            let first = self.next;
            capture.push_with(|channel, run| {
                for (i, sample) in run.iter_mut().enumerate() {
                    *sample = ramp(first + i as u64, channel);
                }
            });
            self.next += BLOCK as u64;
        }
    }

    /// What a writer takes out, block by block: the channels' samples, with lost blocks as `None`.
    fn drain(capture: &Capture, out: &mut Vec<Option<Vec<i32>>>) -> bool {
        let Some(_) = capture.take() else { return false };
        let end = capture.take_end();
        let limit = end.min(capture.written());
        let mut at = capture.read_position();
        while at < limit {
            out.push(capture.block_at(at).map(<[i32]>::to_vec));
            at += BLOCK as u64;
            capture.consumed(at);
        }
        if end != PARKED && at >= end {
            capture.park();
            return true;
        }
        false
    }

    /// Every sample of every channel, as positions: each block must be the ramp at its place.
    fn positions(blocks: &[Option<Vec<i32>>], start: u64) -> Vec<u64> {
        let mut seen = Vec::new();
        for (index, block) in blocks.iter().enumerate() {
            let block = block.as_ref().expect("no block was lost");
            for channel in 0..CHANNELS {
                for i in 0..BLOCK {
                    let n = start + (index * BLOCK + i) as u64;
                    assert_eq!(block[channel * BLOCK + i], ramp(n, channel), "channel {channel}, sample {n}");
                }
            }
            seen.push(start + (index * BLOCK) as u64);
        }
        seen
    }

    #[test]
    fn the_take_starts_at_the_oldest_sample_held_and_runs_on_into_live_audio_without_a_gap() {
        // 8 slots, 5 of them pre-roll.
        let capture = Capture::allocate(CHANNELS, BLOCK, 8 * BLOCK as u64, 5 * BLOCK as u64).unwrap();
        let mut source = Source { next: 0 };
        // Armed long enough to go round the ring several times.
        for _ in 0..37 {
            source.push(&capture);
        }
        assert_eq!(capture.snapshot().held_frames, 5 * BLOCK as u64, "a full pre-roll held");
        capture.want_recording(true);
        let mut taken = Vec::new();
        // Record takes effect at the next block, and the writer drains as it goes, across the wrap.
        for _ in 0..30 {
            source.push(&capture);
            drain(&capture, &mut taken);
        }
        let window = capture.take().expect("a take");
        assert_eq!(window.pressed_at, 37 * BLOCK as u64);
        assert_eq!(window.start, 32 * BLOCK as u64, "five blocks of pre-roll before the press");
        capture.want_recording(false);
        source.push(&capture);
        assert!(drain(&capture, &mut taken), "the take is finished");
        let seen = positions(&taken, window.start);
        // Sample by sample from the oldest held, through the press, to where Stop took effect.
        assert_eq!(seen.first(), Some(&(32 * BLOCK as u64)));
        assert_eq!(seen.last(), Some(&(66 * BLOCK as u64)));
        assert!(seen.windows(2).all(|pair| pair[1] == pair[0] + BLOCK as u64), "one run, no gap");
        assert_eq!(capture.snapshot().overruns, 0);
    }

    #[test]
    fn a_record_pressed_before_the_pre_roll_has_filled_takes_what_there_is() {
        let capture = Capture::allocate(CHANNELS, BLOCK, 8 * BLOCK as u64, 5 * BLOCK as u64).unwrap();
        let mut source = Source { next: 0 };
        source.push(&capture);
        source.push(&capture);
        assert_eq!(capture.snapshot().held_frames, 2 * BLOCK as u64);
        capture.want_recording(true);
        source.push(&capture);
        assert_eq!(capture.take().unwrap().start, 0, "from the very first sample, and not before it");
    }

    #[test]
    fn back_to_back_takes_tile_the_stream_and_the_next_pre_roll_starts_where_the_last_take_ended() {
        let capture = Capture::allocate(CHANNELS, BLOCK, 8 * BLOCK as u64, 5 * BLOCK as u64).unwrap();
        let mut source = Source { next: 0 };
        for _ in 0..20 {
            source.push(&capture);
        }
        let mut first = Vec::new();
        capture.want_recording(true);
        for _ in 0..4 {
            source.push(&capture);
            drain(&capture, &mut first);
        }
        capture.want_recording(false);
        source.push(&capture);
        assert!(drain(&capture, &mut first));
        let first_end = 15 * BLOCK as u64 + first.len() as u64 * BLOCK as u64;
        assert_eq!(first_end, 24 * BLOCK as u64);
        // Two blocks later, Record again: the pre-roll reaches back only as far as the last take's end.
        source.push(&capture);
        assert_eq!(capture.snapshot().held_frames, 2 * BLOCK as u64, "held since the last take ended");
        capture.want_recording(true);
        let mut second = Vec::new();
        source.push(&capture);
        drain(&capture, &mut second);
        let window = capture.take().unwrap();
        assert_eq!(window.start, first_end, "no gap and no overlap between the two takes");
        positions(&second, window.start);
    }

    #[test]
    fn a_writer_that_falls_behind_loses_the_new_block_not_the_waiting_one_and_is_told_where() {
        let capture = Capture::allocate(CHANNELS, BLOCK, 8 * BLOCK as u64, 5 * BLOCK as u64).unwrap();
        let mut source = Source { next: 0 };
        for _ in 0..10 {
            source.push(&capture);
        }
        capture.want_recording(true);
        // The take starts at 5 blocks and the writer takes nothing while 6 more arrive: slots for
        // positions 5..13 are 8, so the 9th block from the start (position 13) has nowhere to go.
        for _ in 0..6 {
            source.push(&capture);
        }
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.overruns, 3, "blocks 13, 14 and 15 would have overwritten blocks 5, 6 and 7");
        assert_eq!(snapshot.last_overrun_at, Some(15 * BLOCK as u64));
        let mut taken = Vec::new();
        drain(&capture, &mut taken);
        capture.want_recording(false);
        source.push(&capture);
        assert!(drain(&capture, &mut taken));
        let lost: Vec<usize> = taken.iter().enumerate().filter(|(_, b)| b.is_none()).map(|(i, _)| i).collect();
        assert_eq!(lost, vec![8, 9, 10], "the writer sees exactly which blocks never arrived");
        // Everything that did arrive is the ramp at its own place.
        for (index, block) in taken.iter().enumerate() {
            if let Some(block) = block {
                let n = 5 * BLOCK as u64 + (index * BLOCK) as u64;
                assert_eq!(block[0], ramp(n, 0));
            }
        }
    }

    #[test]
    fn a_stop_and_a_record_before_the_next_block_leave_the_take_running() {
        let capture = Capture::allocate(CHANNELS, BLOCK, 8 * BLOCK as u64, 5 * BLOCK as u64).unwrap();
        let mut source = Source { next: 0 };
        capture.want_recording(true);
        source.push(&capture);
        capture.want_recording(false);
        capture.want_recording(true);
        source.push(&capture);
        assert!(capture.snapshot().recording);
        assert_eq!(capture.take_end(), PARKED);
    }

    #[test]
    fn a_take_still_draining_holds_the_next_one_back_until_it_is_written() {
        let capture = Capture::allocate(CHANNELS, BLOCK, 8 * BLOCK as u64, 5 * BLOCK as u64).unwrap();
        let mut source = Source { next: 0 };
        capture.want_recording(true);
        source.push(&capture);
        source.push(&capture);
        capture.want_recording(false);
        source.push(&capture);
        capture.want_recording(true);
        source.push(&capture);
        assert!(!capture.snapshot().recording, "the first take has not been written yet");
        let mut taken = Vec::new();
        assert!(drain(&capture, &mut taken));
        source.push(&capture);
        let window = capture.take().unwrap();
        assert_eq!(window.start, 2 * BLOCK as u64, "it starts where the first ended, so nothing was missed");
    }

    #[test]
    fn a_stream_that_stops_mid_take_ends_the_take_where_the_audio_ended() {
        let capture = Capture::allocate(CHANNELS, BLOCK, 8 * BLOCK as u64, 5 * BLOCK as u64).unwrap();
        let mut source = Source { next: 0 };
        capture.want_recording(true);
        for _ in 0..3 {
            source.push(&capture);
        }
        capture.close_after_stream_stopped();
        assert_eq!(capture.take_end(), 3 * BLOCK as u64);
        let mut taken = Vec::new();
        assert!(drain(&capture, &mut taken));
        assert_eq!(taken.len(), 3);
    }

    #[test]
    fn a_take_after_a_count_in_reaches_back_as_any_take_and_keeps_the_press_and_the_downbeat() {
        let capture = Capture::allocate(CHANNELS, BLOCK, 16 * BLOCK as u64, 10 * BLOCK as u64).unwrap();
        let mut source = Source { next: 0 };
        for _ in 0..20 {
            source.push(&capture);
        }
        // Pressed at block 16, the downbeat seven samples into block 20, which is now.
        capture.start_take_counted(16 * BLOCK as u64, 20 * BLOCK as u64 + 7);
        source.push(&capture);
        let window = capture.take().unwrap();
        assert_eq!(window, Window { start: 10 * BLOCK as u64, pressed_at: 16 * BLOCK as u64, cue: Some(20 * BLOCK as u64 + 7) });
        capture.want_recording(false);
        source.push(&capture);
        let mut taken = Vec::new();
        assert!(drain(&capture, &mut taken));
        // A press before the pre-roll reaches is held to the take's start; a plain take has no cue.
        capture.start_take_counted(0, 22 * BLOCK as u64);
        source.push(&capture);
        assert_eq!(capture.take().unwrap().pressed_at, capture.take().unwrap().start);
        capture.want_recording(false);
        source.push(&capture);
        assert!(drain(&capture, &mut taken));
        capture.want_recording(true);
        source.push(&capture);
        assert_eq!(capture.take().unwrap().cue, None);
    }

    #[test]
    fn peaks_are_the_loudest_sample_per_channel_since_the_last_look() {
        let capture = Capture::allocate(2, 4, 16, 8).unwrap();
        capture.push_with(|channel, run| run.copy_from_slice(&[0, if channel == 0 { -1000 } else { 7 }, 3, 0]));
        assert_eq!(capture.take_peaks(), vec![1000, 7]);
        assert_eq!(capture.take_peaks(), vec![0, 0]);
    }

    #[test]
    fn a_ring_too_large_to_have_is_refused_in_words() {
        let refused = Capture::allocate(1 << 20, 1 << 20, 1 << 40, 1 << 39).err().expect("refused");
        assert!(refused.contains("larger than this PC can address") || refused.contains("could not be reserved"), "{refused}");
        assert!(Capture::allocate(0, 16, 64, 32).is_err());
    }
}
