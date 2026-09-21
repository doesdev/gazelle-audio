//! Holding one device back so that every device lines up.
//!
//! Two interfaces do not report the same latency: phase 0 measured 639 in and 799 out on one and
//! 636 in and 700 out on the other, at 96 kHz. Crossing between two callback threads adds a whole
//! buffer on top of that. The aggregate reports one figure for the lot, so each device is held
//! back by the difference between its own path and the longest one, in samples rather than in
//! whole buffers.
//!
//! Every sample of memory is allocated when the buffers are made, and the work per block is a
//! copy: no allocation, no branch per sample worth speaking of, nothing to lock.

/// A fixed delay over a block of channel-major samples.
pub struct Delay {
    /// How many samples each channel is held back by. Zero is a straight copy.
    samples: usize,
    /// The longest delay this one can be moved to, which is what it has the memory for. The audio
    /// path may change the length inside this and never outside it.
    room: usize,
    channels: usize,
    history: Vec<i32>,
    /// Where in each channel's history the oldest sample sits.
    at: usize,
}

impl Delay {
    /// A delay of `samples` over `channels` channels, starting silent, that will never be anything
    /// else.
    pub fn new(channels: usize, samples: usize) -> Delay {
        Delay::with_room(channels, samples, samples)
    }

    /// The same, with the memory for a delay of up to `room` samples, so that a measurement taken
    /// once the session has started can move it without allocating on the audio path.
    pub fn with_room(channels: usize, samples: usize, room: usize) -> Delay {
        let room = room.max(samples);
        Delay { samples, room, channels, history: vec![0i32; channels * room], at: 0 }
    }

    pub fn samples(&self) -> usize {
        self.samples
    }

    /// The longest this delay can be made.
    pub fn room(&self) -> usize {
        self.room
    }

    /// **Hold the audio back by a different number of samples**, which is what applying a
    /// measurement comes to. Everything held is dropped, because a delay that changed length is a
    /// discontinuity however it is done, and a sample kept across one would be played twice or
    /// not at all.
    ///
    /// Allocates nothing: a length past the room this delay was made with is refused instead, and
    /// the delay is left exactly as it was.
    pub fn set_samples(&mut self, samples: usize) -> bool {
        if samples > self.room {
            return false;
        }
        if samples != self.samples {
            self.samples = samples;
            self.clear();
        }
        true
    }

    /// Hold `block` back in place. `block` is `channels` runs of `len` samples, one after another.
    pub fn process(&mut self, block: &mut [i32], len: usize) {
        if self.samples == 0 || self.channels == 0 {
            return;
        }
        debug_assert_eq!(block.len(), self.channels * len);
        let size = self.samples;
        for channel in 0..self.channels {
            let history = &mut self.history[channel * size..(channel + 1) * size];
            let run = &mut block[channel * len..(channel + 1) * len];
            let mut at = self.at;
            for sample in run.iter_mut() {
                // The oldest sample leaves as this one arrives, which is the whole of a delay.
                std::mem::swap(sample, &mut history[at]);
                at += 1;
                if at == size {
                    at = 0;
                }
            }
        }
        self.at = (self.at + len) % size;
    }

