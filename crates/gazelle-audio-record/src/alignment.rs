//! **Checking the alignment while Gazelle records**, over the cable the phase is measured on.
//!
//! At the start of a session the aggregate measures where the follower's capture landed and lines
//! it up (`gazelle_aggregate::phase`), and then nothing looks again. A measurement that went wrong,
//! or a follower that slipped mid-take (after a dropout, say), would go unnoticed and land in the
//! files. So while the recorder is armed, the same signal keeps going down the same cable:
//!
//! - **The signal** is the start of the session's own: a burst of [`phase::BURST`] samples at
//!   [`phase::AMPLITUDE`], once every [`CHECK_SECONDS`], on the master's output that feeds the cable,
//!   which nothing else plays to and which goes nowhere else, so nobody hears it. Over a digital
//!   cable it arrives bit for bit, so the first sample past [`phase::THRESHOLD`] is its arrival to
//!   the sample, which is exactly how the start of the session finds it.
//! - **The audio path** sends it and copies the follower's measurement channel into a lock-free
//!   ring, one block at a time, and does nothing else for it ([`phase::Watch`]). The burst only goes
//!   out once the start of the session's own measurement is over, so the two never meet.
//! - **This module** reads that ring on a thread of its own ([`Checker::step`]), finds each burst,
//!   and measures it exactly as the start of the session did: where it arrived less where the
//!   drivers' own figures put it. Compared with what the alignment in force assumes, that is how
//!   far the follower is from where it was lined up, in samples: zero while it holds.
//! - **What it is compared with** is the phase the alignment in force corresponds to: what the start
//!   of the session measured, when that was used or there was nothing to line it up to; the
//!   reference its trim was measured at, when the start's measurement was refused, because then the
//!   session runs as though it were there; and, with neither, the first check, which is said.
//! - **When the start heard nothing**, nothing is sent at all, and that is said instead: the burst
//!   plays wherever its channel is routed, and a cable it never reached is a routing that may take
//!   it somewhere a person would hear it, once a second.
//!
//! Nothing is corrected: a slip is reported, in the take's log and on the page, and the files keep
//! what was recorded.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gazelle_aggregate::phase::{self, Heard, Watch};
use gazelle_audio_aggregate_status::record::phase as codes;
use serde::Serialize;

use crate::host::{PhasePath, UNSET};

/// How often a check goes down the cable.
pub const CHECK_SECONDS: f64 = 1.0;
/// How much the watch's ring holds that this module has not read yet, in seconds of blocks: many
/// times longer than the checker ever waits between looks.
const RING_SECONDS: f64 = 0.5;
/// The fewest blocks the ring holds, whatever the block size.
const RING_MIN_BLOCKS: usize = 16;
/// How many decided checks wait for the writer before the oldest are let go: a day of them.
const FRESH_LIMIT: usize = 86_400;
/// Why nothing is sent when the start of the session heard nothing on the cable.
pub const START_NOT_HEARD: &str = "the measurement at the start of the session heard nothing on the phase cable, so no check is sent down it: the signal plays wherever that channel is routed, and the routing may send it somewhere you would hear it. Check the cable and its routing on the Aggregate page, then disarm and arm again";

/// How long the page waits with no check finding the signal before it calls the signal missing.
pub const SILENT_AFTER_SECONDS: f64 = 2.5;

/// The watch for a session at this rate and block size, over this cable.
pub fn watch_for(path: &PhasePath, rate: f64, block: usize) -> Arc<Watch> {
    let block = block.max(1);
    let per_second = if rate.is_finite() && rate > 0.0 { rate / block as f64 } else { 1.0 };
    let every = (per_second * CHECK_SECONDS).round().max(1.0) as u64;
    let slots = ((per_second * RING_SECONDS).ceil() as usize).max(RING_MIN_BLOCKS);
    Arc::new(Watch::new(path.device, every, block, slots))
}

/// One check, once it has come to something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    /// Where the burst went out, as a position of the capture ring (counted from Arm), which is
    /// what a take's window is in.
    pub at: u64,
    /// How far from where the alignment in force puts it the follower was, in samples: positive is
    /// late. None when the burst never arrived.
    pub off: Option<i32>,
}

/// What a check is compared with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Against {
    /// The start of the session's measurement, which the follower was lined up by.
    Start,
    /// The start's measurement, which there was no reference to line it up to.
    StartUnreferenced,
    /// The reference its trim was measured at, since the start's measurement was refused.
    Reference,
    /// The first check, since the start's measurement was refused and there is no reference.
    FirstCheck,
}

