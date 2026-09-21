//! Everything a DAW asks for that is not audio: opening the sub-devices, answering for them as one
//! device, and making and disposing of the buffers.
//!
//! None of this runs on a callback thread. The audio path is [`crate::stream`].

use std::sync::Arc;

use gazelle_audio_stream_abi::raw::{selector, CallbacksRaw};
use gazelle_audio_stream_abi::{sample, Entry};

use crate::config::{Config, DeviceConfig};
use crate::plan::{self, Found, Plan};
use crate::status::{Glitches, Reporter};
use crate::stream::Stream;
use crate::sub::{Description, Host, SubDriver};

/// How many sub-devices one aggregate can hold. Each needs its own set of static callbacks,
/// because the interface gives a callback nothing to say which device called it. Raise this and
/// the table in `windows_host` together.
pub const MAX_DEVICES: usize = 8;

/// What the DAW asked for, one entry per buffer: an input or an output, and which channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub is_input: bool,
    pub channel: i32,
}

/// One of the aggregate's own buffers, as a pair of pointers for the two halves.
pub type BufferPair = [*mut std::ffi::c_void; 2];

/// What a channel is called and what it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelInfo {
    pub name: String,
    pub active: bool,
    pub group: i32,
    pub sample_type: i32,
}

/// The driver, outside the audio path.
pub struct Aggregate {
    host: Box<dyn Host>,
    config: Config,
    /// Where the configuration was read from, for anything that has to say so.
    pub config_source: String,
    subs: Vec<Box<dyn SubDriver>>,
    descriptions: Vec<Description>,
    /// The configuration entry each device was found by, kept because the plan is made again at
    /// `createBuffers` from what the drivers report, which carries neither the trims nor the names
    /// the person gave the channels.
    from_file: Vec<DeviceConfig>,
    plan: Option<Plan>,
    stream: Option<Arc<Stream>>,
    /// The channels the DAW asked for, in its own order.
    wanted: Vec<Wanted>,
    rate: f64,
    started: bool,
    /// When the audio started, on the machine's own clock. Kept because how long a session ran is
    /// half of what its line in the event log is for, and nothing else remembers it once the
    /// record has been cleared.
    session_nanos: i64,
    /// What each device had lost when this session started. A DAW that stops and starts again
    /// without letting the buffers go keeps the same rings, so what the session lost is the
    /// difference from here, not the whole of the count.
    session_lost: Vec<Glitches>,
    /// The last refusal, which is what `getErrorMessage` hands back.
    error: String,
    /// A configuration handed in rather than read from a file, which is how a test puts a PC made
    /// of fakes behind the COM object.
    pub pending: Option<(Config, String)>,
    /// Where the driver publishes what it is doing, and keeps what happened. A silent one says
    /// nothing and changes nothing, which is what the driver gets when it has nowhere to report.
    pub reporter: Arc<Reporter>,
    /// The generation of the configuration actually in force.
    generation: u64,
    /// A configuration that arrived while a DAW was streaming, waiting for it to come back
    /// through `createBuffers`.
    queued: Option<(Config, String, u64)>,
}

impl Aggregate {
    /// A driver that has not opened anything yet, and reports nothing.
    pub fn new(host: Box<dyn Host>) -> Aggregate {
        Aggregate::reporting(host, Arc::new(Reporter::silent()))
    }

    /// A driver that publishes what it is doing.
    pub fn reporting(host: Box<dyn Host>, reporter: Arc<Reporter>) -> Aggregate {
        Aggregate {
            host,
            config: Config::default(),
            config_source: String::new(),
            subs: Vec::new(),
            descriptions: Vec::new(),
            from_file: Vec::new(),
            plan: None,
            stream: None,
            wanted: Vec::new(),
            rate: 0.0,
            started: false,
            session_nanos: 0,
            session_lost: Vec::new(),
            error: String::new(),
            pending: None,
            reporter,
            generation: 0,
            queued: None,
        }
    }

