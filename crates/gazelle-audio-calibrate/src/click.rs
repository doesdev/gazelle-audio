//! The click: what is played, and when.
//!
//! **Not one sample.** A converter's anti-aliasing and reconstruction filters turn a single sample
//! into a symmetrical smear a hundred samples wide with no start worth measuring. What survives
//! them is a short shaped burst, and the shape that survives them best is a sweep: its
//! autocorrelation is one sharp peak with nothing beside it, so a correlation cannot settle on the
//! wrong lobe the way it can with a tone burst.
//!
//! The burst is windowed at both ends, so it starts and ends at silence and the amplifier at the
//! far end of the cable is never asked for a step.

use std::f64::consts::PI;

/// The click, as a run of samples between minus one and one, at the level asked for.
///
/// `length` samples of a sweep from a sixteenth of the sample rate to a quarter of it, under a
/// raised cosine window.
pub fn shape(length: usize, level: f64) -> Vec<f64> {
    if length == 0 {
        return Vec::new();
    }
    let n = length as f64;
    (0..length)
        .map(|index| {
            let at = index as f64;
            // The window: zero at both ends, one in the middle, and no discontinuity anywhere.
            let window = 0.5 - 0.5 * (2.0 * PI * at / n).cos();
            // The sweep, as the integral of a frequency rising linearly across the burst.
            let (from, to) = (1.0 / 16.0, 1.0 / 4.0);
            let phase = 2.0 * PI * (from * at + (to - from) * at * at / (2.0 * n));
            level * window * phase.sin()
        })
        .collect()
}

/// The click as samples of the kind the driver carries, which is what the audio path writes.
pub fn as_samples(length: usize, level: f64) -> Vec<i32> {
    shape(length, level).into_iter().map(|value| (value * i32::MAX as f64) as i32).collect()
}

/// Where each click starts, in samples from the first sample of the run.
pub fn schedule(clicks: u32, settle: f64, spacing: f64, rate: f64) -> Vec<usize> {
    if !(rate.is_finite() && rate > 0.0) {
        return Vec::new();
    }
    let settle = (settle.max(0.0) * rate).round() as usize;
    let spacing = (spacing.max(0.0) * rate).round().max(1.0) as usize;
    (0..clicks as usize).map(|index| settle + index * spacing).collect()
}

/// How many samples a run of this shape has to capture: the last click, the room it needs to come
/// back in, and one spacing of tail so that the last one is never cut in half.
pub fn run_length(clicks: u32, settle: f64, spacing: f64, rate: f64) -> usize {
    match schedule(clicks, settle, spacing, rate).last() {
        Some(&last) => last + 2 * (spacing.max(0.0) * rate).round().max(1.0) as usize,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_click_starts_and_ends_at_silence_and_never_leaves_the_level_asked_for() {
        let click = shape(64, 0.1);
        assert_eq!(click.len(), 64);
        assert!(click[0].abs() < 1e-12, "it begins at silence");
        assert!(click.iter().all(|value| value.abs() <= 0.1 + 1e-12), "{:?}", click.iter().cloned().fold(0.0, f64::max));
        assert!(click.iter().any(|value| value.abs() > 0.05), "and it is not silence in the middle");
        assert!(shape(0, 0.1).is_empty());
    }

    #[test]
    fn the_click_correlates_with_itself_at_one_place_and_nowhere_near_as_well_anywhere_else() {
        // This is the whole reason for a sweep rather than a tone burst: a lag found by
        // correlation is only as trustworthy as the gap between the peak and its neighbours.
        let click = shape(64, 1.0);
        let energy: f64 = click.iter().map(|value| value * value).sum();
        let at = |lag: usize| -> f64 {
            click.iter().skip(lag).zip(click.iter()).map(|(a, b)| a * b).sum::<f64>() / energy
        };
        assert!((at(0) - 1.0).abs() < 1e-12);
        for lag in 1..48 {
            assert!(at(lag) < at(0), "a lag of {lag} samples correlates {}", at(lag));
        }
        for lag in 8..48 {
            assert!(at(lag).abs() < 0.35, "a lag of {lag} samples still correlates {}", at(lag));
        }
    }

    #[test]
    fn the_clicks_are_spaced_out_from_the_settling_time_onwards() {
        let at = schedule(4, 0.5, 0.5, 48_000.0);
        assert_eq!(at, vec![24_000, 48_000, 72_000, 96_000]);
        assert_eq!(schedule(0, 0.5, 0.5, 48_000.0), Vec::<usize>::new());
        assert_eq!(schedule(2, 0.0, 0.5, 0.0), Vec::<usize>::new(), "no rate is no schedule");
        // The run holds the last click and room for it to come back and be windowed.
        assert!(run_length(4, 0.5, 0.5, 48_000.0) > 96_000 + 64);
    }

    #[test]
    fn the_samples_played_are_the_shape_scaled_and_nothing_louder() {
        let samples = as_samples(64, 0.1);
        let loudest = samples.iter().map(|value| value.unsigned_abs()).max().expect("a click has samples");
        assert!(loudest as f64 <= 0.1 * i32::MAX as f64 + 1.0);
        assert!(loudest > (0.05 * i32::MAX as f64) as u32, "and it is loud enough to find");
    }
}