    /// Forget what is held, so a device that went away does not play what it missed.
    pub fn clear(&mut self) {
        self.history.fill(0);
        self.at = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(delay: &mut Delay, channels: usize, blocks: &[&[i32]]) -> Vec<Vec<i32>> {
        let mut out = Vec::new();
        for block in blocks {
            let mut copy = block.to_vec();
            delay.process(&mut copy, block.len() / channels);
            out.push(copy);
        }
        out
    }

    #[test]
    fn no_delay_is_a_straight_copy() {
        let mut delay = Delay::new(2, 0);
        let got = run(&mut delay, 2, &[&[1, 2, 3, 4]]);
        assert_eq!(got[0], vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_delay_shorter_than_the_block_starts_with_silence() {
        let mut delay = Delay::new(1, 2);
        let got = run(&mut delay, 1, &[&[1, 2, 3, 4], &[5, 6, 7, 8]]);
        assert_eq!(got[0], vec![0, 0, 1, 2]);
        assert_eq!(got[1], vec![3, 4, 5, 6]);
    }

    #[test]
    fn a_delay_longer_than_the_block_holds_across_several_of_them() {
        let mut delay = Delay::new(1, 5);
        let got = run(&mut delay, 1, &[&[1, 2], &[3, 4], &[5, 6], &[7, 8]]);
        assert_eq!(got[0], vec![0, 0]);
        assert_eq!(got[1], vec![0, 0]);
        assert_eq!(got[2], vec![0, 1], "five samples of delay, so sample one arrives sixth");
        assert_eq!(got[3], vec![2, 3]);
    }

    #[test]
    fn each_channel_is_held_back_by_itself() {
        let mut delay = Delay::new(2, 1);
        // Two channels, three samples each, laid out one channel after the other.
        let got = run(&mut delay, 2, &[&[1, 2, 3, 10, 20, 30], &[4, 5, 6, 40, 50, 60]]);
        assert_eq!(got[0], vec![0, 1, 2, 0, 10, 20]);
        assert_eq!(got[1], vec![3, 4, 5, 30, 40, 50]);
    }

    #[test]
    fn clearing_drops_what_was_held() {
        let mut delay = Delay::new(1, 3);
        run(&mut delay, 1, &[&[1, 2, 3]]);
        delay.clear();
        let got = run(&mut delay, 1, &[&[7, 8, 9]]);
        assert_eq!(got[0], vec![0, 0, 0], "what was in the delay must not come back");
    }

    #[test]
    fn a_delay_made_with_room_can_be_moved_inside_it_and_never_outside_it() {
        // The room is allocated once, when the buffers are made, so that a phase measured after a
        // session has started moves the delay without the audio path allocating anything.
        let mut delay = Delay::with_room(1, 2, 8);
        assert_eq!(delay.samples(), 2);
        assert_eq!(delay.room(), 8);
        assert!(delay.set_samples(5), "inside the room it was made with");
        assert_eq!(delay.samples(), 5);
        assert!(!delay.set_samples(9), "and never outside it");
        assert_eq!(delay.samples(), 5, "a refusal leaves the delay exactly as it was");
        assert!(delay.set_samples(0), "including down to nothing at all");

        // What was held is dropped, because changing the length is a discontinuity whatever else
        // is done, and a sample carried across one would be heard twice.
        let mut held = Delay::with_room(1, 4, 8);
        run(&mut held, 1, &[&[1, 2, 3, 4]]);
        held.set_samples(2);
        assert_eq!(run(&mut held, 1, &[&[7, 8, 9, 10]])[0], vec![0, 0, 7, 8]);
        // And the new length is a real delay of that many samples, over every channel.
        let mut wider = Delay::with_room(2, 0, 4);
        assert!(wider.set_samples(1));
        assert_eq!(run(&mut wider, 2, &[&[1, 2, 10, 20]])[0], vec![0, 1, 0, 10]);
    }

    #[test]
    fn a_delay_that_was_never_given_room_cannot_be_moved_at_all() {
        let mut fixed = Delay::new(1, 3);
        assert_eq!(fixed.room(), 3);
        assert!(!fixed.set_samples(4));
        assert!(fixed.set_samples(3), "to what it already is, which changes nothing");
    }

    #[test]
    fn a_long_run_never_loses_or_repeats_a_sample() {
        // Every delay length around and across the block size, because the index wraps inside a
        // block for some of them and across blocks for others.
        for held in [1usize, 3, 7, 8, 9, 16, 31] {
            let mut delay = Delay::new(1, held);
            let mut sent = 0i32;
            let mut got = Vec::new();
            for _ in 0..12 {
                let block: Vec<i32> = (0..8).map(|_| {
                    sent += 1;
                    sent
                })
                .collect();
                let mut copy = block.clone();
                delay.process(&mut copy, 8);
                got.extend(copy);
            }
            let expected: Vec<i32> = (0..96).map(|i| if i < held as i32 { 0 } else { i - held as i32 + 1 }).collect();
            assert_eq!(got, expected, "a delay of {held} samples");
        }
    }
}
