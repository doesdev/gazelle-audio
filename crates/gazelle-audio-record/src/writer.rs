//! **The writer**: drains a take out of the capture ring into one Broadcast WAV per channel, and
//! keeps the take's log beside them.
//!
//! It runs on a thread of its own and never on the audio thread. Everything it does is a step
//! ([`Drain::step`]) that a test can call itself, block by block, with no thread at all.
//!
//! # What goes in the log
//!
//! A text file beside the take's files, named by the same pattern with `log` for the channel: the
//! preset, when the take started (date, time, and the `bext` TimeReference every file carries), its
//! rate and format, its files, how much of it is pre-roll, and then anything that happened to it:
//! **audio the writer lost because it fell behind**, with where in the take and how much, each filled
//! with silence so the files stay lined up; **audio the aggregate lost**, interface by interface
//! (what its own counters say it dropped or was starved of); a disk running low; whether the
//! alignment between the interfaces held, from the checks over the phase cable that went out during
//! it ([`crate::alignment`]), or why it was not checked; and why the take ended.
//!
//! # The disk
//!
//! Free space is checked before Record ([`room_to_record`]) and every second while a take is being
//! written. Below ten minutes of room it is a warning; below [`stop_reserve`] the take is stopped the
//! ordinary way and its files finished, while there is still room to finish them. A disk that fails
//! a write anyway stops the take too, and the files keep everything up to the last time their sizes
//! were written (`crate::wav`).
//!
//! # Into Cubase
//!
//! When this PC has a Cubase seed set ([`TakeSettings::cubase_seed`]), a finished take gets a track
//! archive beside its files, `<take> Cubase.xml`, made from the seed ([`crate::cubase`]). It is
//! written whole or not at all, and never over a file that is there. Anything that goes wrong making
//! it is a line in the take's log: the take itself is already finished by then and is never failed
//! by it.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gazelle_aggregate::status::Glitches;
use serde::Serialize;

use crate::alignment::{self, Check, SharedBoard};
use crate::capture::{Capture, Window, PARKED};
use crate::cubase::{self, TakeFile};
use crate::names::{self, TakeWords};
use crate::system::{Civil, Clock, Disk, FileOut};
use crate::wav::{Bext, SampleFormat, WavWriter, RIFF_LIMIT};

/// What the cue on the downbeat after a count-in is called.
pub const CUE_LABEL: &str = "Downbeat";
/// Below this many seconds of room left the page warns.
pub const WARN_SECONDS: f64 = 600.0;
/// The least room a take is stopped with, whatever its rate: enough to finish every file.
pub const STOP_RESERVE_BYTES: u64 = 256 << 20;
/// How many takes are remembered for the page.
pub const RECENT_TAKES: usize = 20;
/// How many blocks one step writes before it looks up again.
const BLOCKS_A_STEP: usize = 512;

/// The room a take is stopped with: 256 MB, or ten seconds of it if that is more.
pub fn stop_reserve(bytes_per_second: f64) -> u64 {
    STOP_RESERVE_BYTES.max((bytes_per_second * 10.0) as u64)
}

/// Whether there is room to start a take: its reserve, and a minute on top.
pub fn room_to_record(free: Option<u64>, bytes_per_second: f64) -> Result<(), String> {
    let Some(free) = free else { return Ok(()) };
    let needed = stop_reserve(bytes_per_second) + (bytes_per_second * 60.0) as u64;
    if free < needed {
        return Err(format!("only {} MB is free on that disk, which is not a minute of this take: make room, or choose another folder", free >> 20));
    }
    Ok(())
}

/// How a take is written, fixed at Arm.
#[derive(Clone, Debug)]
pub struct TakeSettings {
    pub folder: PathBuf,
    pub pattern: String,
    pub preset: String,
    pub format: SampleFormat,
    pub rate: f64,
    /// One per recorded channel, as the aggregate names it.
    pub channels: Vec<String>,
    /// What made the files, for `bext` and the log: "Gazelle 1.5.0".
    pub originator: String,
    /// The RIFF size past which a file becomes RF64. 4 GB except in a test.
    pub rf64_limit: u64,
    /// The Cubase track archive each take's archive is made from, when this PC has one set. Read
    /// when a take finishes, so a seed set while armed counts from the next take.
    pub cubase_seed: SeedSlot,
}

/// Where this PC's Cubase seed is, shared by the recorder and every take it writes.
pub type SeedSlot = Arc<Mutex<Option<PathBuf>>>;

impl TakeSettings {
    /// Bytes a second, across every file.
    pub fn bytes_per_second(&self) -> f64 {
        self.rate * self.channels.len() as f64 * f64::from(self.format.bytes())
    }
}

/// The take being written, as the page shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TakeNow {
    pub number: u32,
    pub folder: String,
    pub files: Vec<String>,
    pub date: String,
    pub time: String,
}