impl Against {
    fn words(self) -> &'static str {
        match self {
            Against::Start => "against the phase measured at the start of the session, which the interfaces were lined up by",
            Against::StartUnreferenced => "against the phase measured at the start of the session (there is no reference to line it up to, so only a change since then shows)",
            Against::Reference => "against the phase its trim was measured at, because the measurement at the start of the session was not used",
            Against::FirstCheck => "against the first check, because the measurement at the start of the session was not used and there is no reference to compare with",
        }
    }
}

/// What the checker, the writer and the page share. Locked briefly, never on the audio thread.
#[derive(Debug, Default)]
pub struct Board {
    /// Why the alignment is not being checked, when it is not.
    pub why_not: Option<String>,
    /// The interface whose cable carries the check.
    pub device: String,
    /// What the checks are compared with, once the first one has said.
    pub against: Option<Against>,
    /// Checks the writer has not taken yet, oldest first.
    fresh: VecDeque<Check>,
    /// Every check since Arm, counted.
    pub checks: u64,
    /// The last check, and when it came to something.
    pub last: Option<Check>,
    /// When a check last found the signal, and where it put the follower.
    pub heard_at: Option<Instant>,
    pub heard_off: Option<i32>,
    /// When the run of checks that found nothing began, while the last one found nothing.
    pub silent_since: Option<Instant>,
    /// The check that found the latest move away from where the follower was lined up.
    pub slip: Option<Check>,
}

pub type SharedBoard = Arc<Mutex<Board>>;

impl Board {
    /// A board for a recorder that is checking over `device`'s cable.
    pub fn checking(device: &str) -> SharedBoard {
        Arc::new(Mutex::new(Board { device: device.into(), ..Board::default() }))
    }

    /// A board that says why nothing is checked.
    pub fn not_checking(why: &str) -> SharedBoard {
        Arc::new(Mutex::new(Board { why_not: Some(why.into()), ..Board::default() }))
    }

    /// The checks the writer has not taken yet.
    pub fn take_fresh(&mut self) -> Vec<Check> {
        self.fresh.drain(..).collect()
    }

    fn decided(&mut self, check: Check, now: Instant) {
        self.checks += 1;
        self.last = Some(check);
        match check.off {
            Some(off) => {
                // A slip is dated by the first check that found it, not the latest.
                if off != 0 && self.heard_off != Some(off) {
                    self.slip = Some(check);
                }
                self.heard_at = Some(now);
                self.heard_off = Some(off);
                self.silent_since = None;
            }
            None => {
                self.silent_since.get_or_insert(now);
            }
        }
        if self.fresh.len() >= FRESH_LIMIT {
            self.fresh.pop_front();
        }
        self.fresh.push_back(check);
    }
}

/// **Reads the watch and decides each check.** One per armed recorder, stepped by a thread of its
/// own; a test steps it by hand.
pub struct Checker {
    watch: Arc<Watch>,
    origin: Arc<AtomicU64>,
    board: SharedBoard,
    reference: Option<i32>,
    block: i64,
    /// What the alignment in force puts the measurement at, once known.
    baseline: Option<i32>,
    /// A burst that has gone out and not come back: where the drivers' figures put its arrival, and
    /// where it left, in samples of the session.
    pending: Option<(i64, u64)>,
    last_block: Option<u64>,
    /// The start of the session heard nothing, so the audio path sends nothing, and that is said.
    gave_up: bool,
}

impl Checker {
    pub fn new(watch: Arc<Watch>, origin: Arc<AtomicU64>, board: SharedBoard, path: &PhasePath) -> Checker {
        let block = watch.block() as i64;
        Checker { watch, origin, board, reference: path.reference, block, baseline: None, pending: None, last_block: None, gave_up: false }
    }

    /// Read everything the audio path has handed over and decide what can be decided. Answers
    /// whether there was anything to read.
    pub fn step(&mut self) -> bool {
        let watch = Arc::clone(&self.watch);
        let mut decided = Vec::new();
        let mut any = false;
        while watch.read(|heard| self.hear(heard, &mut decided)) {
            any = true;
        }
        if !decided.is_empty() {
            let now = Instant::now();
            if let Ok(mut board) = self.board.lock() {
                for check in decided {
                    board.decided(check, now);
                }
            }
        }
        any
    }