    /// Remember a refusal that happened before the aggregate itself was asked anything, so that
    /// `getErrorMessage` still has something to say.
    pub fn refuse(&mut self, message: String) {
        self.error = message;
    }

    pub fn last_error(&self) -> &str {
        &self.error
    }

    /// Every refusal in this driver goes through here, which is why every one of them reaches the
    /// DAW, the shared record and the event log without anything having to remember to say so.
    fn fail<T>(&mut self, message: String) -> Result<T, String> {
        self.error = message.clone();
        let reporter = Arc::clone(&self.reporter);
        reporter.refused(self.generation, &message);
        Err(message)
    }

    pub fn is_initialised(&self) -> bool {
        self.plan.is_some()
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }

    pub fn stream(&self) -> Option<&Arc<Stream>> {
        self.stream.as_ref()
    }

    /// Open every configured device and work out what the aggregate looks like. This is where the
    /// configuration file first turns into a plan.
    pub fn init(&mut self, config: Config, source: String) -> Result<(), String> {
        if self.plan.is_some() {
            return Ok(());
        }
        let generation = self.reporter.generation();
        self.open_everything(config, source, generation)
    }

    /// Whether a configuration could be run at all, decided without opening a thing: it has to
    /// name devices this PC actually has. Everything past that needs a driver to answer, which is
    /// what adopting it finds out.
    pub fn check(&self, config: &Config) -> Result<(), String> {
        let entries = self.host.entries().map_err(|why| format!("the drivers on this PC could not be listed: {why}"))?;
        choose(&entries, config).map(|_| ())
    }

    /// Whether a DAW has the driver open with its buffers made, which is the difference between a
    /// change that can be made quietly and one the host has to be asked about.
    pub fn has_buffers(&self) -> bool {
        self.stream.is_some()
    }

    /// The generation of the configuration in force.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Take this configuration when the DAW next comes back for buffers. Used while a DAW is
    /// streaming, where nothing may be pulled out from under it until it asks.
    pub fn queue(&mut self, config: Config, source: String, generation: u64) {
        self.queued = Some((config, source, generation));
    }

    /// **Take a new configuration now.** Everything open is closed first, because two instances of
    /// one vendor driver in one process is not a thing to try. If the new configuration will not
    /// open, what was there before is opened again, so that a refusal leaves a working driver
    /// rather than none.
    pub fn reconfigure(&mut self, config: Config, source: String, generation: u64) -> Result<(), String> {
        if self.has_buffers() {
            return self.fail("the configuration cannot be changed while a DAW has the buffers".to_string());
        }
        let was = (self.config.clone(), self.config_source.clone(), self.generation);
        let had_plan = self.plan.is_some();
        self.let_everything_go();
        match self.open_everything(config, source, generation) {
            Ok(()) => Ok(()),
            Err(why) => {
                // Put back what was working. It opened once, so it should open again; if it does
                // not, the driver is left saying why at the DAW's next call rather than pretending.
                self.let_everything_go();
                if had_plan {
                    let _ = self.open_everything(was.0, was.1, was.2);
                }
                Err(why)
            }
        }
    }

    /// Let go of every device, leaving a driver that has opened nothing.
    fn let_everything_go(&mut self) {
        self.dispose_buffers();
        self.subs.clear();
        self.descriptions.clear();
        self.from_file.clear();
        self.plan = None;
        self.rate = 0.0;
    }