/// A take that has been written.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TakeRecord {
    pub number: u32,
    pub preset: String,
    pub folder: String,
    /// The WAV files, whole paths, in the preset's channel order.
    pub files: Vec<String>,
    pub log: String,
    /// The take's Cubase track archive, when one was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cubase: Option<String>,
    pub date: String,
    pub time: String,
    /// How long it is, pre-roll and all.
    pub seconds: f64,
    /// How much of that was pre-roll.
    pub preroll_seconds: f64,
    /// Blocks the writer lost because it fell behind, each filled with silence.
    pub overruns: u64,
    /// Blocks the aggregate lost while the take was running, every interface together.
    pub dropouts: u64,
    /// After a count-in, where the downbeat after it is, in seconds into the take. Every file marks
    /// it with a cue named "Downbeat".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downbeat_seconds: Option<f64>,
    /// The alignment between the interfaces moved during it, by the checks over the phase cable. Its
    /// log says when and by how much.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub alignment_slipped: bool,
    /// Why Gazelle ended it, when it was not a person pressing Stop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_by: Option<String>,
    /// What went wrong writing it, when something did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// What the writer says, for the page. Read and written off the audio thread only.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WriterState {
    pub take: Option<TakeNow>,
    /// Newest first.
    pub takes: Vec<TakeRecord>,
    /// Free space on the take's disk, when last looked at.
    pub disk_free_bytes: Option<u64>,
    /// How long a take at this rate could run in that.
    pub disk_seconds_left: Option<f64>,
    /// The disk is getting full.
    pub disk_low: bool,
    /// The last thing that went wrong, for the page to say.
    pub problem: Option<String>,
}

/// One file of the open take: writing, or given up on after an error.
type File = Option<WavWriter<Box<dyn FileOut>>>;

struct Open {
    number: u32,
    window: Window,
    files: Vec<File>,
    paths: Vec<PathBuf>,
    log: Option<Box<dyn FileOut>>,
    log_path: PathBuf,
    /// Where the take's Cubase track archive goes, if one is made.
    cubase_path: PathBuf,
    date: String,
    time: String,
    lost_blocks: u64,
    /// A run of lost blocks not yet written to the log: where it started and how long it is.
    losing: Option<(u64, u64)>,
    problem: Option<String>,
    stopped_by: Option<String>,
    dropouts_at_start: Vec<Glitches>,
    /// The alignment checks that went out during it, oldest first.
    checks: Vec<Check>,
    /// Nothing is written any more: the take could not be opened, or a file failed.
    skipping: bool,
}

/// The writer.
pub struct Drain {
    settings: TakeSettings,
    disk: Arc<dyn Disk>,
    clock: Arc<dyn Clock>,
    /// What the host thread last read of the aggregate's own counters.
    dropouts: Arc<Mutex<Vec<Glitches>>>,
    state: Arc<Mutex<WriterState>>,
    open: Option<Open>,
    next_take: u32,
    last_look: Option<Instant>,
    /// The alignment checks, when the recorder is keeping track of them, and the ones taken from it
    /// that no take has claimed yet.
    alignment: Option<SharedBoard>,
    checks: std::collections::VecDeque<Check>,
}

impl Drain {
    pub fn new(settings: TakeSettings, disk: Arc<dyn Disk>, clock: Arc<dyn Clock>, dropouts: Arc<Mutex<Vec<Glitches>>>) -> Drain {
        Drain {
            settings,
            disk,
            clock,
            dropouts,
            state: Arc::new(Mutex::new(WriterState::default())),
            open: None,
            next_take: 1,
            last_look: None,
            alignment: None,
            checks: std::collections::VecDeque::new(),
        }
    }

    /// Say in each take's log whether the alignment held, from the checks on `board`.
    pub fn with_alignment(mut self, board: SharedBoard) -> Drain {
        self.alignment = Some(board);
        self
    }

    /// Take the checks decided since the last look, and give the open take those that went out
    /// during it, up to `upto`. With no take open, only the checks a take could still reach back to
    /// are kept.
    fn claim_checks(&mut self, capture: &Capture, upto: u64) {
        let Some(board) = &self.alignment else { return };
        if let Ok(mut board) = board.lock() {
            self.checks.extend(board.take_fresh());
        }
        match self.open.as_mut() {
            Some(open) => {
                while let Some(check) = self.checks.front().copied() {
                    if check.at >= upto {
                        break;
                    }
                    self.checks.pop_front();
                    if check.at >= open.window.start {
                        open.checks.push(check);
                    }
                }
            }
            None => {
                let reach = capture.written().saturating_sub(capture.capacity_frames());
                while self.checks.front().is_some_and(|check| check.at < reach) {
                    self.checks.pop_front();
                }
            }
        }
    }

    /// What the page reads, shared.
    pub fn state(&self) -> Arc<Mutex<WriterState>> {
        Arc::clone(&self.state)
    }

    /// Whether a take is open.
    pub fn is_writing(&self) -> bool {
        self.open.is_some()
    }

    fn with_state(&self, change: impl FnOnce(&mut WriterState)) {
        if let Ok(mut state) = self.state.lock() {
            change(&mut state);
        }
    }

