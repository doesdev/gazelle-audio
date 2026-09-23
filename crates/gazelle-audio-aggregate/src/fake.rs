//! A PC and a set of devices made of data, so that every rule in this crate is tested without a
//! driver, a device or a converter anywhere near it.
//!
//! The fakes own real memory and hand out real pointers to it, and the test fires their callbacks
//! itself. The audio path therefore runs exactly the code it will run at the hardware: the same
//! conversion, the same rings, the same delays, the same stall handling. What the fakes stand in
//! for is the vendor driver's own behaviour, which a test can then make do things no real device
//! would do on purpose: arrive late, arrive twice, stop dead, or refuse a rate.
//!
//! Or hold on to one. [`Takes`] is each way a driver can say yes to a rate and not move, so the
//! aggregate's answer to each is tested here rather than discovered at somebody's interface.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gazelle_audio_stream_abi::{sample, Entry};

use crate::stream::Stream;
use crate::sub::{Description, DeviceBuffers, Host, Requests, SubDriver};
use gazelle_audio_stream_abi::raw::selector;

/// A step a fake device can refuse at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Init,
    Describe,
    SetRate,
    CreateBuffers,
    Start,
}

/// When a fake device's driver actually takes a rate it answered yes to.
///
/// A driver that remembers its rate puts the device back to it whenever it is opened, so one that
/// has not taken the rate it was asked for moves the interface, and everything clocked from it, the
/// moment a DAW opens it. Each of these is a driver the aggregate has to see through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Takes {
    /// As soon as it is asked, which is what the Antelope drivers do.
    #[default]
    AtOnce,
    /// It says yes and keeps the old rate until it is closed and opened again, when it opens at the
    /// rate it was last asked for.
    WhenReopened,
    /// It says yes and keeps the old rate until its buffers are made.
    WithBuffers,
    /// It says yes and reports the new rate, but asks to be reset as soon as it has anyone to ask,
    /// and runs at the new rate only once it has been opened again.
    AfterReset,
    /// It says yes and never moves.
    Never,
}

/// What a fake device's driver keeps between one opening and the next, which is what makes a
/// stubborn driver stubborn.
#[derive(Debug, Default)]
struct Held {
    /// A rate it said yes to and opens at next time.
    remembered: Option<f64>,
    /// A rate it said yes to and takes when its buffers are made.
    pending: Option<f64>,
    /// It owes the host a reset request, which it sends once it has somewhere to send it.
    owes_reset: bool,
    /// What the device is really running at, where that is not what the driver reports.
    running: Option<f64>,
    /// How many times it has been opened.
    opens: u32,
    /// What it asked for with nobody to hand it to.
    requests: Requests,
}

/// What a fake device answers.
#[derive(Clone, Debug)]
pub struct Spec {
    pub driver_name: String,
    pub inputs: i32,
    pub outputs: i32,
    pub min: i32,
    pub max: i32,
    pub preferred: i32,
    pub granularity: i32,
    pub rate: f64,
    pub rates: Vec<f64>,
    pub latency_in: i32,
    pub latency_out: i32,
    pub input_type: i32,
    pub output_type: i32,
    pub output_ready: bool,
    pub fails_at: Option<(Step, String)>,
    /// When it takes a rate it is asked for.
    pub takes: Takes,
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            driver_name: "A Fake Interface".to_string(),
            inputs: 4,
            outputs: 4,
            min: 32,
            max: 2048,
            preferred: 512,
            granularity: -1,
            rate: 96_000.0,
            rates: vec![44_100.0, 48_000.0, 88_200.0, 96_000.0],
            latency_in: 600,
            latency_out: 700,
            input_type: sample::INT32_LSB,
            output_type: sample::INT32_LSB,
            output_ready: true,
            fails_at: None,
            takes: Takes::AtOnce,
        }
    }
}

impl Spec {
    pub fn named(name: &str) -> Spec {
        Spec { driver_name: name.to_string(), ..Spec::default() }
    }

    pub fn with_channels(mut self, inputs: i32, outputs: i32) -> Spec {
        self.inputs = inputs;
        self.outputs = outputs;
        self
    }

