//! What the aggregate driver says about itself, and how Gazelle tells it something changed.
//!
//! The driver runs inside whatever process opened it, which is a DAW, so none of this is a
//! request and a reply. Three things pass between the two:
//!
//! 1. **Live status** is a fixed size record the driver publishes in shared memory. It is
//!    published under a sequence counter, so the driver never waits for a reader and a reader
//!    never sees half of an update. The record, the counter and the retry all live in
//!    `gazelle_audio_aggregate_status`, which is the one place either side learns the format;
//!    nothing about it is declared a second time here.
//! 2. **Durable events** are lines in a small log file the driver appends to. They survive the
//!    driver exiting, which is exactly when someone wants to know why last night's session would
//!    not start. Its kinds are the driver's own words, `glitched` and `session-ended` among them,
//!    which is where the blocks a session lost are written down once the live counters below have
//!    gone with the driver.
//! 3. **A change** is Gazelle writing the configuration file, bumping a generation counter in the
//!    record and signalling a named event. The driver's watcher thread re-plans.
//!
//! All three sit behind [`StatusLink`], with [`NoLink`] for a PC where the driver has published
//! nothing and [`FakeLink`] for tests, so no test maps a section or signals anything.

use std::sync::{Arc, Mutex};

use serde::Serialize;

/// The plan the driver is running, as it reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AggregatePlan {
    /// The device driving the callback, by the name the configuration gives it.
    pub master: String,
    pub rate: u32,
    pub buffer_size: i32,
    /// The channels the DAW asked for.
    pub inputs: u32,
    pub outputs: u32,
    /// `aligned` or `lowest_latency`.
    pub alignment: String,
    /// The one figure each way the aggregate reports for the whole of itself, in samples.
    pub input_latency: i32,
    pub output_latency: i32,
}

/// One sub-device, as the driver reports it now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DeviceStatus {
    pub name: String,
    /// What its own driver calls itself.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub driver_name: String,
    pub streaming: bool,
    /// True while the device has missed enough of the master's buffers to be called stalled: its
    /// inputs read as silence and its outputs are muted until it calls back again.
    pub stalled: bool,
    pub is_master: bool,
    /// How many of its channels the aggregate exposed, which is the one count that comes from the
    /// audio driver itself rather than from what Gazelle knows about the interface.
    pub inputs: u32,
    pub outputs: u32,
    /// How far this device's stream is from the master's, in samples. Zero on the master itself,
    /// and only worth reading while both are streaming. Growing in one direction is two clocks.
    pub sample_gap: i64,
    /// How many times its driver has called back in this session.
    pub callbacks: u64,
    /// Blocks thrown away because its ring was full.
    pub dropped: u64,
    /// Blocks that were not there when they were wanted, which is what a person hears as a click.
    pub starved: u64,
}

/// The whole record, read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AggregateStatus {
    /// True while a DAW holds the driver open with its buffers made.
    pub open: bool,
    /// True while audio is running.
    pub streaming: bool,
    /// The generation Gazelle last asked for, which it bumps when it writes the configuration.
    pub generation: u64,
    /// The generation the driver is actually running.
    pub generation_in_force: u64,
    /// False while the driver has not taken up what Gazelle last asked for.
    pub up_to_date: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<AggregatePlan>,
    pub devices: Vec<DeviceStatus>,
    /// The last thing the driver refused to do, in the same words the DAW was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refusal: Option<String>,
    /// Where the configuration in force was read from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_source: Option<String>,
}

/// The status, or why there is none. A driver that has never run is not a fault.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StatusReading {
    /// The driver has published nothing: it has not been opened since this PC started.
    Silent { message: String },
    /// It could not be read.
    Unread { message: String },
    Read(AggregateStatus),
}

/// One line of the driver's event log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AggregateEvent {
    /// When it happened, as the driver wrote it: `YYYY-MM-DD HH:MM:SS`, local time, because the
    /// person reading it is the person it happened to.
    pub at: String,
    /// What kind of thing it was, in the driver's own one word.
    pub kind: String,
    /// The rest of the line, in the same words the DAW or the person was given.
    pub message: String,
}

