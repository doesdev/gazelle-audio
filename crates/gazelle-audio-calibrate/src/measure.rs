//! The arithmetic, with no hardware anywhere in it.
//!
//! Everything here is a function over captured samples. The captured channels came out of the
//! aggregate's own input buffers, so whatever is between them is what the aggregate has left over
//! after all of its lining up, in the aggregate's own coordinates.
//!
//! **Why correlation rather than a threshold.** A threshold answers "where did this channel first
//! cross a level", which depends on the level, on the converter's filters and on the noise floor,
//! and it answers in whole samples. Correlating the two captured channels with each other answers
//! "how far apart are these two recordings of the same event", which is the actual question, and
//! it answers in fractions of a sample once the peak is fitted.

use serde::Serialize;

/// How well two channels lined up at one click, and how far apart they were.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ClickLag {
    /// Which click of the run this was, counting from zero.
    pub click: usize,
    /// Where it was found in the reference channel, in samples from the start of the capture.
    pub at: usize,
    /// How far behind the reference this channel's copy landed. Positive is later.
    pub lag_samples: f64,
    /// The correlation at the peak, between zero and one. A weak one is a channel with something
    /// on it that is not this click.
    pub strength: f64,
}

/// A lag that grows steadily across a run, which is not an offset at all.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Drift {
    /// How many samples the lag grows by every second.
    pub samples_per_second: f64,
    /// The same thing as the two interfaces' clocks differing, in parts per million.
    pub parts_per_million: f64,
}

/// One device's whole result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reading {
    /// The interface, as `aggregate.json` names it.
    pub device: String,
    /// The lag of this device's copy behind the reference's, in samples: the median of the clicks,
    /// with a fraction. Positive means this device recorded late.
    pub lag_samples: f64,
    /// Widest and narrowest click of the run, apart. Small means every click agreed.
    pub spread_samples: f64,
    pub clicks_found: usize,
    pub clicks_expected: usize,
    /// Set when the lag grows steadily rather than staying put, which is a clocking fault and not
    /// something a trim can fix.
    pub drift: Option<Drift>,
    /// True when no click was found at all, which is a cable and not a measurement.
    pub nothing_arrived: bool,
    /// One sentence for the person reading it.
    pub note: String,
}

impl Reading {
    /// Whether this reading is worth turning into a trim: something arrived, enough clicks agreed,
    /// and the two interfaces are not running off each other's clocks.
    pub fn is_usable(&self) -> bool {
        !self.nothing_arrived && self.drift.is_none() && self.clicks_found >= 2
    }
}

/// How well a correlation has to land before it counts as this click rather than something else on
/// the channel.
pub const WEAKEST_PEAK: f64 = 0.2;

/// Where in `capture` the click is, searching from `from` for `horizon` samples.
///
/// The click's own shape is what is looked for here, because at this point nothing is known about
/// where the round trip put it. What comes back is used only to place the window; the lag itself
/// is measured between the captured channels, never against the shape.
pub fn find_click(capture: &[f64], click: &[f64], from: usize, horizon: usize) -> Option<usize> {
    if click.is_empty() || from >= capture.len() {
        return None;
    }
    let click_energy: f64 = click.iter().map(|value| value * value).sum();
    if click_energy <= 0.0 {
        return None;
    }
    let last = (from + horizon).min(capture.len().saturating_sub(click.len()));
    let mut best: Option<(f64, usize)> = None;
    for at in from..=last {
        let window = &capture[at..at + click.len()];
        let energy: f64 = window.iter().map(|value| value * value).sum();
        if energy <= 0.0 {
            continue;
        }
        let dot: f64 = window.iter().zip(click).map(|(a, b)| a * b).sum();
        let score = (dot / (energy * click_energy).sqrt()).abs();
        if best.is_none_or(|(had, _)| score > had) {
            best = Some((score, at));
        }
    }
    best.filter(|&(score, _)| score >= WEAKEST_PEAK).map(|(_, at)| at)
}

