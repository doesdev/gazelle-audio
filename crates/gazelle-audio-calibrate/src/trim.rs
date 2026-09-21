//! What a measurement means for `aggregate.json`.
//!
//! # The sign rule, which is the one thing here that is easy to get backwards
//!
//! **The interface whose copy of the click lands later is recording late, and it takes a positive
//! trim.** A device that records late has a longer path than its driver admits to; a positive trim
//! is what tells the aggregate the path is longer, and the aggregate then holds the other devices
//! back to meet it. A negative trim does the opposite and doubles the error. This was done by hand
//! at the devices on 2026-09-21 and got it wrong the first time: an offset of 28 samples, a trim of
//! -28, and the offset became 53; +28 nulled it.
//!
//! # Old plus measured
//!
//! The run happened with whatever trim was already in `aggregate.json` in force, so what was
//! measured is what is **left over** after it, not the whole of the error. The new trim is
//! therefore the old one plus the measured lag, and all three are reported so that nobody has to
//! work that out or remember which of them they are looking at.
//!
//! # A trim and the phase it was measured at
//!
//! **A trim is only true of the state its session was in.** Each interface's capture pipeline
//! settles on a different phase every session, and the driver lines every session up to the phase
//! that was measured while the trim was: the trim's phase reference. A run measures the phase and
//! moves nothing for it, so the lag it hears is the raw one, and the phase it measured beside that
//! lag is exactly the reference the new trim needs. The two are offered together and written
//! together: a new trim beside an old reference would line every session up to a state the trim
//! was never measured in.

use serde::Serialize;

use crate::measure::Reading;
use crate::rig::Direction;

/// What one device's trim was, what this run measured, and what it should become.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrimChange {
    /// The interface, as `aggregate.json` names it.
    pub device: String,
    /// Which trim this is: `input_trim` or `output_trim`.
    pub field: &'static str,
    pub direction: Direction,
    /// What was in the file while the run happened.
    pub old: i32,
    /// What this run measured, in whole samples: the lag behind the reference, rounded.
    pub measured: i32,
    /// What to write into the file: `old + measured`.
    pub new: i32,
    /// True for the interface everything else was measured against, whose trim never moves.
    pub is_reference: bool,
    /// Set when the reading was not one a trim can be made from, and why.
    pub not_applied: Option<String>,
    /// The phase reference that goes with this trim, for an input trim on an interface whose phase
    /// the driver measures. None for everything else: an output trim, because the phase is the
    /// capture pipeline's, the interface everything was measured against, and an interface with no
    /// phase setting.
    pub phase_reference: Option<PhaseReference>,
}

/// The phase a trim was measured at, in the same voice as the trim: what the file had, and what to
/// write beside the new trim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PhaseReference {
    /// What the file had. None when it had none, which is an interface whose sessions have been
    /// running on the drivers' own figures.
    pub old: Option<i32>,
    /// What to write. The phase this run measured while it measured the trim, or None when the
    /// driver heard nothing on the measurement cable, in which case writing the trim takes the old
    /// reference out rather than leaving it beside a trim it does not belong to. Always the old one
    /// when the trim itself is not offered.
    pub new: Option<i32>,
}

/// What a run found out about one interface's phase, for pairing with its trim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseAtRun {
    /// Nothing asked the driver to measure it.
    NotMeasured,
    /// It was measured at this many samples, and nothing was moved for it.
    Heard(i32),
    /// It was asked for and did not come to a number.
    NotHeard,
}