    pub fn with_latency(mut self, input: i32, output: i32) -> Spec {
        self.latency_in = input;
        self.latency_out = output;
        self
    }

    pub fn failing(mut self, step: Step, why: &str) -> Spec {
        self.fails_at = Some((step, why.to_string()));
        self
    }

    pub fn taking(mut self, takes: Takes) -> Spec {
        self.takes = takes;
        self
    }

    pub fn of_type(mut self, input: i32, output: i32) -> Spec {
        self.input_type = input;
        self.output_type = output;
        self
    }
}

/// Everything every device was asked to do, in one list, so that a test can see the order calls
/// were made in across devices as well as within one.
pub type Journal = Arc<Mutex<Vec<String>>>;

/// What a fake device is doing now. Shared with the test, which drives its callbacks.
pub struct FakeDevice {
    pub spec: Mutex<Spec>,
    inner: Mutex<Inner>,
    /// Callbacks this device has been made to fire.
    pub fired: AtomicU64,
    /// What this device is called on the PC, for the journal.
    key: String,
    held: Mutex<Held>,
    journal: Journal,
}

#[derive(Default)]
struct Inner {
    /// One box per channel, two halves one after the other.
    inputs: Vec<Box<[u8]>>,
    outputs: Vec<Box<[u8]>>,
    block: usize,
    input_width: usize,
    output_width: usize,
    created: bool,
    started: bool,
    disposed: bool,
    stream: Option<Arc<Stream>>,
    index: usize,
    calls: Vec<String>,
}

impl FakeDevice {
    pub fn new(key: &str, spec: Spec, journal: Journal) -> Arc<FakeDevice> {
        Arc::new(FakeDevice {
            spec: Mutex::new(spec),
            inner: Mutex::new(Inner::default()),
            fired: AtomicU64::new(0),
            key: key.to_string(),
            held: Mutex::new(Held::default()),
            journal,
        })
    }

    fn note(&self, what: &str) {
        self.inner.lock().expect("no test panics while holding this").calls.push(what.to_string());
        self.journal.lock().expect("not poisoned").push(format!("{}: {what}", self.key));
    }

    /// Everything that was asked of this device, in order.
    pub fn calls(&self) -> Vec<String> {
        self.inner.lock().expect("not poisoned").calls.clone()
    }

    /// How many times its driver has been opened.
    pub fn opens(&self) -> u32 {
        self.held.lock().expect("not poisoned").opens
    }

    /// The rate the device is really running at, which is what its driver reports unless the
    /// driver is one that says it has moved before it has.
    pub fn running_rate(&self) -> f64 {
        let reported = self.spec.lock().expect("not poisoned").rate;
        self.held.lock().expect("not poisoned").running.unwrap_or(reported)
    }

    /// The driver asks to be reset: up to the DAW once its buffers belong to a stream, and kept for
    /// the aggregate before that, as the real host does.
    pub fn ask_for_reset(&self) {
        let stream = self.inner.lock().expect("not poisoned").stream.clone();
        match stream {
            Some(stream) => {
                stream.forward(selector::RESET_REQUEST, 0);
            }
            None => self.held.lock().expect("not poisoned").requests.reset = true,
        }
    }

    /// The driver says its rate moved, the same way.
    pub fn say_rate_moved(&self, hz: f64) {
        let stream = self.inner.lock().expect("not poisoned").stream.clone();
        match stream {
            Some(stream) => stream.forward_rate(hz),
            None => self.held.lock().expect("not poisoned").requests.rate = Some(hz),
        }
    }

    pub fn is_started(&self) -> bool {
        self.inner.lock().expect("not poisoned").started
    }

    pub fn has_buffers(&self) -> bool {
        let inner = self.inner.lock().expect("not poisoned");
        inner.created && !inner.disposed
    }

    /// Put a block of audio on one of the device's inputs, as the converter would.
    pub fn set_input(&self, channel: usize, half: usize, samples: &[i32]) {
        let mut inner = self.inner.lock().expect("not poisoned");
        let (block, width) = (inner.block, inner.input_width);
        let code = self.spec.lock().expect("not poisoned").input_type;
        let buffer = &mut inner.inputs[channel];
        for (index, value) in samples.iter().take(block).enumerate() {
            let at = (half * block + index) * width;
            sample::write(code, *value, &mut buffer[at..]);
        }
    }

