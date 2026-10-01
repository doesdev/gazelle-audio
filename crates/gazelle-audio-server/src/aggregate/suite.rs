//! **The alignment suite**: every setup a person uses, measured in turn, and each trim kept for its
//! own setup.
//!
//! A trim is only true of the rate and the buffer size it was measured at, so lining the
//! interfaces up once is not enough for somebody who works at more than one. The suite takes a list
//! of setups (a rate and a buffer size each) and, for each in turn, puts the aggregate there through
//! the same two paths the page itself uses (the rate in the setup, and every interface's driver on
//! the buffer size, as Match buffer sizes does), measures it a few times over, and keeps the trim
//! only when the runs agree. When it is over, or stopped, the rate and the buffer size that were in
//! force before it started are put back.
//!
//! **It runs here, not in the page**, on a thread of its own, so closing the page does not stop it
//! and a page that comes back finds it where it is. It holds the measurement for the whole of its
//! run ([`Held`]), so no single run starts and the recorder will not arm until it has finished.
//!
//! # When runs agree
//!
//! Nothing in a run says whether another run would come to the same thing, so the rule is here.
//! Two runs are compared by what is the same in every session at one setup: **the lag minus the
//! phase**. Where an interface's capture starts moves by whole steps of 32 samples from session to
//! session, and the click lag moves with it exactly (the driver's own README shows six runs whose
//! lags ranged over 223 samples and whose lag minus phase was 144.4 every time), so the lags of two
//! runs are not comparable and that difference is. For an interface with no phase setup the lag
//! itself is what is compared, with the trim the runs ran on added back. The runs agree when, for
//! every interface measured, those figures are within [`AGREEMENT_SAMPLES`] of each other: a trim
//! is a whole number of samples, and the hardware itself was seen to wobble by one.
//!
//! The trim kept is the middle run's, with the phase reference that run measured beside it, so the
//! pair written down is always one real session's.
//!
//! # What is not kept
//!
//! A run the measurement refused, one that lost a block, one whose clicks did not agree with each
//! other, and one where nothing was heard on an interface's phase cable: the measurement's own
//! refusals apply to every run here exactly as to a single one. A refusal that no retry can change
//! (a channel the interface has not got, a driver another program has open) fails the setup at
//! once; a run spoiled by the audio, and runs that do not agree, are measured again, up to
//! [`ROUNDS`] times, and then the setup is failed with the figures.
//!
//! Everything this does to the PC is behind [`Bench`], and the measurement is the calibration's own
//! [`crate::aggregate::calibrate::Measurer`], so every rule here is tested on data.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gazelle_calibrate::{Pick, Rig, Settings};
use serde::{Deserialize, Serialize};

use crate::aggregate::calibrate::{measured, sentence, Ask, Calibration, Held, Measured};
use crate::aggregate::config::{SetupInForce, BUFFER_SIZES};
use crate::aggregate::service::AggregateService;
use crate::aggregate::{khz, rate_index, DeviceReport};
use crate::workspace::model::{AggregateTrim, Workspace};
use crate::workspace::store::WorkspaceStore;

/// How many runs each setup is measured with when the page does not say.
pub const RUNS_DEFAULT: u32 = 3;

/// The fewest runs a setup may be measured with: one run has nothing to agree with.
pub const RUNS_MIN: u32 = 2;

/// The most. Each run plays its clicks into the room, and ten agreeing is as sure as it gets.
pub const RUNS_MAX: u32 = 10;

/// How many times one setup is measured over, its runs each time, before it is failed: the first
/// time and two more.
pub const ROUNDS: u32 = 3;

/// How far apart, in samples, the runs of one setup may be and still be one measurement.
pub const AGREEMENT_SAMPLES: f64 = 1.0;

/// What the page asks for.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteAsk {
    /// Each rate and buffer size to measure, in the order to measure them.
    pub setups: Vec<SetupInForce>,
    /// How many runs each setup is measured with. [`RUNS_DEFAULT`] when left out.
    #[serde(default)]
    pub runs: Option<u32>,
    /// The cabling, exactly as a single run takes it: the output the click leaves on and the input
    /// it comes back on, one per interface, in the setup's order.
    pub outputs: Vec<Pick>,
    pub inputs: Vec<Pick>,
    #[serde(default)]
    pub clicks: Option<u32>,
    #[serde(default)]
    pub level_dbfs: Option<f64>,
    /// True when the person has been shown every change the suite makes to the setup and to the
    /// drivers, and said yes to all of them at once. Nothing starts without it.
    #[serde(default)]
    pub confirmed: bool,
}

/// What the suite was asked for, checked.
#[derive(Clone, Debug)]
pub struct Plan {
    pub setups: Vec<SetupInForce>,
    pub runs: u32,
    pub rig: Rig,
    pub settings: Settings,
}

impl SuiteAsk {
    /// The plan, or the sentence that says why this is not one. Everything that can be refused is
    /// refused here, before the setup is touched.
    pub fn taken(&self) -> Result<Plan, String> {
        if !self.confirmed {
            return Err("The suite changes the aggregate's rate and every interface's buffer size as it goes. Confirm those changes first.".into());
        }
        if self.setups.is_empty() {
            return Err("Choose at least one rate and buffer size to measure.".into());
        }
        for (at, setup) in self.setups.iter().enumerate() {
            if rate_index(setup.rate).is_none() {
                return Err(format!("{} Hz is not a rate these interfaces run at.", setup.rate));
            }
            if !BUFFER_SIZES.contains(&setup.buffer_size) {
                return Err(format!("{} samples is not a buffer size the drivers offer.", setup.buffer_size));
            }
            if self.setups[..at].contains(setup) {
                return Err(format!("{} is asked for twice.", setup.words()));
            }
        }
        let runs = self.runs.unwrap_or(RUNS_DEFAULT);
        if !(RUNS_MIN..=RUNS_MAX).contains(&runs) {
            return Err(format!("Each setup is measured between {RUNS_MIN} and {RUNS_MAX} times, not {runs}."));
        }
        let ask = Ask {
            direction: "inputs".into(),
            outputs: self.outputs.clone(),
            inputs: self.inputs.clone(),
            witnesses: None,
            clicks: self.clicks,
            level_dbfs: self.level_dbfs,
            check: false,
        };
        let (rig, settings) = ask.taken()?;
        Ok(Plan { setups: self.setups.clone(), runs, rig, settings })
    }
}

