//! What the driver tells the outside world about itself, and what it keeps for afterwards.
//!
//! Two things, and they are on purpose not the same thing.
//!
//! - **Live state** goes into the shared record from `gazelle-audio-aggregate-status`. The audio
//!   path writes the cheap fields every block; everything else is written when it changes. Nothing
//!   here allocates, locks or opens a file on a callback thread.
//! - **Durable events** go into a small log file, and only when something happens: a refusal, a
//!   device stalling and coming back, a session starting and ending. Never on a timer, so the file
//!   stays small and every line in it means something. It is the half that survives the driver
//!   exiting, which is exactly when a person wants to know why last night's session would not
//!   start.
//!
//! **Reporting is never a reason to fail.** A [`Reporter`] with no section and no log is a
//! [`Reporter::silent`], every call on it does nothing, and the driver behaves exactly as it does
//! with Gazelle closed, which is the state it has to work in anyway.

use std::sync::Mutex;

use gazelle_audio_aggregate_status::events::{self, Event};
use gazelle_audio_aggregate_status::record::{alignment as codes, DriverArea, Line, Name};
use gazelle_audio_aggregate_status::Publisher;

use crate::config::Alignment;
use crate::plan::Plan;

/// Somewhere to put a line of the event log. The real one is a file; a test's is a list.
pub trait LogSink: Send {
    fn append(&mut self, line: &str);
}

/// What time it is, in the words the log is written in. Behind a trait so that a test's log is the
/// same every time it runs.
pub trait Clock: Send + Sync {
    /// The local time as `YYYY-MM-DD HH:MM:SS`.
    fn stamp(&self) -> String;
}

/// The machine's own clock.
pub struct LocalClock;

impl Clock for LocalClock {
    #[cfg(windows)]
    fn stamp(&self) -> String {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut now = unsafe { std::mem::zeroed() };
        // Safety: it fills in a structure this function owns.
        unsafe { GetLocalTime(&mut now) };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
        )
    }

    #[cfg(not(windows))]
    fn stamp(&self) -> String {
        "0000-00-00 00:00:00".to_string()
    }
}

/// The event log as a file under `%APPDATA%\gazelle`.
///
/// Opening it trims it to the last few hundred lines, which is the only time it is ever rewritten.
/// A line is appended by opening, writing and closing: events are rare, and a handle held open for
/// a whole session is a handle that stops the file being moved or read by anything else.
pub struct FileLog {
    path: std::path::PathBuf,
}

impl FileLog {
    /// Open the log at the path both sides agree on, trimming what is there. Nothing at all if the
    /// folder cannot be made, because reporting never stops the driver.
    pub fn open() -> Option<FileLog> {
        let path = gazelle_audio_aggregate_status::names::event_log_path()?;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).ok()?;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            let kept = events::trimmed(&text, events::KEEP_LINES);
            if kept.len() < text.lines().count() {
                let mut rewritten = kept.join("\n");
                rewritten.push('\n');
                let _ = std::fs::write(&path, rewritten);
            }
        }
        Some(FileLog { path })
    }
}