    /// What the aggregate wrote to one of the device's outputs.
    pub fn output(&self, channel: usize, half: usize) -> Vec<i32> {
        let inner = self.inner.lock().expect("not poisoned");
        let (block, width) = (inner.block, inner.output_width);
        let code = self.spec.lock().expect("not poisoned").output_type;
        let buffer = &inner.outputs[channel];
        (0..block).map(|index| sample::read(code, &buffer[(half * block + index) * width..])).collect()
    }

    /// Fill every output buffer with something that is not silence and not audio, so that a test
    /// can tell "written as silence" from "never written at all".
    pub fn poison_outputs(&self) {
        let mut inner = self.inner.lock().expect("not poisoned");
        for buffer in inner.outputs.iter_mut() {
            buffer.fill(0x5A);
        }
    }

    /// Whether any output still holds the poison, which would mean a buffer was handed back to the
    /// device without being written.
    pub fn any_output_unwritten(&self, half: usize) -> bool {
        let inner = self.inner.lock().expect("not poisoned");
        let (block, width) = (inner.block, inner.output_width);
        inner
            .outputs
            .iter()
            .any(|buffer| buffer[half * block * width..(half + 1) * block * width].iter().all(|&b| b == 0x5A))
    }

    /// Make this device call back, as its own thread would.
    pub fn fire(&self, half: usize) {
        let (stream, index) = {
            let inner = self.inner.lock().expect("not poisoned");
            (inner.stream.clone(), inner.index)
        };
        if let Some(stream) = stream {
            self.fired.fetch_add(1, Ordering::Relaxed);
            stream.device_callback(index, half);
        }
    }

    /// Send a message up to the DAW, as a vendor driver does when it wants a reset.
    pub fn send(&self, selector: i32, value: i32) -> i32 {
        let stream = self.inner.lock().expect("not poisoned").stream.clone();
        match stream {
            Some(stream) => stream.forward(selector, value),
            None => 0,
        }
    }
}

/// One fake device, as the driver holds it.
pub struct FakeSub {
    device: Arc<FakeDevice>,
}

impl FakeSub {
    fn refuse(&self, step: Step) -> Result<(), String> {
        match &self.device.spec.lock().expect("not poisoned").fails_at {
            Some((at, why)) if *at == step => Err(why.clone()),
            _ => Ok(()),
        }
    }
}

impl SubDriver for FakeSub {
    fn init(&mut self) -> Result<(), String> {
        self.device.note("init");
        self.refuse(Step::Init)?;
        // A driver that remembers a rate opens at it, and whatever it was waiting to do about the
        // last one is over.
        let mut held = self.device.held.lock().expect("not poisoned");
        held.opens += 1;
        held.owes_reset = false;
        held.running = None;
        held.pending = None;
        if let Some(hz) = held.remembered.take() {
            self.device.spec.lock().expect("not poisoned").rate = hz;
        }
        Ok(())
    }

    fn describe(&mut self) -> Result<Description, String> {
        self.device.note("describe");
        self.refuse(Step::Describe)?;
        let spec = self.device.spec.lock().expect("not poisoned");
        Ok(Description {
            name: spec.driver_name.clone(),
            version: 1,
            inputs: spec.inputs,
            outputs: spec.outputs,
            min: spec.min,
            max: spec.max,
            preferred: spec.preferred,
            granularity: spec.granularity,
            rate: spec.rate,
            latency_in: spec.latency_in,
            latency_out: spec.latency_out,
            input_type: spec.input_type,
            output_type: spec.output_type,
            output_ready: spec.output_ready,
        })
    }

    fn can_rate(&mut self, hz: f64) -> bool {
        self.device.spec.lock().expect("not poisoned").rates.contains(&hz)
    }