/// The rate and the buffer size in force before the suite started, which it puts back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Before {
    /// The setup's own rate: none is "whatever the interfaces are on".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<u32>,
    /// The buffer size the setup offers a DAW.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_size: Option<u32>,
    /// The buffer size the interfaces' drivers were on, when it could be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_buffer: Option<u32>,
    /// The rate the interfaces themselves were running at, when it could be read. Every run moves
    /// them to its own rate, and they stay there until something moves them back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interfaces_rate: Option<u32>,
}

/// One interface's trim, as the suite keeps it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Kept {
    /// The interface's place in the setup, from zero.
    pub index: usize,
    /// What the driver's file and the measurement call it.
    pub device: String,
    pub input_trim: i32,
    /// The phase measured beside it, when the interface has a phase setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<i32>,
    /// How far apart the runs were, in samples: the widest difference between them.
    pub spread_samples: f64,
}

/// Everything the suite does to the PC, behind one trait so that no test changes a rate.
pub trait Bench: Send + Sync {
    /// What is in force now, to be put back.
    fn before(&self) -> Result<Before, String>;
    /// Put the aggregate at this rate and every interface's driver on this buffer size.
    fn switch_to(&self, setup: SetupInForce) -> Result<(), String>;
    /// Put back what [`Bench::before`] read.
    fn put_back(&self, before: &Before) -> Result<(), String>;
    /// Keep these trims, each for this setup, beside whatever else is kept.
    fn keep(&self, setup: SetupInForce, trims: &[Kept]) -> Result<(), String>;
}

/// How one setup is getting on, as the page reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SetupProgress {
    pub rate: u32,
    pub buffer_size: u32,
    /// `waiting`, `switching`, `measuring`, `saved`, `failed`, or `stopped` for the one a stop
    /// interrupted and every one after it.
    pub state: &'static str,
    /// Which run is going, from one, while it measures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<u32>,
    /// Which time over this setup is being measured, from one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
    /// What was kept, once it is saved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trims: Vec<Kept>,
    /// Why it was not saved, in a sentence, with the figures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// The whole suite, as `GET /api/v1/aggregate/suite` answers it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SuiteState {
    /// `idle`, `running`, `stopping`, `done`, `stopped` or `failed`.
    pub state: &'static str,
    /// How many runs each setup is measured with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs: Option<u32>,
    pub setups: Vec<SetupProgress>,
    /// How many setups have had their trims saved so far, which is how a page knows to read the
    /// setup again.
    pub saved: u32,
    /// What was in force before, which is what is put back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Before>,
    /// What putting it back came to, once it has been.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored: Option<String>,
    /// True when putting it back did not go through.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub restore_failed: bool,
    /// Why the suite as a whole did not run, when it did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

impl Default for SuiteState {
    fn default() -> Self {
        SuiteState { state: "idle", runs: None, setups: Vec::new(), saved: 0, before: None, restored: None, restore_failed: false, refusal: None }
    }
}

