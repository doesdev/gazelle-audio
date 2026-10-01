//! **Where a performer playing with the click lands in a take**, and so where its Downbeat goes.
//!
//! The click is placed on the session's sample grid as it is written to the outputs, and the take is
//! the inputs as the aggregate hands them over. Neither is what happens in the room: the click is
//! heard the aggregate's output latency after it was written, and what the performer plays in time
//! with it reaches the take the aggregate's input latency after that. So a performance in time with
//! the click sits one round trip after the click's grid in the take:
//!
//! ```text
//!   written to the outputs   p
//!   heard                    p + output latency
//!   played, and recorded     p + output latency + input latency (+ the person's own offset)
//! ```
//!
//! **The marker moves, not the click.** The Downbeat cue (and the log's words about it) is put where
//! the performance lands. Playing the click a round trip early instead would mean scheduling it ahead
//! of the grid it is counted on, which a click that starts on the next block cannot do, and it would
//! still leave the files' own positions to be explained; the files are the inputs exactly as they
//! arrived, as in a DAW, and only the marker is placed by the figures.
//!
//! The figures are the ones the aggregate gives a DAW for the same purpose: each interface's own
//! driver figures, the block an interface that does not drive the callback costs, the trims in its
//! setup, and the phase measured this session, which can lengthen the input figure in the first
//! moments of a session. They are read off the aggregate between blocks ([`Reported::of`]), never on
//! the audio path, and the writer reads the latest when a take finishes. On top goes a per PC offset
//! the person sets for what no driver can know: a converter's own delay, a monitoring path, where
//! they hear the click from.

use std::sync::{Arc, Mutex};

use gazelle_aggregate::aggregate::Aggregate;
use gazelle_aggregate::config::Alignment;

/// The largest offset, either way, in milliseconds.
pub const OFFSET_MS_MAX: f64 = 100.0;

/// One interface's own driver figures, before any trim, in samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub input: i32,
    pub output: i32,
}

/// What the aggregate reports, in samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reported {
    /// The input figure in force, measured phase and all.
    pub input: i32,
    pub output: i32,
    /// What each interface's driver said, in the setup's order.
    pub interfaces: Vec<Interface>,
    /// The interfaces are not lined up, so the figures are the longest of their paths.
    pub lowest_latency: bool,
}

impl Reported {
    /// What an open aggregate reports now. Off the audio path: the host thread reads it.
    pub fn of(aggregate: &Aggregate) -> Option<Reported> {
        let (input, output) = aggregate.latencies()?;
        let plan = aggregate.plan()?;
        let interfaces = plan
            .devices
            .iter()
            .zip(aggregate.descriptions())
            .map(|(device, description)| Interface { name: device.name.clone(), input: description.latency_in, output: description.latency_out })
            .collect();
        Some(Reported { input, output, interfaces, lowest_latency: plan.alignment == Alignment::LowestLatency })
    }

    /// Whether any driver gave a figure at all. Without one the aggregate's own figures are only the
    /// blocks it adds, which says nothing about the room.
    pub fn from_the_drivers(&self) -> bool {
        self.interfaces.iter().any(|interface| interface.input > 0 || interface.output > 0)
    }
}

/// The figures of the session open now, kept up to date by its host thread.
pub type LatencySlot = Arc<Mutex<Option<Reported>>>;
/// This PC's offset, in milliseconds, which every take reads when it finishes.
pub type OffsetSlot = Arc<Mutex<f64>>;

/// Milliseconds as whole samples at `rate`. Nonsense is no offset.
pub fn offset_samples(ms: f64, rate: f64) -> i64 {
    if !(ms.is_finite() && rate.is_finite()) {
        return 0;
    }
    (ms.clamp(-OFFSET_MS_MAX, OFFSET_MS_MAX) * rate / 1000.0).round() as i64
}

/// How far the Downbeat goes after the click's own position, and the log's sentence saying why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compensation {
    pub samples: i64,
    pub words: String,
}