/// The trim a reading implies, given what the file already said.
///
/// The sign rule lives in the one line that adds them: a lag **behind** the reference is positive,
/// and it is **added**.
pub fn implied(reading: &Reading, direction: Direction, old: i32, is_reference: bool) -> TrimChange {
    let measured = rounded(reading.lag_samples);
    let applies = !is_reference && reading.is_usable();
    let not_applied = if is_reference {
        Some(format!(
            "{} is the interface the others were measured against, so its trim is what everything else moved to meet \
             and it does not change",
            reading.device
        ))
    } else if !reading.is_usable() {
        Some(reading.note.clone())
    } else {
        None
    };
    TrimChange {
        device: reading.device.clone(),
        field: direction.trim_field(),
        direction,
        old,
        measured: if applies { measured } else { 0 },
        new: if applies { old + measured } else { old },
        is_reference,
        not_applied,
        phase_reference: None,
    }
}

/// Put each input trim beside the phase it was measured at.
///
/// `old` is the reference the file already had for each interface, in device order, and `at_run`
/// what this run measured of each one's phase. Output trims are left alone, because the phase is
/// the capture pipeline's and nothing lines the outputs up by it.
pub fn pair_with_phases(trims: &mut [TrimChange], old: &[Option<i32>], at_run: &[PhaseAtRun]) {
    for (index, trim) in trims.iter_mut().enumerate() {
        if trim.direction != Direction::Inputs || trim.is_reference {
            continue;
        }
        let was = old.get(index).copied().flatten();
        let new = match at_run.get(index).copied().unwrap_or(PhaseAtRun::NotMeasured) {
            PhaseAtRun::NotMeasured => continue,
            _ if trim.not_applied.is_some() => was,
            PhaseAtRun::Heard(phase) => Some(phase),
            PhaseAtRun::NotHeard => None,
        };
        trim.phase_reference = Some(PhaseReference { old: was, new });
    }
}

