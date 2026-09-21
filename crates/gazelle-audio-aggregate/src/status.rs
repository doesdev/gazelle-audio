//! What the driver tells the outside world about itself, and what it keeps for afterwards.
//!
//! Two things, and they are on purpose not the same thing.
//!
//! - **Live state** goes into the shared record from `gazelle-audio-aggregate-status`. The audio
//!   path writes the cheap fields every block; everything else is written when it changes. Nothing
//!   here allocates, locks or opens a file on a callback thread.
//! - **Durable events** go into a small log file, and only when something happens: a refusal, a
//!   device stalling and coming back, the first block a session lost, a session starting and
//!   ending. Never on a timer, so the file stays small and every line in it means something. It is
//!   the half that survives the driver exiting, which is exactly when a person wants to know why
//!   last night's session would not start.
//!
//! **Blocks lost are counted on the audio path and written here from somewhere else.** The rings
//! count a dropped or missing block with one relaxed add; turning that count into a line of the log
//! is the watcher thread's work ([`GlitchWatch`]), and the totals for a whole session are written
//! when the session ends, on the thread the DAW stopped it from. Nothing below ever writes a line
//! from a callback.
//!
//! **Reporting is never a reason to fail.** A [`Reporter`] with no section and no log is a
//! [`Reporter::silent`], every call on it does nothing, and the driver behaves exactly as it does
//! with Gazelle closed, which is the state it has to work in anyway.

use std::sync::Mutex;

use gazelle_audio_aggregate_status::events::{self, Event};
use gazelle_audio_aggregate_status::record::{alignment as codes, phase as phase_codes, DriverArea, Line, Name};
use gazelle_audio_aggregate_status::Publisher;

use crate::config::Alignment;
use crate::plan::Plan;

/// What one interface lost while a session ran.
///
/// Both numbers are read out of that device's rings, which the audio path counts into and nothing
/// else writes. They are the difference between a clean night and a bad one, and until a session
/// ends they are only in the shared record, where they go the moment the driver exits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Glitches {
    /// What the configuration file calls the interface.
    pub device: String,
    /// Blocks thrown away because this device's ring was full.
    pub dropped: u64,
    /// Blocks that were not there when they were wanted, which is what a person hears as a click.
    pub starved: u64,
}

impl Glitches {
    /// Blocks this interface lost, either way.
    pub fn lost(&self) -> u64 {
        self.dropped + self.starved
    }

    pub fn is_clean(&self) -> bool {
        self.lost() == 0
    }
}

/// What was lost between two readings of the counters, device by device.
///
/// The rings count from the moment the buffers were made, and a DAW may stop and start the audio
/// several times without letting them go. What a session lost is the difference, so that the
/// second session of an evening is not blamed for the first one's dropouts.
pub fn lost_since(before: &[Glitches], now: &[Glitches]) -> Vec<Glitches> {
    now.iter()
        .enumerate()
        .map(|(index, glitch)| {
            let was = before.get(index);
            Glitches {
                device: glitch.device.clone(),
                dropped: glitch.dropped.saturating_sub(was.map_or(0, |had| had.dropped)),
                starved: glitch.starved.saturating_sub(was.map_or(0, |had| had.starved)),
            }
        })
        .collect()
}

/// `n` blocks, said the way a person says it.
fn blocks(count: u64) -> String {
    match count {
        0 => "no blocks".to_string(),
        1 => "1 block".to_string(),
        many => format!("{many} blocks"),
    }
}

fn counted(count: u64, thing: &str) -> String {
    if count == 1 {
        format!("1 {thing}")
    } else {
        format!("{count} {thing}s")
    }
}

/// How long something ran, in the words a person would use. The two largest units it has, because
/// nobody reading a log at nine in the morning wants a session measured in seconds.
pub fn spoken_length(seconds: f64) -> String {
    if !(seconds.is_finite() && seconds >= 1.0) {
        return "less than a second".to_string();
    }
    let whole = seconds as u64;
    let (hours, minutes, rest) = (whole / 3600, (whole % 3600) / 60, whole % 60);
    let mut said = Vec::new();
    if hours > 0 {
        said.push(counted(hours, "hour"));
    }
    if minutes > 0 {
        said.push(counted(minutes, "minute"));
    }
    if rest > 0 && hours == 0 {
        said.push(counted(rest, "second"));
    }
    said.join(" ")
}

/// The line a session leaves behind it: how long it ran, and what each interface lost.
///
/// This is the whole point of writing anything when a session ends. The counters are only in the
/// shared record while the DAW has the driver open, so without this line a night of dropouts and a
/// clean night look exactly the same the next morning.
pub fn ended_detail(seconds: f64, glitches: &[Glitches]) -> String {
    let ran = format!("ran for {}", spoken_length(seconds));
    if glitches.is_empty() {
        return ran;
    }
    if glitches.iter().all(Glitches::is_clean) {
        return format!("{ran}, and no interface lost a block");
    }
    let each: Vec<String> = glitches
        .iter()
        .map(|glitch| {
            if glitch.is_clean() {
                format!("{} lost nothing", glitch.device)
            } else {
                format!("{} dropped {} and missed {}", glitch.device, blocks(glitch.dropped), blocks(glitch.starved))
            }
        })
        .collect();
    format!("{ran}. {}", each.join("; "))
}