/// What one run came to, for the suite.
#[derive(Clone, Debug, PartialEq)]
pub struct RunFigures {
    /// Per interface measured (every one but the one the others are measured against): what is
    /// compared between runs, and what would be kept from this run.
    pub interfaces: Vec<RunInterface>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunInterface {
    pub index: usize,
    pub device: String,
    /// The lag minus the phase, with the trim it ran on added back: the same in every session at
    /// one setup.
    pub comparable: f64,
    /// What this run would write: the trim, and the phase reference beside it.
    pub input_trim: i32,
    pub reference: Option<i32>,
}

/// Why a run cannot be used.
#[derive(Clone, Debug, PartialEq)]
pub enum Unusable {
    /// Measuring again cannot change it: the setup is failed now.
    Final(String),
    /// The audio spoiled it, or nothing was heard on a cable this once: measure again.
    Again(String),
}

/// **Whether one run can be used, and what it measured.** A refusal is final; a run at a setup
/// other than the one asked for is final too, because the interfaces did not take it. A run that
/// lost blocks, one with a trim the measurement would not offer, and one where an interface's phase
/// was not heard are measured again.
pub fn read_run(outcome: &Measured, setup: SetupInForce) -> Result<RunFigures, Unusable> {
    if outcome.rate != setup.rate || u32::try_from(outcome.buffer_size).ok() != Some(setup.buffer_size) {
        return Err(Unusable::Final(format!(
            "The run happened at {} at {} samples rather than {}, so the interfaces did not take this setup.",
            khz(outcome.rate),
            outcome.buffer_size,
            setup.words()
        )));
    }
    if !outcome.clean {
        return Err(Unusable::Again(format!("A run lost {} while it went, so nothing from it is kept.", blocks(outcome.blocks_lost))));
    }
    let mut interfaces = Vec::new();
    for (index, trim) in outcome.trims.iter().enumerate() {
        if trim.is_reference || trim.field != "input_trim" {
            continue;
        }
        if let Some(why) = &trim.not_applied {
            return Err(Unusable::Again(format!("{}: {}", trim.device, sentence(why))));
        }
        let Some(reading) = outcome.readings.get(index) else {
            return Err(Unusable::Final(format!("{} has a trim and no reading, which a run never gives.", trim.device)));
        };
        let (comparable, reference) = match trim.phase_reference {
            None => (f64::from(trim.was) + reading.lag_samples, None),
            Some(reference) => match reference.now {
                Some(phase) => (f64::from(trim.was) + reading.lag_samples - f64::from(phase), Some(phase)),
                None => {
                    return Err(Unusable::Again(format!(
                        "Nothing was heard on {}'s phase cable in one run, so its trim would have no reference.",
                        trim.device
                    )))
                }
            },
        };
        interfaces.push(RunInterface { index, device: trim.device.clone(), comparable, input_trim: trim.now, reference });
    }
    if interfaces.is_empty() {
        return Err(Unusable::Final("The run measured no interface against another, so there is no trim to keep.".into()));
    }
    Ok(RunFigures { interfaces })
}

/// **Whether the runs of one setup agree**, and the trims to keep when they do: for each interface
/// the middle run's trim and reference. When they do not, the sentence that says by how much, with
/// every figure.
pub fn agreement(runs: &[RunFigures]) -> Result<Vec<Kept>, String> {
    let Some(first) = runs.first() else { return Err("There were no runs to compare.".into()) };
    let mut kept = Vec::new();
    let mut apart = Vec::new();
    for (at, interface) in first.interfaces.iter().enumerate() {
        let mut each: Vec<&RunInterface> = runs.iter().filter_map(|run| run.interfaces.get(at)).collect();
        if each.len() != runs.len() || each.iter().any(|one| one.index != interface.index) {
            return Err("The runs did not measure the same interfaces.".into());
        }
        each.sort_by(|a, b| a.comparable.total_cmp(&b.comparable));
        let spread = each.last().map_or(0.0, |last| last.comparable) - each.first().map_or(0.0, |first| first.comparable);
        if spread > AGREEMENT_SAMPLES {
            let figures: Vec<String> = runs.iter().filter_map(|run| run.interfaces.get(at)).map(|one| format!("{:.1}", one.comparable)).collect();
            apart.push(format!("{} came to {} ({} samples apart)", interface.device, listed(&figures), format_args!("{spread:.1}")));
            continue;
        }
        let middle = each[(each.len() - 1) / 2];
        kept.push(Kept {
            index: middle.index,
            device: middle.device.clone(),
            input_trim: middle.input_trim,
            reference: middle.reference,
            spread_samples: (spread * 10.0).round() / 10.0,
        });
    }
    if apart.is_empty() {
        Ok(kept)
    } else {
        Err(format!("The runs did not agree within {AGREEMENT_SAMPLES} sample: {}.", apart.join("; ")))
    }
}

/// The one suite at a time, and everything anybody asks about it.
pub struct Suite {
    state: Mutex<SuiteState>,
    stop: AtomicBool,
    running: AtomicBool,
    calibration: Arc<Calibration>,
    bench: Arc<dyn Bench>,
    /// How long to leave the drivers after a buffer change before measuring, which they need at
    /// the hardware and a test does not.
    settle: Duration,
}

impl Suite {
    pub fn new(calibration: Arc<Calibration>, bench: Arc<dyn Bench>, settle: Duration) -> Suite {
        Suite { state: Mutex::new(SuiteState::default()), stop: AtomicBool::new(false), running: AtomicBool::new(false), calibration, bench, settle }
    }

    pub fn state(&self) -> SuiteState {
        self.state.lock().map(|state| state.clone()).unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Stop after the run that is going, which is given up on there and then. What was saved stays
    /// saved, and what was in force before is put back.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut state) = self.state.lock() {
            if state.state == "running" {
                state.state = "stopping";
            }
        }
    }