    /// **One step**: open a take the callback has started, write what has arrived, look at the disk,
    /// and finish the take once the callback has ended it. Answers whether it did anything, so the
    /// thread knows whether to wait.
    pub fn step(&mut self, capture: &Capture) -> bool {
        let Some(window) = capture.take() else {
            self.claim_checks(capture, u64::MAX);
            self.look_at_disk(capture, false);
            return false;
        };
        if self.open.is_none() {
            self.open_take(window, capture);
        }
        let block = capture.block() as u64;
        let end = capture.take_end();
        self.claim_checks(capture, end);
        let limit = end.min(capture.written());
        let mut at = capture.read_position();
        let mut wrote = 0;
        while at < limit && wrote < BLOCKS_A_STEP {
            self.write_block(capture, at);
            at += block;
            capture.consumed(at);
            wrote += 1;
        }
        self.look_at_disk(capture, true);
        if end != PARKED && at >= end {
            self.finish_take(capture, end);
            capture.park();
            return true;
        }
        wrote > 0
    }

    fn write_block(&mut self, capture: &Capture, at: u64) {
        let block = capture.block();
        let Some(open) = self.open.as_mut() else { return };
        let arrived = capture.block_at(at);
        if arrived.is_none() {
            open.lost_blocks += 1;
            open.losing = Some(match open.losing {
                Some((from, frames)) => (from, frames + block as u64),
                None => (at, block as u64),
            });
        } else if let Some((from, frames)) = open.losing.take() {
            let line = lost_line(open.window.start, from, frames, self.settings.rate);
            log(open, &line);
        }
        if open.skipping {
            return;
        }
        let mut failed = None;
        for (channel, file) in open.files.iter_mut().enumerate() {
            let Some(writer) = file else { continue };
            let wrote = match arrived {
                Some(samples) => writer.write(&samples[channel * block..(channel + 1) * block]),
                None => writer.write_silence(block as u64),
            };
            if let Err(why) = wrote {
                failed = Some(format!("{} could not be written: {why}", open.paths[channel].display()));
                break;
            }
        }
        if let Some(why) = failed {
            self.give_up(capture, why);
        }
    }

    /// A take that cannot go on: stop it, keep what was written, and say why.
    fn give_up(&mut self, capture: &Capture, why: String) {
        capture.want_recording(false);
        let Some(open) = self.open.as_mut() else { return };
        log(open, &format!("Stopped: {why}. What was written before it is kept."));
        open.problem.get_or_insert(why.clone());
        open.stopped_by.get_or_insert_with(|| "a file could not be written".into());
        open.skipping = true;
        self.with_state(|state| state.problem = Some(why));
    }

    fn open_take(&mut self, window: Window, capture: &Capture) {
        let settings = &self.settings;
        // When the take's first sample was, from where the stream is now.
        let behind = capture.written().saturating_sub(window.start) as f64 / settings.rate;
        let started = self.clock.now().checked_sub(Duration::from_secs_f64(behind)).unwrap_or_else(|| self.clock.now());
        let civil: Civil = self.clock.civil(started);
        let date = civil.date();
        let time = civil.time();
        let time_words = time.replace(':', "-");
        let mut words = TakeWords { preset: &settings.preset, take: self.next_take, date: &date, time: &time_words };
        let disk = Arc::clone(&self.disk);
        let dropouts_at_start = self.dropouts.lock().map(|d| d.clone()).unwrap_or_default();
        let mut open = Open {
            number: 0,
            window,
            files: Vec::new(),
            paths: Vec::new(),
            log: None,
            log_path: PathBuf::new(),
            cubase_path: PathBuf::new(),
            date: date.clone(),
            time: time.clone(),
            lost_blocks: 0,
            losing: None,
            problem: None,
            stopped_by: None,
            dropouts_at_start,
            checks: Vec::new(),
            skipping: false,
        };
        let made = disk
            .create_dir_all(&settings.folder)
            .map_err(|why| format!("the folder {} could not be made: {why}", settings.folder.display()))
            .and_then(|()| names::next_take(&settings.folder, &settings.pattern, &mut words, &settings.channels, self.next_take, &|path| disk.exists(path)));
        let (number, paths, log_path) = match made {
            Ok(found) => found,
            Err(why) => {
                self.open = Some(open);
                self.give_up(capture, why);
                return;
            }
        };
        self.next_take = number + 1;
        open.number = number;
        open.log_path = log_path.clone();
        open.cubase_path = names::cubase_path(&settings.folder, &settings.pattern, &words);
        open.paths = paths.clone();
        let time_reference = (civil.since_midnight() * settings.rate).round() as u64;
        let rate = settings.rate.round() as u32;
        let history = match settings.format {
            SampleFormat::Int24 => format!("A=PCM,F={rate},W=24,M=mono,T={}\r\n", settings.originator),
            SampleFormat::Float32 => format!("A=PCM,F={rate},W=32,M=mono,T={}; IEEE float\r\n", settings.originator),
        };
        let mut problem = None;
        for (channel, path) in paths.iter().enumerate() {
            let bext = Bext {
                description: settings.channels[channel].clone(),
                originator: "Gazelle".into(),
                originator_reference: format!("Gazelle T{number:03}"),
                origination_date: date.clone(),
                origination_time: time.clone(),
                time_reference,
                coding_history: history.clone(),
            };
            let file = disk
                .create_new(path)
                .and_then(|out| WavWriter::create(out, settings.format, rate, &bext))
                .map(|writer| writer.with_limit(settings.rf64_limit));
            match file {
                Ok(writer) => open.files.push(Some(writer)),
                Err(why) => {
                    problem.get_or_insert(format!("{} could not be made: {why}", path.display()));
                    open.files.push(None);
                }
            }
        }
        open.log = disk.create_new(&log_path).ok();
        let preroll = (window.pressed_at - window.start) as f64 / settings.rate;
        let mut head = vec![
            format!("{} take {number:03}", settings.originator),
            format!("Preset: {}", settings.preset),
            format!("Started: {date} {time}, TimeReference {time_reference} samples since midnight"),
            format!("Rate: {rate} Hz, {}", settings.format.words()),
            format!("Pre-roll: {preroll:.2} s before Record was pressed"),
            "Files:".to_string(),
        ];
        head.extend(paths.iter().map(|path| format!("  {}", path.display())));
        for line in head {
            log(&mut open, &line);
        }
        let now = TakeNow {
            number,
            folder: settings.folder.display().to_string(),
            files: paths.iter().map(|p| p.display().to_string()).collect(),
            date: date.clone(),
            time: time.clone(),
        };
        self.open = Some(open);
        self.with_state(|state| {
            state.take = Some(now);
            state.problem = None;
        });
        if let Some(why) = problem {
            self.give_up(capture, why);
        }
    }

