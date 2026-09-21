//! Measuring where an interface's capture pipeline actually started, and deciding whether to
//! believe it.
//!
//! **What this is for.** Two interfaces locked by a digital cable record a fixed number of samples
//! apart within one session, and between sessions that number jumps by a whole multiple of 32
//! samples. The vendor drivers report the same latency figures on every open, so the aggregate's
//! own padding is the same every run and is not the cause: it is each interface's capture pipeline
//! settling on a different phase when its stream starts. Measured at the devices on 2026-09-21, at
//! 64, 128 and 256 sample buffers, with the vendor drivers' Safe Mode on and off. Witness channels
//! settled what it belongs to: an interface's analogue input and its S/PDIF input moved together in
//! all six runs, their difference constant to two decimals while the absolute figures jumped by 96
//! and 160 samples. **The phase belongs to the capture pipeline as a whole**, which is why a
//! measurement taken on the digital input corrects the analogue inputs too.
//!
//! **A phase is not a trim.** The trim is the constant a person measured once with a cable and a
//! click; it is written in the configuration file and it is the same every session. The phase is
//! what changes every session, and it is measured rather than configured. Both apply, and
//! confusing them is how somebody would correct the same thing twice.
//!
//! **What runs where.** [`Detector`] runs on the audio path and does nothing that is forbidden
//! there: it writes a short burst into a buffer that was already allocated, looks for it in
//! another, and does integer arithmetic. It allocates nothing, locks nothing, logs nothing and
//! calls no vendor driver. Turning what it found into a line of the event log is the watcher
//! thread's work, exactly as it is for a device that stalled.

use gazelle_audio_aggregate_status::record::phase as codes;

/// What the hardware does: the phase moves in whole multiples of this many samples, never
/// anything else. A measurement that is not near one of these is a measurement of something else.
pub const GRID: i32 = 32;

/// How far from a multiple of [`GRID`] a measurement may land and still be believed. The digital
/// path has a little of its own in it (the converters' own group delay, a cable, a sample rate
/// converter left on), and rounding to the grid is what takes a small constant back out again.
pub const TOLERANCE: i32 = 8;

/// The largest phase this driver will believe, either way. Multiples of 32 well inside this were
/// what the hardware showed; anything past it is far likelier to be the measurement finding
/// something that is not the signal.
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

/// What a measurement came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It was believed, and this is what to add to the interface's input path.
    Applied(i32),
    /// It was not, and this is why. Nothing is corrected: a refusal is not a correction of zero.
    Refused(Refusal),
}

/// Why a measurement was not believed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The window went by and nothing came back on the measurement channel.
    NotHeard,
    /// Something arrived, and it was not near a whole multiple of 32 samples.
    OffTheGrid,
    /// It was further out than any phase this driver will believe.
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
            Outcome::Refused(why) => why.code(),
        }
    }

    /// What is actually added to the interface's input path, which is nothing at all unless the
    /// measurement was believed.
    pub fn applied(self) -> i32 {
        match self {
            Outcome::Applied(samples) => samples,
            Outcome::Refused(_) => 0,
        }
    }
}

/// One measurement, start to finish: what it came to, and what became of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurement {
    /// How far the interface's capture landed from where the drivers' own figures put it, in
    /// samples, before anything was rounded. Zero when nothing arrived at all, where it means
    /// nothing was measured rather than that nothing was out.
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

/// **Whether to believe a measurement, and what to do with it.** The whole of the policy, and it is
/// pure: a number in, an answer out.
///
/// `residual` is how far the interface's capture landed from where the drivers' own figures said it
/// would. The hardware moves in whole multiples of [`GRID`] samples, so the answer is the nearest
/// multiple of 32; a residual that is not near one of them is not this phenomenon and is refused.
pub fn decide(residual: i32) -> Outcome {
    let steps = (residual as f64 / GRID as f64).round() as i32;
    let nearest = steps.saturating_mul(GRID);
    if residual.saturating_sub(nearest).saturating_abs() > TOLERANCE {
        return Outcome::Refused(Refusal::OffTheGrid);
    }
    if nearest.saturating_abs() > LIMIT {
        return Outcome::Refused(Refusal::TooFar);
    }
    Outcome::Applied(nearest)
}

