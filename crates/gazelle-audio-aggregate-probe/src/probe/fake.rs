//! A PC and a pair of drivers made of data, so every rule in [`super::run`] is tested without a
//! driver, a device or a converter anywhere near it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use super::{AsioEntry, BufferSizes, ClockSource, Description, Host, Step, SubDriver, QUADRO_CLSID, STUDIO_CLSID};

/// What a fake driver answers, and where it refuses.
#[derive(Clone, Debug)]
pub struct Spec {
    pub name: String,
    pub version: i32,
    pub inputs: i32,
    pub outputs: i32,
    pub preferred: i32,
    pub rate: f64,
    /// Callbacks the driver makes per second while it runs.
    pub callbacks_per_second: f64,
    /// The step this driver refuses at, and what it says.
    pub fails_at: Option<(Step, String)>,
    /// Rates `canSampleRate` says yes to.
    pub rates: Vec<f64>,
}

impl Spec {
    /// The Quadro as its panel reads: 44.1 kHz, 512 samples, the clock master.
    pub fn quadro() -> Spec {
        Spec {
            name: "Zen Quadro Synergy Core".into(),
            version: 2,
            inputs: 32,
            outputs: 32,
            preferred: 512,
            rate: 44_100.0,
            callbacks_per_second: 44_100.0 / 512.0,
            fails_at: None,
            rates: vec![44_100.0, 48_000.0, 88_200.0, 96_000.0],
        }
    }

    /// The Studio+, which follows the Quadro over S/PDIF.
    pub fn studio() -> Spec {
        Spec {
            name: "ZenStudioTB ASIO Driver".into(),
            version: 1,
            inputs: 26,
            outputs: 26,
            preferred: 512,
            rate: 44_100.0,
            callbacks_per_second: 44_100.0 / 512.0,
            fails_at: None,
            rates: vec![44_100.0, 48_000.0, 88_200.0, 96_000.0],
        }
    }

    pub fn failing(mut self, step: Step, message: &str) -> Spec {
        self.fails_at = Some((step, message.into()));
        self
    }

    pub fn preferring(mut self, preferred: i32) -> Spec {
        self.preferred = preferred;
        self
    }

    /// A driver running at its own pace, which is what an unlocked device looks like.
    pub fn at(mut self, callbacks_per_second: f64) -> Spec {
        self.callbacks_per_second = callbacks_per_second;
        self
    }
}

/// What the fakes were asked to do, in order, so a test can see the shape of a run as well as its
/// answer: every entry reads "<driver>: <call>".
pub type Log = Rc<RefCell<Vec<String>>>;

pub struct FakeSub {
    name: String,
    spec: Spec,
    log: Log,
    ticks: Rc<RefCell<HashMap<String, u64>>>,
    started: bool,
    created: bool,
}

impl FakeSub {
    fn refuse(&self, step: Step) -> Option<String> {
        match &self.spec.fails_at {
            Some((at, message)) if *at == step => Some(message.clone()),
            _ => None,
        }
    }

    fn note(&self, call: &str) {
        self.log.borrow_mut().push(format!("{}: {call}", self.name));
    }
}

