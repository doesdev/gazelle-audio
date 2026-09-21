//! Measuring where an interface's capture pipeline actually started, and lining every session up
//! to the one its trim was measured in.
//!
//! **What this is for.** Two interfaces locked by a digital cable record a fixed number of samples
//! apart within one session, and between sessions that number moves in steps of 32 samples. The
//! vendor drivers report the same latency figures on every open, so the aggregate's own padding is
//! the same every run and is not the cause: it is each interface's capture pipeline settling on a
//! different phase when its stream starts. Measured at the devices on 2026-09-21, at 64, 128 and
//! 256 sample buffers, with the vendor drivers' Safe Mode on and off. Witness channels settled what
//! it belongs to: an interface's analogue input and its S/PDIF input moved together in all six
//! runs. **The phase belongs to the capture pipeline as a whole**, which is why a measurement taken
//! on the digital input corrects the analogue inputs too.
//!
//! **Three numbers, and they are not the same thing.** The trim is the constant a person measured
//! once with a click, written in the configuration file. The reference is the phase this driver
//! measured in the session that trim was measured in, written beside it. The phase is what each
//! session measures. A trim is only true of the state its session was in, so every session is put
//! back into that state first, by the reference minus the phase, and then the trim applies.
//!
//! **Why not a grid.** The first version of this asked whether the measured phase was itself near
//! a whole multiple of 32, and refused every real measurement: the value carries a large constant
//! of its own (the cable, the difference between the interfaces' converters, where the reading is
//! taken), and only the change between sessions moves in steps. Those steps were seen at 63, 64,
//! 159 and 160 samples, which is 32 sample steps plus a sample of wobble that the measurement
//! caught, so the correction is applied exactly and never rounded.
//!
//! **What runs where.** [`Detector`] runs on the audio path and does nothing that is forbidden
//! there: it writes a short burst into a buffer that was already allocated, looks for it in
//! another, and does integer arithmetic. It allocates nothing, locks nothing, logs nothing and
//! calls no vendor driver. Turning what it found into a line of the event log is the watcher
//! thread's work, exactly as it is for a device that stalled.

use gazelle_audio_aggregate_status::record::phase as codes;

/// How the hardware moves between sessions: in steps of this many samples. A change from the
/// reference that is nowhere near a whole number of them is a measurement of something else.
pub const STEP: i32 = 32;

/// How far from a whole number of [`STEP`]s a change may land and still be believed. The hardware
/// showed one sample of wobble either way (changes of 63, 64, 159 and 160); this allows a few, and
/// the wobble itself is corrected, because the correction is never rounded.
pub const WOBBLE: i32 = 4;

/// The largest correction this driver will make, either way, which is the room every input delay
/// is given for it. Steps of 32 well inside this were what the hardware showed; a correction past
/// it is far likelier to be the measurement finding something that is not the signal.
pub const LIMIT: i32 = 512;

/// How loud the measurement signal is: about 42 dB below full scale. The path is a digital cable,
/// so nothing is gained by making it louder, and this is quiet enough that a monitor left up on
/// the measurement channel would not be startled by it.
pub const AMPLITUDE: i32 = 1 << 24;

/// How much of the signal has to arrive before a sample counts as the signal. An eighth of what
/// was sent, which nothing on a silent digital channel comes near.
pub const THRESHOLD: i32 = AMPLITUDE / 8;

/// How many samples the signal lasts: long enough to survive a converter in the path, short enough
/// that nobody would call it a sound.
pub const BURST: usize = 4;

/// How many of the master's blocks go by before the signal is sent. The rings and both devices'
/// callbacks are in step by then, and nothing is being recorded this early in a session.
pub const SETTLE_BLOCKS: u64 = 4;

/// How long the driver listens for the signal before giving up, as a fraction of a second. The
/// whole measurement is over inside the first fraction of a second of a session.
pub const WINDOW_SECONDS: f64 = 0.25;

/// The smallest number of blocks to listen for, whatever the rate and the block size work out to.
pub const MIN_WINDOW_BLOCKS: u64 = 8;