/// The line the event log keeps, which is the only thing that survives the session.
pub fn detail(device: &str, state: u32, measured: i32, applied: i32) -> String {
    match state {
        codes::APPLIED if applied == 0 => format!(
            "{device} was measured at {measured} samples from where its driver's figures put it, which is where it should be, so nothing was moved"
        ),
        codes::APPLIED => format!(
            "{device} was measured at {measured} samples from where its driver's figures put it, and was lined up by {applied}"
        ),
        codes::NOT_HEARD => format!(
            "{device} was not lined up: nothing arrived on its measurement channel. Check the cable and the channels the file names, or take the phase setting out. The session ran on the figures the drivers reported."
        ),
        codes::OFF_THE_GRID => format!(
            "{device} was not lined up: it measured {measured} samples, which is not near a whole multiple of {GRID}, and the interfaces only ever move by whole multiples of {GRID}. The session ran on the figures the drivers reported."
        ),
        codes::TOO_FAR => format!(
            "{device} was not lined up: it measured {measured} samples, which is further out than a phase goes. The session ran on the figures the drivers reported."
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
    /// input latency, and the block the aggregate's own ring costs.
    pub fn new(master_slot: usize, input_slot: usize, block: usize, expected: i32, window: u64) -> Detector {
        Detector {
            master_slot,
            input_slot,
            block: block as i64,
            emit_at: SETTLE_BLOCKS,
            give_up_after: SETTLE_BLOCKS.saturating_add(window),
            sent_at: 0,
            expected,
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
            return Some(Measurement { measured, outcome: decide(measured) });
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

    #[test]
    fn a_measurement_on_the_grid_is_taken_as_the_multiple_of_32_it_is_nearest() {
        assert_eq!(decide(0), Outcome::Applied(0));
        assert_eq!(decide(32), Outcome::Applied(32));
        assert_eq!(decide(-96), Outcome::Applied(-96), "the hardware jumps either way");
        assert_eq!(decide(160), Outcome::Applied(160), "one of the two jumps the witness channels showed");
        // A few samples either side of a multiple is the same multiple: the digital path has a
        // little of its own in it, and rounding to the grid is what takes it back out.
        assert_eq!(decide(34), Outcome::Applied(32));
        assert_eq!(decide(32 - TOLERANCE), Outcome::Applied(32));
        assert_eq!(decide(96 + TOLERANCE), Outcome::Applied(96));
    }

    #[test]
    fn a_measurement_that_is_not_near_a_multiple_of_32_is_refused_rather_than_rounded_to_one() {
        assert_eq!(decide(16), Outcome::Refused(Refusal::OffTheGrid), "halfway is not near anything");
        assert_eq!(decide(32 + TOLERANCE + 1), Outcome::Refused(Refusal::OffTheGrid));
        assert_eq!(decide(-45), Outcome::Refused(Refusal::OffTheGrid));
        // And a refusal corrects nothing at all, which is not the same as correcting by zero.
        assert_eq!(decide(16).applied(), 0);
        assert_eq!(decide(16).code(), codes::OFF_THE_GRID);
    }

    #[test]
    fn a_measurement_further_out_than_a_phase_goes_is_refused_as_something_else() {
        assert_eq!(decide(LIMIT), Outcome::Applied(LIMIT), "the edge of what is believed is still believed");
        assert_eq!(decide(LIMIT + GRID), Outcome::Refused(Refusal::TooFar));
        assert_eq!(decide(-(LIMIT + GRID)), Outcome::Refused(Refusal::TooFar));
        assert_eq!(decide(i32::MAX), Outcome::Refused(Refusal::TooFar), "and nothing overflows on the way there");
        assert_eq!(decide(i32::MIN), Outcome::Refused(Refusal::TooFar));
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
        let mut detector = Detector::new(0, 0, BLOCK, 32, 8);
        for block in 0..SETTLE_BLOCKS {
            assert!(!detector.emits_on(block, 0), "nothing goes out while the devices are settling");
        }
        assert!(detector.emits_on(SETTLE_BLOCKS, 0), "and then it goes out, once");
        assert!(!detector.emits_on(SETTLE_BLOCKS + 1, 0), "and never again");

        // It comes back three blocks later at offset 4, which is 3 * 16 + 4 = 52 samples after it
        // left. The drivers' figures expected 32, so the interface is 20 samples late... which is
        // not near a multiple of 32 and is refused.
        let mut heard = vec![0i32; BLOCK];
        heard[4] = AMPLITUDE;
        assert_eq!(detector.listen(&[0; BLOCK], SETTLE_BLOCKS + 1), None, "a silent block says nothing yet");
        let refused = detector.listen(&heard, SETTLE_BLOCKS + 3).expect("the burst was found");
        assert_eq!(refused.measured, 20, "52 samples back, 32 of them expected");
        assert_eq!(refused.outcome, Outcome::Refused(Refusal::OffTheGrid));
        assert!(detector.is_done());
        assert_eq!(detector.listen(&heard, SETTLE_BLOCKS + 4), None, "a measurement happens once a session");

        // The same signal arriving where a whole multiple of 32 puts it is believed.
        let mut on_time = Detector::new(0, 0, BLOCK, 32, 8);
        on_time.emits_on(SETTLE_BLOCKS, 0);
        let mut late = vec![0i32; BLOCK];
        late[0] = -AMPLITUDE;
        let believed = on_time.listen(&late, SETTLE_BLOCKS + 4).expect("the burst was found");
        assert_eq!(believed.measured, 32, "64 samples back, 32 of them expected");
        assert_eq!(believed.applied(), 32);
    }

    #[test]
    fn a_measurement_channel_with_nothing_on_it_refuses_rather_than_correcting_by_nothing() {
        const BLOCK: usize = 16;
        let mut detector = Detector::new(0, 0, BLOCK, 0, 3);
        detector.emits_on(SETTLE_BLOCKS, 0);
        let silence = vec![0i32; BLOCK];
        for block in SETTLE_BLOCKS + 1..SETTLE_BLOCKS + 3 {
            assert_eq!(detector.listen(&silence, block), None);
        }
        let nothing = detector.listen(&silence, SETTLE_BLOCKS + 3).expect("the window closed");
        assert_eq!(nothing.outcome, Outcome::Refused(Refusal::NotHeard));
        assert_eq!(nothing.applied(), 0, "a refusal corrects nothing, which is not a correction of zero");
        assert_eq!(nothing.measured, 0);
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
    fn every_line_the_log_can_keep_says_what_happened_and_never_mentions_a_trim() {
        let applied = detail("Studio+", codes::APPLIED, 98, 96);
        assert!(applied.contains("Studio+") && applied.contains("96"), "{applied}");
        let nothing = detail("Studio+", codes::NOT_HEARD, 0, 0);
        assert!(nothing.contains("nothing arrived"), "{nothing}");
        assert!(nothing.contains("the figures the drivers reported"), "{nothing}");
        let off = detail("Studio+", codes::OFF_THE_GRID, 45, 0);
        assert!(off.contains("45") && off.contains("32"), "{off}");
        let far = detail("Studio+", codes::TOO_FAR, 4000, 0);
        assert!(far.contains("further out"), "{far}");
        assert_eq!(detail("Studio+", codes::NOT_CONFIGURED, 0, 0), "Studio+ was not phase measured");
        for state in [codes::APPLIED, codes::NOT_HEARD, codes::OFF_THE_GRID, codes::TOO_FAR] {
            let line = detail("Studio+", state, 45, 32);
            assert!(!line.to_ascii_lowercase().contains("trim"), "a phase is not a trim: {line}");
        }
    }
}
