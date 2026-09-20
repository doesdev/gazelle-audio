//! The probe's logic: which drivers to open, in what order, and the arithmetic of the report.
//!
//! Everything a PC does (the registry, COM, the vendor driver objects, the clock) sits behind
//! [`Host`] and [`SubDriver`], the way `driver/mod.rs` in the server puts the vendor API DLL
//! behind `DriverApi` and the PC behind `DriverHost`. [`run`] therefore runs whole against fakes,
//! and no test opens a driver.

#[cfg(test)]
pub mod fake;
#[cfg(windows)]
pub mod windows;

#[cfg(test)]
mod tests;

use std::time::Duration;

/// The environment variable every test and script sets. When it is `1` the probe refuses: it
/// opens real drivers and drives real converters, and nothing that sets this wants that.
pub const NO_HARDWARE: &str = "GAZELLE_NO_HARDWARE";

/// Zen Quadro Synergy Core, as its ASIO entry registers it on this PC.
pub const QUADRO_CLSID: &str = "{12217625-CB57-11EE-908D-7085C2FB2DD5}";
/// Zen Studio+ (the "ZenStudioTB ASIO Driver" entry).
pub const STUDIO_CLSID: &str = "{AE4A4452-A316-11E5-A113-080027F6C1F4}";

/// The drivers this probe will open, and the short name it calls each by. Everything else found
/// in the registry is listed and left alone.
pub const TARGETS: &[(&str, &str)] = &[("quadro", QUADRO_CLSID), ("studio", STUDIO_CLSID)];

/// How many input and output channels are opened per driver. Two of each is enough to carry a
/// callback and small enough to be obviously harmless.
pub const CHANNELS: i32 = 2;

/// A callback difference this small over a whole run is the skew between two `start` calls a few
/// milliseconds apart, not drift. Anything larger is the two devices running off each other.
pub const SKEW_ALLOWANCE: i64 = 1;

/// Whether two registry CLSID strings name the same class, whatever their case or braces.
pub fn same_clsid(a: &str, b: &str) -> bool {
    fn bare(s: &str) -> String {
        s.trim().trim_start_matches('{').trim_end_matches('}').to_ascii_lowercase()
    }
    bare(a) == bare(b)
}

/// One entry under `HKLM\SOFTWARE\ASIO`, with the DLL its class id points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsioEntry {
    /// The registry key's own name, which is what a DAW shows in its driver list.
    pub key: String,
    /// The entry's `Description` value, when it has one.
    pub description: Option<String>,
    pub clsid: String,
    /// `InprocServer32`'s default value, or why it could not be read.
    pub dll: Result<String, String>,
}

impl AsioEntry {
    /// The short name in [`TARGETS`], when this is one of the drivers the probe opens.
    pub fn target(&self) -> Option<&'static str> {
        TARGETS.iter().find(|(_, clsid)| same_clsid(clsid, &self.clsid)).map(|(name, _)| *name)
    }
}

/// The buffer sizes a driver offers, in samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferSizes {
    pub min: i32,
    pub max: i32,
    pub preferred: i32,
    pub granularity: i32,
}

/// One entry from `getClockSources`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockSource {
    pub index: i32,
    pub name: String,
    pub current: bool,
}

/// Everything the probe reads out of a driver after `init`. Read only: nothing here is set back.
#[derive(Clone, Debug, PartialEq)]
pub struct Description {
    pub name: String,
    pub version: i32,
    pub inputs: i32,
    pub outputs: i32,
    pub buffers: BufferSizes,
    pub rate: f64,
    pub latency_in: i32,
    pub latency_out: i32,
    /// The sample type of input channel 0 and output channel 0, named.
    pub input_format: String,
    pub output_format: String,
    /// Bytes per sample on the outputs, which is how much of each output buffer is zeroed.
    pub output_bytes: usize,
    pub clocks: Vec<ClockSource>,
}

/// One vendor driver, opened. Dropping it releases the COM object.
pub trait SubDriver {
    fn init(&mut self) -> Result<(), String>;
    fn describe(&mut self) -> Result<Description, String>;
    /// `canSampleRate`, which asks and changes nothing.
    fn can_rate(&mut self, hz: f64) -> bool;
    /// Open `inputs` inputs and `outputs` outputs at `size` samples, with the silent callback.
    fn create_buffers(&mut self, inputs: i32, outputs: i32, size: i32) -> Result<(), String>;
    fn start(&mut self) -> Result<(), String>;
    fn stop(&mut self);
    fn dispose_buffers(&mut self);
    /// How many times the driver has called back since `create_buffers`.
    fn callbacks(&self) -> u64;
}