/// How far `channel` is behind `reference` around one click, to a fraction of a sample.
///
/// The window is `[at, at + length)` of the reference, and `channel` is slid over `search` samples
/// either way. A positive answer means this channel's copy landed later.
pub fn lag_between(reference: &[f64], channel: &[f64], at: usize, length: usize, search: usize) -> Option<(f64, f64)> {
    if length == 0 || at + length > reference.len() {
        return None;
    }
    let window = &reference[at..at + length];
    let reference_energy: f64 = window.iter().map(|value| value * value).sum();
    if reference_energy <= 0.0 {
        return None;
    }
    let search = search as isize;
    let mut scores = vec![0.0f64; (2 * search + 1) as usize];
    for (index, score) in scores.iter_mut().enumerate() {
        let lag = index as isize - search;
        let start = at as isize + lag;
        if start < 0 || start as usize + length > channel.len() {
            continue;
        }
        let against = &channel[start as usize..start as usize + length];
        let energy: f64 = against.iter().map(|value| value * value).sum();
        if energy <= 0.0 {
            continue;
        }
        let dot: f64 = window.iter().zip(against).map(|(a, b)| a * b).sum();
        *score = dot / (reference_energy * energy).sqrt();
    }
    let (peak, strength) = scores.iter().enumerate().fold((0usize, f64::MIN), |(had, best), (index, &score)| {
        if score > best {
            (index, score)
        } else {
            (had, best)
        }
    });
    if strength < WEAKEST_PEAK {
        return None;
    }
    // A peak at the very edge of the search is a peak that was cut off, and fitting it would
    // invent a fraction of a sample from one side of a curve.
    if peak == 0 || peak + 1 >= scores.len() {
        return None;
    }
    let fraction = interpolate(scores[peak - 1], scores[peak], scores[peak + 1]);
    Some(((peak as isize - search) as f64 + fraction, strength))
}

/// Where the top of the curve really is, from the three samples around it: a parabola through
/// them, and its vertex. This is what turns whole samples into a fraction of one.
pub fn interpolate(before: f64, peak: f64, after: f64) -> f64 {
    let curve = before - 2.0 * peak + after;
    if curve.abs() < f64::EPSILON {
        return 0.0;
    }
    // The vertex of the parabola through the three points, which cannot be more than half a sample
    // either side of the middle one when the middle one is the largest.
    (0.5 * (before - after) / curve).clamp(-0.5, 0.5)
}

/// Every click of one channel against the reference.
///
/// `emits` are where the clicks were played, in the capture's own coordinates; the round trip puts
/// them back somewhat later, which is what `horizon` leaves room for. `horizon` has to be shorter
/// than the gap between clicks, or a search for one click can find the next one instead.
pub fn lags_of_channel(
    reference: &[f64],
    channel: &[f64],
    click: &[f64],
    emits: &[usize],
    horizon: usize,
    search: usize,
) -> Vec<ClickLag> {
    let length = click.len() * 4;
    let mut found = Vec::new();
    for (index, &emit) in emits.iter().enumerate() {
        let Some(at) = find_click(reference, click, emit, horizon) else { continue };
        // A window that starts a little before the click holds the converter's pre-ring as well as
        // the click itself, and both of them are the same event on both channels.
        let start = at.saturating_sub(click.len());
        let Some((lag, strength)) = lag_between(reference, channel, start, length, search) else { continue };
        found.push(ClickLag { click: index, at, lag_samples: lag, strength });
    }
    found
}

/// The middle of a set of numbers, which is what a run of clicks is reported by: one click that
/// landed on a glitch moves a mean and does not move this.
pub fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