/// How many blocks a measurement listens for at this rate and block size. Worked out when the
/// buffers are made, never on a callback.
pub fn window_blocks(rate: f64, block: usize) -> u64 {
    if !(rate.is_finite() && rate > 0.0) || block == 0 {
        return MIN_WINDOW_BLOCKS;
    }
    let blocks = (rate * WINDOW_SECONDS / block as f64).ceil();
    (blocks.max(MIN_WINDOW_BLOCKS as f64) as u64).min(u32::MAX as u64)
}

/// What a session does with what it measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    /// Line the interface up to this reference: the phase measured in the session its trim was
    /// measured in. `None` is an interface that has never had one, and it is left where it is.
    LineUpTo(Option<i32>),
    /// Measure it and move nothing, which is what a calibration run does. The click lag a run
    /// measures becomes the trim and the phase measured beside it becomes that trim's reference,
    /// so the lag has to be the raw one, on the drivers' own figures. A run that lined itself up
    /// first would measure a trim on top of a correction made from the old reference, and the pair
    /// it wrote down would describe a state no session is ever in.
    MeasureOnly,
}

/// What a measurement came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It was used, and this is what to add to the interface's input path: what was measured
    /// minus the reference, which holds the interface back by the reference minus what was
    /// measured.
    Applied(i32),
    /// It was measured, and there is no reference to line it up to, so nothing is moved.
    Unreferenced,
    /// It was measured, and a calibration run keeps it rather than using it.
    MeasuredOnly,
    /// It was not believed, and this is why. Nothing is corrected: a refusal is not a correction of
    /// zero.
    Refused(Refusal),
}

/// Why a measurement was not believed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The window went by and nothing came back on the measurement channel.
    NotHeard,
    /// Something arrived, and how far it had moved from the reference was nowhere near a whole
    /// number of 32 sample steps.
    OffTheGrid,
    /// Lining it up would have taken more than the room the delays have.
    TooFar,
}

impl Refusal {
    pub fn code(self) -> u32 {
        match self {
            Refusal::NotHeard => codes::NOT_HEARD,
            Refusal::OffTheGrid => codes::OFF_THE_GRID,
            Refusal::TooFar => codes::TOO_FAR,
        }
    }
}

impl Outcome {
    /// The code this outcome goes into the shared record as.
    pub fn code(self) -> u32 {
        match self {
            Outcome::Applied(_) => codes::APPLIED,
            Outcome::Unreferenced => codes::NO_REFERENCE,
            Outcome::MeasuredOnly => codes::MEASURED_ONLY,
            Outcome::Refused(why) => why.code(),
        }
    }

    /// What is actually added to the interface's input path, which is nothing at all unless the
    /// measurement was used.
    pub fn applied(self) -> i32 {
        match self {
            Outcome::Applied(samples) => samples,
            _ => 0,
        }
    }
}

/// One measurement, start to finish: what it came to, and what became of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurement {
    /// How far the interface's capture landed from where the drivers' own figures put it, in
    /// samples, exactly. Zero when nothing arrived at all, where it means nothing was measured
    /// rather than that nothing was out.
    pub measured: i32,
    pub outcome: Outcome,
}

impl Measurement {
    pub fn code(self) -> u32 {
        self.outcome.code()
    }

    pub fn applied(self) -> i32 {
        self.outcome.applied()
    }
}

/// **What to do with a measurement.** The whole of the policy, and it is pure: numbers in, an
/// answer out.
///
/// `measured` is how far the interface's capture landed from where the drivers' own figures said
/// it would. It carries a large constant of its own, so nothing is judged by its size: what is
/// judged is how far it moved from the reference, which is the one thing that moves in steps.
pub fn decide(measured: i32, how: Use) -> Outcome {
    let reference = match how {
        Use::MeasureOnly => return Outcome::MeasuredOnly,
        Use::LineUpTo(None) => return Outcome::Unreferenced,
        Use::LineUpTo(Some(reference)) => reference,
    };
    // Exact, because the sample of wobble in it is real and the measurement caught it.
    let moved = measured.saturating_sub(reference);
    if moved.saturating_abs() > LIMIT {
        return Outcome::Refused(Refusal::TooFar);
    }
    let steps = (moved as f64 / STEP as f64).round() as i32;
    if moved.saturating_sub(steps.saturating_mul(STEP)).saturating_abs() > WOBBLE {
        return Outcome::Refused(Refusal::OffTheGrid);
    }
    Outcome::Applied(moved)
}