    fn finish_take(&mut self, capture: &Capture, end: u64) {
        let Some(mut open) = self.open.take() else { return };
        let rate = self.settings.rate;
        if let Some((from, frames)) = open.losing.take() {
            let line = lost_line(open.window.start, from, frames, rate);
            log(&mut open, &line);
        }
        let start = open.window.start;
        let cue = open.window.cue.filter(|&cue| cue >= start && cue < end).map(|cue| cue - start);
        if let Some(at) = cue {
            let line = format!("Count-in: the downbeat after it is sample {at} of the take ({}), marked in every file with a cue named {CUE_LABEL}.", clock_words(at as f64 / rate));
            log(&mut open, &line);
        }
        // What the Cubase archive needs of each file that was made: its length, and where its audio starts.
        let mut made = Vec::with_capacity(open.files.len());
        for (channel, file) in open.files.iter_mut().enumerate() {
            if let Some(writer) = file.take() {
                let layout = (channel, writer.frames(), writer.data_start());
                match writer.finish_with_cue(cue.map(|at| (at, CUE_LABEL))) {
                    Ok(_) => made.push(layout),
                    Err(why) => {
                        let why = format!("{} could not be finished: {why}", open.paths[channel].display());
                        open.problem.get_or_insert(why);
                    }
                }
            }
        }
        let now = self.dropouts.lock().map(|d| d.clone()).unwrap_or_default();
        let lost = gazelle_aggregate::status::lost_since(&open.dropouts_at_start, &now);
        let dropouts: u64 = lost.iter().map(Glitches::lost).sum();
        for device in lost.iter().filter(|device| !device.is_clean()) {
            let line = format!(
                "The aggregate lost audio on {}: {} blocks dropped, {} starved while this take ran. Those blocks are silence in the take.",
                device.device, device.dropped, device.starved
            );
            log(&mut open, &line);
        }
        let alignment_slipped = self.alignment.is_some() && alignment::slipped(&open.checks);
        if let Some(board) = &self.alignment {
            let lines = board.lock().map(|board| alignment::take_lines(&board, &open.checks, start, rate)).unwrap_or_default();
            for line in lines {
                log(&mut open, &line);
            }
        }
        let seconds = end.saturating_sub(open.window.start) as f64 / rate;
        let summary = format!(
            "Ended after {seconds:.3} s ({} samples). {}",
            end.saturating_sub(open.window.start),
            match (open.lost_blocks, dropouts) {
                (0, 0) => "Nothing was lost.".to_string(),
                (writer, aggregate) => format!("The writer lost {writer} blocks and the aggregate {aggregate}."),
            }
        );
        log(&mut open, &summary);
        if let Some(why) = &open.stopped_by {
            let line = format!("Stopped by Gazelle: {why}.");
            log(&mut open, &line);
        }
        let mut cubase = None;
        if let Some((line, written)) = self.write_cubase(&open, &made) {
            log(&mut open, &line);
            cubase = written.then(|| open.cubase_path.display().to_string());
        }
        if let Some(mut out) = open.log.take() {
            let _ = out.flush();
        }
        let record = TakeRecord {
            number: open.number,
            preset: self.settings.preset.clone(),
            folder: self.settings.folder.display().to_string(),
            files: open.paths.iter().map(|p| p.display().to_string()).collect(),
            log: open.log_path.display().to_string(),
            cubase,
            date: open.date,
            time: open.time,
            seconds,
            preroll_seconds: (open.window.pressed_at - open.window.start) as f64 / rate,
            overruns: open.lost_blocks,
            dropouts,
            downbeat_seconds: cue.map(|at| at as f64 / rate),
            alignment_slipped,
            stopped_by: open.stopped_by,
            problem: open.problem.clone(),
        };
        let _ = capture;
        self.with_state(|state| {
            state.take = None;
            state.takes.insert(0, record);
            state.takes.truncate(RECENT_TAKES);
            if let Some(problem) = open.problem {
                state.problem = Some(problem);
            }
        });
    }

