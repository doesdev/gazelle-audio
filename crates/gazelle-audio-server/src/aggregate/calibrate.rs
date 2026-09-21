//! Lining the interfaces up by measuring, rather than by eye.
//!
//! The trims in the aggregate's setup are a correction to the latency figures the vendor drivers
//! report, and until now the only way to arrive at one was to record something on both interfaces
//! and judge two waveforms against each other. `gazelle-audio-calibrate` does it properly: it
//! plays a click out of one interface, into both, and reads the difference out of the aggregate's
//! own input buffer, so what it measures is exactly what the aggregate produces.
//!
//! What is here is the half around it. A run takes seconds, makes a noise in the room and holds
//! both audio drivers while it goes, so it is **one at a time**, it happens on a thread of its
//! own, it can be given up on, and while it runs the page can watch it. The measurement itself is
//! behind [`Measurer`], so every rule in this module is tested against runs made of data and no
//! test opens a driver or plays anything.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gazelle_calibrate::{Direction, Outcome, Rig, Settings};
use serde::{Deserialize, Serialize};

/// The most clicks a run may be asked for. Eight is the default and plenty; this only catches a
/// number that would keep the room making a noise for minutes.
pub const CLICKS_MAX: u32 = 64;

/// What the page asks for: the cabling, and how hard to measure it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    /// `inputs` or `outputs`: which side of the interfaces this pass measures.
    pub direction: String,
    /// The aggregate output channel the click leaves on, one per interface, in the setup's order.
    pub outputs: Vec<i32>,
    /// The aggregate input channel it comes back on, one per interface.
    pub inputs: Vec<i32>,
    #[serde(default)]
    pub clicks: Option<u32>,
    #[serde(default)]
    pub level_dbfs: Option<f64>,
}

impl Ask {
    /// The rig and the settings this asks for, or the sentence that says why it is not one. Only
    /// what can be decided here is decided here; the cabling's own rules belong to the crate that
    /// knows them, and it checks them again before it opens anything.
    pub fn taken(&self) -> Result<(Rig, Settings), String> {
        let direction = match self.direction.as_str() {
            "inputs" => Direction::Inputs,
            "outputs" => Direction::Outputs,
            other => return Err(format!("A pass is inputs or outputs, not {other:?}.")),
        };
        let rig = Rig::new(direction, self.outputs.clone(), self.inputs.clone());
        if let Some(why) = rig.refusal() {
            return Err(sentence(&why));
        }
        let mut settings = Settings::default();
        if let Some(clicks) = self.clicks {
            if clicks == 0 || clicks > CLICKS_MAX {
                return Err(format!("A run plays between 1 and {CLICKS_MAX} clicks, not {clicks}."));
            }
            settings.clicks = clicks;
        }
        if let Some(level) = self.level_dbfs {
            settings.level_dbfs = level;
        }
        if let Some(why) = settings.refusal() {
            return Err(sentence(&why));
        }
        Ok((rig, settings))
    }
}

/// How far a run has got, as the page reads it. One run at a time, so this is the whole state.
#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    /// `idle`, `running`, `done` or `failed`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// What it is doing now, in words, while it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// How far through, between zero and one, while it runs. Worked out from how long the run
    /// was always going to take, because the interfaces are the ones keeping time, not this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    /// Why nothing was measured, when nothing was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Measured>,
}

impl Default for Progress {
    fn default() -> Self {
        Progress { state: "idle", started_at_ms: None, step: None, progress: None, refusal: None, outcome: None }
    }
}

/// What a run came to, in the shape the page reads. The measuring crate's own answer carries a
/// little more than a person needs; this is that answer said once, in one place.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Measured {
    pub direction: String,
    pub rate: u32,
    pub buffer_size: i32,
    /// The interface everything else was measured against.
    pub reference: String,
    pub readings: Vec<Reading>,
    pub trims: Vec<Trim>,
    /// Anything the person should read before believing the numbers.
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reading {
    pub device: String,
    pub is_reference: bool,
    /// How far behind the reference this interface recorded, in samples and fractions of one.
    pub lag_samples: f64,
    /// How much the clicks disagreed with each other. Under a sample is a measurement to trust.
    pub spread_samples: f64,
    pub clicks_found: usize,
    pub clicks_expected: usize,
    /// The sentence for this interface, whether it went well or not.
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift: Option<Drift>,
}