/// Reading the driver's record, its log, and telling it something changed.
pub trait StatusLink: Send + Sync {
    /// The live record.
    fn status(&self) -> StatusReading;
    /// The last `limit` entries of the event log, oldest first. A log that is not there is an
    /// empty list, not a fault.
    fn events(&self, limit: usize) -> Result<Vec<AggregateEvent>, String>;
    /// Bump the generation and signal the driver. Answers the generation it wrote.
    fn signal(&self) -> Result<u64, String>;
}

/// A PC where nothing has published a record.
pub struct NoLink {
    pub message: String,
}

impl Default for NoLink {
    fn default() -> Self {
        NoLink { message: "The aggregate driver has not published anything on this PC. It publishes while a DAW has it open.".into() }
    }
}

impl StatusLink for NoLink {
    fn status(&self) -> StatusReading {
        StatusReading::Silent { message: self.message.clone() }
    }
    fn events(&self, _limit: usize) -> Result<Vec<AggregateEvent>, String> {
        Ok(Vec::new())
    }
    fn signal(&self) -> Result<u64, String> {
        Err("there is nothing to signal: the aggregate driver has not published anything on this PC.".into())
    }
}

/// A link made of data, for tests. It counts the signals rather than sending any.
pub struct FakeLink {
    pub status: Mutex<StatusReading>,
    pub events: Mutex<Vec<AggregateEvent>>,
    /// The generation the last signal wrote, and how many were sent.
    pub signals: Mutex<Vec<u64>>,
    pub signal_fails: Option<String>,
}

impl Default for FakeLink {
    fn default() -> Self {
        FakeLink {
            status: Mutex::new(StatusReading::Silent { message: "nothing published".into() }),
            events: Mutex::new(Vec::new()),
            signals: Mutex::new(Vec::new()),
            signal_fails: None,
        }
    }
}

impl FakeLink {
    pub fn with_status(status: AggregateStatus) -> Self {
        FakeLink { status: Mutex::new(StatusReading::Read(status)), ..FakeLink::default() }
    }

    /// How many times the driver has been signalled.
    pub fn signalled(&self) -> Vec<u64> {
        self.signals.lock().unwrap().clone()
    }
}

impl StatusLink for FakeLink {
    fn status(&self) -> StatusReading {
        self.status.lock().unwrap().clone()
    }
    fn events(&self, limit: usize) -> Result<Vec<AggregateEvent>, String> {
        let events = self.events.lock().unwrap();
        let from = events.len().saturating_sub(limit);
        Ok(events[from..].to_vec())
    }
    fn signal(&self) -> Result<u64, String> {
        if let Some(why) = &self.signal_fails {
            return Err(why.clone());
        }
        let mut signals = self.signals.lock().unwrap();
        let generation = signals.len() as u64 + 1;
        signals.push(generation);
        Ok(generation)
    }
}

/// The link for this PC: the shared section and the log file on Windows, and elsewhere a link
/// that says nothing has been published, which is what is true there.
pub fn for_this_pc() -> Arc<dyn StatusLink> {
    #[cfg(windows)]
    {
        Arc::new(ThisPc)
    }
    #[cfg(not(windows))]
    {
        Arc::new(NoLink::default())
    }
}

/// The real link, through the crate both sides share
/// (`gazelle_audio_aggregate_status`), which is the only place either of them learns the format.
///
/// Nothing is held open between requests. The section exists only while a DAW has the driver
/// loaded, so keeping a mapping would mean holding a handle to something that comes and goes, and
/// a page that polls this every second can afford to open it each time.
#[cfg(windows)]
pub struct ThisPc;

