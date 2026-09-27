//! **How big the pre-roll is**, worked out at Arm from how much memory is free then.
//!
//! A preset asks for a percentage of the physical memory that is free when Arm is pressed
//! (`GlobalMemoryStatusEx`'s `ullAvailPhys`). That is turned into a buffer, clamped, and split in two:
//!
//! - **The pre-roll**, the part Record reaches back into: the last so many seconds before the press.
//! - **Headroom**, the rest. It is what the writer drains the take out of once Record is pressed, and
//!   at that moment the whole pre-roll is still waiting to be written while the audio keeps coming.
//!   With no headroom the very next block would have nowhere to go. A third of the pre-roll, and at
//!   least two seconds, gives a disk writing at ten times the audio's rate (a slow USB stick still
//!   does better) the time to catch up with plenty spare.
//!
//! # The numbers
//!
//! - **At least five seconds of pre-roll** ([`MIN_PREROLL_SECONDS`]). The pre-roll is there for
//!   "that was the one, press Record", and a person takes a second or two to decide and another to
//!   reach the button; less than five would miss the start of what they meant to keep.
//! - **Never more than half of what is free** ([`CEILING_FRACTION`]), whatever the preset asks, so
//!   the DAW, the browser and the system keep their footing while Gazelle is armed.
//! - **Never more than 4 GiB** ([`CEILING_BYTES`]). Committing and touching it takes time at Arm,
//!   about a second per gigabyte on a busy machine, and 4 GiB is already over three and a half
//!   minutes of pre-roll with 40 channels at 96 kHz, or hours with two at 48 kHz.
//! - **An optional cap in seconds** from the preset ("keep the last 30 s"), which makes the buffer
//!   itself smaller rather than using less of a bigger one: memory nobody will ever read back is
//!   memory the rest of the PC could have had. It never goes below the five second floor.
//!
//! Samples are held as the driver hands them over, 32-bit integers, whatever the preset writes:
//! the callback then copies and never converts, one buffer serves both bit depths, and packing
//! 24-bit into three bytes would buy a quarter more seconds for work done on the audio thread. The
//! seconds shown are worked out from that: rate × channels × 4 bytes.

use serde::Serialize;

/// The shortest pre-roll there is.
pub const MIN_PREROLL_SECONDS: f64 = 5.0;
/// The shortest headroom there is.
pub const MIN_HEADROOM_SECONDS: f64 = 2.0;
/// The most of the free memory the buffer may take, whatever the preset asks.
pub const CEILING_FRACTION: f64 = 0.5;
/// The most the buffer may take at all.
pub const CEILING_BYTES: u64 = 4 << 30;
/// The percentages a preset may ask for.
pub const PERCENT_MIN: f64 = 1.0;
pub const PERCENT_MAX: f64 = 50.0;
/// What a new preset asks for.
pub const PERCENT_DEFAULT: f64 = 10.0;
/// Bytes one sample takes in the buffer.
pub const HELD_BYTES: u64 = 4;

/// The buffer Arm reserves, and what it amounts to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Sizing {
    /// The free memory it was sized from, in bytes.
    pub available_bytes: u64,
    /// What the preset asked for, as a percentage of that.
    pub percent_asked: f64,
    /// What the buffer actually takes, as a percentage of that.
    pub percent: f64,
    /// The buffer, in bytes.
    pub bytes: u64,
    /// The buffer, in frames: a whole number of blocks.
    pub capacity_frames: u64,
    /// The pre-roll, in frames: a whole number of blocks, at most the capacity less the headroom.
    pub preroll_frames: u64,
    /// The pre-roll in seconds, which is what the page shows.
    pub preroll_seconds: f64,
    /// Why the pre-roll is not what the percentage alone would have made it, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clamped: Option<&'static str>,
}

/// What the buffer is being sized for.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub rate: f64,
    pub channels: usize,
    pub block: usize,
}

impl Shape {
    fn bytes_per_second(&self) -> f64 {
        self.rate * self.channels as f64 * HELD_BYTES as f64
    }

    fn frames(&self, seconds: f64) -> u64 {
        (seconds * self.rate).ceil().max(0.0) as u64
    }

    /// Frames rounded up to whole blocks.
    fn blocks_up(&self, frames: u64) -> u64 {
        let block = self.block.max(1) as u64;
        frames.div_ceil(block) * block
    }