    /// Start, on a thread of its own. The refusal is what the page shows, so it is a sentence.
    pub fn start(self: &Arc<Self>, ask: &SuiteAsk) -> Result<(), String> {
        let plan = ask.taken()?;
        if self.running.swap(true, Ordering::AcqRel) {
            return Err("The alignment suite is already running. Wait for it, or stop it first.".into());
        }
        let held = match self.calibration.hold() {
            Ok(held) => held,
            Err(why) => {
                self.running.store(false, Ordering::Release);
                return Err(why);
            }
        };
        self.stop.store(false, Ordering::Release);
        *self.state.lock().expect("the suite's state") = SuiteState {
            state: "running",
            runs: Some(plan.runs),
            setups: plan
                .setups
                .iter()
                .map(|setup| SetupProgress { rate: setup.rate, buffer_size: setup.buffer_size, state: "waiting", run: None, round: None, trims: Vec::new(), why: None })
                .collect(),
            ..SuiteState::default()
        };
        let job = Arc::clone(self);
        std::thread::Builder::new().name("gazelle-suite".into()).spawn(move || job.run(plan, held)).map_err(|e| {
            self.running.store(false, Ordering::Release);
            format!("The alignment suite could not be started: {e}")
        })?;
        Ok(())
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn change(&self, update: impl FnOnce(&mut SuiteState)) {
        if let Ok(mut state) = self.state.lock() {
            update(&mut state);
        }
    }

    fn at(&self, index: usize, update: impl FnOnce(&mut SetupProgress)) {
        self.change(|state| {
            if let Some(setup) = state.setups.get_mut(index) {
                update(setup);
            }
        });
    }

    fn run(self: Arc<Self>, plan: Plan, held: Held) {
        let before = match self.bench.before() {
            Ok(before) => before,
            Err(why) => {
                // Nothing has been changed, so there is nothing to put back.
                self.change(|state| {
                    state.state = "failed";
                    state.refusal = Some(format!("What is in force now could not be read, so nothing was changed: {}", sentence(&why)));
                    for setup in &mut state.setups {
                        setup.state = "failed";
                    }
                });
                drop(held);
                self.running.store(false, Ordering::Release);
                return;
            }
        };
        self.change(|state| state.before = Some(before));
        for (index, setup) in plan.setups.iter().enumerate() {
            if self.stopped() {
                break;
            }
            self.one_setup(&held, &plan, index, *setup);
        }
        if self.stopped() {
            self.change(|state| {
                for setup in &mut state.setups {
                    if matches!(setup.state, "waiting" | "switching" | "measuring") {
                        setup.state = "stopped";
                        setup.run = None;
                    }
                }
            });
        }
        // Whatever happened, what was in force is put back, and the measurement let go of.
        let restored = self.bench.put_back(&before);
        drop(held);
        self.change(|state| {
            state.state = if self.stop.load(Ordering::Acquire) { "stopped" } else { "done" };
            match restored {
                Ok(()) => state.restored = Some(restored_words(&before)),
                Err(why) => {
                    state.restored = Some(format!("Putting back what was in force did not go through: {}", sentence(&why)));
                    state.restore_failed = true;
                }
            }
        });
        self.running.store(false, Ordering::Release);
    }

    /// One setup: switch to it, measure it, keep its trims or say why not.
    fn one_setup(&self, held: &Held, plan: &Plan, index: usize, setup: SetupInForce) {
        self.at(index, |progress| progress.state = "switching");
        if let Err(why) = self.bench.switch_to(setup) {
            self.at(index, |progress| {
                progress.state = "failed";
                progress.why = Some(format!("The aggregate could not be put at {}: {}", setup.words(), sentence(&why)));
            });
            return;
        }
        if !self.settle.is_zero() {
            std::thread::sleep(self.settle);
        }
        let mut settings = plan.settings;
        settings.rate = Some(f64::from(setup.rate));
        settings.buffer_size = Some(setup.buffer_size as i32);
        let mut last = String::new();
        for round in 1..=ROUNDS {
            let mut runs = Vec::new();
            let mut spoiled = None;
            for run in 1..=plan.runs {
                if self.stopped() {
                    return;
                }
                self.at(index, |progress| {
                    progress.state = "measuring";
                    progress.run = Some(run);
                    progress.round = Some(round);
                });
                let outcome = held.measure(&plan.rig, &settings, &mut |_| !self.stopped());
                if self.stopped() {
                    return;
                }
                if let Some(why) = &outcome.refusal {
                    self.fail(index, sentence(why));
                    return;
                }
                match read_run(&measured(&outcome, &plan.rig), setup) {
                    Ok(figures) => runs.push(figures),
                    Err(Unusable::Final(why)) => {
                        self.fail(index, why);
                        return;
                    }
                    Err(Unusable::Again(why)) => {
                        spoiled = Some(why);
                        break;
                    }
                }
            }
            let tried = match spoiled {
                Some(why) => why,
                None => match agreement(&runs) {
                    Ok(kept) => {
                        match self.bench.keep(setup, &kept) {
                            Ok(()) => self.change(|state| {
                                state.saved += 1;
                                if let Some(progress) = state.setups.get_mut(index) {
                                    progress.state = "saved";
                                    progress.run = None;
                                    progress.trims = kept;
                                }
                            }),
                            Err(why) => self.fail(index, format!("The trims agreed and could not be saved: {}", sentence(&why))),
                        }
                        return;
                    }
                    Err(why) => why,
                },
            };
            last = tried;
        }
        self.fail(index, format!("{last} Measured {} over, and nothing was kept for this setup.", times(ROUNDS)));
    }

    fn fail(&self, index: usize, why: String) {
        self.at(index, |progress| {
            progress.state = "failed";
            progress.run = None;
            progress.why = Some(why);
        });
    }
}

/// **This PC**: the setup in the workspace, saved through the store so the driver's file follows
/// it, every interface's driver put on a buffer size the way Match buffer sizes does it, and the
/// interfaces' own rate put back the way the Devices page sets it.
pub struct ThisPc {
    pub store: Arc<dyn WorkspaceStore>,
    pub service: Arc<AggregateService>,
    /// The server's `--dry-run`, which every write here honours.
    pub dry_run: bool,
}

impl ThisPc {
    fn load(&self) -> Result<Workspace, String> {
        self.store.load().map_err(|e| e.to_string())
    }

    fn save(&self, workspace: &Workspace) -> Result<(), String> {
        self.store.save(workspace).map_err(|e| e.to_string())
    }

    /// The setup's own rate and buffer size, and nothing else of it.
    fn setup(&self, rate: Option<u32>, buffer_size: Option<u32>) -> Result<(), String> {
        let mut workspace = self.load()?;
        let Some(config) = workspace.aggregate.as_mut() else { return Err("no interfaces have been chosen for the aggregate".into()) };
        config.rate = rate;
        config.buffer_size = buffer_size;
        self.save(&workspace)
    }

    /// Every configured interface's driver on one buffer size, refused by name when one will not.
    fn drivers(&self, size: u32) -> Result<(), String> {
        let workspace = self.load()?;
        let Some(config) = workspace.aggregate.clone() else { return Err("no interfaces have been chosen for the aggregate".into()) };
        let refused: Vec<String> = self
            .service
            .match_buffers(&config, &workspace, size, false, self.dry_run)
            .into_iter()
            .filter_map(|outcome| outcome.error.map(|error| format!("{}: {}", outcome.device, error.message)))
            .collect();
        if refused.is_empty() {
            Ok(())
        } else {
            Err(format!("not every driver took {size} samples. {}", refused.join(" ")))
        }
    }