impl LogSink for FileLog {
    fn append(&mut self, line: &str) {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// Everything the driver says about itself.
pub struct Reporter {
    publisher: Option<Publisher>,
    log: Mutex<Option<Box<dyn LogSink>>>,
    clock: Box<dyn Clock>,
}

impl Reporter {
    /// A reporter that says nothing, which is what the driver gets when it cannot make a section
    /// or a log. Every call on it does nothing at all.
    pub fn silent() -> Reporter {
        Reporter { publisher: None, log: Mutex::new(None), clock: Box::new(LocalClock) }
    }

    pub fn new(publisher: Option<Publisher>, log: Option<Box<dyn LogSink>>, clock: Box<dyn Clock>) -> Reporter {
        Reporter { publisher, log: Mutex::new(log), clock }
    }

    /// Whether there is a shared section at all. A driver without one still runs; it just cannot
    /// be watched, and there is nothing for a watcher thread to wait on either.
    pub fn is_publishing(&self) -> bool {
        self.publisher.is_some()
    }

    /// The generation Gazelle has asked for, or zero when nobody is asking.
    pub fn generation(&self) -> u64 {
        self.publisher.as_ref().map_or(0, Publisher::generation)
    }

    /// **The audio path's one call.** It never waits: a block whose update did not fit is simply
    /// written by the next one.
    pub fn try_update(&self, change: impl FnOnce(&mut DriverArea)) -> bool {
        match &self.publisher {
            Some(publisher) => publisher.try_update(change),
            None => false,
        }
    }

    /// Everything off the audio path writes through this.
    pub fn update(&self, change: impl FnOnce(&mut DriverArea)) {
        if let Some(publisher) = &self.publisher {
            publisher.update(change);
        }
    }

    /// What the record says now, for the driver's own use.
    pub fn snapshot(&self) -> Option<DriverArea> {
        self.publisher.as_ref().map(Publisher::driver_area)
    }

    /// Write one line of the event log. Off the audio path, always.
    pub fn note(&self, event: Event, detail: &str) {
        let line = events::line(&self.clock.stamp(), event, detail);
        if let Ok(mut log) = self.log.lock() {
            if let Some(sink) = log.as_mut() {
                sink.append(&line);
            }
        }
    }

    /// The plan that is now in force, and where it came from.
    pub fn plan_in_force(&self, plan: &Plan, rate: f64, source: &str, generation: u64) {
        self.update(|area| {
            area.device_count = plan.devices.len().min(area.devices.len()) as u32;
            area.master = plan.master as u32;
            area.buffer_size = plan.block;
            area.sample_rate = rate;
            area.alignment =
                if plan.alignment == Alignment::LowestLatency { codes::LOWEST_LATENCY } else { codes::ALIGNED };
            area.input_latency = plan.input_latency;
            area.output_latency = plan.output_latency;
            area.config_source = Line::from(source);
            area.generation_in_force = generation;
            for (index, device) in plan.devices.iter().take(area.devices.len()).enumerate() {
                let into = &mut area.devices[index];
                into.name = Name::from(&device.name);
                into.driver_name = Name::from(&device.driver_name);
                into.inputs = device.inputs.len() as u32;
                into.outputs = device.outputs.len() as u32;
                into.is_master = u32::from(index == plan.master);
                into.latency_in = device.latency_in;
                into.latency_out = device.latency_out;
                into.pad_in = device.pad_in;
                into.pad_out = device.pad_out;
                into.streaming = 0;
                into.stalled = 0;
                into.gap = 0;
                into.callbacks = 0;
                into.dropped = 0;
                into.starved = 0;
            }
            for spare in area.devices.iter_mut().skip(plan.devices.len()) {
                *spare = Default::default();
            }
        });
    }

    /// A DAW has the buffers made, or has let them go.
    pub fn buffers(&self, open: bool, daw_inputs: usize, daw_outputs: usize) {
        self.update(|area| {
            area.open = u32::from(open);
            area.daw_inputs = daw_inputs as u32;
            area.daw_outputs = daw_outputs as u32;
            if !open {
                area.streaming = 0;
            }
        });
    }

    /// Audio has started, or stopped. This is one of the events worth keeping.
    pub fn session(&self, running: bool, nanos: i64, detail: &str) {
        self.update(|area| {
            area.streaming = u32::from(running);
            if running {
                area.session_nanos = nanos;
                area.callbacks = 0;
                area.position = 0;
            } else {
                area.session_nanos = 0;
            }
            for device in area.devices.iter_mut() {
                device.streaming = u32::from(running);
                if !running {
                    device.gap = 0;
                }
            }
        });
        self.note(if running { Event::SessionStarted } else { Event::SessionEnded }, detail);
    }

    /// The driver would not do something, and the DAW was told why. Worth keeping, because this is
    /// the line somebody reads the morning after.
    pub fn refused(&self, generation: u64, why: &str) {
        self.update(|area| {
            area.refusal = Line::from(why);
            if generation != 0 {
                area.refused_generation = generation;
            }
        });
        self.note(Event::Refused, why);
    }

    /// A device has gone away, or come back. Called from the watcher thread, never the audio one:
    /// the audio path only sets the flag in the record.
    pub fn stall_changed(&self, device: &str, stalled: bool) {
        self.note(if stalled { Event::Stalled } else { Event::Recovered }, device);
    }
}

impl Default for Reporter {
    fn default() -> Self {
        Reporter::silent()
    }
}

/// Which devices have changed their mind about being here since the last look.
///
/// The audio path decides a device has stalled, because it is the only thing that can see the
/// callbacks; it writes a flag and nothing else. Turning a change of that flag into a line of the
/// log is this, on the watcher's thread.
#[derive(Default)]
pub struct StallWatch {
    last: Vec<bool>,
}

impl StallWatch {
    pub fn new() -> StallWatch {
        StallWatch::default()
    }

    /// The devices whose state is not what it was, and what it is now. Nothing at all the first
    /// time, unless a device is already stalled when we start looking.
    pub fn changes(&mut self, now: &[bool]) -> Vec<(usize, bool)> {
        if self.last.len() != now.len() {
            self.last = vec![false; now.len()];
        }
        let mut changed = Vec::new();
        for (index, &stalled) in now.iter().enumerate() {
            if self.last[index] != stalled {
                self.last[index] = stalled;
                changed.push((index, stalled));
            }
        }
        changed
    }

    /// Forget everything, which is what a new session is.
    pub fn clear(&mut self) {
        self.last.clear();
    }
}

/// A section, a log and a clock made of data, so that every test in this crate can watch what the
/// driver reports without a shared section, a file or a clock that moves.
#[cfg(test)]
pub mod fake {
    use super::{Clock, LogSink, Reporter};
    use gazelle_audio_aggregate_status::map::Scratch;
    use gazelle_audio_aggregate_status::{Publisher, Reader};
    use std::sync::{Arc, Mutex};

    /// An event log made of a list.
    #[derive(Clone, Default)]
    pub struct Written(Arc<Mutex<Vec<String>>>);

    impl LogSink for Written {
        fn append(&mut self, line: &str) {
            self.0.lock().expect("not poisoned").push(line.to_string());
        }
    }

    impl Written {
        /// Every line written, in order.
        pub fn lines(&self) -> Vec<String> {
            self.0.lock().expect("not poisoned").clone()
        }

        /// The lines of one kind, which is what a test usually wants to count.
        pub fn of(&self, word: &str) -> Vec<String> {
            self.lines().into_iter().filter(|line| line.split(' ').nth(2) == Some(word)).collect()
        }
    }

    /// A clock that has stopped, so that a line of the log is the same every time it is written.
    pub struct Stopped;

    impl Clock for Stopped {
        fn stamp(&self) -> String {
            "2026-09-20 21:14:07".to_string()
        }
    }

    /// A reporter with a scratch section behind it, the reader for that section, and its log.
    pub fn watched() -> (Arc<Reporter>, Reader, Written) {
        let scratch = Scratch::new();
        let publisher = Publisher::map(Box::new(scratch.clone())).expect("a scratch section");
        let reader = Reader::map(Box::new(scratch.opened())).expect("the same one");
        let written = Written::default();
        let reporter = Reporter::new(Some(publisher), Some(Box::new(written.clone())), Box::new(Stopped));
        (Arc::new(reporter), reader, written)
    }

    /// A reporter whose section could not be made, which is what a driver gets when there is
    /// nowhere to publish. It still keeps its event log.
    pub fn unmapped() -> (Arc<Reporter>, Written) {
        let too_small = Scratch::of(64);
        let publisher = Publisher::map(Box::new(too_small)).ok();
        assert!(publisher.is_none(), "the point of this one is that the mapping failed");
        let written = Written::default();
        (Arc::new(Reporter::new(publisher, Some(Box::new(written.clone())), Box::new(Stopped))), written)
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{watched, Written};
    use super::*;
    use gazelle_audio_aggregate_status::Reader;
    use std::sync::Arc;

    fn reporter() -> (Arc<Reporter>, Written, Reader) {
        let (reporter, reader, written) = watched();
        (reporter, written, reader)
    }

    #[test]
    fn a_reporter_with_nowhere_to_report_does_nothing_and_says_so() {
        let silent = Reporter::silent();
        assert!(!silent.is_publishing());
        assert_eq!(silent.generation(), 0);
        assert!(!silent.try_update(|area| area.callbacks = 1), "nothing was written, and nothing panicked");
        silent.update(|area| area.callbacks = 1);
        silent.note(Event::Refused, "a reason nobody will read");
        silent.refused(0, "another one");
        assert!(silent.snapshot().is_none());
    }

    #[test]
    fn a_refusal_is_both_in_the_record_and_in_the_log() {
        let (reporter, written, reader) = reporter();
        reporter.refused(3, "Studio+ will not run at 96000 Hz, so neither will the aggregate");
        let seen = reader.read().expect("a settled record");
        assert_eq!(seen.driver.refusal.get(), "Studio+ will not run at 96000 Hz, so neither will the aggregate");
        assert_eq!(seen.driver.refused_generation, 3);
        assert_eq!(
            written.lines(),
            vec!["2026-09-20 21:14:07 refused Studio+ will not run at 96000 Hz, so neither will the aggregate"]
        );
    }

    #[test]
    fn a_session_starting_and_ending_is_one_line_each_and_leaves_the_record_honest() {
        let (reporter, written, reader) = reporter();
        reporter.update(|area| area.device_count = 2);
        reporter.session(true, 185_895_584_792_000, "40 in, 40 out at 96000 Hz");
        let seen = reader.read().unwrap();
        assert_eq!(seen.driver.streaming, 1);
        assert_eq!(seen.driver.session_nanos, 185_895_584_792_000);
        assert!(seen.devices().iter().all(|d| d.streaming == 1));

        reporter.session(false, 0, "");
        let seen = reader.read().unwrap();
        assert_eq!(seen.driver.streaming, 0);
        assert_eq!(seen.driver.session_nanos, 0, "there is no session to be in");
        assert!(seen.devices().iter().all(|d| d.streaming == 0 && d.gap == 0));
        assert_eq!(written.lines().len(), 2);
        assert!(written.lines()[0].contains("session-started 40 in, 40 out at 96000 Hz"));
        assert_eq!(written.lines()[1], "2026-09-20 21:14:07 session-ended");
    }

    #[test]
    fn nothing_is_written_to_the_log_on_a_timer() {
        let (reporter, written, _reader) = reporter();
        // A thousand blocks' worth of the cheap update, which is what the audio path does.
        for block in 0..1000u64 {
            reporter.try_update(|area| area.callbacks = block);
        }
        reporter.buffers(true, 40, 40);
        reporter.plan_in_force(&crate::plan::Plan::empty_for_tests(), 96_000.0, "a test", 0);
        assert!(written.lines().is_empty(), "the log only holds things that happened");
    }

    #[test]
    fn a_stall_is_noticed_once_and_so_is_coming_back() {
        let mut watch = StallWatch::new();
        assert_eq!(watch.changes(&[false, false]), vec![], "nothing has happened");
        assert_eq!(watch.changes(&[false, true]), vec![(1, true)]);
        assert_eq!(watch.changes(&[false, true]), vec![], "still stalled is not news");
        assert_eq!(watch.changes(&[false, false]), vec![(1, false)]);
        // A new plan with a different number of devices starts the counting again.
        watch.clear();
        assert_eq!(watch.changes(&[true]), vec![(0, true)], "a device already gone when we start looking");
    }

    #[test]
    fn a_stall_and_its_recovery_are_a_line_each() {
        let (reporter, written, _reader) = reporter();
        reporter.stall_changed("Studio+", true);
        reporter.stall_changed("Studio+", false);
        assert_eq!(
            written.lines(),
            vec!["2026-09-20 21:14:07 stalled Studio+", "2026-09-20 21:14:07 recovered Studio+"]
        );
    }

    #[test]
    fn letting_the_buffers_go_is_not_a_driver_that_is_still_streaming() {
        let (reporter, _written, reader) = reporter();
        reporter.buffers(true, 40, 40);
        reporter.update(|area| area.streaming = 1);
        reporter.buffers(false, 0, 0);
        let seen = reader.read().unwrap();
        assert_eq!(seen.driver.open, 0);
        assert_eq!(seen.driver.streaming, 0);
    }
}