    /// Frames rounded down to whole blocks.
    fn blocks_down(&self, frames: u64) -> u64 {
        let block = self.block.max(1) as u64;
        frames / block * block
    }
}

/// The headroom a pre-roll of this many seconds needs.
fn headroom(preroll_seconds: f64) -> f64 {
    (preroll_seconds / 3.0).max(MIN_HEADROOM_SECONDS)
}

/// The pre-roll a buffer of this many seconds holds, once its headroom is taken out.
fn preroll_in(buffer_seconds: f64) -> f64 {
    // Headroom is a third of the pre-roll, so a quarter of the buffer, down to two seconds.
    let quarter = buffer_seconds * 0.75;
    if quarter / 3.0 >= MIN_HEADROOM_SECONDS {
        quarter
    } else {
        buffer_seconds - MIN_HEADROOM_SECONDS
    }
}

/// Size the buffer, or say in a sentence why it cannot be.
pub fn size(available: Option<u64>, percent: f64, cap_seconds: Option<f64>, shape: Shape) -> Result<Sizing, String> {
    let Some(available) = available else {
        return Err("Windows would not say how much memory is free, so the pre-roll cannot be sized".into());
    };
    if !(PERCENT_MIN..=PERCENT_MAX).contains(&percent) {
        return Err(format!("the pre-roll takes between {PERCENT_MIN} and {PERCENT_MAX} percent of the free memory, not {percent}"));
    }
    if !(shape.rate.is_finite() && shape.rate > 0.0) || shape.channels == 0 || shape.block == 0 {
        return Err("there is nothing to size a pre-roll for: no channels, or no rate".into());
    }
    let per_second = shape.bytes_per_second();
    let ceiling = ((available as f64 * CEILING_FRACTION) as u64).min(CEILING_BYTES);
    let asked = (available as f64 * percent / 100.0) as u64;

    let mut clamped = None;
    let mut buffer_seconds = asked.min(ceiling) as f64 / per_second;
    if asked > ceiling {
        clamped = Some(if ceiling == CEILING_BYTES { "held to 4 GiB, the most Gazelle reserves" } else { "held to half of the free memory" });
    }
    let mut preroll = preroll_in(buffer_seconds);
    // Sized from the budget, the buffer is rounded down to whole blocks, so it never comes out over
    // what was asked. Sized from seconds (a cap, or the floor), it is rounded up, so it holds them.
    let mut from_budget = true;
    if let Some(cap) = cap_seconds.filter(|cap| cap.is_finite() && *cap > 0.0) {
        let cap = cap.max(MIN_PREROLL_SECONDS);
        if cap < preroll {
            preroll = cap;
            buffer_seconds = preroll + headroom(preroll);
            clamped = Some("held to the preset's own limit");
            from_budget = false;
        }
    }
    if preroll < MIN_PREROLL_SECONDS {
        preroll = MIN_PREROLL_SECONDS;
        buffer_seconds = preroll + headroom(preroll);
        clamped = Some("raised to five seconds, the shortest pre-roll there is");
        from_budget = false;
    }

    let capacity_frames = if from_budget {
        shape.blocks_down((buffer_seconds * shape.rate) as u64)
    } else {
        shape.blocks_up(shape.frames(buffer_seconds))
    }
    .max(2 * shape.block as u64);
    let preroll_frames = if from_budget { shape.blocks_down((preroll * shape.rate) as u64) } else { shape.blocks_up(shape.frames(preroll)) };
    let bytes = capacity_frames * shape.channels as u64 * HELD_BYTES;
    if bytes > ceiling {
        let needed = bytes.div_ceil(1 << 20);
        let free = available >> 20;
        return Err(format!(
            "only {free} MB of memory is free, and the shortest pre-roll for {} channels at {} Hz needs {needed} MB with half of what is free left over: \
             record fewer channels, or close something",
            shape.channels, shape.rate
        ));
    }
    // Never all of it: at least a block of headroom whatever the rounding did.
    let preroll_frames = shape.blocks_down(preroll_frames.min(capacity_frames - shape.block as u64));
    Ok(Sizing {
        available_bytes: available,
        percent_asked: percent,
        percent: bytes as f64 * 100.0 / available.max(1) as f64,
        bytes,
        capacity_frames,
        preroll_frames,
        preroll_seconds: preroll_frames as f64 / shape.rate,
        clamped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn shape(channels: usize) -> Shape {
        Shape { rate: 96_000.0, channels, block: 512 }
    }

    #[test]
    fn ten_percent_of_16_gb_free_is_a_quarter_headroom_and_the_rest_pre_roll() {
        let sizing = size(Some(16 * GB), 10.0, None, shape(8)).unwrap();
        // 1.6 GB at 96 kHz, 8 channels, 4 bytes: 3.072 MB a second, so about 559 s of buffer.
        let buffer = sizing.capacity_frames as f64 / 96_000.0;
        assert!((buffer - 559.24).abs() < 1.0, "{buffer}");
        assert!((sizing.preroll_seconds - buffer * 0.75).abs() < 0.1, "{sizing:?}");
        assert!((sizing.percent - 10.0).abs() < 0.01, "{sizing:?}");
        assert_eq!(sizing.clamped, None);
        assert_eq!(sizing.capacity_frames % 512, 0, "whole blocks");
        assert_eq!(sizing.preroll_frames % 512, 0);
        assert_eq!(sizing.bytes, sizing.capacity_frames * 8 * 4);
    }

    #[test]
    fn a_buffer_never_takes_more_than_half_of_what_is_free_or_4_gib() {
        let half = size(Some(GB), 50.0, None, shape(2)).unwrap();
        assert!(half.bytes <= GB / 2, "{half:?}");
        assert_eq!(half.clamped, None, "50 percent is half, which is allowed");
        let most = size(Some(64 * GB), 50.0, None, shape(40)).unwrap();
        assert!(most.bytes <= CEILING_BYTES, "{most:?}");
        assert_eq!(most.clamped, Some("held to 4 GiB, the most Gazelle reserves"));
        assert!(most.preroll_seconds > 200.0, "over three minutes with 40 channels at 96 kHz: {}", most.preroll_seconds);
    }

    #[test]
    fn a_small_percentage_is_raised_to_five_seconds_and_a_cap_holds_it_down() {
        let tiny = size(Some(GB), 1.0, None, shape(40)).unwrap();
        // 10 MB is under a second of 40 channels, so it is raised to the floor.
        assert!((tiny.preroll_seconds - MIN_PREROLL_SECONDS).abs() < 0.01, "{tiny:?}");
        assert_eq!(tiny.clamped, Some("raised to five seconds, the shortest pre-roll there is"));
        assert!(tiny.capacity_frames as f64 / 96_000.0 >= MIN_PREROLL_SECONDS + MIN_HEADROOM_SECONDS);

        let capped = size(Some(16 * GB), 10.0, Some(30.0), shape(8)).unwrap();
        assert!((capped.preroll_seconds - 30.0).abs() < 0.01, "{capped:?}");
        assert!((capped.capacity_frames as f64 / 96_000.0 - 40.0).abs() < 0.02, "a third of the pre-roll again for headroom: {capped:?}");
        assert_eq!(capped.clamped, Some("held to the preset's own limit"));
        let floor = size(Some(16 * GB), 10.0, Some(1.0), shape(8)).unwrap();
        assert!((floor.preroll_seconds - MIN_PREROLL_SECONDS).abs() < 0.01, "a cap never goes under the floor");
    }

    #[test]
    fn too_little_memory_is_a_refusal_that_says_how_much_is_free_and_what_would_help() {
        let refused = size(Some(20 << 20), 50.0, None, shape(40)).unwrap_err();
        assert!(refused.contains("only 20 MB of memory is free"), "{refused}");
        assert!(refused.contains("fewer channels"), "{refused}");
        assert!(size(None, 10.0, None, shape(2)).unwrap_err().contains("would not say"));
        assert!(size(Some(GB), 0.5, None, shape(2)).unwrap_err().contains("between 1 and 50"));
        assert!(size(Some(GB), 60.0, None, shape(2)).is_err());
    }

    #[test]
    fn the_pre_roll_always_leaves_at_least_a_block_and_two_seconds_of_headroom() {
        for (free, percent, channels) in [(GB, 1.0, 1), (GB, 50.0, 1), (8 * GB, 3.0, 24), (2 * GB, 7.5, 40)] {
            let sizing = size(Some(free), percent, None, shape(channels)).unwrap();
            let headroom = sizing.capacity_frames - sizing.preroll_frames;
            assert!(headroom as f64 / 96_000.0 >= MIN_HEADROOM_SECONDS - 0.01, "{sizing:?}");
            assert!(headroom >= 512);
        }
    }
}