/// A lag in whole samples, which is what the file carries. Half a sample is as close as a fixed
/// trim can get, and rounding to the nearer one is the closest it gets.
fn rounded(lag: f64) -> i32 {
    if !lag.is_finite() {
        return 0;
    }
    lag.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

/// Every device's trim, in device order.
pub fn implied_for_all(readings: &[Reading], direction: Direction, old: &[i32], reference: usize) -> Vec<TrimChange> {
    readings
        .iter()
        .enumerate()
        .map(|(index, reading)| implied(reading, direction, old.get(index).copied().unwrap_or(0), index == reference))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::{Drift, Glitches};

    fn reading(device: &str, lag: f64) -> Reading {
        Reading {
            device: device.to_string(),
            lag_samples: lag,
            spread_samples: 0.2,
            spread_limit_samples: 7.0,
            clicks_found: 8,
            clicks_expected: 8,
            glitches: Glitches::default(),
            drift: None,
            nothing_arrived: false,
            note: "measured".to_string(),
        }
    }

    #[test]
    fn the_interface_that_recorded_late_takes_a_positive_trim() {
        // This is the rule that was got backwards by hand on 2026-09-21. It has its own test
        // because getting it wrong does not fail quietly: it doubles the error.
        let late = implied(&reading("Studio+", 28.0), Direction::Inputs, 0, false);
        assert_eq!(late.measured, 28);
        assert_eq!(late.new, 28, "positive, not negative: a trim of -28 made 28 samples into 53");
        assert_eq!(late.field, "input_trim");
        assert!(late.not_applied.is_none());

        // And the one that recorded early takes a negative one, by the same arithmetic.
        let early = implied(&reading("Studio+", -28.0), Direction::Inputs, 0, false);
        assert_eq!(early.new, -28);
    }

    #[test]
    fn a_trim_that_was_already_there_is_added_to_and_never_replaced() {
        // The run happened with the old trim in force, so what came out is what is left over.
        let change = implied(&reading("Studio+", 4.0), Direction::Inputs, 28, false);
        assert_eq!((change.old, change.measured, change.new), (28, 4, 32));
        // A run that measures nothing left over leaves the trim exactly where it was.
        let settled = implied(&reading("Studio+", 0.2), Direction::Inputs, 28, false);
        assert_eq!(settled.new, 28);
        assert_eq!(settled.measured, 0);
    }

    #[test]
    fn the_reference_interfaces_trim_never_moves() {
        let change = implied(&reading("Quadro", 0.0), Direction::Inputs, 12, true);
        assert_eq!((change.old, change.new), (12, 12));
        assert!(change.is_reference);
        assert!(change.not_applied.as_deref().is_some_and(|why| why.contains("Quadro")), "{change:?}");
    }

    #[test]
    fn a_reading_a_trim_cannot_be_made_from_leaves_the_file_alone_and_says_why() {
        let mut nothing = reading("Studio+", 0.0);
        nothing.nothing_arrived = true;
        nothing.clicks_found = 0;
        nothing.note = "nothing arrived on Studio+'s input".to_string();
        let change = implied(&nothing, Direction::Inputs, 28, false);
        assert_eq!(change.new, 28, "a cable fault must never rewrite a trim");
        assert!(change.not_applied.as_deref().is_some_and(|why| why.contains("nothing arrived")));

        let mut drifting = reading("Studio+", 6.0);
        drifting.drift = Some(Drift { samples_per_second: 10.0, parts_per_million: 208.3 });
        let change = implied(&drifting, Direction::Inputs, 0, false);
        assert_eq!(change.new, 0, "a drift is not something a fixed trim can cancel");
        assert!(change.not_applied.is_some());

        // A run the audio went wrong under measured something, but not what it thinks it did.
        let mut across_a_dropout = reading("Studio+", 28.0);
        across_a_dropout.glitches = Glitches { dropped: 1, starved: 0 };
        across_a_dropout.note = "the audio was not clean while Studio+ was measured".to_string();
        let change = implied(&across_a_dropout, Direction::Inputs, 12, false);
        assert_eq!(change.new, 12, "a measurement taken across a lost block never rewrites a trim");
        assert_eq!(change.measured, 0);
        assert!(change.not_applied.as_deref().is_some_and(|why| why.contains("not clean")));

        // And so did a run whose clicks did not agree with each other.
        let mut scattered = reading("Studio+", 60.85);
        scattered.spread_samples = 64.0;
        scattered.spread_limit_samples = 15.21;
        scattered.note = "Studio+'s clicks did not agree with each other".to_string();
        let change = implied(&scattered, Direction::Inputs, 0, false);
        assert_eq!(change.new, 0, "a spread the size of the answer is not an answer");
        assert!(change.not_applied.as_deref().is_some_and(|why| why.contains("did not agree")));
    }

    /// The trim a run offers and the phase it was measured at go together, so that writing the one
    /// always writes the other.
    #[test]
    fn an_input_trim_is_offered_beside_the_phase_it_was_measured_at() {
        let readings = vec![reading("Quadro", 0.0), reading("Studio+", 60.4)];
        let mut trims = implied_for_all(&readings, Direction::Inputs, &[0, 0], 0);
        pair_with_phases(&mut trims, &[None, None], &[PhaseAtRun::NotMeasured, PhaseAtRun::Heard(-84)]);
        assert_eq!(trims[0].phase_reference, None, "the interface everything was measured against has none");
        assert_eq!(trims[1].new, 60);
        assert_eq!(trims[1].phase_reference, Some(PhaseReference { old: None, new: Some(-84) }));

        // A run in a different state measures a different lag and a different phase, and it is the
        // pair that is offered: a new trim beside the phase it was measured at, replacing the old
        // pair whole.
        let readings = vec![reading("Quadro", 0.0), reading("Studio+", -64.0)];
        let mut trims = implied_for_all(&readings, Direction::Inputs, &[0, 60], 0);
        pair_with_phases(&mut trims, &[None, Some(-84)], &[PhaseAtRun::NotMeasured, PhaseAtRun::Heard(-148)]);
        assert_eq!((trims[1].old, trims[1].new), (60, -4));
        assert_eq!(trims[1].phase_reference, Some(PhaseReference { old: Some(-84), new: Some(-148) }));
    }

    #[test]
    fn a_trim_with_no_phase_heard_beside_it_takes_the_old_reference_out() {
        let readings = vec![reading("Quadro", 0.0), reading("Studio+", 28.0)];
        let mut trims = implied_for_all(&readings, Direction::Inputs, &[0, 0], 0);
        pair_with_phases(&mut trims, &[None, Some(-84)], &[PhaseAtRun::NotMeasured, PhaseAtRun::NotHeard]);
        assert_eq!(trims[1].phase_reference, Some(PhaseReference { old: Some(-84), new: None }), "the old one belongs to the old trim");
    }

    #[test]
    fn a_trim_that_is_not_offered_leaves_its_reference_where_it_was() {
        let mut nothing = reading("Studio+", 0.0);
        nothing.nothing_arrived = true;
        let readings = vec![reading("Quadro", 0.0), nothing];
        let mut trims = implied_for_all(&readings, Direction::Inputs, &[0, 60], 0);
        pair_with_phases(&mut trims, &[None, Some(-84)], &[PhaseAtRun::NotMeasured, PhaseAtRun::Heard(-148)]);
        assert!(trims[1].not_applied.is_some());
        assert_eq!(trims[1].phase_reference, Some(PhaseReference { old: Some(-84), new: Some(-84) }));
    }

    #[test]
    fn output_trims_and_interfaces_with_no_phase_setting_carry_no_reference() {
        let readings = vec![reading("Quadro", 0.0), reading("Studio+", 28.0)];
        let mut outputs = implied_for_all(&readings, Direction::Outputs, &[0, 0], 0);
        pair_with_phases(&mut outputs, &[None, Some(-84)], &[PhaseAtRun::NotMeasured, PhaseAtRun::Heard(-148)]);
        assert!(outputs.iter().all(|trim| trim.phase_reference.is_none()), "the phase is the capture pipeline's");
        let mut unphased = implied_for_all(&readings, Direction::Inputs, &[0, 0], 0);
        pair_with_phases(&mut unphased, &[None, None], &[PhaseAtRun::NotMeasured, PhaseAtRun::NotMeasured]);
        assert!(unphased.iter().all(|trim| trim.phase_reference.is_none()));
        // And a run that could not say anything about the phases leaves every trim without one.
        pair_with_phases(&mut unphased, &[], &[]);
        assert!(unphased.iter().all(|trim| trim.phase_reference.is_none()));
    }

    #[test]
    fn measuring_the_outputs_writes_the_other_field_by_the_same_rule() {
        let change = implied(&reading("Studio+", 28.0), Direction::Outputs, 0, false);
        assert_eq!(change.field, "output_trim");
        assert_eq!(change.new, 28, "the interface that played late is late the same way round");
        assert_eq!(change.direction, Direction::Outputs);
    }

    #[test]
    fn half_a_sample_goes_to_the_nearer_one_because_the_file_holds_whole_samples() {
        assert_eq!(implied(&reading("B", 27.6), Direction::Inputs, 0, false).new, 28);
        assert_eq!(implied(&reading("B", 27.4), Direction::Inputs, 0, false).new, 27);
        assert_eq!(implied(&reading("B", -27.6), Direction::Inputs, 0, false).new, -28);
        assert_eq!(implied(&reading("B", f64::NAN), Direction::Inputs, 5, false).new, 5);
    }

    #[test]
    fn every_interface_gets_a_line_in_device_order() {
        let readings = vec![reading("Quadro", 0.0), reading("Studio+", 28.0), reading("Orion", -3.0)];
        let changes = implied_for_all(&readings, Direction::Inputs, &[0, 28, 0], 0);
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].new, 0);
        assert_eq!(changes[1].new, 56, "twenty eight already in the file, and twenty eight more left over");
        assert_eq!(changes[2].new, -3);
        assert_eq!(changes.iter().filter(|change| change.is_reference).count(), 1);
    }
}