#[cfg(windows)]
impl ThisPc {
    fn reader() -> Result<shared::Reader, String> {
        let section = shared::windows::Section::open()?;
        shared::Reader::map(Box::new(section)).map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
use gazelle_audio_aggregate_status as shared;

#[cfg(windows)]
impl StatusLink for ThisPc {
    fn status(&self) -> StatusReading {
        let silent = "The aggregate driver has not published anything on this PC. It publishes while a DAW has it open.";
        let Ok(reader) = ThisPc::reader() else {
            return StatusReading::Silent { message: silent.into() };
        };
        match reader.read() {
            Ok(snapshot) => StatusReading::Read(from_snapshot(&snapshot)),
            // Nothing mapped is the ordinary case; anything else is worth saying out loud.
            Err(shared::StatusError::NotMapped) => StatusReading::Silent { message: silent.into() },
            Err(why) => StatusReading::Unread { message: format!("The aggregate driver's status could not be read: {why}.") },
        }
    }

    fn events(&self, limit: usize) -> Result<Vec<AggregateEvent>, String> {
        let Some(path) = shared::names::event_log_path() else {
            return Ok(Vec::new());
        };
        read_log(&path, limit)
    }

    fn signal(&self) -> Result<u64, String> {
        let reader = ThisPc::reader()?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos().min(i64::MAX as u128) as i64);
        let generation = reader.request(nanos);
        // The generation is what the driver acts on; the event only saves it a wait.
        if let Ok(event) = shared::windows::ReloadEvent::open() {
            use gazelle_audio_aggregate_status::map::Notifier;
            event.signal();
        }
        Ok(generation)
    }
}

/// The shared record as this server reports it.
#[cfg(windows)]
fn from_snapshot(snapshot: &shared::Snapshot) -> AggregateStatus {
    let driver = &snapshot.driver;
    let devices: Vec<DeviceStatus> = snapshot
        .devices()
        .iter()
        .map(|device| DeviceStatus {
            name: device.name.get().to_string(),
            driver_name: device.driver_name.get().to_string(),
            streaming: device.streaming != 0,
            stalled: device.stalled != 0,
            is_master: device.is_master != 0,
            inputs: device.inputs,
            outputs: device.outputs,
            sample_gap: device.gap,
            callbacks: device.callbacks,
            dropped: device.dropped,
            starved: device.starved,
        })
        .collect();
    let master = devices.iter().find(|device| device.is_master).map(|device| device.name.clone()).unwrap_or_default();
    AggregateStatus {
        open: driver.open != 0,
        streaming: driver.streaming != 0,
        generation: snapshot.control.generation,
        generation_in_force: driver.generation_in_force,
        up_to_date: snapshot.is_up_to_date(),
        // A record with no devices in it is a driver that has never planned anything, and an
        // empty plan would read as one made of zeroes.
        plan: (!devices.is_empty()).then(|| AggregatePlan {
            master,
            rate: driver.sample_rate as u32,
            buffer_size: driver.buffer_size,
            inputs: driver.daw_inputs,
            outputs: driver.daw_outputs,
            alignment: shared::record::alignment::name(driver.alignment).to_string(),
            input_latency: driver.input_latency,
            output_latency: driver.output_latency,
        }),
        devices,
        last_refusal: Some(driver.refusal.get().to_string()).filter(|text| !text.is_empty()),
        config_source: Some(driver.config_source.get().to_string()).filter(|text| !text.is_empty()),
    }
}

/// The last `limit` entries of an event log, oldest first. A file that is not there is an empty
/// list: the driver writes one only once something has happened.
#[cfg(windows)]
fn read_log(path: &std::path::Path, limit: usize) -> Result<Vec<AggregateEvent>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("The aggregate driver's event log ({}) could not be read: {e}.", path.display())),
    };
    let parsed: Vec<AggregateEvent> = text
        .lines()
        // A line a newer driver wrote in a shape this build does not know is left out rather than
        // shown as something it is not.
        .filter_map(shared::events::parse)
        .map(|line| AggregateEvent { at: line.at, kind: line.event.word().to_string(), message: line.detail })
        .collect();
    let from = parsed.len().saturating_sub(limit);
    Ok(parsed[from..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pc_with_nothing_published_says_so_and_refuses_to_signal() {
        let link = NoLink::default();
        assert!(matches!(link.status(), StatusReading::Silent { .. }));
        assert!(link.events(10).unwrap().is_empty());
        assert!(link.signal().is_err());
    }

    #[test]
    fn the_fake_link_counts_signals_and_gives_back_the_last_events() {
        let link = FakeLink::default();
        link.events.lock().unwrap().extend((0..5).map(|n| AggregateEvent { at: n.to_string(), kind: "refused".into(), message: n.to_string() }));
        assert_eq!(link.events(2).unwrap().iter().map(|e| e.at.clone()).collect::<Vec<_>>(), vec!["3", "4"]);
        assert_eq!(link.signal().unwrap(), 1);
        assert_eq!(link.signal().unwrap(), 2);
        assert_eq!(link.signalled(), vec![1, 2]);
    }
}