/// A lag that grows through the run, which is two clocks rather than one. No trim fixes it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Drift {
    pub samples_per_second: f64,
    pub ppm: f64,
    /// Always true: a drift is only reported when it is one.
    pub real: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Trim {
    pub device: String,
    pub direction: String,
    /// The field in the setup this changes: `input_trim` or `output_trim`.
    pub field: String,
    /// What the setup says now.
    pub was: i32,
    /// What this run measured, which is what is added to it.
    pub measured: i32,
    /// What it would become.
    pub now: i32,
    pub is_reference: bool,
    /// Why this one is not offered, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_applied: Option<String>,
}

/// One measurement. Behind a trait because the real one opens both audio drivers and plays into
/// whatever is plugged in, which no test does.
pub trait Measurer: Send + Sync {
    /// Measure, calling `watch` between blocks with how many blocks have gone by. Answering false
    /// gives the run up there and then.
    fn measure(&self, rig: &Rig, settings: &Settings, watch: &mut dyn FnMut(usize) -> bool) -> Outcome;
}

/// The real one: this PC, its interfaces and the aggregate's own file.
pub struct ThisPc;

impl Measurer for ThisPc {
    #[cfg(windows)]
    fn measure(&self, rig: &Rig, settings: &Settings, watch: &mut dyn FnMut(usize) -> bool) -> Outcome {
        gazelle_calibrate::session::measure_with(rig, settings, watch)
    }

    #[cfg(not(windows))]
    fn measure(&self, rig: &Rig, _settings: &Settings, _watch: &mut dyn FnMut(usize) -> bool) -> Outcome {
        Outcome::refused(rig.direction, "measuring the interfaces needs Windows, which this is not running on")
    }
}

/// The one run at a time, and everything anybody asks about it.
pub struct Calibration {
    state: Mutex<Progress>,
    /// Set by a person pressing stop, read by the run between blocks.
    stop: AtomicBool,
    running: AtomicBool,
    measurer: Arc<dyn Measurer>,
}

impl Calibration {
    pub fn new(measurer: Arc<dyn Measurer>) -> Calibration {
        Calibration { state: Mutex::new(Progress::default()), stop: AtomicBool::new(false), running: AtomicBool::new(false), measurer }
    }

    /// This PC's, which is what the server runs with.
    pub fn this_pc() -> Calibration {
        Calibration::new(Arc::new(ThisPc))
    }

    pub fn state(&self) -> Progress {
        self.state.lock().map(|state| state.clone()).unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Give up on the run that is going, if one is. A run that has already finished is left as it
    /// is: stopping is about the noise in the room, not about the answer.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Start one, on a thread of its own. The refusal is what the page shows, so it is a sentence.
    pub fn start(self: &Arc<Self>, ask: &Ask) -> Result<(), String> {
        let (rig, settings) = ask.taken()?;
        if self.running.swap(true, Ordering::AcqRel) {
            return Err("A measurement is already running. Wait for it, or stop it first.".into());
        }
        self.stop.store(false, Ordering::Release);
        *self.state.lock().expect("the calibration state") = Progress {
            state: "running",
            started_at_ms: Some(now_ms()),
            step: Some("Opening the interfaces".into()),
            progress: Some(0.0),
            refusal: None,
            outcome: None,
        };
        let job = Arc::clone(self);
        std::thread::Builder::new()
            .name("gazelle-calibrate".into())
            .spawn(move || job.run(rig, settings))
            .map_err(|e| {
                self.running.store(false, Ordering::Release);
                format!("The measurement could not be started: {e}")
            })?;
        Ok(())
    }

    fn run(self: Arc<Self>, rig: Rig, settings: Settings) {
        let expected = expected_seconds(&settings);
        let since = Instant::now();
        let mut playing = false;
        let outcome = {
            let job = Arc::clone(&self);
            let mut watch = move |_block: usize| {
                if !playing {
                    playing = true;
                    job.step("Playing the clicks");
                }
                job.at(since.elapsed().as_secs_f64() / expected);
                !job.stop.load(Ordering::Acquire)
            };
            self.measurer.measure(&rig, &settings, &mut watch)
        };
        let mut state = self.state.lock().expect("the calibration state");
        *state = match outcome.refusal {
            Some(why) => Progress { state: "failed", refusal: Some(sentence(&why)), ..Progress::default() },
            None => Progress { state: "done", outcome: Some(measured(&outcome, &rig)), ..Progress::default() },
        };
        state.started_at_ms = Some(now_ms());
        drop(state);
        self.running.store(false, Ordering::Release);
    }

    fn step(&self, words: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.step = Some(words.to_string());
        }
    }