    /// Every configured interface Gazelle is connected to, put at this rate with `set_samp_rate`.
    fn interfaces_at(&self, devices: &[DeviceReport], hz: u32) -> Result<(), String> {
        let Some(index) = rate_index(hz) else { return Err(format!("{hz} Hz is not a rate the interfaces take")) };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        let mut refused = Vec::new();
        for device in devices.iter().filter(|device| device.attached) {
            let Some(id) = device.device_id.clone() else { continue };
            let sent = self.service.devices.handle(&id).map_err(|e| e.to_string()).and_then(|handle| {
                let values = crate::value::json_to_payload_values(&serde_json::json!({ "srate_idx": index })).map_err(|e| e.to_string())?;
                runtime.block_on(handle.request("set_samp_rate", values, None, self.dry_run)).map(|_| ()).map_err(|e| e.to_string())
            });
            if let Err(why) = sent {
                refused.push(format!("{}: {why}", device.name));
            }
        }
        if refused.is_empty() {
            Ok(())
        } else {
            Err(format!("not every interface went back to {}. {}", khz(hz), refused.join(" ")))
        }
    }
}

impl Bench for ThisPc {
    fn before(&self) -> Result<Before, String> {
        let workspace = self.load()?;
        let Some(config) = workspace.aggregate.clone() else { return Err("no interfaces have been chosen for the aggregate".into()) };
        let answer = self.service.answer(&workspace);
        let first = |of: &dyn Fn(&DeviceReport) -> Option<u32>| answer.devices.iter().find(|d| d.is_master).and_then(of).or_else(|| answer.devices.iter().find_map(of));
        Ok(Before {
            rate: config.rate,
            buffer_size: config.buffer_size,
            driver_buffer: first(&|device| device.driver.buffer_size),
            interfaces_rate: first(&|device| device.clock.as_ref().and_then(|clock| clock.running_rate())),
        })
    }

    fn switch_to(&self, setup: SetupInForce) -> Result<(), String> {
        self.setup(Some(setup.rate), Some(setup.buffer_size))?;
        self.drivers(setup.buffer_size)
    }

    fn put_back(&self, before: &Before) -> Result<(), String> {
        self.setup(before.rate, before.buffer_size)?;
        let mut problems = Vec::new();
        if let Some(size) = before.driver_buffer {
            if let Err(why) = self.drivers(size) {
                problems.push(why);
            }
        }
        if let Some(hz) = before.interfaces_rate {
            let workspace = self.load()?;
            let devices = self.service.answer(&workspace).devices;
            if let Err(why) = self.interfaces_at(&devices, hz) {
                problems.push(why);
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join(" "))
        }
    }