    fn hear(&mut self, heard: Heard<'_>, decided: &mut Vec<Check>) {
        if heard.state == codes::NOT_HEARD && !self.gave_up {
            self.gave_up = true;
            if let Ok(mut board) = self.board.lock() {
                board.why_not = Some(START_NOT_HEARD.into());
            }
        }
        // A block missing from the run (the ring was full) or a session started over: a burst in
        // flight cannot be found now, so it is a check that found nothing.
        if self.last_block.is_some_and(|last| heard.block_number != last.wrapping_add(1)) {
            if let Some((_, sent)) = self.pending.take() {
                self.decide(sent, None, &heard, decided);
            }
        }
        self.last_block = Some(heard.block_number);
        if let Some((due, sent)) = self.pending {
            // The first sample past the threshold, exactly as the start of the session finds it.
            if let Some(at) = heard.samples.iter().position(|sample| sample.saturating_abs() >= phase::THRESHOLD) {
                let arrived = heard.block_number as i64 * self.block + at as i64;
                let measured = (arrived - due).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                self.pending = None;
                self.decide(sent, Some(measured), &heard, decided);
            }
        }
        if let Some(due) = heard.due {
            // The next burst is going out, so the last one is not coming.
            if let Some((_, sent)) = self.pending.take() {
                self.decide(sent, None, &heard, decided);
            }
            self.pending = Some((due, heard.block_number.wrapping_mul(self.block as u64)));
        }
    }

    fn decide(&mut self, sent: u64, measured: Option<i32>, heard: &Heard<'_>, decided: &mut Vec<Check>) {
        if self.baseline.is_none() {
            let settled = self.settle(heard, measured);
            if let Some((baseline, against)) = settled {
                self.baseline = Some(baseline);
                if let Ok(mut board) = self.board.lock() {
                    board.against = Some(against);
                }
            }
        }
        let origin = self.origin.load(Ordering::Acquire);
        if origin == UNSET || sent < origin {
            // Sent before the recorder's first block: it belongs to no take.
            return;
        }
        let off = match (measured, self.baseline) {
            (Some(measured), Some(baseline)) => Some(measured.saturating_sub(baseline)),
            _ => None,
        };
        decided.push(Check { at: sent - origin, off });
    }

    /// What the alignment in force puts the measurement at, from what the start of the session made
    /// of this follower; or, with nothing to go on, the first check that found the signal.
    fn settle(&self, heard: &Heard<'_>, first: Option<i32>) -> Option<(i32, Against)> {
        match heard.state {
            // Applied: what was measured is the reference plus what was applied, which is exactly
            // the phase the follower was lined up for.
            codes::APPLIED | codes::MEASURED_ONLY => Some((heard.measured, Against::Start)),
            codes::NO_REFERENCE => Some((heard.measured, Against::StartUnreferenced)),
            codes::OFF_THE_GRID | codes::TOO_FAR => match self.reference {
                Some(reference) => Some((reference, Against::Reference)),
                None => first.map(|first| (first, Against::FirstCheck)),
            },
            _ => None,
        }
    }
}

/// The alignment, as the page shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AlignmentLive {
    /// `checking`, `waiting` (for the first check), or `off` (not checked, and `reason` says why).
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Checks since Arm.
    pub checks: u64,
    /// Seconds since a check last found the signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_check_seconds: Option<f64>,
    /// Where the last check that found the signal put the follower, in samples from where it was
    /// lined up: positive is late.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i32>,
    /// While the last checks have found nothing: for how long, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub silent_seconds: Option<f64>,
    /// The last slip since Arm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slip: Option<SlipLive>,
}

/// A slip, as the page shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SlipLive {
    /// By how much, in samples: positive is late.
    pub samples: i32,
    /// Seconds into the take it was found at, when it was found during the take being recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take_seconds: Option<f64>,
}