/// **The compensation for a take**: the aggregate's round trip and the person's offset, or the offset
/// alone when the drivers gave nothing.
pub fn compensation(reported: Option<&Reported>, offset_ms: f64, rate: f64) -> Compensation {
    let offset = offset_samples(offset_ms, rate);
    let offset_words = format!("offset {offset} ({})", ms_words(offset, rate));
    let Some(reported) = reported.filter(|reported| reported.from_the_drivers()) else {
        let samples = offset;
        return Compensation {
            samples,
            words: format!(
                "Downbeat placed {samples} samples after the click: the drivers reported no latency, so only the {offset_words} set for this PC is applied."
            ),
        };
    };
    let output = i64::from(reported.output.max(0));
    let input = i64::from(reported.input.max(0));
    let samples = output + input + offset;
    let drivers: Vec<String> = reported.interfaces.iter().map(|interface| format!("{} {} in, {} out", interface.name, interface.input, interface.output)).collect();
    let mut words = format!(
        "Downbeat placed {samples} samples after the click: output {output} + input {input} reported by the aggregate, {offset_words}. \
         The aggregate's figures are its interfaces' drivers' ({}) with a block for the interface that does not drive the callback, the trims in its setup and the phase measured this session.",
        drivers.join("; ")
    );
    if reported.lowest_latency {
        words.push_str(" The aggregate is set to lowest latency, so its interfaces are not lined up and these are its longest paths.");
    }
    Compensation { samples, words }
}

/// Samples as milliseconds, for the log.
fn ms_words(samples: i64, rate: f64) -> String {
    format!("{:.2} ms", samples as f64 * 1000.0 / rate.max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reported(input: i32, output: i32) -> Reported {
        Reported {
            input,
            output,
            interfaces: vec![Interface { name: "Quadro".into(), input: 639, output: 799 }, Interface { name: "Studio+".into(), input: 636, output: 700 }],
            lowest_latency: false,
        }
    }

    #[test]
    fn the_downbeat_goes_one_round_trip_and_the_offset_after_the_click() {
        let figures = reported(1151, 1311);
        let none = compensation(Some(&figures), 0.0, 96_000.0);
        assert_eq!(none.samples, 1311 + 1151);
        assert!(none.words.starts_with("Downbeat placed 2462 samples after the click: output 1311 + input 1151 reported by the aggregate, offset 0 (0.00 ms)."), "{}", none.words);
        assert!(none.words.contains("Quadro 639 in, 799 out; Studio+ 636 in, 700 out"), "{}", none.words);
        // An offset either way, in milliseconds, as whole samples at the session's rate.
        assert_eq!(compensation(Some(&figures), 2.5, 96_000.0).samples, 2462 + 240);
        assert_eq!(compensation(Some(&figures), -1.0, 48_000.0).samples, 2462 - 48);
        assert!(compensation(Some(&figures), -1.0, 48_000.0).words.contains("offset -48 (-1.00 ms)"));
    }

    #[test]
    fn with_no_figures_from_the_drivers_only_the_offset_counts_and_the_log_says_so() {
        for nothing in [None, Some(Reported { interfaces: vec![Interface { name: "A".into(), input: 0, output: 0 }], ..reported(64, 64) })] {
            let applied = compensation(nothing.as_ref(), 3.0, 48_000.0);
            assert_eq!(applied.samples, 144, "the aggregate's own blocks say nothing about the room");
            assert!(applied.words.contains("the drivers reported no latency, so only the offset 144 (3.00 ms)"), "{}", applied.words);
        }
    }

    #[test]
    fn the_offset_is_whole_samples_held_to_its_range_and_nonsense_is_none() {
        assert_eq!(offset_samples(0.0, 48_000.0), 0);
        assert_eq!(offset_samples(0.01, 96_000.0), 1);
        assert_eq!(offset_samples(1.0 / 44.1, 44_100.0), 1);
        assert_eq!(offset_samples(500.0, 48_000.0), 4_800, "held at 100 ms");
        assert_eq!(offset_samples(-500.0, 48_000.0), -4_800);
        assert_eq!(offset_samples(f64::NAN, 48_000.0), 0);
    }

    #[test]
    fn lowest_latency_is_said() {
        let figures = Reported { lowest_latency: true, ..reported(700, 800) };
        assert!(compensation(Some(&figures), 0.0, 48_000.0).words.contains("lowest latency"));
    }
}