    fn set_rate(&mut self, hz: f64) -> Result<(), String> {
        self.device.note(&format!("set_rate {hz}"));
        self.refuse(Step::SetRate)?;
        let (takes, was) = {
            let spec = self.device.spec.lock().expect("not poisoned");
            (spec.takes, spec.rate)
        };
        if hz == was {
            return Ok(());
        }
        let has_buffers = self.device.has_buffers();
        let mut held = self.device.held.lock().expect("not poisoned");
        match takes {
            Takes::AtOnce => self.device.spec.lock().expect("not poisoned").rate = hz,
            Takes::WhenReopened => held.remembered = Some(hz),
            Takes::WithBuffers => held.pending = Some(hz),
            Takes::AfterReset => {
                held.remembered = Some(hz);
                held.running.get_or_insert(was);
                self.device.spec.lock().expect("not poisoned").rate = hz;
                held.owes_reset = !has_buffers;
                drop(held);
                if has_buffers {
                    self.device.ask_for_reset();
                }
            }
            Takes::Never => {}
        }
        Ok(())
    }

    fn read_rate(&mut self) -> Result<f64, String> {
        Ok(self.device.spec.lock().expect("not poisoned").rate)
    }

    fn take_requests(&mut self) -> Requests {
        std::mem::take(&mut self.device.held.lock().expect("not poisoned").requests)
    }

    fn create_buffers(&mut self, inputs: &[i32], outputs: &[i32], block: i32) -> Result<DeviceBuffers, String> {
        self.device.note(&format!("create_buffers {} in, {} out, {block}", inputs.len(), outputs.len()));
        self.refuse(Step::CreateBuffers)?;
        let (input_type, output_type) = {
            let spec = self.device.spec.lock().expect("not poisoned");
            (spec.input_type, spec.output_type)
        };
        let block = block.max(0) as usize;
        let input_width = sample::width(input_type).unwrap_or(4);
        let output_width = sample::width(output_type).unwrap_or(4);
        let mut inner = self.device.inner.lock().expect("not poisoned");
        inner.inputs = inputs.iter().map(|_| vec![0u8; block * 2 * input_width].into_boxed_slice()).collect();
        inner.outputs = outputs.iter().map(|_| vec![0u8; block * 2 * output_width].into_boxed_slice()).collect();
        inner.block = block;
        inner.input_width = input_width;
        inner.output_width = output_width;
        inner.created = true;
        inner.disposed = false;
        // A driver that takes a rate with its buffers takes it now, and one that owes a reset has
        // somewhere to ask for it. Nothing is attached yet, so the request is kept for the
        // aggregate, as the real host keeps it.
        {
            let mut held = self.device.held.lock().expect("not poisoned");
            if let Some(hz) = held.pending.take() {
                self.device.spec.lock().expect("not poisoned").rate = hz;
            }
            if std::mem::take(&mut held.owes_reset) {
                held.requests.reset = true;
            }
        }
        Ok(DeviceBuffers {
            inputs: inner
                .inputs
                .iter_mut()
                .map(|buffer| {
                    let base = buffer.as_mut_ptr();
                    [base, unsafe { base.add(block * input_width) }]
                })
                .collect(),
            outputs: inner
                .outputs
                .iter_mut()
                .map(|buffer| {
                    let base = buffer.as_mut_ptr();
                    [base, unsafe { base.add(block * output_width) }]
                })
                .collect(),
            block,
        })
    }

    fn attach(&mut self, stream: Arc<Stream>, device: usize) {
        let mut inner = self.device.inner.lock().expect("not poisoned");
        inner.stream = Some(stream);
        inner.index = device;
    }

    fn detach(&mut self) {
        self.device.inner.lock().expect("not poisoned").stream = None;
    }

    fn start(&mut self) -> Result<(), String> {
        self.device.note("start");
        self.refuse(Step::Start)?;
        self.device.inner.lock().expect("not poisoned").started = true;
        Ok(())
    }

    fn stop(&mut self) {
        let stopping = {
            let mut inner = self.device.inner.lock().expect("not poisoned");
            let was = inner.started;
            inner.started = false;
            was
        };
        if stopping {
            self.device.note("stop");
        }
    }

    fn dispose_buffers(&mut self) {
        let disposing = {
            let mut inner = self.device.inner.lock().expect("not poisoned");
            let now = inner.created && !inner.disposed;
            inner.disposed = true;
            now
        };
        if disposing {
            self.device.note("dispose_buffers");
        }
    }
}