    fn at(&self, fraction: f64) {
        if let Ok(mut state) = self.state.lock() {
            // Never all the way: the run is over when the answer is in, not when the clock says so.
            state.progress = Some(fraction.clamp(0.0, 0.99));
        }
    }
}

/// How long a run was always going to take: what it waits out before the first click, the clicks
/// themselves, and a little after the last one for it to come back.
pub fn expected_seconds(settings: &Settings) -> f64 {
    let clicks = f64::from(settings.clicks.max(1));
    (settings.settle_seconds + clicks * settings.spacing_seconds + 0.5).max(0.5)
}

/// The measuring crate's answer, in the shape the page reads.
pub fn measured(outcome: &Outcome, rig: &Rig) -> Measured {
    let reference = outcome
        .readings
        .get(rig.reference)
        .map(|reading| reading.device.clone())
        .or_else(|| outcome.trims.iter().find(|trim| trim.is_reference).map(|trim| trim.device.clone()))
        .unwrap_or_default();
    let readings: Vec<Reading> = outcome
        .readings
        .iter()
        .enumerate()
        .map(|(index, reading)| Reading {
            device: reading.device.clone(),
            is_reference: index == rig.reference,
            lag_samples: reading.lag_samples,
            spread_samples: reading.spread_samples,
            clicks_found: reading.clicks_found,
            clicks_expected: reading.clicks_expected,
            note: reading.note.clone(),
            drift: reading.drift.as_ref().map(|drift| Drift {
                samples_per_second: drift.samples_per_second,
                ppm: drift.parts_per_million,
                real: true,
            }),
        })
        .collect();
    let trims: Vec<Trim> = outcome
        .trims
        .iter()
        .map(|trim| Trim {
            device: trim.device.clone(),
            direction: trim.direction.as_str().to_string(),
            field: trim.field.to_string(),
            was: trim.old,
            measured: trim.measured,
            now: trim.new,
            is_reference: trim.is_reference,
            not_applied: trim.not_applied.clone(),
        })
        .collect();
    // What to read before believing any of it: an interface that heard little or nothing, two
    // clocks rather than one, and any trim the measurement will not stand behind.
    let mut warnings: Vec<String> = Vec::new();
    for reading in &outcome.readings {
        if !reading.is_usable() || reading.drift.is_some() {
            warnings.push(reading.note.clone());
        }
    }
    for trim in &outcome.trims {
        if let Some(why) = &trim.not_applied {
            warnings.push(format!("{}: {}", trim.device, sentence(why)));
        }
    }
    Measured {
        direction: outcome.direction.as_str().to_string(),
        rate: outcome.rate.round().max(0.0) as u32,
        buffer_size: outcome.block,
        reference,
        readings,
        trims,
        warnings,
    }
}

/// A refusal from anywhere, as a sentence: the crates below write them lower case and unpunctuated
/// so they can be quoted inside another message, and the page shows them on their own.
fn sentence(why: &str) -> String {
    let trimmed = why.trim();
    let mut text: String = trimmed.to_string();
    if let Some(first) = trimmed.chars().next() {
        if first.is_lowercase() {
            text = first.to_uppercase().collect::<String>() + &trimmed[first.len_utf8()..];
        }
    }
    if !text.ends_with('.') && !text.ends_with('!') && !text.ends_with('?') {
        text.push('.');
    }
    text
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_millis() as u64).unwrap_or_default()
}