    fn keep(&self, setup: SetupInForce, trims: &[Kept]) -> Result<(), String> {
        let mut workspace = self.load()?;
        let Some(config) = workspace.aggregate.as_mut() else { return Err("no interfaces have been chosen for the aggregate".into()) };
        for kept in trims {
            let Some(device) = config.devices.get_mut(kept.index) else {
                return Err(format!("{} is not in the aggregate any more", kept.device));
            };
            device.keep_trim(AggregateTrim { rate: setup.rate, buffer_size: setup.buffer_size, input_trim: kept.input_trim, reference: kept.reference });
        }
        self.save(&workspace)
    }
}

/// What putting back came to, in words.
fn restored_words(before: &Before) -> String {
    let rate = before.rate.or(before.interfaces_rate).map_or_else(|| "the rate it had".to_string(), khz);
    let buffer = match (before.driver_buffer, before.buffer_size) {
        (Some(size), _) | (None, Some(size)) => format!("{size} samples"),
        (None, None) => "the buffer size it had".to_string(),
    };
    format!("Put back to {rate} and {buffer}, as it was before the suite.")
}

fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

fn blocks(count: u64) -> String {
    format!("{count} block{}", if count == 1 { "" } else { "s" })
}

fn times(count: u32) -> String {
    match count {
        1 => "once".to_string(),
        2 => "twice".to_string(),
        n => format!("{n} times"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Instant;

    use gazelle_calibrate::measure::{Glitches, Reading};
    use gazelle_calibrate::trim::TrimChange;
    use gazelle_calibrate::{Direction, Outcome, PhaseReference};

    use super::*;
    use crate::aggregate::calibrate::Measurer;

    /// What the fake measurer does when asked: answer, or play until it is stopped.
    enum Run {
        Answer(Outcome),
        UntilStopped,
    }

    /// A measurer made of data, answering in turn and remembering what it was asked for.
    struct Script {
        runs: Mutex<VecDeque<Run>>,
        asked: Mutex<Vec<(Option<f64>, Option<i32>)>>,
    }

    impl Script {
        fn of(runs: Vec<Run>) -> Arc<Script> {
            Arc::new(Script { runs: Mutex::new(runs.into()), asked: Mutex::new(Vec::new()) })
        }

        fn asked(&self) -> Vec<(Option<f64>, Option<i32>)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl Measurer for Script {
        fn measure(&self, rig: &Rig, settings: &Settings, watch: &mut dyn FnMut(usize) -> bool) -> Outcome {
            self.asked.lock().unwrap().push((settings.rate, settings.buffer_size));
            let next = self.runs.lock().unwrap().pop_front();
            match next {
                Some(Run::Answer(outcome)) => outcome,
                Some(Run::UntilStopped) => {
                    let mut block = 0;
                    while watch(block) {
                        block += 1;
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Outcome::refused(rig.direction, gazelle_calibrate::session::STOPPED)
                }
                None => Outcome::refused(rig.direction, "the script ran out"),
            }
        }
    }

    /// Everything the suite asked the PC to do, in order.
    #[derive(Default)]
    struct FakeBench {
        did: Mutex<Vec<String>>,
        kept: Mutex<Vec<(SetupInForce, Vec<Kept>)>>,
        refuse_switch_to: Option<SetupInForce>,
    }

    impl FakeBench {
        fn did(&self) -> Vec<String> {
            self.did.lock().unwrap().clone()
        }
    }

    impl Bench for FakeBench {
        fn before(&self) -> Result<Before, String> {
            self.did.lock().unwrap().push("read".into());
            Ok(Before { rate: Some(96000), buffer_size: Some(512), driver_buffer: Some(512), interfaces_rate: Some(96000) })
        }

        fn switch_to(&self, setup: SetupInForce) -> Result<(), String> {
            self.did.lock().unwrap().push(format!("switch {}", setup.words()));
            if self.refuse_switch_to == Some(setup) {
                return Err("Studio+: the interface is in use".into());
            }
            Ok(())
        }

        fn put_back(&self, before: &Before) -> Result<(), String> {
            self.did.lock().unwrap().push(format!("put back {:?} {:?}", before.rate, before.driver_buffer));
            Ok(())
        }

        fn keep(&self, setup: SetupInForce, trims: &[Kept]) -> Result<(), String> {
            self.did.lock().unwrap().push(format!("keep {}", setup.words()));
            self.kept.lock().unwrap().push((setup, trims.to_vec()));
            Ok(())
        }
    }

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
            note: format!("{device} recorded {lag} samples behind"),
        }
    }

    fn trim(device: &str, lag: f64, is_reference: bool, phase: Option<Option<i32>>) -> TrimChange {
        let measured = if is_reference { 0 } else { lag.round() as i32 };
        TrimChange {
            device: device.into(),
            field: "input_trim",
            direction: Direction::Inputs,
            old: 0,
            measured,
            new: measured,
            is_reference,
            not_applied: None,
            phase_reference: phase.map(|new| PhaseReference { old: None, new }),
        }
    }

    /// One run at a setup: the Studio+ `lag` samples behind the Quadro, with the phase it heard
    /// (outer None for an interface with no phase setup, inner None for nothing heard).
    fn outcome_at(setup: SetupInForce, lag: f64, phase: Option<Option<i32>>) -> Outcome {
        Outcome {
            direction: Direction::Inputs,
            rate: f64::from(setup.rate),
            block: setup.buffer_size as i32,
            clicks: 8,
            readings: vec![reading("Quadro", 0.0), reading("Studio+", lag)],
            witnesses: Vec::new(),
            trims: vec![trim("Quadro", 0.0, true, None), trim("Studio+", lag, false, phase)],
            phases: Vec::new(),
            checking: false,
            refusal: None,
        }
    }

    fn run_at(setup: SetupInForce, lag: f64, phase: Option<Option<i32>>) -> Run {
        Run::Answer(outcome_at(setup, lag, phase))
    }

    const AT_96: SetupInForce = SetupInForce { rate: 96000, buffer_size: 512 };
    const AT_48: SetupInForce = SetupInForce { rate: 48000, buffer_size: 256 };

    fn ask(setups: Vec<SetupInForce>, runs: u32) -> SuiteAsk {
        SuiteAsk {
            setups,
            runs: Some(runs),
            outputs: vec![Pick::new(0, 0), Pick::new(0, 1)],
            inputs: vec![Pick::new(0, 0), Pick::new(1, 0)],
            clicks: Some(2),
            level_dbfs: None,
            confirmed: true,
        }
    }

    fn rig() -> Rig {
        ask(vec![AT_96], 3).taken().expect("a plan").rig
    }

    fn suite(script: &Arc<Script>, bench: &Arc<FakeBench>) -> (Arc<Suite>, Arc<Calibration>) {
        let calibration = Arc::new(Calibration::new(Arc::clone(script) as Arc<dyn Measurer>));
        (Arc::new(Suite::new(Arc::clone(&calibration), Arc::clone(bench) as Arc<dyn Bench>, Duration::ZERO)), calibration)
    }

    fn finished(job: &Arc<Suite>) -> SuiteState {
        let since = Instant::now();
        while job.is_running() {
            assert!(since.elapsed() < Duration::from_secs(10), "the suite never finished");
            std::thread::sleep(Duration::from_millis(2));
        }
        job.state()
    }

    #[test]
    fn what_the_suite_is_asked_for_is_checked_before_anything_is_changed() {
        let unconfirmed = SuiteAsk { confirmed: false, ..ask(vec![AT_96], 3) };
        assert!(unconfirmed.taken().unwrap_err().contains("Confirm those changes first"));
        assert_eq!(ask(Vec::new(), 3).taken().unwrap_err(), "Choose at least one rate and buffer size to measure.");
        assert_eq!(ask(vec![AT_96], 1).taken().unwrap_err(), "Each setup is measured between 2 and 10 times, not 1.");
        assert!(ask(vec![AT_96], 11).taken().is_err());
        assert_eq!(ask(vec![AT_96, AT_96], 3).taken().unwrap_err(), "96 kHz at 512 samples is asked for twice.");
        assert!(ask(vec![SetupInForce { rate: 97000, buffer_size: 512 }], 3).taken().unwrap_err().contains("97000 Hz"));
        assert!(ask(vec![SetupInForce { rate: 96000, buffer_size: 500 }], 3).taken().unwrap_err().contains("500 samples"));
        let plain: SuiteAsk = serde_json::from_str(
            r#"{"setups":[{"rate":96000,"buffer_size":512}],"outputs":[{"device":0,"channel":0},{"device":0,"channel":1}],"inputs":[{"device":0,"channel":0},{"device":1,"channel":0}],"confirmed":true}"#,
        )
        .expect("the page's request");
        let plan = plain.taken().expect("a plan");
        assert_eq!(plan.runs, RUNS_DEFAULT, "three runs when the page does not say");
        assert!(!plan.settings.checking, "the suite measures, it never checks");
    }

    /// Runs whose lags differ by whole steps of 32 and whose lag minus phase is the same are one
    /// measurement, which is what the hardware showed on 2026-09-21.
    #[test]
    fn runs_agree_by_their_lag_minus_their_phase_and_the_middle_one_is_kept() {
        let runs: Vec<RunFigures> = [(60.4, -84), (-3.6, -148), (-162.6, -307)]
            .into_iter()
            .map(|(lag, phase)| read_run(&measured(&outcome_at(AT_96, lag, Some(Some(phase))), &rig()), AT_96).expect("a usable run"))
            .collect();
        let kept = agreement(&runs).expect("they are one measurement");
        assert_eq!(kept.len(), 1, "the interface everything is measured against keeps nothing");
        assert_eq!((kept[0].index, kept[0].device.as_str()), (1, "Studio+"));
        assert_eq!((kept[0].input_trim, kept[0].reference), (-4, Some(-148)), "the middle run's trim with that run's own reference");
        assert!(kept[0].spread_samples <= AGREEMENT_SAMPLES, "{}", kept[0].spread_samples);

        // Without a phase the lag itself is what is compared, and a sample and a half apart is not one measurement.
        let unphased: Vec<RunFigures> = [27.8, 28.1, 29.3]
            .into_iter()
            .map(|lag| RunFigures { interfaces: vec![RunInterface { index: 1, device: "Studio+".into(), comparable: lag, input_trim: lag.round() as i32, reference: None }] })
            .collect();
        let why = agreement(&unphased).unwrap_err();
        assert_eq!(why, "The runs did not agree within 1 sample: Studio+ came to 27.8, 28.1 and 29.3 (1.5 samples apart).");
        assert!(agreement(&unphased[..2]).is_ok(), "the first two alone agree");
    }

    #[test]
    fn a_run_that_cannot_be_kept_is_measured_again_or_ends_the_setup_and_says_why() {
        let read = |outcome: Outcome| read_run(&measured(&outcome, &rig()), AT_96);
        let mut lost = outcome_at(AT_96, 28.0, None);
        lost.readings[1].glitches = Glitches { dropped: 2, starved: 0 };
        assert_eq!(read(lost), Err(Unusable::Again("A run lost 2 blocks while it went, so nothing from it is kept.".into())));
        assert!(matches!(read(outcome_at(AT_96, 28.0, Some(None))), Err(Unusable::Again(why)) if why.contains("Nothing was heard on Studio+'s phase cable")));
        let mut spread = outcome_at(AT_96, 28.0, None);
        spread.trims[1].not_applied = Some("the clicks did not agree with each other".into());
        assert_eq!(read(spread), Err(Unusable::Again("Studio+: The clicks did not agree with each other.".into())));
        assert!(matches!(read(outcome_at(AT_48, 28.0, None)), Err(Unusable::Final(why)) if why.starts_with("The run happened at 48 kHz at 256 samples rather than 96 kHz at 512 samples")));
        assert_eq!(read(outcome_at(AT_96, 27.6, None)).expect("a clean run").interfaces[0].comparable, 27.6);
    }

    /// The whole of it: each setup switched to through the bench, measured at exactly that setup,
    /// kept when its runs agree, and what was in force put back at the end.
    #[test]
    fn each_setup_is_switched_to_measured_kept_and_the_setup_before_is_put_back() {
        let script = Script::of(vec![
            run_at(AT_96, 27.8, Some(Some(-148))),
            run_at(AT_96, 28.1, Some(Some(-148))),
            run_at(AT_96, 27.9, Some(Some(-148))),
            run_at(AT_48, 14.2, None),
            run_at(AT_48, 14.4, None),
            run_at(AT_48, 14.3, None),
        ]);
        let bench = Arc::new(FakeBench::default());
        let (job, calibration) = suite(&script, &bench);
        job.start(&ask(vec![AT_96, AT_48], 3)).expect("it starts");
        assert!(calibration.is_running(), "the measurement is held while it goes, so the recorder will not arm");
        let state = finished(&job);
        assert!(!calibration.is_running(), "and let go of afterwards");
        assert_eq!(state.state, "done");
        assert_eq!(state.saved, 2);
        assert_eq!(state.setups.iter().map(|setup| setup.state).collect::<Vec<_>>(), ["saved", "saved"]);
        assert_eq!(state.setups[0].trims[0].input_trim, 28);
        assert_eq!(state.setups[0].trims[0].reference, Some(-148));
        assert_eq!(state.setups[1].trims[0].input_trim, 14);
        assert_eq!(state.setups[1].trims[0].reference, None);
        assert_eq!(
            bench.did(),
            ["read", "switch 96 kHz at 512 samples", "keep 96 kHz at 512 samples", "switch 48 kHz at 256 samples", "keep 48 kHz at 256 samples", "put back Some(96000) Some(512)"]
        );
        let at_96 = [(Some(96000.0), Some(512)); 3];
        let at_48 = [(Some(48000.0), Some(256)); 3];
        assert_eq!(script.asked(), at_96.into_iter().chain(at_48).collect::<Vec<_>>(), "every run at its own setup");
        assert_eq!(state.restored.as_deref(), Some("Put back to 96 kHz and 512 samples, as it was before the suite."));
        assert!(!state.restore_failed);
    }

    #[test]
    fn runs_that_disagree_are_measured_again_and_kept_when_they_then_agree() {
        let script = Script::of(vec![
            run_at(AT_96, 27.8, None),
            run_at(AT_96, 31.0, None),
            run_at(AT_96, 28.0, None),
            run_at(AT_96, 28.0, None),
            run_at(AT_96, 28.2, None),
            run_at(AT_96, 27.9, None),
        ]);
        let bench = Arc::new(FakeBench::default());
        let (job, _) = suite(&script, &bench);
        job.start(&ask(vec![AT_96], 3)).expect("it starts");
        let state = finished(&job);
        assert_eq!(state.setups[0].state, "saved");
        assert_eq!(state.setups[0].round, Some(2), "the second time over");
        assert_eq!(state.setups[0].trims[0].input_trim, 28);
        assert_eq!(script.asked().len(), 6);
    }

    #[test]
    fn a_setup_whose_runs_never_agree_is_failed_with_the_figures_and_the_next_one_goes_on() {
        let mut runs = Vec::new();
        for _ in 0..ROUNDS {
            runs.extend([run_at(AT_96, 20.0, None), run_at(AT_96, 28.0, None)]);
        }
        runs.extend([run_at(AT_48, 14.0, None), run_at(AT_48, 14.0, None)]);
        let script = Script::of(runs);
        let bench = Arc::new(FakeBench::default());
        let (job, _) = suite(&script, &bench);
        job.start(&ask(vec![AT_96, AT_48], 2)).expect("it starts");
        let state = finished(&job);
        assert_eq!(state.setups[0].state, "failed");
        let why = state.setups[0].why.clone().expect("why");
        assert!(why.starts_with("The runs did not agree within 1 sample: Studio+ came to 20.0 and 28.0 (8.0 samples apart)."), "{why}");
        assert!(why.ends_with("Measured 3 times over, and nothing was kept for this setup."), "{why}");
        assert!(state.setups[0].trims.is_empty());
        assert_eq!(state.setups[1].state, "saved", "the next setup is measured all the same");
        assert_eq!(state.saved, 1);
        assert_eq!(bench.kept.lock().unwrap().len(), 1, "nothing at all was kept for the one that failed");
    }

    #[test]
    fn a_refused_run_fails_its_setup_at_once_and_a_spoiled_one_is_measured_again() {
        let mut lost = outcome_at(AT_48, 14.0, None);
        lost.readings[1].glitches = Glitches { dropped: 1, starved: 0 };
        let script = Script::of(vec![
            Run::Answer(Outcome::refused(Direction::Inputs, "nothing arrived on Studio+ 1, so there is nothing to measure it against")),
            Run::Answer(lost),
            run_at(AT_48, 14.0, None),
            run_at(AT_48, 14.0, None),
        ]);
        let bench = Arc::new(FakeBench::default());
        let (job, _) = suite(&script, &bench);
        job.start(&ask(vec![AT_96, AT_48], 2)).expect("it starts");
        let state = finished(&job);
        assert_eq!(state.setups[0].state, "failed");
        assert_eq!(state.setups[0].why.as_deref(), Some("Nothing arrived on Studio+ 1, so there is nothing to measure it against."));
        assert_eq!(state.setups[1].state, "saved", "the run that lost a block was measured again");
        assert_eq!(script.asked().len(), 4, "one refused, one spoiled, two kept");
    }

    #[test]
    fn a_setup_the_pc_will_not_be_put_at_is_failed_and_nothing_is_measured_at_it() {
        let script = Script::of(vec![run_at(AT_48, 14.0, None), run_at(AT_48, 14.0, None)]);
        let bench = Arc::new(FakeBench { refuse_switch_to: Some(AT_96), ..FakeBench::default() });
        let (job, _) = suite(&script, &bench);
        job.start(&ask(vec![AT_96, AT_48], 2)).expect("it starts");
        let state = finished(&job);
        assert_eq!(state.setups[0].why.as_deref(), Some("The aggregate could not be put at 96 kHz at 512 samples: Studio+: the interface is in use."));
        assert_eq!(state.setups[1].state, "saved");
        assert_eq!(script.asked(), [(Some(48000.0), Some(256)); 2]);
    }

    /// **Stopping finishes cleanly**: the run going is given up on, nothing from it is kept, what was
    /// already saved stays saved, and what was in force is put back.
    #[test]
    fn stopping_keeps_what_was_saved_and_puts_the_setup_back() {
        let script = Script::of(vec![run_at(AT_48, 14.0, None), run_at(AT_48, 14.0, None), Run::UntilStopped]);
        let bench = Arc::new(FakeBench::default());
        let (job, calibration) = suite(&script, &bench);
        job.start(&ask(vec![AT_48, AT_96, SetupInForce { rate: 44100, buffer_size: 128 }], 2)).expect("it starts");
        let since = Instant::now();
        while job.state().setups[1].state != "measuring" {
            assert!(since.elapsed() < Duration::from_secs(10), "{:?}", job.state());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(job.state().setups[1].run, Some(1), "measuring 1 of 2");
        job.stop();
        assert_eq!(job.state().state, "stopping");
        let state = finished(&job);
        assert_eq!(state.state, "stopped");
        assert_eq!(state.setups.iter().map(|setup| setup.state).collect::<Vec<_>>(), ["saved", "stopped", "stopped"]);
        assert_eq!(state.saved, 1);
        assert_eq!(bench.did().last().map(String::as_str), Some("put back Some(96000) Some(512)"));
        assert!(!bench.did().iter().any(|did| did == "keep 96 kHz at 512 samples"));
        assert!(!calibration.is_running());
    }

    #[test]
    fn only_one_suite_at_a_time_and_never_beside_a_single_run() {
        let bench = Arc::new(FakeBench::default());
        let (job, _) = suite(&Script::of(vec![Run::UntilStopped]), &bench);
        job.start(&ask(vec![AT_96], 2)).expect("it starts");
        assert_eq!(job.start(&ask(vec![AT_96], 2)).unwrap_err(), "The alignment suite is already running. Wait for it, or stop it first.");
        job.stop();
        finished(&job);

        // A single run going holds the measurement, and the suite says so rather than waiting.
        let (job, calibration) = suite(&Script::of(vec![Run::UntilStopped]), &bench);
        let single = Ask {
            direction: "inputs".into(),
            outputs: vec![Pick::new(0, 0), Pick::new(0, 1)],
            inputs: vec![Pick::new(0, 0), Pick::new(1, 0)],
            witnesses: None,
            clicks: Some(2),
            level_dbfs: None,
            check: false,
        };
        calibration.start(&single).expect("a single run");
        assert!(job.start(&ask(vec![AT_96], 2)).unwrap_err().starts_with("A measurement is already running"));
        assert!(!job.is_running(), "and it is not left looking as if it started");
        calibration.stop();
    }

    #[test]
    fn an_idle_suite_says_nothing_about_a_suite_that_has_not_happened() {
        let (job, _) = suite(&Script::of(Vec::new()), &Arc::new(FakeBench::default()));
        let state = serde_json::to_value(job.state()).unwrap();
        assert_eq!(state, serde_json::json!({ "state": "idle", "setups": [], "saved": 0 }));
    }
}