/// A PC with a registry and a set of devices behind it.
pub struct FakePc {
    pub entries: Vec<Entry>,
    devices: Vec<(String, Arc<FakeDevice>)>,
    /// Class ids this PC refuses to create at all.
    pub uncreatable: Vec<String>,
    journal: Journal,
}

/// A registry entry, named the way the vendors name theirs.
pub fn entry(key: &str, clsid: &str, dll: &str) -> Entry {
    Entry { key: key.to_string(), description: Some(key.to_string()), clsid: clsid.to_string(), dll: Ok(dll.to_string()) }
}

impl FakePc {
    pub fn new() -> FakePc {
        FakePc {
            entries: Vec::new(),
            devices: Vec::new(),
            uncreatable: Vec::new(),
            journal: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Everything every device was asked to do, in the order it was asked.
    pub fn journal(&self) -> Vec<String> {
        self.journal.lock().expect("not poisoned").clone()
    }

    /// Add one device, with the registry entry a DAW would see.
    pub fn with(mut self, key: &str, clsid: &str, dll: &str, spec: Spec) -> FakePc {
        self.entries.push(entry(key, clsid, dll));
        self.devices.push((key.to_string(), FakeDevice::new(key, spec, Arc::clone(&self.journal))));
        self
    }

    /// Add an entry with no device behind it, which is every other driver on a real PC.
    pub fn with_stranger(mut self, key: &str, clsid: &str, dll: &str) -> FakePc {
        self.entries.push(entry(key, clsid, dll));
        self
    }

    pub fn refusing(mut self, clsid: &str) -> FakePc {
        self.uncreatable.push(clsid.to_string());
        self
    }

    /// The device registered under this key, for a test to drive.
    pub fn device(&self, key: &str) -> Arc<FakeDevice> {
        self.devices
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, device)| Arc::clone(device))
            .unwrap_or_else(|| panic!("no fake device called {key}"))
    }

    /// This PC as phase 0 found it: the two Antelope interfaces and three strangers.
    pub fn this_pc() -> FakePc {
        FakePc::new()
            .with_stranger("Realtek ASIO", "{22222222-2222-2222-2222-222222222222}", r"c:\realtek\rtasio.dll")
            .with(
                "Zen Quadro Synergy Core",
                "{12217625-CB57-11EE-908D-7085C2FB2DD5}",
                r"c:\program files\antelope audio\zen quadro\zen_quadro.dll",
                Spec::named("Zen Quadro Synergy Core").with_channels(16, 16).with_latency(639, 799),
            )
            .with(
                "ZenStudioTB ASIO Driver",
                "{AE4A4452-A316-11E5-A113-080027F6C1F4}",
                r"c:\program files\antelope audio\zenstudiotb\zenstudiotb.dll",
                Spec::named("ZenStudioTB ASIO Driver").with_channels(24, 24).with_latency(636, 700),
            )
            .with_stranger("Yamaha Steinberg USB ASIO", "{33333333-3333-3333-3333-333333333333}", r"c:\steinberg\ysusb.dll")
    }
}

impl Default for FakePc {
    fn default() -> Self {
        FakePc::new()
    }
}

/// The handle a driver is given: the same PC, shared.
pub struct FakeHost {
    pub pc: Arc<FakePc>,
}

impl Host for FakeHost {
    fn entries(&self) -> Result<Vec<Entry>, String> {
        Ok(self.pc.entries.clone())
    }

    fn open(&self, entry: &Entry, _slot: usize) -> Result<Box<dyn SubDriver>, String> {
        if self.pc.uncreatable.iter().any(|clsid| gazelle_audio_stream_abi::same_clsid(clsid, &entry.clsid)) {
            return Err("this driver will not share a process".to_string());
        }
        let device = self
            .pc
            .devices
            .iter()
            .find(|(key, _)| key == &entry.key)
            .map(|(_, device)| Arc::clone(device))
            .ok_or_else(|| format!("{} has no driver behind it", entry.key))?;
        Ok(Box::new(FakeSub { device }))
    }
}