impl AlignmentLive {
    /// The page's view of `board`. `take_start` is the take being recorded's first sample, as a
    /// ring position, while one is.
    pub fn of(board: &Board, take_start: Option<u64>, rate: f64, now: Instant) -> AlignmentLive {
        if let Some(why) = &board.why_not {
            return AlignmentLive { state: "off", device: None, reason: Some(why.clone()), checks: 0, since_check_seconds: None, offset: None, silent_seconds: None, slip: None };
        }
        let silent = board.silent_since.map(|since| now.saturating_duration_since(since).as_secs_f64() + CHECK_SECONDS);
        AlignmentLive {
            state: if board.checks == 0 { "waiting" } else { "checking" },
            device: Some(board.device.clone()),
            reason: None,
            checks: board.checks,
            since_check_seconds: board.heard_at.map(|at| now.saturating_duration_since(at).as_secs_f64()),
            offset: board.heard_off,
            silent_seconds: silent.filter(|&seconds| seconds >= SILENT_AFTER_SECONDS),
            slip: board.slip.map(|slip| SlipLive {
                samples: slip.off.unwrap_or(0),
                take_seconds: take_start.filter(|&start| slip.at >= start).map(|start| (slip.at - start) as f64 / rate),
            }),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The take's log.
// ---------------------------------------------------------------------------------------------

/// How a check's place in a take is said: `m:ss.mmm`, and the sample.
fn when(at: u64, start: u64, rate: f64) -> String {
    let into = at.saturating_sub(start);
    format!("{} (sample {into} of the take)", crate::writer::clock_words(into as f64 / rate))
}

/// Which way, and by how much, in words: "B is 32 samples late".
fn away(device: &str, off: i32) -> String {
    let samples = off.unsigned_abs();
    let plural = if samples == 1 { "" } else { "s" };
    if off > 0 {
        format!("{device} is {samples} sample{plural} late")
    } else {
        format!("{device} is {samples} sample{plural} early")
    }
}

/// Whether any check during a take found the follower away from where it was lined up.
pub fn slipped(checks: &[Check]) -> bool {
    checks.iter().any(|check| check.off.is_some_and(|off| off != 0))
}

/// **What the take's log says about the alignment**, from the checks that went out during it,
/// oldest first. The first line is the answer; any after it say when.
pub fn take_lines(board: &Board, checks: &[Check], start: u64, rate: f64) -> Vec<String> {
    if let Some(why) = &board.why_not {
        return vec![format!("Alignment between the interfaces was not checked during this take: {why}.")];
    }
    let device = board.device.as_str();
    if checks.is_empty() {
        return vec![format!(
            "Alignment was not checked during this take: no check over the phase cable into {device} came to anything before it ended. Checks go out once a second, starting once the measurement at the start of the session is over."
        )];
    }
    let found: Vec<i32> = checks.iter().filter_map(|check| check.off).collect();
    let missing = checks.len() - found.len();
    let off = found.iter().filter(|&&off| off != 0).count();
    let total = checks.len();
    let checks_word = |n: usize| if n == 1 { "check" } else { "checks" };

    let mut details = Vec::new();
    let mut previous: Option<i32> = None;
    let mut silent: Option<(u64, usize)> = None;
    for check in checks {
        let Some(now) = check.off else {
            silent = Some(match silent {
                Some((from, count)) => (from, count + 1),
                None => (check.at, 1),
            });
            continue;
        };
        if let Some((from, count)) = silent.take() {
            details.push(format!(
                "No check signal arrived from {} until {}: {count} {} found nothing, so the alignment is not known for that stretch.",
                when(from, start, rate),
                when(check.at, start, rate),
                checks_word(count)
            ));
        }
        match (previous, now) {
            (None, 0) => {}
            (None, _) => details.push(format!("At the first check, {}, {} against the others.", when(check.at, start, rate), away(device, now))),
            (Some(was), _) if was == now => {}
            (Some(_), 0) => details.push(format!("Alignment came back at {}: {device} is where it was lined up again.", when(check.at, start, rate))),
            (Some(0), _) => details.push(format!("Alignment slipped at {}: {} against the others.", when(check.at, start, rate), away(device, now))),
            (Some(_), _) => details.push(format!("Alignment moved again at {}: {} against the others.", when(check.at, start, rate), away(device, now))),
        }
        previous = Some(now);
    }
    if let Some((from, count)) = silent {
        details.push(format!(
            "No check signal arrived from {} to the end of the take: {count} {} found nothing, so the alignment is not known for that stretch.",
            when(from, start, rate),
            checks_word(count)
        ));
    }

    let missing_words = format!("{missing} {} found no signal, so it is not known for those stretches", checks_word(missing));
    let summary = if off == 0 && missing == 0 {
        format!("Alignment held: {total} {}, all at 0 samples.", checks_word(total))
    } else if off == 0 && found.is_empty() {
        format!("Alignment was not confirmed during this take: none of its {total} {} over the phase cable into {device} found the signal. Check the cable and its routing.", checks_word(total))
    } else if off == 0 {
        format!("Alignment held at every check that found the signal ({} of {total}, all at 0 samples); {missing_words}.", found.len())
    } else {
        let end = match previous {
            Some(0) => "it was back where it was lined up by the end of the take".to_string(),
            Some(last) => format!("at the last check {}", away(device, last)),
            None => String::new(),
        };
        let gaps = if missing > 0 { format!("; {missing_words}") } else { String::new() };
        format!(
            "Alignment did not hold: {off} of {total} {} found {device} away from where it was lined up, and {end}{gaps}. Nothing was corrected: the files are as recorded.",
            checks_word(total)
        )
    };
    let mut lines = vec![summary];
    lines.push(format!("Checked once a second over the phase cable into {device}, {}.", board.against.map_or("against the alignment in force", Against::words)));
    lines.extend(details);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;
    const SECOND: u64 = 48_000;

    fn board() -> Board {
        Board { device: "B".into(), against: Some(Against::Start), ..Board::default() }
    }

    fn checks(offs: &[Option<i32>]) -> Vec<Check> {
        offs.iter().enumerate().map(|(n, &off)| Check { at: (n as u64 + 1) * SECOND, off }).collect()
    }

    #[test]
    fn a_take_whose_checks_all_landed_where_it_was_lined_up_held() {
        let lines = take_lines(&board(), &checks(&[Some(0); 48]), 0, RATE);
        assert_eq!(lines[0], "Alignment held: 48 checks, all at 0 samples.");
        assert!(lines[1].contains("phase cable into B") && lines[1].contains("lined up by"), "{lines:?}");
        assert_eq!(lines.len(), 2, "nothing happened to say when: {lines:?}");
    }

    #[test]
    fn a_slip_says_when_by_how_much_which_way_and_whether_it_came_back() {
        let mut offs = vec![Some(0); 10];
        offs.extend([Some(32); 5]);
        offs.extend([Some(0); 3]);
        let lines = take_lines(&board(), &checks(&offs), SECOND / 2, RATE);
        assert!(lines[0].starts_with("Alignment did not hold: 5 of 18 checks found B away"), "{lines:?}");
        assert!(lines[0].contains("back where it was lined up by the end") && lines[0].contains("Nothing was corrected"), "{lines:?}");
        // The eleventh check went out at 11 s, which is 10.5 s into a take that started at 0.5 s.
        assert!(lines.iter().any(|l| l == "Alignment slipped at 0:10.500 (sample 504000 of the take): B is 32 samples late against the others."), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("Alignment came back at 0:15.500")), "{lines:?}");

        let stayed = take_lines(&board(), &checks(&[Some(0), Some(-32), Some(-32)]), 0, RATE);
        assert!(stayed[0].contains("at the last check B is 32 samples early"), "{stayed:?}");
    }

    #[test]
    fn checks_that_found_nothing_are_said_and_never_counted_as_holding() {
        let mut offs = vec![Some(0); 5];
        offs.extend([None; 4]);
        offs.extend([Some(0); 3]);
        let lines = take_lines(&board(), &checks(&offs), 0, RATE);
        assert_eq!(lines[0], "Alignment held at every check that found the signal (8 of 12, all at 0 samples); 4 checks found no signal, so it is not known for those stretches.");
        assert!(lines.iter().any(|l| l.starts_with("No check signal arrived from 0:06.000 (sample 288000 of the take) until 0:10.000") && l.contains("4 checks found nothing")), "{lines:?}");

        let gone = take_lines(&board(), &checks(&[None; 6]), 0, RATE);
        assert!(gone[0].starts_with("Alignment was not confirmed during this take: none of its 6 checks"), "{gone:?}");
        assert!(gone.iter().any(|l| l.contains("to the end of the take")), "{gone:?}");
    }

    #[test]
    fn with_no_phase_path_the_log_says_it_was_not_checked_and_why() {
        let board = Board { why_not: Some(crate::host::NO_PHASE_PATH.into()), ..Board::default() };
        let lines = take_lines(&board, &checks(&[Some(0)]), 0, RATE);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("Alignment between the interfaces was not checked during this take: no phase path is set up"), "{lines:?}");
        let live = AlignmentLive::of(&board, None, RATE, Instant::now());
        assert_eq!(live.state, "off");
        assert!(live.reason.unwrap().contains("Phase path"));
    }

    #[test]
    fn a_take_too_short_for_a_check_says_so_rather_than_that_it_held() {
        let lines = take_lines(&board(), &[], 0, RATE);
        assert!(lines[0].starts_with("Alignment was not checked during this take"), "{lines:?}");
    }
}