/// A run that takes about as long as a real one would, for a fake to wait out.
pub fn block_wait() -> Duration {
    Duration::from_millis(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gazelle_calibrate::measure::Reading as Measured1;
    use gazelle_calibrate::trim::TrimChange;

    /// A measurer made of data: it answers what it was given, after as many blocks as it was told
    /// to take, and remembers whether it was stopped.
    struct Fake {
        answer: Mutex<Option<Outcome>>,
        blocks: usize,
        stopped: AtomicBool,
    }

    impl Fake {
        fn new(answer: Outcome, blocks: usize) -> Arc<Fake> {
            Arc::new(Fake { answer: Mutex::new(Some(answer)), blocks, stopped: AtomicBool::new(false) })
        }
    }

    impl Measurer for Fake {
        fn measure(&self, rig: &Rig, _settings: &Settings, watch: &mut dyn FnMut(usize) -> bool) -> Outcome {
            for block in 0..self.blocks {
                if !watch(block) {
                    self.stopped.store(true, Ordering::Release);
                    return Outcome::refused(rig.direction, gazelle_calibrate::session::STOPPED);
                }
                std::thread::sleep(block_wait());
            }
            self.answer.lock().unwrap().take().unwrap_or_else(|| Outcome::refused(rig.direction, "asked twice"))
        }
    }

    fn reading(device: &str, lag: f64) -> Measured1 {
        Measured1 {
            device: device.to_string(),
            lag_samples: lag,
            spread_samples: 0.2,
            clicks_found: 8,
            clicks_expected: 8,
            drift: None,
            nothing_arrived: false,
            note: format!("{device} recorded {lag} samples behind"),
        }
    }

    fn outcome() -> Outcome {
        Outcome {
            direction: Direction::Inputs,
            rate: 96000.0,
            block: 512,
            clicks: 8,
            readings: vec![reading("Quadro", 0.0), reading("Studio+", 27.8)],
            trims: vec![
                TrimChange {
                    device: "Quadro".into(),
                    field: "input_trim",
                    direction: Direction::Inputs,
                    old: 0,
                    measured: 0,
                    new: 0,
                    is_reference: true,
                    not_applied: None,
                },
                TrimChange {
                    device: "Studio+".into(),
                    field: "input_trim",
                    direction: Direction::Inputs,
                    old: 0,
                    measured: 28,
                    new: 28,
                    is_reference: false,
                    not_applied: None,
                },
            ],
            refusal: None,
        }
    }

    fn ask() -> Ask {
        Ask { direction: "inputs".into(), outputs: vec![0, 1], inputs: vec![0, 16], clicks: Some(2), level_dbfs: None }
    }

    fn settled(job: &Arc<Calibration>) -> Progress {
        for _ in 0..2000 {
            if !job.is_running() {
                return job.state();
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the run never finished");
    }

    #[test]
    fn an_ask_that_is_not_a_pass_or_not_a_rig_is_refused_in_a_sentence() {
        let sideways = Ask { direction: "sideways".into(), ..ask() };
        assert_eq!(sideways.taken().unwrap_err(), "A pass is inputs or outputs, not \"sideways\".");
        let lopsided = Ask { inputs: vec![0], ..ask() };
        assert!(lopsided.taken().unwrap_err().ends_with('.'), "a refusal from below is still a sentence");
        let none = Ask { clicks: Some(0), ..ask() };
        assert_eq!(none.taken().unwrap_err(), "A run plays between 1 and 64 clicks, not 0.");
        let forever = Ask { clicks: Some(CLICKS_MAX + 1), ..ask() };
        assert!(forever.taken().unwrap_err().contains("not 65"));
    }

    #[test]
    fn a_run_reports_what_it_measured_in_the_shape_the_page_reads() {
        let job = Arc::new(Calibration::new(Fake::new(outcome(), 4)));
        job.start(&ask()).expect("it starts");
        let state = settled(&job);
        assert_eq!(state.state, "done");
        let measured = state.outcome.expect("an outcome");
        assert_eq!(measured.direction, "inputs");
        assert_eq!((measured.rate, measured.buffer_size), (96000, 512));
        assert_eq!(measured.reference, "Quadro");
        assert!(measured.readings[0].is_reference);
        assert!(!measured.readings[1].is_reference);
        assert_eq!(measured.readings[1].lag_samples, 27.8);
        assert_eq!(measured.trims[1].was, 0);
        assert_eq!(measured.trims[1].measured, 28, "the late interface takes a positive trim");
        assert_eq!(measured.trims[1].now, 28);
        assert_eq!(measured.trims[1].field, "input_trim");
        assert!(measured.warnings.is_empty(), "a clean run warns about nothing");
    }

    #[test]
    fn only_one_run_at_a_time_and_the_second_ask_says_so() {
        let job = Arc::new(Calibration::new(Fake::new(outcome(), 200)));
        job.start(&ask()).expect("it starts");
        assert_eq!(job.start(&ask()).unwrap_err(), "A measurement is already running. Wait for it, or stop it first.");
        job.stop();
        settled(&job);
    }

    #[test]
    fn stopping_gives_the_run_up_and_says_so_rather_than_reporting_half_a_measurement() {
        let fake = Fake::new(outcome(), 100_000);
        let job = Arc::new(Calibration::new(Arc::clone(&fake) as Arc<dyn Measurer>));
        job.start(&ask()).expect("it starts");
        job.stop();
        let state = settled(&job);
        assert_eq!(state.state, "failed");
        assert!(fake.stopped.load(Ordering::Acquire), "the run itself was told");
        assert!(state.outcome.is_none(), "half a measurement is not an outcome");
        let refusal = state.refusal.expect("a reason");
        assert!(refusal.starts_with("The measurement was stopped"), "{refusal}");
        assert!(refusal.ends_with('.'));
    }

    #[test]
    fn a_refused_run_carries_the_reason_the_measuring_gave_and_nothing_else() {
        let refused = Outcome::refused(Direction::Inputs, "nothing arrived on Studio+ 1, so there is nothing to measure it against");
        let job = Arc::new(Calibration::new(Fake::new(refused, 2)));
        job.start(&ask()).expect("it starts");
        let state = settled(&job);
        assert_eq!(state.state, "failed");
        assert_eq!(
            state.refusal.as_deref(),
            Some("Nothing arrived on Studio+ 1, so there is nothing to measure it against.")
        );
        assert!(state.outcome.is_none());
    }

    #[test]
    fn what_is_worth_knowing_before_believing_the_numbers_is_collected_as_warnings() {
        let mut answer = outcome();
        answer.readings[1].drift = Some(gazelle_calibrate::measure::Drift { samples_per_second: 4.1, parts_per_million: 42.7 });
        answer.readings[1].note = "Studio+ drifted away from the Quadro through the run".into();
        answer.trims[1].not_applied = Some("the two are not holding one clock, and no trim fixes that".into());
        let job = Arc::new(Calibration::new(Fake::new(answer, 2)));
        job.start(&ask()).expect("it starts");
        let measured = settled(&job).outcome.expect("an outcome");
        assert_eq!(measured.readings[1].drift.as_ref().map(|drift| drift.ppm), Some(42.7));
        assert!(measured.readings[1].drift.as_ref().is_some_and(|drift| drift.real));
        assert_eq!(measured.warnings.len(), 2);
        assert!(measured.warnings[0].contains("drifted away"));
        assert!(measured.warnings[1].starts_with("Studio+: The two are not holding one clock"));
    }

    #[test]
    fn an_idle_calibration_says_nothing_about_a_run_that_has_not_happened() {
        let job = Arc::new(Calibration::new(Fake::new(outcome(), 1)));
        let state = job.state();
        assert_eq!(state.state, "idle");
        assert!(state.outcome.is_none() && state.refusal.is_none() && state.progress.is_none());
        assert!(!job.is_running());
    }

    #[test]
    fn a_run_is_followed_while_it_goes_and_never_reads_as_finished_before_it_is() {
        let job = Arc::new(Calibration::new(Fake::new(outcome(), 400)));
        job.start(&ask()).expect("it starts");
        let mut seen = None;
        for _ in 0..500 {
            let state = job.state();
            if state.step.as_deref() == Some("Playing the clicks") {
                seen = state.progress;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let progress = seen.expect("it says how far it has got");
        assert!((0.0..=0.99).contains(&progress), "{progress}");
        job.stop();
        settled(&job);
    }

    #[test]
    fn how_long_a_run_takes_is_what_it_was_told_to_play() {
        let settings = Settings { clicks: 8, spacing_seconds: 0.5, settle_seconds: 0.5, ..Settings::default() };
        assert_eq!(expected_seconds(&settings), 5.0);
        let quick = Settings { clicks: 1, spacing_seconds: 0.25, settle_seconds: 0.25, ..Settings::default() };
        assert_eq!(expected_seconds(&quick), 1.0);
    }
}