    /// The take's Cubase track archive, when this PC has a seed: the log's line about it and whether
    /// it was written, or nothing when there is no seed. Never fails the take.
    fn write_cubase(&self, open: &Open, made: &[(usize, u64, u64)]) -> Option<(String, bool)> {
        let seed = self.settings.cubase_seed.lock().ok().and_then(|seed| seed.clone())?;
        let settings = &self.settings;
        let files: Vec<TakeFile> = made
            .iter()
            .map(|&(channel, frames, data_offset)| TakeFile {
                channel: settings.channels[channel].clone(),
                path: open.paths[channel].clone(),
                frames,
                rate: settings.rate,
                format: settings.format,
                data_offset,
            })
            .collect();
        let written = self
            .disk
            .read_file(&seed)
            .map_err(|why| format!("the seed {} could not be read: {why}", seed.display()))
            .and_then(|bytes| String::from_utf8(bytes).map_err(|_| format!("the seed {} is not UTF-8 text", seed.display())))
            .and_then(|text| cubase::build(&text, &files).map_err(|why| format!("it could not be made from the seed {}: {why}", seed.display())))
            .and_then(|xml| self.disk.write_new(&open.cubase_path, xml.as_bytes()).map_err(|why| format!("{} could not be written: {why}", open.cubase_path.display())));
        Some(match written {
            Ok(()) => (format!("Cubase track archive: {} ({} tracks, from the seed {}).", open.cubase_path.display(), files.len(), seed.display()), true),
            Err(why) => (format!("No Cubase track archive for this take: {why}. The take itself is complete."), false),
        })
    }

    /// Free space, once a second: while writing, stop a take before the disk is full.
    fn look_at_disk(&mut self, capture: &Capture, writing: bool) {
        if self.last_look.is_some_and(|last| last.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.last_look = Some(Instant::now());
        let per_second = self.settings.bytes_per_second();
        let free = self.disk.free_bytes(&self.settings.folder);
        let left = free.map(|free| free.saturating_sub(stop_reserve(per_second)) as f64 / per_second.max(1.0));
        let low = left.is_some_and(|left| left < WARN_SECONDS);
        self.with_state(|state| {
            state.disk_free_bytes = free;
            state.disk_seconds_left = left;
            state.disk_low = low;
        });
        if !writing {
            return;
        }
        let Some(free) = free else { return };
        if free < stop_reserve(per_second) {
            if let Some(open) = self.open.as_mut() {
                if open.stopped_by.is_none() && capture.wants_recording() {
                    let why = format!("the disk was nearly full, {} MB left", free >> 20);
                    log(open, &format!("Stopping: {why}. The files are finished while there is room to."));
                    open.stopped_by = Some(why.clone());
                    capture.want_recording(false);
                    self.with_state(|state| state.problem = Some(format!("The take was stopped because {why}.")));
                }
            }
        }
    }
}

/// The log's line for a run of blocks the writer never got.
fn lost_line(start: u64, from: u64, frames: u64, rate: f64) -> String {
    let into = from - start;
    format!(
        "Lost {frames} samples at sample {into} of the take ({}): the writer fell behind. Filled with silence, so the files stay lined up.",
        clock_words(into as f64 / rate)
    )
}

/// `m:ss.mmm`.
pub(crate) fn clock_words(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor();
    format!("{}:{:06.3}", minutes as u64, seconds - minutes * 60.0)
}

/// One line of the take's log, handed to the disk at once: lines are few, and a process ended by
/// force keeps every line written before it rather than a log left empty in a buffer.
fn log(open: &mut Open, line: &str) {
    if let Some(out) = open.log.as_mut() {
        if out.write_all(format!("{line}\r\n").as_bytes()).and_then(|()| out.flush()).is_err() {
            open.log = None;
        }
    }
}

/// The 4 GB a file may reach before it becomes RF64.
pub const RF64_AT: u64 = RIFF_LIMIT;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{MemoryDisk, StoppedClock};
    use std::path::Path;

    const BLOCK: usize = 32;
    const RATE: f64 = 48_000.0;

    fn settings(channels: usize) -> TakeSettings {
        TakeSettings {
            folder: PathBuf::from("C:/Takes"),
            pattern: names::DEFAULT_PATTERN.into(),
            preset: "Band".into(),
            format: SampleFormat::Int24,
            rate: RATE,
            channels: (0..channels).map(|c| format!("In {}", c + 1)).collect(),
            originator: "Gazelle 9.9.9".into(),
            rf64_limit: RF64_AT,
            cubase_seed: SeedSlot::default(),
        }
    }