/// The line the event log keeps, which is the only thing that survives the session.
///
/// Everything it says is worked out from the three numbers the record carries: for a measurement
/// that was used, the reference is what was measured less what was applied.
pub fn detail(device: &str, state: u32, measured: i32, applied: i32) -> String {
    let reference = measured.saturating_sub(applied);
    match state {
        codes::APPLIED if applied == 0 => format!(
            "{device} was measured at {measured} samples from where its driver's figures put it, which is where it was when its trim was measured, so nothing was moved"
        ),
        codes::APPLIED if applied < 0 => format!(
            "{device} was measured at {measured} samples from where its driver's figures put it, against {reference} when its trim was measured, so it was held back by {} samples to put it back where its trim holds",
            applied.saturating_neg()
        ),
        codes::APPLIED => format!(
            "{device} was measured at {measured} samples from where its driver's figures put it, against {reference} when its trim was measured, so it was brought {applied} samples earlier to put it back where its trim holds"
        ),
        codes::NO_REFERENCE => format!(
            "{device} was measured at {measured} samples from where its driver's figures put it, and was not lined up: there is no phase from the session its trim was measured in to line it up to. Measure the interfaces once on the Aggregate page and every session after that is lined up. The session ran on the figures the drivers reported."
        ),
        codes::MEASURED_ONLY => format!(
            "{device} was measured at {measured} samples from where its driver's figures put it, and was left there on purpose: this session was measuring the trim, and a trim is measured on the drivers' own figures"
        ),
        codes::NOT_HEARD => format!(
            "{device} was not lined up: nothing arrived on its measurement channel. Check the cable and the channels the file names, or take the phase setting out. The session ran on the figures the drivers reported."
        ),
        codes::OFF_THE_GRID => format!(
            "{device} was not lined up: it measured {measured} samples, and how far that is from the phase its trim was measured at is not a whole number of {STEP} sample steps, which is the only way the interfaces move. The session ran on the figures the drivers reported."
        ),
        codes::TOO_FAR => format!(
            "{device} was not lined up: it measured {measured} samples, which is further from the phase its trim was measured at than the {LIMIT} samples the driver keeps room for. The session ran on the figures the drivers reported."
        ),
        _ => format!("{device} was not phase measured"),
    }
}

/// One follower's measurement, as the audio path runs it.
///
/// The master's callback sends the signal once, at a block it chooses, and then looks at the
/// interface's measurement channel on every block until it finds it or the window closes. All of
/// that is arithmetic over buffers that already exist.
pub struct Detector {
    /// The master's output slot the burst is written into.
    pub master_slot: usize,
    /// The follower's input slot the burst is listened for on.
    pub input_slot: usize,
    /// Samples in one block, which is what turns a block number into a sample position.
    block: i64,
    /// The block the burst goes out on.
    emit_at: u64,
    /// The last block worth listening on.
    give_up_after: u64,
    /// Where the burst left, in samples of the master's own stream.
    sent_at: i64,
    /// What the drivers' own figures say the signal should take to come back.
    expected: i32,
    /// What this session does with what it finds.
    how: Use,
    state: State,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Nothing has been sent yet.
    Waiting,
    /// The burst has gone out and nothing has come back.
    Listening,
    /// It is over, one way or the other.
    Done,
}

impl Detector {
    /// A measurement of one follower. `expected` is how long the drivers' own figures say the
    /// signal takes to come back: the master's reported output latency, the follower's reported
    /// input latency, and the block the aggregate's own ring costs. `how` is what the session does
    /// with the answer.
    pub fn new(master_slot: usize, input_slot: usize, block: usize, expected: i32, window: u64, how: Use) -> Detector {
        Detector {
            master_slot,
            input_slot,
            block: block as i64,
            emit_at: SETTLE_BLOCKS,
            give_up_after: SETTLE_BLOCKS.saturating_add(window),
            sent_at: 0,
            expected,
            how,
            state: State::Waiting,
        }
    }

    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// Ready to measure again, which is what a DAW starting the audio a second time without
    /// letting the buffers go is. Nothing is allocated: it is the same detector.
    pub fn rearm(&mut self) {
        self.state = State::Waiting;
        self.sent_at = 0;
    }