/// The PC: the registry, COM, and the wait while the drivers run.
pub trait Host {
    /// Every entry under `HKLM\SOFTWARE\ASIO`. An empty list is an ordinary answer.
    fn entries(&self) -> Result<Vec<AsioEntry>, String>;
    /// Create the driver object. The caller keeps every driver it opened alive until the end.
    fn open(&self, entry: &AsioEntry) -> Result<Box<dyn SubDriver>, String>;
    /// Let the drivers run, and say how long that actually was.
    fn wait(&self, seconds: u64) -> Duration;
}

/// What the probe was asked to do.
#[derive(Clone, Debug)]
pub struct Options {
    /// Without this only the listing is printed, and no driver is opened.
    pub proceed: bool,
    pub seconds: u64,
    /// Run one driver alone: a short name from [`TARGETS`], or any part of a registry key name.
    pub only: Option<String>,
    /// A rate to ask each driver about. Asked, never set.
    pub rate: Option<f64>,
}

impl Default for Options {
    fn default() -> Self {
        Options { proceed: false, seconds: 20, only: None, rate: None }
    }
}

/// Which call did not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Create,
    Init,
    Read,
    CreateBuffers,
    Start,
}

impl Step {
    pub fn call(self) -> &'static str {
        match self {
            Step::Create => "CoCreateInstance",
            Step::Init => "init",
            Step::Read => "reading the driver",
            Step::CreateBuffers => "createBuffers",
            Step::Start => "start",
        }
    }
}

/// A step that failed, and how far the probe had got. `already_open` is how many other drivers
/// were open at the time, which is the whole question this probe exists to answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub driver: String,
    pub step: Step,
    pub message: String,
    pub already_open: usize,
}

/// One driver that was opened and read.
#[derive(Clone, Debug, PartialEq)]
pub struct Opened {
    pub name: String,
    pub description: Description,
    /// `Some(can)` when a rate was asked about.
    pub can_rate: Option<bool>,
}

/// One driver that ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub name: String,
    pub callbacks: u64,
    pub buffer_size: i32,
}

impl Run {
    pub fn samples(&self) -> i64 {
        self.callbacks as i64 * self.buffer_size as i64
    }
}

/// Whether the two drivers stayed together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The counts match, allowing for the gap between the two `start` calls.
    InStep,
    /// The counts moved apart: the two devices are running off different clocks.
    Drifting,
}

/// The difference between two runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Drift {
    pub a: String,
    pub b: String,
    /// `a` minus `b`, in callbacks.
    pub callbacks: i64,
    pub callbacks_per_second: f64,
    /// `a` minus `b`, in samples.
    pub samples: i64,
    pub samples_per_second: f64,
    pub verdict: Verdict,
}

/// The difference between the first two runs, if there are two.
pub fn drift(runs: &[Run], elapsed: Duration) -> Option<Drift> {
    let (a, b) = (runs.first()?, runs.get(1)?);
    let secs = elapsed.as_secs_f64();
    let callbacks = a.callbacks as i64 - b.callbacks as i64;
    let samples = a.samples() - b.samples();
    let per = |n: i64| if secs > 0.0 { n as f64 / secs } else { 0.0 };
    Some(Drift {
        a: a.name.clone(),
        b: b.name.clone(),
        callbacks,
        callbacks_per_second: per(callbacks),
        samples,
        samples_per_second: per(samples),
        verdict: if callbacks.abs() <= SKEW_ALLOWANCE { Verdict::InStep } else { Verdict::Drifting },
    })
}

/// The preferred buffer sizes, when the drivers opened do not agree on one. The aggregate driver
/// planned later needs them matched, so this is worth saying out loud.
pub fn buffer_mismatch(opened: &[Opened]) -> Option<Vec<(String, i32)>> {
    let sizes: Vec<(String, i32)> = opened.iter().map(|o| (o.name.clone(), o.description.buffers.preferred)).collect();
    let first = sizes.first()?.1;
    if sizes.iter().all(|(_, size)| *size == first) {
        None
    } else {
        Some(sizes)
    }
}

/// Callbacks per second, for the report.
pub fn rate_of(run: &Run, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs > 0.0 {
        run.callbacks as f64 / secs
    } else {
        0.0
    }
}

/// Everything the probe found, whether or not it got all the way through.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// Every ASIO entry on the PC, in registry order.
    pub entries: Vec<AsioEntry>,
    /// The entries the probe would open.
    pub targets: Vec<AsioEntry>,
    /// True when the probe stopped after the listing, which is what a run without `--yes` does.
    pub listed_only: bool,
    pub opened: Vec<Opened>,
    pub runs: Vec<Run>,
    pub elapsed: Duration,
    pub failure: Option<Failure>,
}