/// A line through the per click lags, when the slope is real.
///
/// **A slope means the interfaces are not sharing a clock.** Two converters running off their own
/// crystals drift apart for as long as they run, and no fixed trim can cancel something that
/// grows: the cure is a word clock or a digital cable between them, not a number in a file. So the
/// slope is only reported when it is far larger than the scatter of the clicks around it, because
/// calling ordinary jitter a drift would send somebody looking for a fault that is not there.
pub fn fit_drift(lags: &[ClickLag], rate: f64) -> Option<Drift> {
    if lags.len() < 4 || !(rate.is_finite() && rate > 0.0) {
        return None;
    }
    let seconds: Vec<f64> = lags.iter().map(|lag| lag.at as f64 / rate).collect();
    let values: Vec<f64> = lags.iter().map(|lag| lag.lag_samples).collect();
    let n = lags.len() as f64;
    let mean_x = seconds.iter().sum::<f64>() / n;
    let mean_y = values.iter().sum::<f64>() / n;
    let mut top = 0.0;
    let mut bottom = 0.0;
    for (x, y) in seconds.iter().zip(&values) {
        top += (x - mean_x) * (y - mean_y);
        bottom += (x - mean_x) * (x - mean_x);
    }
    if bottom <= 0.0 {
        return None;
    }
    let slope = top / bottom;
    let intercept = mean_y - slope * mean_x;
    let scatter = (seconds
        .iter()
        .zip(&values)
        .map(|(x, y)| {
            let left = y - (slope * x + intercept);
            left * left
        })
        .sum::<f64>()
        / n)
        .sqrt();
    let span = seconds.last().copied().unwrap_or(0.0) - seconds.first().copied().unwrap_or(0.0);
    let grew = (slope * span).abs();
    // A whole sample across the run at the very least, and three times whatever the clicks were
    // scattered by: less than that is a run of numbers that happen to lean.
    if grew < 1.0 || grew < 3.0 * scatter {
        return None;
    }
    Some(Drift { samples_per_second: slope, parts_per_million: slope / rate * 1_000_000.0 })
}

/// One channel's whole result, written the way a person reads it.
pub fn summarise(device: &str, lags: &[ClickLag], expected: usize, rate: f64) -> Reading {
    let values: Vec<f64> = lags.iter().map(|lag| lag.lag_samples).collect();
    let lag = median(&values);
    let spread = match (values.iter().cloned().reduce(f64::min), values.iter().cloned().reduce(f64::max)) {
        (Some(low), Some(high)) => high - low,
        _ => 0.0,
    };
    let drift = fit_drift(lags, rate);
    let nothing_arrived = lags.is_empty();
    let note = note_for(device, lag, spread, lags.len(), expected, drift, nothing_arrived);
    Reading {
        device: device.to_string(),
        lag_samples: lag,
        spread_samples: spread,
        clicks_found: lags.len(),
        clicks_expected: expected,
        drift,
        nothing_arrived,
        note,
    }
}