/// The line the first lost block of a session leaves, which is the moment a person usually wants:
/// what they were doing when the audio first went wrong.
pub fn first_glitch_detail(device: &str, dropped: u64, starved: u64) -> String {
    if dropped > 0 && starved > 0 {
        format!("{device} dropped a block and missed another, the first this session has lost")
    } else if dropped > 0 {
        format!("{device} dropped a block, the first this session has lost: it was handing them over faster than they could be taken")
    } else {
        format!("{device} had no block ready, the first this session has missed, which is what a person hears as a click")
    }
}

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
                // Nothing has been measured yet, and whether anything will be is what the file
                // asked for. A session that lines nothing up measures nothing either.
                into.phase_state = if device.phase.is_some() && plan.alignment == Alignment::Aligned {
                    phase_codes::MEASURING
                } else {
                    phase_codes::NOT_CONFIGURED
                };
                into.phase_measured = 0;
                into.phase_applied = 0;
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

    /// Audio has stopped, and this is what the session came to: how long it ran, and what each
    /// interface lost while it did. Called from the thread the DAW stopped the driver on.
    pub fn session_ended(&self, seconds: f64, glitches: &[Glitches]) {
        self.session(false, 0, &ended_detail(seconds, glitches));
    }

    /// The first block a device lost in this session. Called from the watcher thread, never the
    /// audio one: the audio path only counts, and only the first is ever written, because a line
    /// per lost block would bury the file at the moment somebody needs to read it.
    pub fn first_glitch(&self, device: &str, dropped: u64, starved: u64) {
        self.note(Event::Glitched, &first_glitch_detail(device, dropped, starved));
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

    /// **What a session made of one interface's phase**: what was measured, what was applied, or
    /// why nothing was. Called from the watcher thread, never the audio one, which only measures
    /// and writes three numbers.
    ///
    /// This is the line that matters most of the ones here. The measurement is a different number
    /// every session and the record holds it only while the DAW has the driver open, so without
    /// this line there is nothing the next morning to say why two interfaces landed where they did.
    pub fn phase_settled(&self, device: &str, state: u32, measured: i32, applied: i32) {
        self.note(Event::Phase, &crate::phase::detail(device, state, measured, applied));
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

/// Which devices have just lost their first block of a session.
///
/// The same division of labour as [`StallWatch`]. The audio path counts a dropped or missing block
/// with one add and knows nothing about logs; this reads those counts on the watcher's thread and
/// decides whether a line is worth writing. Only the first is, per device per session: the totals
/// belong to the session's own line, and a busy log is a useless one.
#[derive(Default)]
pub struct GlitchWatch {
    /// What each device had lost at the last look. Zero means it has lost nothing yet, in this
    /// session or the next one.
    seen: Vec<u64>,
}

impl GlitchWatch {
    pub fn new() -> GlitchWatch {
        GlitchWatch::default()
    }

    /// The devices whose first lost block has just happened, with what they lost, from each
    /// device's dropped and missing counts as they stand now.
    pub fn first(&mut self, now: &[(u64, u64)]) -> Vec<(usize, u64, u64)> {
        if self.seen.len() != now.len() {
            self.seen = vec![0; now.len()];
        }
        let mut news = Vec::new();
        for (index, &(dropped, starved)) in now.iter().enumerate() {
            let lost = dropped + starved;
            // Counts that have gone back to nothing are a new session, whose first lost block is
            // news again.
            if lost == 0 {
                self.seen[index] = 0;
                continue;
            }
            if self.seen[index] == 0 {
                news.push((index, dropped, starved));
            }
            self.seen[index] = lost;
        }
        news
    }

    /// Forget everything, which is what a new plan is.
    pub fn clear(&mut self) {
        self.seen.clear();
    }
}

/// Which interfaces' phase measurements have just settled.
///
/// The same division of labour as [`StallWatch`] once more. The audio path measures and writes
/// three numbers into the record; deciding that one of them is worth a line of the log is this, on
/// the watcher's thread. One line per interface per session: the state only changes when a
/// measurement finishes, and a session starting again puts it back to measuring, which is news of
/// its own the next time it settles.
#[derive(Default)]
pub struct PhaseWatch {
    /// What each device's state was at the last look.
    seen: Vec<u32>,
}

impl PhaseWatch {
    pub fn new() -> PhaseWatch {
        PhaseWatch::default()
    }

    /// The devices whose measurement has just come to something, and what it came to. A device
    /// still measuring, or with nothing set up, is not news.
    pub fn settled(&mut self, now: &[u32]) -> Vec<usize> {
        if self.seen.len() != now.len() {
            self.seen = vec![phase_codes::NOT_CONFIGURED; now.len()];
        }
        let mut news = Vec::new();
        for (index, &state) in now.iter().enumerate() {
            let changed = self.seen[index] != state;
            self.seen[index] = state;
            if changed && (phase_codes::is_applied(state) || phase_codes::is_refused(state)) {
                news.push(index);
            }
        }
        news
    }

    /// Forget everything, which is what a new plan is.
    pub fn clear(&mut self) {
        self.seen.clear();
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

    fn glitch(device: &str, dropped: u64, starved: u64) -> Glitches {
        Glitches { device: device.to_string(), dropped, starved }
    }

    #[test]
    fn a_session_that_lost_nothing_says_so_in_one_line() {
        let (reporter, written, _reader) = reporter();
        reporter.session_ended(2_832.0, &[glitch("Quadro", 0, 0), glitch("Studio+", 0, 0)]);
        assert_eq!(
            written.lines(),
            vec!["2026-09-20 21:14:07 session-ended ran for 47 minutes 12 seconds, and no interface lost a block"]
        );
    }

    #[test]
    fn a_session_that_lost_blocks_names_the_interface_and_says_how_many() {
        let (reporter, written, _reader) = reporter();
        reporter.session_ended(2_832.0, &[glitch("Quadro", 0, 0), glitch("Studio+", 3, 1)]);
        let line = &written.lines()[0];
        assert!(line.contains("session-ended ran for 47 minutes 12 seconds."), "{line}");
        assert!(line.contains("Quadro lost nothing"), "{line}");
        assert!(line.contains("Studio+ dropped 3 blocks and missed 1 block"), "{line}");
    }

    #[test]
    fn a_second_session_is_not_blamed_for_what_the_first_one_lost() {
        // A DAW that stops and starts again keeps the same rings, and their counts with them.
        let at_the_start = vec![glitch("Quadro", 0, 0), glitch("Studio+", 3, 1)];
        let now = vec![glitch("Quadro", 0, 0), glitch("Studio+", 5, 1)];
        assert_eq!(lost_since(&at_the_start, &now), vec![glitch("Quadro", 0, 0), glitch("Studio+", 2, 0)]);
        // Counters that went backwards, which is a new set of buffers, read as nothing lost rather
        // than as an enormous number.
        assert_eq!(lost_since(&now, &at_the_start)[1], glitch("Studio+", 0, 0));
        assert_eq!(lost_since(&[], &now), now, "and a session with nothing to compare with is its own whole count");
    }

    #[test]
    fn how_long_a_session_ran_is_said_the_way_a_person_says_it() {
        assert_eq!(spoken_length(0.4), "less than a second");
        assert_eq!(spoken_length(1.0), "1 second");
        assert_eq!(spoken_length(42.9), "42 seconds");
        assert_eq!(spoken_length(60.0), "1 minute");
        assert_eq!(spoken_length(3_600.0), "1 hour");
        assert_eq!(spoken_length(7_512.0), "2 hours 5 minutes");
        assert_eq!(spoken_length(f64::NAN), "less than a second");
        assert_eq!(spoken_length(-5.0), "less than a second");
    }

    #[test]
    fn the_first_block_a_session_loses_is_one_line_and_the_ones_after_it_are_none() {
        let mut watch = GlitchWatch::new();
        assert_eq!(watch.first(&[(0, 0), (0, 0)]), vec![], "a session that has lost nothing is not news");
        assert_eq!(watch.first(&[(0, 0), (1, 0)]), vec![(1, 1, 0)]);
        assert_eq!(watch.first(&[(0, 0), (9, 4)]), vec![], "losing more of them is the same news");
        // The counters go back to nothing when a new plan is in force, and the next session's
        // first lost block is worth saying again.
        assert_eq!(watch.first(&[(0, 0), (0, 0)]), vec![]);
        assert_eq!(watch.first(&[(0, 0), (0, 2)]), vec![(1, 0, 2)]);
    }

    #[test]
    fn the_first_lost_block_says_which_of_the_two_things_happened() {
        let (reporter, written, _reader) = reporter();
        reporter.first_glitch("Studio+", 1, 0);
        reporter.first_glitch("Studio+", 0, 1);
        reporter.first_glitch("Studio+", 1, 1);
        let lines = written.of("glitched");
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("Studio+ dropped a block, the first this session has lost"), "{}", lines[0]);
        assert!(lines[1].contains("had no block ready"), "{}", lines[1]);
        assert!(lines[1].contains("click"), "it says what a person hears: {}", lines[1]);
        assert!(lines[2].contains("dropped a block and missed another"), "{}", lines[2]);
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