    /// Whether the burst goes out on this block, and where in the master's own stream it will
    /// leave: the block it is written into plus however far the master's outputs are held back.
    pub fn emits_on(&mut self, block_number: u64, pad_out: i32) -> bool {
        if self.state != State::Waiting || block_number < self.emit_at {
            return false;
        }
        self.sent_at = block_number as i64 * self.block + pad_out as i64;
        self.state = State::Listening;
        true
    }

    /// Write the burst into one run of the master's output stage.
    pub fn write_burst(run: &mut [i32]) {
        for sample in run.iter_mut().take(BURST) {
            *sample = AMPLITUDE;
        }
    }

    /// Look for the burst in one block of the follower's own inputs, as the device handed it over
    /// and before anything holds it back. Answers what the measurement came to, once.
    pub fn listen(&mut self, run: &[i32], block_number: u64) -> Option<Measurement> {
        if self.state != State::Listening {
            return None;
        }
        if let Some(at) = run.iter().position(|sample| sample.saturating_abs() >= THRESHOLD) {
            self.state = State::Done;
            let arrived = block_number as i64 * self.block + at as i64;
            let measured = (arrived - self.sent_at - self.expected as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            return Some(Measurement { measured, outcome: decide(measured, self.how) });
        }
        if block_number >= self.give_up_after {
            self.state = State::Done;
            return Some(Measurement { measured: 0, outcome: Outcome::Refused(Refusal::NotHeard) });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference the table below lines every session up to: what the phase measured in the
    /// run whose click lag was +60.4 samples.
    const REFERENCE: i32 = -84;

    #[test]
    fn a_session_is_lined_up_to_its_reference_exactly_and_never_rounded() {
        let to = Use::LineUpTo(Some(REFERENCE));
        assert_eq!(decide(REFERENCE, to), Outcome::Applied(0), "the state the trim was measured in moves nothing");
        // What the hardware did, sample for sample: changes of 64, 63, 223 and 160, which are
        // steps of 32 and the sample of wobble the measurement caught.
        assert_eq!(decide(-148, to), Outcome::Applied(-64));
        assert_eq!(decide(-147, to), Outcome::Applied(-63), "the wobble is corrected, not rounded away");
        assert_eq!(decide(-307, to), Outcome::Applied(-223));
        assert_eq!(decide(-244, to), Outcome::Applied(-160));
        // And the other way, for a session that settled later than the one the trim was measured in.
        assert_eq!(decide(REFERENCE + 159, to), Outcome::Applied(159));
        assert_eq!(decide(REFERENCE + 32 + WOBBLE, to), Outcome::Applied(32 + WOBBLE));
    }

    #[test]
    fn the_size_of_the_measurement_itself_means_nothing_only_its_change_does() {
        // Every one of these was refused by the rule that asked whether the value itself was near
        // a multiple of 32. Measured against a reference of their own, each is simply a session in
        // the state its trim was measured in.
        for measured in [-84, -148, -147, -307, -244, 45, 17] {
            assert_eq!(decide(measured, Use::LineUpTo(Some(measured))), Outcome::Applied(0), "{measured}");
        }
    }

    #[test]
    fn with_no_reference_nothing_is_moved_and_that_is_not_a_refusal() {
        let answer = decide(-148, Use::LineUpTo(None));
        assert_eq!(answer, Outcome::Unreferenced);
        assert_eq!(answer.applied(), 0);
        assert_eq!(answer.code(), codes::NO_REFERENCE);
        assert!(!codes::is_refused(answer.code()), "the measurement was fine: there is nothing to line it up to");
    }

    #[test]
    fn a_calibration_run_measures_and_moves_nothing_whatever_it_measured() {
        for measured in [0, -148, 4000, i32::MIN] {
            let answer = decide(measured, Use::MeasureOnly);
            assert_eq!(answer, Outcome::MeasuredOnly, "{measured}");
            assert_eq!(answer.applied(), 0);
            assert_eq!(answer.code(), codes::MEASURED_ONLY);
        }
    }

    #[test]
    fn a_change_nowhere_near_a_whole_number_of_steps_is_refused_rather_than_used() {
        let to = Use::LineUpTo(Some(REFERENCE));
        assert_eq!(decide(REFERENCE + 16, to), Outcome::Refused(Refusal::OffTheGrid), "halfway is not near anything");
        assert_eq!(decide(REFERENCE - 32 - WOBBLE - 1, to), Outcome::Refused(Refusal::OffTheGrid));
        assert_eq!(decide(REFERENCE + 45, to), Outcome::Refused(Refusal::OffTheGrid));
        // And a refusal corrects nothing at all, which is not the same as correcting by zero.
        assert_eq!(decide(REFERENCE + 16, to).applied(), 0);
        assert_eq!(decide(REFERENCE + 16, to).code(), codes::OFF_THE_GRID);
    }

    #[test]
    fn a_correction_past_the_room_the_delays_have_is_refused() {
        let to = Use::LineUpTo(Some(REFERENCE));
        assert_eq!(decide(REFERENCE + LIMIT, to), Outcome::Applied(LIMIT), "the edge of the room is still inside it");
        assert_eq!(decide(REFERENCE - LIMIT, to), Outcome::Applied(-LIMIT));
        assert_eq!(decide(REFERENCE + LIMIT + STEP, to), Outcome::Refused(Refusal::TooFar));
        assert_eq!(decide(REFERENCE - LIMIT - STEP, to), Outcome::Refused(Refusal::TooFar));
        assert_eq!(decide(i32::MAX, to), Outcome::Refused(Refusal::TooFar), "and nothing overflows on the way there");
        assert_eq!(decide(i32::MIN, to), Outcome::Refused(Refusal::TooFar));
        assert_eq!(decide(0, Use::LineUpTo(Some(i32::MIN))), Outcome::Refused(Refusal::TooFar));
    }

    #[test]
    fn the_window_is_a_fraction_of_a_second_however_the_session_is_set_up() {
        // A quarter of a second at 96 kHz, whatever the block size is.
        assert_eq!(window_blocks(96_000.0, 64), 375);
        assert_eq!(window_blocks(96_000.0, 512), 47);
        assert_eq!(window_blocks(44_100.0, 4096), MIN_WINDOW_BLOCKS, "a big block still gets a few of them");
        assert_eq!(window_blocks(0.0, 512), MIN_WINDOW_BLOCKS, "and a rate nobody has set is not a division by zero");
        assert_eq!(window_blocks(96_000.0, 0), MIN_WINDOW_BLOCKS);
    }

    /// One measurement, run as the audio path runs it: nothing goes out until the devices have
    /// settled, and what comes back is measured against what the drivers' own figures expect.
    #[test]
    fn a_detector_sends_once_and_measures_what_comes_back_against_what_was_expected() {
        const BLOCK: usize = 16;
        let mut detector = Detector::new(0, 0, BLOCK, 32, 8, Use::LineUpTo(Some(52)));
        for block in 0..SETTLE_BLOCKS {
            assert!(!detector.emits_on(block, 0), "nothing goes out while the devices are settling");
        }
        assert!(detector.emits_on(SETTLE_BLOCKS, 0), "and then it goes out, once");
        assert!(!detector.emits_on(SETTLE_BLOCKS + 1, 0), "and never again");

        // It comes back three blocks later at offset 4, which is 3 * 16 + 4 = 52 samples after it
        // left. The drivers' figures expected 32, so the interface is 20 samples late, and 32 more
        // than that when its trim was measured: 32 samples earlier than it was then.
        let mut heard = vec![0i32; BLOCK];
        heard[4] = AMPLITUDE;
        assert_eq!(detector.listen(&[0; BLOCK], SETTLE_BLOCKS + 1), None, "a silent block says nothing yet");
        let found = detector.listen(&heard, SETTLE_BLOCKS + 3).expect("the burst was found");
        assert_eq!(found.measured, 20, "52 samples back, 32 of them expected");
        assert_eq!(found.outcome, Outcome::Applied(-32), "what was measured, less the reference");
        assert!(detector.is_done());
        assert_eq!(detector.listen(&heard, SETTLE_BLOCKS + 4), None, "a measurement happens once a session");

        // The same signal, for a calibration run, is measured exactly the same and used for nothing.
        let mut calibrating = Detector::new(0, 0, BLOCK, 32, 8, Use::MeasureOnly);
        calibrating.emits_on(SETTLE_BLOCKS, 0);
        let kept = calibrating.listen(&heard, SETTLE_BLOCKS + 3).expect("the burst was found");
        assert_eq!(kept.measured, 20);
        assert_eq!(kept.outcome, Outcome::MeasuredOnly);
        assert_eq!(kept.applied(), 0);
    }

    #[test]
    fn a_measurement_channel_with_nothing_on_it_refuses_rather_than_correcting_by_nothing() {
        const BLOCK: usize = 16;
        for how in [Use::LineUpTo(Some(0)), Use::LineUpTo(None), Use::MeasureOnly] {
            let mut detector = Detector::new(0, 0, BLOCK, 0, 3, how);
            detector.emits_on(SETTLE_BLOCKS, 0);
            let silence = vec![0i32; BLOCK];
            for block in SETTLE_BLOCKS + 1..SETTLE_BLOCKS + 3 {
                assert_eq!(detector.listen(&silence, block), None);
            }
            let nothing = detector.listen(&silence, SETTLE_BLOCKS + 3).expect("the window closed");
            assert_eq!(nothing.outcome, Outcome::Refused(Refusal::NotHeard), "{how:?}: nothing heard is nothing heard");
            assert_eq!(nothing.applied(), 0, "a refusal corrects nothing, which is not a correction of zero");
            assert_eq!(nothing.measured, 0);
        }
    }

    #[test]
    fn the_burst_is_short_and_is_written_where_it_was_asked_for() {
        let mut run = vec![0i32; 16];
        Detector::write_burst(&mut run);
        assert_eq!(run[..BURST], [AMPLITUDE; BURST], "as long as it takes to survive a converter");
        assert!(run[BURST..].iter().all(|&sample| sample == 0), "and no longer");
        const {
            // Quiet: about 42 dB below full scale, which is nothing a monitor left up on the
            // measurement channel would be startled by.
            assert!(AMPLITUDE < i32::MAX / 64);
        }
    }

    #[test]
    fn every_line_the_log_can_keep_says_what_happened_in_words_a_person_can_check() {
        // Measured at -148 against a reference of -84: 64 samples earlier than when its trim was
        // measured, so it is held back by 64.
        let held = detail("Studio+", codes::APPLIED, -148, -64);
        assert!(held.contains("measured at -148") && held.contains("against -84"), "{held}");
        assert!(held.contains("held back by 64 samples"), "{held}");
        let brought = detail("Studio+", codes::APPLIED, -20, 64);
        assert!(brought.contains("against -84") && brought.contains("brought 64 samples earlier"), "{brought}");
        let still = detail("Studio+", codes::APPLIED, -84, 0);
        assert!(still.contains("nothing was moved"), "{still}");

        let unreferenced = detail("Studio+", codes::NO_REFERENCE, -148, 0);
        assert!(unreferenced.contains("measured at -148") && unreferenced.contains("was not lined up"), "{unreferenced}");
        assert!(unreferenced.contains("Measure the interfaces once"), "it says what to do about it: {unreferenced}");
        assert!(unreferenced.contains("the figures the drivers reported"), "{unreferenced}");

        let kept = detail("Studio+", codes::MEASURED_ONLY, -148, 0);
        assert!(kept.contains("measured at -148") && kept.contains("on purpose"), "{kept}");

        let nothing = detail("Studio+", codes::NOT_HEARD, 0, 0);
        assert!(nothing.contains("nothing arrived"), "{nothing}");
        assert!(nothing.contains("the figures the drivers reported"), "{nothing}");
        let off = detail("Studio+", codes::OFF_THE_GRID, 45, 0);
        assert!(off.contains("45") && off.contains("32 sample steps"), "{off}");
        let far = detail("Studio+", codes::TOO_FAR, 4000, 0);
        assert!(far.contains("further from") && far.contains("512"), "{far}");
        assert_eq!(detail("Studio+", codes::NOT_CONFIGURED, 0, 0), "Studio+ was not phase measured");
    }
}
