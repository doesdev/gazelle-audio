//! Everything a DAW asks for that is not audio: opening the sub-devices, answering for them as one
//! device, and making and disposing of the buffers.
//!
//! None of this runs on a callback thread. The audio path is [`crate::stream`].

use std::sync::Arc;

use gazelle_audio_stream_abi::raw::{selector, CallbacksRaw};
use gazelle_audio_stream_abi::{sample, Entry};

use crate::config::{Config, DeviceConfig};
use crate::plan::{self, Found, Plan};
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
    /// Each device's trims from the file, kept because the plan is made again at `createBuffers`
    /// from what the drivers report, which does not carry them.
    trims: Vec<(i32, i32)>,
    plan: Option<Plan>,
    stream: Option<Arc<Stream>>,
    /// The channels the DAW asked for, in its own order.
    wanted: Vec<Wanted>,
    rate: f64,
    started: bool,
    /// The last refusal, which is what `getErrorMessage` hands back.
    error: String,
    /// A configuration handed in rather than read from a file, which is how a test puts a PC made
    /// of fakes behind the COM object.
    pub pending: Option<(Config, String)>,
}

impl Aggregate {
    /// A driver that has not opened anything yet.
    pub fn new(host: Box<dyn Host>) -> Aggregate {
        Aggregate {
            host,
            config: Config::default(),
            config_source: String::new(),
            subs: Vec::new(),
            descriptions: Vec::new(),
            trims: Vec::new(),
            plan: None,
            stream: None,
            wanted: Vec::new(),
            rate: 0.0,
            started: false,
            error: String::new(),
            pending: None,
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

    fn fail<T>(&mut self, message: String) -> Result<T, String> {
        self.error = message.clone();
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

    /// Open every configured device and work out what the aggregate looks like. Reads the
    /// configuration file, which is the one and only moment any file is read.
    pub fn init(&mut self, config: Config, source: String) -> Result<(), String> {
        if self.plan.is_some() {
            return Ok(());
        }
        self.config = config;
        self.config_source = source;

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
            });
            self.trims.push((wanted.input_trim.unwrap_or(0), wanted.output_trim.unwrap_or(0)));
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
        let stream = Arc::new(Stream::new(&plan, &self.config, self.rate, buffers, in_map, out_map, host, time_info));
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
                Found {
                    name: names.get(index).cloned().unwrap_or_else(|| description.name.clone()),
                    description: description.clone(),
                    wanted_inputs: device.map(|d| d.inputs.clone()),
                    wanted_outputs: device.map(|d| d.outputs.clone()),
                    input_trim: self.trims.get(index).map_or(0, |t| t.0),
                    output_trim: self.trims.get(index).map_or(0, |t| t.1),
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
        Ok(())
    }

    /// Stop every device, the one driving the callback first.
    pub fn stop(&mut self) {
        if let Some(stream) = &self.stream {
            stream.halt();
            let master = stream.master;
            let order: Vec<usize> = std::iter::once(master).chain((0..self.subs.len()).filter(|&i| i != master)).collect();
            for index in order {
                self.subs[index].stop();
            }
        }
        self.started = false;
    }

    /// Let go of every buffer. Nothing can be calling back by now: every device was stopped first.
    pub fn dispose_buffers(&mut self) {
        self.stop();
        for sub in self.subs.iter_mut() {
            sub.dispose_buffers();
            sub.detach();
        }
        self.stream = None;
        self.wanted.clear();
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