impl SubDriver for FakeSub {
    fn init(&mut self) -> Result<(), String> {
        self.note("init");
        match self.refuse(Step::Init) {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    fn describe(&mut self) -> Result<Description, String> {
        self.note("describe");
        if let Some(message) = self.refuse(Step::Read) {
            return Err(message);
        }
        Ok(Description {
            name: self.spec.name.clone(),
            version: self.spec.version,
            inputs: self.spec.inputs,
            outputs: self.spec.outputs,
            buffers: BufferSizes { min: 32, max: 2048, preferred: self.spec.preferred, granularity: -1 },
            rate: self.spec.rate,
            latency_in: self.spec.preferred + 59,
            latency_out: self.spec.preferred + 120,
            input_format: "Int32 little endian (type 18)".into(),
            output_format: "Int32 little endian (type 18)".into(),
            output_bytes: 4,
            clocks: vec![ClockSource { index: 0, name: "Internal".into(), current: true }],
        })
    }

    fn can_rate(&mut self, hz: f64) -> bool {
        self.note("canSampleRate");
        self.spec.rates.contains(&hz)
    }

    fn create_buffers(&mut self, inputs: i32, outputs: i32, size: i32) -> Result<(), String> {
        self.note(&format!("createBuffers {inputs} in, {outputs} out, {size}"));
        if let Some(message) = self.refuse(Step::CreateBuffers) {
            return Err(message);
        }
        self.created = true;
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        self.note("start");
        if let Some(message) = self.refuse(Step::Start) {
            return Err(message);
        }
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) {
        if self.started {
            self.note("stop");
            self.started = false;
        }
    }

    fn dispose_buffers(&mut self) {
        if self.created {
            self.note("disposeBuffers");
            self.created = false;
        }
    }

    fn callbacks(&self) -> u64 {
        self.ticks.borrow().get(&self.name).copied().unwrap_or(0)
    }
}

impl Drop for FakeSub {
    fn drop(&mut self) {
        self.note("release");
    }
}

/// A PC with an ASIO registry and a set of drivers behind it.
pub struct FakePc {
    pub entries: Vec<AsioEntry>,
    pub specs: HashMap<String, Spec>,
    /// Which drivers were actually created, in order.
    pub log: Log,
    ticks: Rc<RefCell<HashMap<String, u64>>>,
    /// Class ids the PC refuses to create at all.
    pub uncreatable: HashMap<String, String>,
}

/// A registry entry for a class id, named as the vendors name theirs.
pub fn entry(key: &str, clsid: &str, dll: &str) -> AsioEntry {
    AsioEntry { key: key.into(), description: Some(key.into()), clsid: clsid.into(), dll: Ok(dll.into()) }
}

impl FakePc {
    /// This PC as it is: five ASIO entries, two of them the Antelope USB drivers.
    pub fn this_pc(quadro: Spec, studio: Spec) -> FakePc {
        let entries = vec![
            entry("Antelope Audio Thunderbolt", "{11111111-1111-1111-1111-111111111111}", r"c:\antelope\tb.dll"),
            entry("Realtek ASIO", "{22222222-2222-2222-2222-222222222222}", r"c:\realtek\rtasio.dll"),
            entry(
                "Zen Quadro Synergy Core",
                QUADRO_CLSID,
                r"c:\program files\antelope audio\zen quadro synergy core usb audio driver\x64\zen_quadro_synergy_coreasio_x64.dll",
            ),
            entry(
                "ZenStudioTB ASIO Driver",
                STUDIO_CLSID,
                r"c:\program files\antelope audio\zenstudiotb usb audio driver\w10_x64\zenstudiotbasio_x64.dll",
            ),
            entry("Yamaha Steinberg USB ASIO", "{33333333-3333-3333-3333-333333333333}", r"c:\steinberg\ysusb.dll"),
        ];
        let specs = HashMap::from([("quadro".to_string(), quadro), ("studio".to_string(), studio)]);
        FakePc {
            entries,
            specs,
            log: Rc::new(RefCell::new(Vec::new())),
            ticks: Rc::new(RefCell::new(HashMap::new())),
            uncreatable: HashMap::new(),
        }
    }

    /// Refuse to create one driver's object, as a driver that will not share a process would.
    pub fn refusing(mut self, name: &str, message: &str) -> FakePc {
        self.uncreatable.insert(name.into(), message.into());
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

impl Host for FakePc {
    fn entries(&self) -> Result<Vec<AsioEntry>, String> {
        Ok(self.entries.clone())
    }

    fn open(&self, entry: &AsioEntry) -> Result<Box<dyn SubDriver>, String> {
        let name = entry.target().unwrap_or(&entry.key).to_string();
        if let Some(message) = self.uncreatable.get(&name) {
            self.log.borrow_mut().push(format!("{name}: create refused"));
            return Err(message.clone());
        }
        let spec = self.specs.get(&name).cloned().ok_or_else(|| format!("no fake driver for {name}"))?;
        self.log.borrow_mut().push(format!("{name}: create"));
        Ok(Box::new(FakeSub {
            name,
            spec,
            log: self.log.clone(),
            ticks: self.ticks.clone(),
            started: false,
            created: false,
        }))
    }

    fn wait(&self, seconds: u64) -> Duration {
        // No sleeping: each driver simply makes the callbacks its own pace would have made.
        for (name, spec) in &self.specs {
            let made = (spec.callbacks_per_second * seconds as f64).round() as u64;
            *self.ticks.borrow_mut().entry(name.clone()).or_insert(0) += made;
        }
        Duration::from_secs(seconds)
    }
}