    /// Open every configured device and work out what the aggregate looks like.
    fn open_everything(&mut self, config: Config, source: String, generation: u64) -> Result<(), String> {
        self.config = config;
        self.config_source = source;
        self.generation = generation;

        let entries = match self.host.entries() {
            Ok(entries) => entries,
            Err(why) => return self.fail(format!("the drivers on this PC could not be listed: {why}")),
        };
        let chosen = match choose(&entries, &self.config) {
            Ok(chosen) => chosen,
            Err(why) => return self.fail(why),
        };
        if chosen.len() > MAX_DEVICES {
            return self.fail(format!("{} devices were configured and this driver holds at most {MAX_DEVICES}", chosen.len()));
        }

        let mut found = Vec::new();
        for (slot, (entry, wanted)) in chosen.iter().enumerate() {
            let name = wanted.name.clone().unwrap_or_else(|| entry.key.clone());
            let mut sub = match self.host.open(entry, slot) {
                Ok(sub) => sub,
                Err(why) => return self.fail(format!("{name} could not be opened: {why}")),
            };
            if let Err(why) = sub.init() {
                return self.fail(format!("{name} refused to start up: {why}"));
            }
            let description = match sub.describe() {
                Ok(description) => description,
                Err(why) => return self.fail(format!("{name} could not be read: {why}")),
            };
            found.push(Found {
                name,
                description: description.clone(),
                wanted_inputs: wanted.inputs.clone(),
                wanted_outputs: wanted.outputs.clone(),
                input_trim: wanted.input_trim.unwrap_or(0),
                output_trim: wanted.output_trim.unwrap_or(0),
                input_labels: wanted.input_names.clone(),
                output_labels: wanted.output_names.clone(),
            });
            self.from_file.push(wanted.clone());
            self.descriptions.push(description);
            self.subs.push(sub);
        }

        // A rate in the file is applied once, here, so that every device is already together
        // before the DAW asks anything.
        if let Some(hz) = self.config.rate {
            if let Err(why) = self.move_every_device_to(hz) {
                return self.fail(why);
            }
            for device in &mut found {
                device.description.rate = hz;
            }
        }
        self.rate = found.first().map(|d| d.description.rate).unwrap_or(0.0);

        match plan::plan(&found, &self.config, None) {
            Ok(plan) => {
                let reporter = Arc::clone(&self.reporter);
                reporter.plan_in_force(&plan, self.rate, &self.config_source, self.generation);
                self.plan = Some(plan);
                Ok(())
            }
            Err(why) => self.fail(why),
        }
    }

    /// How many channels the aggregate has.
    pub fn channels(&self) -> (i32, i32) {
        match &self.plan {
            Some(plan) => (plan.inputs.len() as i32, plan.outputs.len() as i32),
            None => (0, 0),
        }
    }

    /// One channel, named so that a person can tell the devices apart.
    pub fn channel_info(&self, is_input: bool, channel: i32) -> Option<ChannelInfo> {
        let plan = self.plan.as_ref()?;
        let names = if is_input { &plan.input_names } else { &plan.output_names };
        let name = names.get(usize::try_from(channel).ok()?)?.clone();
        let active = self.wanted.iter().any(|w| w.is_input == is_input && w.channel == channel);
        Some(ChannelInfo { name, active, group: 0, sample_type: sample::INT32_LSB })
    }

    /// The buffer sizes every device can take, and the one to prefer.
    pub fn buffer_sizes(&self) -> Option<(i32, i32, i32, i32)> {
        let plan = self.plan.as_ref()?;
        Some((plan.min, plan.max, plan.preferred, plan.granularity))
    }