impl Outcome {
    /// The headline: every target was created, initialised and started at the same time.
    pub fn all_started(&self) -> bool {
        !self.listed_only && self.failure.is_none() && self.runs.len() == self.targets.len() && !self.runs.is_empty()
    }
}

/// Which listed entries the options ask for, in [`TARGETS`] order.
pub fn chosen(entries: &[AsioEntry], only: Option<&str>) -> Vec<AsioEntry> {
    let mut chosen = Vec::new();
    for (name, clsid) in TARGETS {
        let Some(entry) = entries.iter().find(|e| same_clsid(&e.clsid, clsid)) else { continue };
        if let Some(only) = only {
            let only = only.to_ascii_lowercase();
            let matches = name.contains(&only)
                || entry.key.to_ascii_lowercase().contains(&only)
                || entry.description.as_deref().unwrap_or("").to_ascii_lowercase().contains(&only);
            if !matches {
                continue;
            }
        }
        chosen.push(entry.clone());
    }
    chosen
}

/// Why the probe will not run, if it will not.
pub fn refusal(no_hardware: Option<&str>) -> Option<String> {
    match no_hardware {
        Some("1") => Some(format!(
            "{NO_HARDWARE}=1 is set. This probe opens the real audio drivers and starts the real \
             converters, so it will not run here. Unset it and run the probe by hand, with your \
             monitors down."
        )),
        _ => None,
    }
}

/// The whole probe, against whatever PC it is given.
pub fn run(host: &dyn Host, options: &Options) -> Result<Outcome, String> {
    let entries = host.entries()?;
    let targets = chosen(&entries, options.only.as_deref());
    let mut outcome = Outcome {
        entries,
        targets: targets.clone(),
        listed_only: !options.proceed,
        opened: Vec::new(),
        runs: Vec::new(),
        elapsed: Duration::ZERO,
        failure: None,
    };
    if !options.proceed || targets.is_empty() {
        return Ok(outcome);
    }

    // Held in one list so that a driver opened earlier stays open while the next one is tried:
    // that is the coexistence question, and dropping one early would answer it wrongly.
    let mut open: Vec<(String, Box<dyn SubDriver>)> = Vec::new();
    let mut failed = None;

    for entry in &targets {
        let name = entry.target().unwrap_or(entry.key.as_str()).to_string();
        let already_open = open.len();
        let mut sub = match host.open(entry) {
            Ok(sub) => sub,
            Err(message) => {
                failed = Some(Failure { driver: name, step: Step::Create, message, already_open });
                break;
            }
        };
        if let Err(message) = sub.init() {
            failed = Some(Failure { driver: name.clone(), step: Step::Init, message, already_open });
            // Keep it in the list so it is released with the rest, in order.
            open.push((name, sub));
            break;
        }
        let description = match sub.describe() {
            Ok(description) => description,
            Err(message) => {
                failed = Some(Failure { driver: name.clone(), step: Step::Read, message, already_open });
                open.push((name, sub));
                break;
            }
        };
        let can_rate = options.rate.map(|hz| sub.can_rate(hz));
        outcome.opened.push(Opened { name: name.clone(), description, can_rate });
        open.push((name, sub));
    }

    if failed.is_none() {
        let count = open.len();
        for (name, sub) in open.iter_mut() {
            let description = &outcome.opened.iter().find(|o| &o.name == name).expect("opened was recorded").description;
            let inputs = CHANNELS.min(description.inputs);
            let outputs = CHANNELS.min(description.outputs);
            if let Err(message) = sub.create_buffers(inputs, outputs, description.buffers.preferred) {
                let already_open = count.saturating_sub(1);
                failed = Some(Failure { driver: name.clone(), step: Step::CreateBuffers, message, already_open });
                break;
            }
        }
    }

    let mut started = 0usize;
    if failed.is_none() {
        for (name, sub) in open.iter_mut() {
            if let Err(message) = sub.start() {
                failed = Some(Failure { driver: name.clone(), step: Step::Start, message, already_open: started });
                break;
            }
            started += 1;
        }
    }

    if failed.is_none() {
        outcome.elapsed = host.wait(options.seconds);
        outcome.runs = open
            .iter()
            .map(|(name, sub)| Run {
                name: name.clone(),
                callbacks: sub.callbacks(),
                buffer_size: outcome
                    .opened
                    .iter()
                    .find(|o| &o.name == name)
                    .map(|o| o.description.buffers.preferred)
                    .unwrap_or_default(),
            })
            .collect();
    }

    // Whatever happened, everything that was started is stopped and every buffer set disposed,
    // newest first, before anything is released.
    for (_, sub) in open.iter_mut().rev() {
        sub.stop();
        sub.dispose_buffers();
    }
    drop(open);

    outcome.failure = failed;
    Ok(outcome)
}