/// The sentence that goes with a reading.
fn note_for(
    device: &str,
    lag: f64,
    spread: f64,
    found: usize,
    expected: usize,
    drift: Option<Drift>,
    nothing_arrived: bool,
) -> String {
    if nothing_arrived {
        return format!(
            "nothing arrived on {device}'s input, so there is nothing to measure: check the cable, and that the \
             channel it is plugged into is the one this run was told about"
        );
    }
    if let Some(drift) = drift {
        return format!(
            "{device} drifts {:.1} samples a second against the reference, which is {:.1} parts per million: these \
             two interfaces are not sharing a clock, and no trim can cancel something that keeps growing. Lock them \
             together with a word clock or a digital cable and measure again.",
            drift.samples_per_second, drift.parts_per_million
        );
    }
    let missing = if found < expected {
        format!(" {found} of the {expected} clicks were found, so treat this as the weaker sort of measurement, and")
    } else {
        String::new()
    };
    let direction = if lag >= 0.0 { "behind" } else { "ahead of" };
    format!(
        "{device} recorded {:.2} samples {direction} the reference, and the clicks agreed to within {spread:.2} \
         samples.{missing}",
        lag.abs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click;

    const RATE: f64 = 48_000.0;

    /// A capture made of data: a click at each of these places, and silence elsewhere.
    fn planted(length: usize, click: &[f64], at: &[f64]) -> Vec<f64> {
        let mut capture = vec![0.0; length];
        for &position in at {
            // A fractional position is the click resampled, which is what a converter hands back
            // when the two devices are not on the same sample.
            let whole = position.floor() as usize;
            let fraction = position - whole as f64;
            for (index, &sample) in click.iter().enumerate() {
                let at = whole + index;
                if at + 1 >= capture.len() {
                    break;
                }
                // A fraction of a sample later is a weighted pair with the sample **before** it,
                // which is the click arriving between two samples rather than on one.
                let earlier = if index == 0 { 0.0 } else { click[index - 1] };
                capture[at] += sample * (1.0 - fraction) + earlier * fraction;
            }
        }
        capture
    }

    /// Noise that is the same every run, so a failure is a failure and not a bad afternoon.
    fn noise(length: usize, level: f64, seed: u64) -> Vec<f64> {
        let mut state = seed | 1;
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 2.0 * level
            })
            .collect()
    }

    fn add(a: &[f64], b: &[f64]) -> Vec<f64> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    fn emits() -> Vec<usize> {
        click::schedule(8, 0.1, 0.05, RATE)
    }

    /// The reference and one channel, the channel `lag` samples behind it, with the round trip
    /// putting both of them `trip` samples after they were played.
    fn pair(lag: f64, trip: f64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let click = click::shape(64, 0.5);
        let length = click::run_length(8, 0.1, 0.05, RATE);
        let emitted = emits();
        let reference: Vec<f64> = emitted.iter().map(|&at| at as f64 + trip).collect();
        let behind: Vec<f64> = emitted.iter().map(|&at| at as f64 + trip + lag).collect();
        (planted(length, &click, &reference), planted(length, &click, &behind), click)
    }

    #[test]
    fn a_lag_put_in_on_purpose_comes_back_out_to_the_sample() {
        let (reference, channel, click) = pair(28.0, 900.0);
        let lags = lags_of_channel(&reference, &channel, &click, &emits(), 1200, 512);
        assert_eq!(lags.len(), 8, "every click was found");
        let reading = summarise("Studio+", &lags, 8, RATE);
        assert!((reading.lag_samples - 28.0).abs() < 0.05, "{:?}", reading);
        assert!(reading.spread_samples < 0.05);
        assert_eq!(reading.drift, None);
        assert!(reading.is_usable());
        assert!(reading.note.contains("behind"), "{}", reading.note);
    }

    #[test]
    fn a_lag_the_other_way_round_is_a_negative_number_and_says_so() {
        let (reference, channel, click) = pair(-17.0, 900.0);
        let lags = lags_of_channel(&reference, &channel, &click, &emits(), 1200, 512);
        let reading = summarise("Studio+", &lags, 8, RATE);
        assert!((reading.lag_samples + 17.0).abs() < 0.05, "{:?}", reading);
        assert!(reading.note.contains("ahead of"), "{}", reading.note);
    }

    #[test]
    fn a_lag_that_is_not_a_whole_number_of_samples_comes_back_with_its_fraction() {
        let (reference, channel, click) = pair(12.5, 900.0);
        let lags = lags_of_channel(&reference, &channel, &click, &emits(), 1200, 512);
        let reading = summarise("Studio+", &lags, 8, RATE);
        assert!((reading.lag_samples - 12.5).abs() < 0.2, "{:?}", reading);
        assert!(reading.lag_samples.fract().abs() > 0.0, "a whole number here would mean the fitting did nothing");
    }

    #[test]
    fn a_lag_that_grows_across_the_run_is_reported_as_two_clocks_and_not_as_an_offset() {
        let click = click::shape(64, 0.5);
        let length = click::run_length(8, 0.1, 0.05, RATE);
        let emitted = emits();
        let reference: Vec<f64> = emitted.iter().map(|&at| at as f64 + 900.0).collect();
        // Ten samples a second apart, which is about two hundred parts per million: two interfaces
        // each running off their own crystal.
        let drifting: Vec<f64> =
            emitted.iter().map(|&at| at as f64 + 900.0 + 10.0 * (at as f64 / RATE)).collect();
        let reference = planted(length, &click, &reference);
        let channel = planted(length, &click, &drifting);
        let lags = lags_of_channel(&reference, &channel, &click, &emitted, 1200, 512);
        let reading = summarise("Studio+", &lags, 8, RATE);
        let drift = reading.drift.expect("a lag that grows is a drift");
        assert!((drift.samples_per_second - 10.0).abs() < 0.5, "{drift:?}");
        assert!((drift.parts_per_million - 208.3).abs() < 15.0, "{drift:?}");
        assert!(!reading.is_usable(), "a drift is never turned into a trim");
        assert!(reading.note.contains("sharing a clock"), "{}", reading.note);
    }

    #[test]
    fn an_ordinary_steady_lag_is_never_called_a_drift() {
        let (reference, channel, click) = pair(28.0, 900.0);
        let lags = lags_of_channel(&reference, &channel, &click, &emits(), 1200, 512);
        assert_eq!(fit_drift(&lags, RATE), None);
        // Nor is a lag that wobbles about without going anywhere.
        let wobbly: Vec<ClickLag> = (0..8)
            .map(|index| ClickLag {
                click: index,
                at: index * 2400,
                lag_samples: 28.0 + if index % 2 == 0 { 0.4 } else { -0.4 },
                strength: 0.9,
            })
            .collect();
        assert_eq!(fit_drift(&wobbly, RATE), None);
    }

    #[test]
    fn a_channel_with_nothing_on_it_is_a_cable_and_says_so() {
        let (reference, _, click) = pair(28.0, 900.0);
        let silence = vec![0.0; reference.len()];
        let lags = lags_of_channel(&reference, &silence, &click, &emits(), 1200, 512);
        assert!(lags.is_empty());
        let reading = summarise("Studio+", &lags, 8, RATE);
        assert!(reading.nothing_arrived);
        assert!(!reading.is_usable());
        assert_eq!(reading.clicks_found, 0);
        assert!(reading.note.contains("cable"), "{}", reading.note);
        // Hiss with no click in it is the same answer: there is nothing there to line up with.
        let hiss = noise(reference.len(), 0.01, 7);
        let lags = lags_of_channel(&reference, &hiss, &click, &emits(), 1200, 512);
        assert!(summarise("Studio+", &lags, 8, RATE).lag_samples.abs() < 512.0);
        assert!(lags.iter().all(|lag| lag.strength < 0.5), "hiss does not correlate with a click");
    }

    #[test]
    fn noise_on_both_channels_does_not_move_the_answer() {
        let (reference, channel, click) = pair(28.0, 900.0);
        // A click at a tenth of full scale against noise at a thousandth: an ordinary preamp.
        let reference = add(&reference, &noise(reference.len(), 0.01, 11));
        let channel = add(&channel, &noise(channel.len(), 0.01, 29));
        let lags = lags_of_channel(&reference, &channel, &click, &emits(), 1200, 512);
        let reading = summarise("Studio+", &lags, 8, RATE);
        assert_eq!(reading.clicks_found, 8, "noise does not lose the clicks");
        assert!((reading.lag_samples - 28.0).abs() < 0.5, "{:?}", reading);
        assert!(reading.is_usable());
    }

    #[test]
    fn the_peak_is_fitted_between_samples_rather_than_rounded_to_one() {
        // A symmetrical peak is exactly where it looks; a lopsided one is between two samples.
        assert_eq!(interpolate(0.5, 1.0, 0.5), 0.0);
        assert!(interpolate(0.9, 1.0, 0.5) < 0.0, "the weight is on the earlier side");
        assert!(interpolate(0.5, 1.0, 0.9) > 0.0);
        // Three points with no curve at all cannot say where a top is, so it says nothing.
        assert_eq!(interpolate(1.0, 1.0, 1.0), 0.0);
        assert!(interpolate(0.0, 1.0, 0.99).abs() <= 0.5, "and it never lands outside the samples it was given");
    }

    #[test]
    fn the_middle_of_the_clicks_is_what_is_reported_and_one_bad_one_does_not_move_it() {
        assert_eq!(median(&[]), 0.0);
        assert_eq!(median(&[3.0]), 3.0);
        assert_eq!(median(&[1.0, 3.0]), 2.0);
        assert_eq!(median(&[28.0, 27.9, 28.1, 28.0, 400.0]), 28.0);
    }
}