    /// One figure each way, covering the whole aggregate.
    pub fn latencies(&self) -> Option<(i32, i32)> {
        let plan = self.plan.as_ref()?;
        Some((plan.input_latency, plan.output_latency))
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Whether every device will run at this rate. One that will not is the whole answer.
    pub fn can_rate(&mut self, hz: f64) -> bool {
        if !(hz.is_finite() && hz > 0.0) {
            return false;
        }
        self.subs.iter_mut().all(|sub| sub.can_rate(hz))
    }

    /// Move every device, or none of them.
    pub fn set_rate(&mut self, hz: f64) -> Result<(), String> {
        if self.started {
            return self.fail("the sample rate cannot be changed while the driver is running".to_string());
        }
        if (hz - self.rate).abs() < f64::EPSILON {
            return Ok(());
        }
        match self.move_every_device_to(hz) {
            Ok(()) => {
                self.rate = hz;
                Ok(())
            }
            Err(why) => self.fail(why),
        }
    }

    /// Every device to one rate. If one refuses, the ones already moved are put back, because half
    /// an aggregate at the wrong rate is worse than none.
    fn move_every_device_to(&mut self, hz: f64) -> Result<(), String> {
        if !(hz.is_finite() && hz > 0.0) {
            return Err(format!("{hz} is not a sample rate"));
        }
        let names: Vec<String> = self.plan_names();
        for (index, sub) in self.subs.iter_mut().enumerate() {
            if !sub.can_rate(hz) {
                let name = names.get(index).cloned().unwrap_or_else(|| format!("device {index}"));
                return Err(format!("{name} will not run at {hz} Hz, so neither will the aggregate"));
            }
        }
        let was = self.rate;
        for index in 0..self.subs.len() {
            if let Err(why) = self.subs[index].set_rate(hz) {
                let name = names.get(index).cloned().unwrap_or_else(|| format!("device {index}"));
                // Put the ones that did move back, because half an aggregate at the wrong rate is
                // worse than none of it.
                if was > 0.0 {
                    for earlier in self.subs.iter_mut().take(index) {
                        let _ = earlier.set_rate(was);
                    }
                }
                return Err(format!("{name} refused {hz} Hz: {why}"));
            }
        }
        Ok(())
    }

    fn plan_names(&self) -> Vec<String> {
        match &self.plan {
            Some(plan) => plan.devices.iter().map(|d| d.name.clone()).collect(),
            None => self.descriptions.iter().map(|d| d.name.clone()).collect(),
        }
    }

    /// Whether every device takes `outputReady`. If one does not, the aggregate does not offer it.
    pub fn output_ready(&self) -> bool {
        !self.descriptions.is_empty() && self.descriptions.iter().all(|d| d.output_ready)
    }

    /// Make every device's buffers and the aggregate's own, and hand back the pointer pairs for
    /// what the DAW asked for, in the order it asked.
    pub fn create_buffers(
        &mut self,
        wanted: &[Wanted],
        block: i32,
        host: CallbacksRaw,
    ) -> Result<Vec<BufferPair>, String> {
        if self.stream.is_some() {
            return self.fail("the buffers are already made".to_string());
        }
        // A configuration that arrived while the last session was running is taken up here, which
        // is the moment the DAW has let go of everything and is asking for it again. A refusal has
        // already been written down by `reconfigure`, and what was in force is still in force.
        if let Some((config, source, generation)) = self.queued.take() {
            let _ = self.reconfigure(config, source, generation);
        }
        let Some(plan) = self.plan.clone() else {
            return self.fail("the driver was asked for buffers before it was opened".to_string());
        };
        if block < plan.min || block > plan.max {
            return self.fail(format!(
                "{block} samples is outside what these devices will take, which is {} to {}",
                plan.min, plan.max
            ));
        }

        // Which aggregate channel each request is, refused clearly rather than silently dropped.
        let mut in_map = Vec::new();
        let mut out_map = Vec::new();
        let mut order: Vec<(bool, usize)> = Vec::new();
        for want in wanted {
            let list = if want.is_input { &plan.inputs } else { &plan.outputs };
            let Ok(index) = usize::try_from(want.channel) else {
                return self.fail(format!("channel {} is not a channel", want.channel));
            };
            let Some(&reference) = list.get(index) else {
                let what = if want.is_input { "input" } else { "output" };
                return self.fail(format!("there is no {what} channel {index}: this aggregate has {}", list.len()));
            };
            if want.is_input {
                order.push((true, in_map.len()));
                in_map.push(reference);
            } else {
                order.push((false, out_map.len()));
                out_map.push(reference);
            }
        }

        // Re-plan at the block the DAW actually asked for: the latencies and the padding depend
        // on it.
        let found = self.found_again();
        let plan = match plan::plan(&found, &self.config, Some(block)) {
            Ok(plan) => plan,
            Err(why) => return self.fail(why),
        };

        let mut buffers = Vec::new();
        for (index, device) in plan.devices.iter().enumerate() {
            match self.subs[index].create_buffers(&device.inputs, &device.outputs, block) {
                Ok(made) => buffers.push(made),
                Err(why) => {
                    // Undo what was made, newest first, before saying no.
                    for earlier in self.subs.iter_mut().take(index) {
                        earlier.dispose_buffers();
                    }
                    return self.fail(format!("{} would not take {block} samples a buffer: {why}", device.name));
                }
            }
        }

        let time_info = asks_for_time_info(&host);
        let stream = Arc::new(Stream::new(
            &plan,
            &self.config,
            self.rate,
            buffers,
            in_map,
            out_map,
            host,
            time_info,
            Arc::clone(&self.reporter),
        ));
        for (index, sub) in self.subs.iter_mut().enumerate() {
            sub.attach(Arc::clone(&stream), index);
        }

        let pairs = order
            .iter()
            .map(|&(is_input, index)| {
                if is_input {
                    [stream.input_pointer(index, 0), stream.input_pointer(index, 1)]
                } else {
                    [stream.output_pointer(index, 0), stream.output_pointer(index, 1)]
                }
            })
            .collect();

        self.wanted = wanted.to_vec();
        let reporter = Arc::clone(&self.reporter);
        reporter.plan_in_force(&plan, self.rate, &self.config_source, self.generation);
        reporter.buffers(true, stream.daw_inputs(), stream.daw_outputs());
        self.plan = Some(plan);
        self.stream = Some(stream);
        Ok(pairs)
    }

    /// The devices as [`plan::plan`] wants them, from what was read at `init`.
    fn found_again(&self) -> Vec<Found> {
        let names = self.plan_names();
        self.descriptions
            .iter()
            .enumerate()
            .map(|(index, description)| {
                let device = self.plan.as_ref().and_then(|plan| plan.devices.get(index));
                let wanted = self.from_file.get(index);
                Found {
                    name: names.get(index).cloned().unwrap_or_else(|| description.name.clone()),
                    description: description.clone(),
                    wanted_inputs: device.map(|d| d.inputs.clone()),
                    wanted_outputs: device.map(|d| d.outputs.clone()),
                    input_trim: wanted.and_then(|w| w.input_trim).unwrap_or(0),
                    output_trim: wanted.and_then(|w| w.output_trim).unwrap_or(0),
                    input_labels: wanted.map(|w| w.input_names.clone()).unwrap_or_default(),
                    output_labels: wanted.map(|w| w.output_names.clone()).unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Start every device. The ones that follow go first, so that the one driving the callback is
    /// the last thing to begin and never calls the DAW before the others can answer it.
    pub fn start(&mut self) -> Result<(), String> {
        let Some(stream) = self.stream.clone() else {
            return self.fail("the driver was started before its buffers were made".to_string());
        };
        if self.started {
            return Ok(());
        }
        stream.run();
        let master = stream.master;
        let names = self.plan_names();
        let order: Vec<usize> = (0..self.subs.len()).filter(|&i| i != master).chain(std::iter::once(master)).collect();
        let mut started: Vec<usize> = Vec::new();
        for index in order {
            if let Err(why) = self.subs[index].start() {
                for &earlier in started.iter().rev() {
                    self.subs[earlier].stop();
                }
                stream.halt();
                let name = names.get(index).cloned().unwrap_or_else(|| format!("device {index}"));
                return self.fail(format!("{name} would not start: {why}"));
            }
            started.push(index);
        }
        self.started = true;
        self.session_nanos = crate::now_nanos();
        self.session_lost = stream.glitches();
        let reporter = Arc::clone(&self.reporter);
        let (inputs, outputs) = (stream.daw_inputs(), stream.daw_outputs());
        reporter.session(true, self.session_nanos, &format!("{inputs} in, {outputs} out at {} Hz", self.rate));
        Ok(())
    }

    /// Stop every device, the one driving the callback first.
    pub fn stop(&mut self) {
        let was = self.started;
        // What the session lost, read before anything is let go of, because the rings that hold
        // the counts go with the buffers. Off the audio path: every device is about to be stopped.
        let glitches = self
            .stream
            .as_ref()
            .map(|stream| crate::status::lost_since(&self.session_lost, &stream.glitches()))
            .unwrap_or_default();
        if let Some(stream) = &self.stream {
            stream.halt();
            let master = stream.master;
            let order: Vec<usize> = std::iter::once(master).chain((0..self.subs.len()).filter(|&i| i != master)).collect();
            for index in order {
                self.subs[index].stop();
            }
        }
        self.started = false;
        if was {
            // How long it ran and what it lost, in one line, because the counters themselves are
            // gone the moment the DAW lets the driver go.
            let seconds = (crate::now_nanos() - self.session_nanos).max(0) as f64 / 1_000_000_000.0;
            let reporter = Arc::clone(&self.reporter);
            reporter.session_ended(seconds, &glitches);
        }
        self.session_nanos = 0;
        self.session_lost.clear();
    }

    /// Let go of every buffer. Nothing can be calling back by now: every device was stopped first.
    pub fn dispose_buffers(&mut self) {
        self.stop();
        for sub in self.subs.iter_mut() {
            sub.dispose_buffers();
            sub.detach();
        }
        let had = self.stream.take().is_some();
        self.wanted.clear();
        if had {
            let reporter = Arc::clone(&self.reporter);
            reporter.buffers(false, 0, 0);
        }
    }

    /// Where the aggregate is, in samples, and when that was read, in nanoseconds.
    pub fn position(&self) -> (i64, i64) {
        match &self.stream {
            Some(stream) => stream.position(),
            None => (0, 0),
        }
    }
}

impl Drop for Aggregate {
    fn drop(&mut self) {
        self.dispose_buffers();
    }
}

/// Whether the DAW wants the buffer callback that carries the time with it.
fn asks_for_time_info(host: &CallbacksRaw) -> bool {
    let ask = |what: i32, value: i32| unsafe { (host.message)(what, value, std::ptr::null_mut(), std::ptr::null_mut()) };
    ask(selector::SUPPORTED, selector::SUPPORTS_TIME_INFO) == 1 && ask(selector::SUPPORTS_TIME_INFO, 0) == 1
}

/// Which of the PC's drivers this configuration is asking for, in the configuration's own order.
pub fn choose(entries: &[Entry], config: &Config) -> Result<Vec<(Entry, DeviceConfig)>, String> {
    if config.devices.is_empty() {
        // No file, or a file that names no devices: everything this maker registered, in the order
        // the registry has it.
        let ours: Vec<(Entry, DeviceConfig)> = entries
            .iter()
            .filter(|entry| plan::looks_like_ours(&entry.key, entry.description.as_deref(), entry.dll.as_deref().ok()))
            .map(|entry| (entry.clone(), DeviceConfig { name: Some(entry.key.clone()), ..DeviceConfig::default() }))
            .collect();
        if ours.is_empty() {
            return Err(
                "no Antelope drivers are registered on this PC, and the configuration file names none either".to_string()
            );
        }
        return Ok(ours);
    }

    let mut chosen = Vec::new();
    for wanted in &config.devices {
        let found = entries
            .iter()
            .find(|entry| plan::matches(&entry.key, &entry.clsid, entry.description.as_deref(), wanted));
        match found {
            Some(entry) => chosen.push((entry.clone(), wanted.clone())),
            None => {
                return Err(format!(
                    "{} is in the configuration but no driver on this PC matches it",
                    wanted.described()
                ))
            }
        }
    }
    Ok(chosen)
}