    /// The ramp, in the top 24 bits so a 24-bit file keeps it whole.
    fn ramp(n: u64, channel: usize) -> i32 {
        ((((n as i64) * 4 + channel as i64) % (1 << 23)) as i32) << 8
    }

    fn push(capture: &Capture, next: &mut u64) {
        let first = *next;
        capture.push_with(|channel, run| {
            for (i, sample) in run.iter_mut().enumerate() {
                *sample = ramp(first + i as u64, channel);
            }
        });
        *next += BLOCK as u64;
    }

    /// The 24-bit samples of a finished file.
    fn samples(bytes: &[u8]) -> Vec<i32> {
        let data = bytes.windows(4).position(|w| w == b"data").expect("a data chunk") + 8;
        let size = u32::from_le_bytes(bytes[data - 4..data].try_into().unwrap()) as usize;
        bytes[data..data + size].chunks(3).map(|b| i32::from_le_bytes([0, b[0], b[1], b[2]])).collect()
    }

    fn time_reference(bytes: &[u8]) -> u64 {
        let at = bytes.windows(4).position(|w| w == b"bext").unwrap() + 8 + 338;
        u64::from(u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())) | (u64::from(u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap())) << 32)
    }

    fn drain_with(disk: &Arc<MemoryDisk>, channels: usize) -> Drain {
        Drain::new(settings(channels), disk.clone(), Arc::new(StoppedClock), Arc::new(Mutex::new(Vec::new())))
    }

    #[test]
    fn a_take_is_one_file_per_channel_every_sample_in_order_from_the_oldest_held() {
        let disk = Arc::new(MemoryDisk::default());
        let mut drain = drain_with(&disk, 2);
        let capture = Capture::allocate(2, BLOCK, 16 * BLOCK as u64, 10 * BLOCK as u64).unwrap();
        let mut next = 0;
        for _ in 0..40 {
            push(&capture, &mut next);
            drain.step(&capture);
        }
        capture.want_recording(true);
        // Across several laps of the ring, draining as a thread would.
        for _ in 0..50 {
            push(&capture, &mut next);
            drain.step(&capture);
        }
        capture.want_recording(false);
        push(&capture, &mut next);
        while drain.step(&capture) {}
        assert!(capture.take().is_none(), "the take is finished and the ring is free for the next");

        let state = drain.state().lock().unwrap().clone();
        let take = &state.takes[0];
        assert_eq!(take.number, 1);
        let named = |name: &str| PathBuf::from("C:/Takes").join(name).display().to_string();
        assert_eq!(take.files, vec![named("2026-09-27 T001 In 1.wav"), named("2026-09-27 T001 In 2.wav")]);
        assert!((take.preroll_seconds - 10.0 * BLOCK as f64 / RATE).abs() < 1e-9);
        assert_eq!(take.overruns, 0);
        for channel in 0..2 {
            let got = samples(&disk.read(Path::new(&take.files[channel])));
            assert_eq!(got.len(), 60 * BLOCK, "ten blocks of pre-roll and fifty live");
            for (i, sample) in got.iter().enumerate() {
                assert_eq!(*sample, ramp(30 * BLOCK as u64 + i as u64, channel), "channel {channel}, sample {i}: continuous across the press");
            }
        }
    }

    /// A short take of `channels` channels on `disk`, with the Cubase seed at `seed`, if any.
    fn take_with_seed(disk: &Arc<MemoryDisk>, channels: usize, seed: Option<&str>) -> TakeRecord {
        let settings = settings(channels);
        *settings.cubase_seed.lock().unwrap() = seed.map(PathBuf::from);
        let mut drain = Drain::new(settings, disk.clone(), Arc::new(StoppedClock), Arc::new(Mutex::new(Vec::new())));
        let capture = Capture::allocate(channels, BLOCK, 16 * BLOCK as u64, 4 * BLOCK as u64).unwrap();
        let mut next = 0;
        capture.want_recording(true);
        for _ in 0..5 {
            push(&capture, &mut next);
            drain.step(&capture);
        }
        capture.want_recording(false);
        push(&capture, &mut next);
        while drain.step(&capture) {}
        let take = drain.state().lock().unwrap().takes[0].clone();
        take
    }

    #[test]
    fn with_a_seed_a_take_gets_a_cubase_track_archive_beside_its_files() {
        let disk = Arc::new(MemoryDisk::default());
        let seed = "C:/Seeds/Recorded.xml";
        disk.write_new(Path::new(seed), include_bytes!("../testdata/cubase-seed.xml")).unwrap();
        let take = take_with_seed(&disk, 3, Some(seed));
        let archive = PathBuf::from("C:/Takes").join("2026-09-27 T001 Cubase.xml");
        assert_eq!(take.cubase.as_deref(), Some(archive.display().to_string().as_str()));
        let xml = String::from_utf8(disk.read(&archive)).unwrap();
        assert_eq!(cubase::check_archive(&xml).unwrap().listed, 3);
        for (channel, file) in take.files.iter().enumerate() {
            let name = Path::new(file).file_name().unwrap().to_str().unwrap();
            assert!(xml.contains(&format!("value=\"{name}\"")), "the clip names {name}");
            assert!(xml.contains(&format!("value=\"In {}\"", channel + 1)), "the track is named after its channel");
        }
        // The length and the audio's place in the file are the files' own.
        let wav = disk.read(Path::new(&take.files[0]));
        let data = wav.windows(4).position(|w| w == b"data").unwrap() + 8;
        assert!(xml.contains(&format!("<int name=\"DataOffset\" value=\"{data}\"/>")), "the audio starts at byte {data}");
        let frames = samples(&wav).len();
        assert!(xml.contains(&format!("<int name=\"FrameCount\" value=\"{frames}\"/>")));
        assert!(xml.contains("<float name=\"Rate\" value=\"48000\"/>"));
        let log = String::from_utf8(disk.read(Path::new(&take.log))).unwrap();
        assert!(log.contains(&format!("Cubase track archive: {} (3 tracks", archive.display())), "{log}");
    }

    #[test]
    fn a_seed_that_fails_is_a_line_in_the_log_and_the_take_is_whole() {
        let disk = Arc::new(MemoryDisk::default());
        let take = take_with_seed(&disk, 2, Some("C:/Seeds/Gone.xml"));
        assert_eq!(take.cubase, None);
        assert_eq!(take.problem, None, "the take is not failed by it");
        assert!(take.files.iter().all(|f| !disk.read(Path::new(f)).is_empty()));
        let log = String::from_utf8(disk.read(Path::new(&take.log))).unwrap();
        assert!(log.contains("No Cubase track archive for this take: the seed C:/Seeds/Gone.xml could not be read"), "{log}");
        assert!(log.contains("The take itself is complete."), "{log}");

        let disk = Arc::new(MemoryDisk::default());
        disk.write_new(Path::new("C:/Seeds/Bad.xml"), b"<tracklist2/>").unwrap();
        let take = take_with_seed(&disk, 1, Some("C:/Seeds/Bad.xml"));
        let log = String::from_utf8(disk.read(Path::new(&take.log))).unwrap();
        assert!(log.contains("it could not be made from the seed"), "{log}");
        assert!(!disk.exists(&PathBuf::from("C:/Takes").join("2026-09-27 T001 Cubase.xml")));

        let disk = Arc::new(MemoryDisk::default());
        let take = take_with_seed(&disk, 1, None);
        let log = String::from_utf8(disk.read(Path::new(&take.log))).unwrap();
        assert!(!log.contains("Cubase"), "no seed, nothing said: {log}");
    }

    #[test]
    fn every_file_of_a_take_carries_the_same_time_reference_counted_from_the_first_sample() {
        let disk = Arc::new(MemoryDisk::default());
        let mut drain = drain_with(&disk, 3);
        let capture = Capture::allocate(3, BLOCK, 64 * BLOCK as u64, 32 * BLOCK as u64).unwrap();
        let mut next = 0;
        for _ in 0..32 {
            push(&capture, &mut next);
        }
        capture.want_recording(true);
        push(&capture, &mut next);
        drain.step(&capture);
        capture.want_recording(false);
        push(&capture, &mut next);
        while drain.step(&capture) {}
        let take = drain.state().lock().unwrap().takes[0].clone();
        // The clock says 14:03:21 when the writer opened the take, 33 blocks after its first sample.
        let behind = 33.0 * BLOCK as f64 / RATE;
        let expected = ((50_601.0 - behind) * RATE).round() as u64;
        let references: Vec<u64> = take.files.iter().map(|f| time_reference(&disk.read(Path::new(f)))).collect();
        assert!(references.iter().all(|&r| r == references[0]), "{references:?}");
        assert!(references[0].abs_diff(expected) <= 1, "{} against {expected}", references[0]);
        assert_eq!(take.time, "14:03:20", "the first sample was recorded before the clock read 14:03:21");
        let log = String::from_utf8(disk.read(Path::new(&take.log))).unwrap();
        assert!(log.contains(&format!("TimeReference {}", references[0])), "{log}");
        assert!(log.contains("Nothing was lost."), "{log}");
    }

    #[test]
    fn audio_the_writer_lost_is_silence_in_every_file_and_the_log_says_where() {
        let disk = Arc::new(MemoryDisk::default());
        let mut drain = drain_with(&disk, 1);
        let capture = Capture::allocate(1, BLOCK, 8 * BLOCK as u64, 4 * BLOCK as u64).unwrap();
        let mut next = 0;
        capture.want_recording(true);
        push(&capture, &mut next);
        drain.step(&capture);
        // The writer stalls while ten blocks arrive into a ring of eight.
        for _ in 0..10 {
            push(&capture, &mut next);
        }
        capture.want_recording(false);
        push(&capture, &mut next);
        while drain.step(&capture) {}
        let take = drain.state().lock().unwrap().takes[0].clone();
        // The writer took block 0, so blocks 9 and 10 would have overwritten blocks 1 and 2.
        assert_eq!(take.overruns, 2);
        let got = samples(&disk.read(Path::new(&take.files[0])));
        assert_eq!(got.len(), 11 * BLOCK, "the file keeps its length");
        assert!(got[9 * BLOCK..11 * BLOCK].iter().all(|&s| s == 0), "the lost blocks are silence");
        assert_eq!(got[8 * BLOCK], ramp(8 * BLOCK as u64, 0), "and everything around them is where it was");
        assert_eq!(got[9 * BLOCK - 1], ramp(9 * BLOCK as u64 - 1, 0));
        let log = String::from_utf8(disk.read(Path::new(&take.log))).unwrap();
        assert!(log.contains(&format!("Lost {} samples at sample {} of the take", 2 * BLOCK, 9 * BLOCK)), "{log}");
    }

    #[test]
    fn a_disk_nearly_full_stops_the_take_with_its_files_finished() {
        let disk = Arc::new(MemoryDisk::default());
        *disk.free.lock().unwrap() = Some(10 << 30);
        let mut drain = drain_with(&disk, 1);
        let capture = Capture::allocate(1, BLOCK, 8 * BLOCK as u64, 4 * BLOCK as u64).unwrap();
        let mut next = 0;
        capture.want_recording(true);
        push(&capture, &mut next);
        drain.step(&capture);
        *disk.free.lock().unwrap() = Some(100 << 20);
        drain.last_look = None;
        push(&capture, &mut next);
        drain.step(&capture);
        assert!(!capture.wants_recording(), "the writer asked for the take to stop");
        push(&capture, &mut next);
        while drain.step(&capture) {}
        let state = drain.state().lock().unwrap().clone();
        let take = &state.takes[0];
        assert!(take.stopped_by.as_deref().unwrap().contains("nearly full"), "{take:?}");
        assert!(state.problem.as_deref().unwrap().contains("nearly full"));
        assert!(state.disk_low);
        let bytes = disk.read(Path::new(&take.files[0]));
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8, "finished");
        assert_eq!(samples(&bytes).len(), 2 * BLOCK);
    }

    #[test]
    fn a_disk_that_fails_a_write_stops_the_take_and_keeps_what_was_written() {
        let disk = Arc::new(MemoryDisk::default());
        // Enough for the header and a few blocks of one file.
        *disk.fail_after.lock().unwrap() = Some(2_000);
        let mut drain = drain_with(&disk, 1);
        let capture = Capture::allocate(1, BLOCK, 64 * BLOCK as u64, 4 * BLOCK as u64).unwrap();
        let mut next = 0;
        capture.want_recording(true);
        for _ in 0..40 {
            push(&capture, &mut next);
            drain.step(&capture);
        }
        assert!(!capture.wants_recording(), "the failure stopped the take");
        push(&capture, &mut next);
        while drain.step(&capture) {}
        let state = drain.state().lock().unwrap().clone();
        assert!(capture.take().is_none(), "and the ring is free for the next take");
        let take = &state.takes[0];
        assert!(take.problem.as_deref().unwrap().contains("could not be written"), "{take:?}");
        assert_eq!(take.stopped_by.as_deref(), Some("a file could not be written"));
        let bytes = disk.read(Path::new(&take.files[0]));
        assert!(bytes.starts_with(b"RIFF"), "what was written is still a file");
    }

    #[test]
    fn a_take_never_overwrites_a_file_and_numbers_on_from_the_last() {
        let disk = Arc::new(MemoryDisk::default());
        disk.create_new(Path::new("C:/Takes/2026-09-27 T001 In 1.wav")).unwrap();
        let mut drain = drain_with(&disk, 1);
        let capture = Capture::allocate(1, BLOCK, 8 * BLOCK as u64, 4 * BLOCK as u64).unwrap();
        let mut next = 0;
        for _ in 0..2 {
            capture.want_recording(true);
            push(&capture, &mut next);
            drain.step(&capture);
            capture.want_recording(false);
            push(&capture, &mut next);
            while drain.step(&capture) {}
        }
        let numbers: Vec<u32> = drain.state().lock().unwrap().takes.iter().map(|t| t.number).collect();
        assert_eq!(numbers, vec![3, 2], "newest first, and 1 was taken");
        assert!(disk.read(Path::new("C:/Takes/2026-09-27 T001 In 1.wav")).is_empty(), "the file that was there is untouched");
    }

    #[test]
    fn there_is_no_room_to_record_under_a_minute_and_the_reserve() {
        let per_second = 48_000.0 * 8.0 * 3.0;
        assert!(room_to_record(Some(10 << 30), per_second).is_ok());
        assert!(room_to_record(Some(200 << 20), per_second).unwrap_err().contains("only 200 MB is free"));
        assert!(room_to_record(None, per_second).is_ok(), "a disk that will not say is not refused");
        assert_eq!(clock_words(75.5), "1:15.500");
    }
}
