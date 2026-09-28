//! **The metronome**: a click on the beat grid, played into the aggregate's outputs from the same
//! callback that records, locked to the session's samples.
//!
//! # The grid
//!
//! The tempo counts quarter notes a minute, as a DAW does, and a beat is one note of the time
//! signature's denominator: 6/8 at 120 clicks eighth notes, 240 a minute. A tick is a beat, or a
//! subdivision of one. Tick `k` of a stretch of steady tempo starting at sample `s` falls at
//!
//! ```text
//!   s + floor(k * rate * 2400 / (tempo_tenths * denominator * ticks_per_beat))
//! ```
//!
//! worked out in whole numbers from `k` every time, never by adding an interval to the last tick, so
//! there is nothing to drift: the millionth click is exactly where the formula puts it. A change of
//! tempo, signature or subdivision starts a new stretch at the next beat, which falls where the old
//! grid put it. A change of signature also starts a new bar there.
//!
//! # The audio thread
//!
//! [`Generator::fill`] runs on the callback. It allocates nothing, takes no lock, logs nothing and
//! never waits. Its sounds were rendered at session start ([`crate::sounds::Bank`]); its voices and
//! scratch buffer were allocated then too. Everything the person changes reaches it through atomics:
//! the musical choices through a sequence lock it never waits on ([`ParamCell`]), start, stop,
//! count-in and cancel as single flags and counters.
//!
//! # Loudness
//!
//! The volume is the peak of the loudest click. With the accent on, the downbeat plays the accent
//! variant at the volume and every other beat plays 4 dB under it; with it off every beat plays at
//! the volume. Subdivisions play 12 dB under it. **The volume is held at [`CEILING_DBFS`] whatever
//! is asked**, the same ceiling the calibration's click keeps, and every sample leaving the
//! generator is clamped to it as well, so even clicks piling on each other at an absurd tempo cannot
//! pass it.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::sounds::{Bank, Sound};

/// The loudest the metronome ever plays, whatever it is asked for: the calibration click's ceiling.
pub const CEILING_DBFS: f64 = gazelle_calibrate::LOUDEST_DBFS;
/// Where the volume starts.
pub const DEFAULT_VOLUME_DBFS: f64 = -18.0;
/// The quietest volume offered.
pub const QUIETEST_DBFS: f64 = -60.0;
/// How far under the volume a plain beat plays while the accent is on.
pub const BEAT_UNDER_ACCENT_DB: f64 = 4.0;
/// How far under the volume a subdivision plays.
pub const SUBDIVISION_UNDER_DB: f64 = 12.0;
/// How far under the volume a preview plays.
pub const PREVIEW_UNDER_DB: f64 = 12.0;
/// Tempo, in quarter notes a minute.
pub const TEMPO_MIN: f64 = 20.0;
pub const TEMPO_MAX: f64 = 400.0;
pub const NUMERATOR_MAX: u32 = 16;
pub const DENOMINATORS: [u32; 4] = [2, 4, 8, 16];
pub const COUNT_IN_MAX: u32 = 4;
/// Clicks that may sound at once; a new one past this takes over the one that has sounded longest.
const VOICES: usize = 8;
/// Not a position.
const NONE: u64 = u64::MAX;

fn gain_of(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// Clicks between the beats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subdivision {
    /// None: the beats alone.
    #[default]
    None,
    /// Two a beat: eighths when the beat is a quarter.
    Eighths,
    /// Three a beat: triplets.
    Triplets,
    /// Four a beat: sixteenths when the beat is a quarter.
    Sixteenths,
}

impl Subdivision {
    pub const ALL: [Subdivision; 4] = [Subdivision::None, Subdivision::Eighths, Subdivision::Triplets, Subdivision::Sixteenths];

    /// Ticks in a beat.
    pub fn per_beat(self) -> u32 {
        match self {
            Subdivision::None => 1,
            Subdivision::Eighths => 2,
            Subdivision::Triplets => 3,
            Subdivision::Sixteenths => 4,
        }
    }

    fn index(self) -> u32 {
        self.per_beat() - 1
    }

    fn from_index(index: u32) -> Subdivision {
        Subdivision::ALL.get(index as usize).copied().unwrap_or_default()
    }
}

/// Everything musical about the metronome, checked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Quarter notes a minute, in tenths: 1200 is 120.0.
    pub tempo_tenths: u32,
    pub numerator: u32,
    pub denominator: u32,
    pub accent: bool,
    pub subdivision: Subdivision,
    pub sound: Sound,
    /// The peak of the loudest click, in dBFS. Held at [`CEILING_DBFS`].
    pub volume_dbfs: f64,
}

impl Default for Params {
    fn default() -> Params {
        Params { tempo_tenths: 1200, numerator: 4, denominator: 4, accent: true, subdivision: Subdivision::None, sound: Sound::Click, volume_dbfs: DEFAULT_VOLUME_DBFS }
    }
}

impl Params {
    /// A tempo in quarter notes a minute, as tenths.
    pub fn tenths(tempo: f64) -> u32 {
        if !tempo.is_finite() {
            return 0;
        }
        (tempo * 10.0).round().clamp(0.0, u32::MAX as f64) as u32
    }

    pub fn tempo(&self) -> f64 {
        f64::from(self.tempo_tenths) / 10.0
    }

    /// Everything wrong with these, in a sentence, or nothing.
    pub fn problem(&self) -> Option<String> {
        if !(Params::tenths(TEMPO_MIN)..=Params::tenths(TEMPO_MAX)).contains(&self.tempo_tenths) {
            return Some(format!("the tempo is between {TEMPO_MIN} and {TEMPO_MAX} BPM, not {}", self.tempo()));
        }
        if !(1..=NUMERATOR_MAX).contains(&self.numerator) {
            return Some(format!("a bar has between 1 and {NUMERATOR_MAX} beats, not {}", self.numerator));
        }
        if !DENOMINATORS.contains(&self.denominator) {
            return Some(format!("the beat is a half, quarter, eighth or sixteenth note (2, 4, 8 or 16), not {}", self.denominator));
        }
        if self.volume_dbfs.is_nan() {
            return Some("the volume has to be a number of dB".into());
        }
        None
    }

    /// The volume as played: never above the ceiling, never below the quietest, silence for nonsense.
    pub fn level_dbfs(&self) -> f64 {
        if self.volume_dbfs.is_nan() {
            return f64::NEG_INFINITY;
        }
        self.volume_dbfs.clamp(QUIETEST_DBFS, CEILING_DBFS)
    }

    /// The volume as a gain, held at the ceiling.
    pub fn gain(&self) -> f32 {
        let level = self.level_dbfs();
        if level.is_finite() {
            gain_of(level)
        } else {
            0.0
        }
    }

    /// How long a beat is, in samples at `rate`.
    pub fn beat_samples(&self, rate: f64) -> f64 {
        rate * 240.0 / (self.tempo().max(f64::MIN_POSITIVE) * f64::from(self.denominator.max(1)))
    }

    /// Whether the grid differs: tempo, signature or subdivision.
    fn same_grid(&self, other: &Params) -> bool {
        self.tempo_tenths == other.tempo_tenths && self.numerator == other.numerator && self.denominator == other.denominator && self.subdivision == other.subdivision
    }
}

/// **The musical choices, handed to the audio thread without tearing and without waiting.**
///
/// A sequence lock over atomics. The writers (the person's changes, never the callback) take turns
/// behind a mutex and bump the sequence to odd, write, and bump it to even. The reader (the callback)
/// reads the sequence, the fields and the sequence again, and takes what it read only when both
/// readings are the same even number. Otherwise it keeps what it had and looks again at its next
/// block: it never waits, and it never sees half of one change and half of another.
pub struct ParamCell {
    writing: Mutex<()>,
    sequence: AtomicU64,
    tempo: AtomicU32,
    signature: AtomicU32,
    flags: AtomicU32,
    volume: AtomicU64,
}

impl ParamCell {
    pub fn new(params: Params) -> ParamCell {
        let cell = ParamCell { writing: Mutex::new(()), sequence: AtomicU64::new(0), tempo: AtomicU32::new(0), signature: AtomicU32::new(0), flags: AtomicU32::new(0), volume: AtomicU64::new(0) };
        cell.store(params);
        cell
    }

    /// Off the audio thread only: it may wait for another writer.
    pub fn store(&self, params: Params) {
        let _turn = self.writing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence.store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.tempo.store(params.tempo_tenths, Ordering::Relaxed);
        self.signature.store(params.numerator | (params.denominator << 8), Ordering::Relaxed);
        self.flags.store(u32::from(params.accent) | (params.subdivision.index() << 1) | ((params.sound.index() as u32) << 4), Ordering::Relaxed);
        self.volume.store(params.volume_dbfs.to_bits(), Ordering::Relaxed);
        self.sequence.store(sequence.wrapping_add(2), Ordering::Release);
    }

    /// On any thread, the audio thread included: never waits. `None` while a change is being written.
    pub fn load(&self) -> Option<Params> {
        let before = self.sequence.load(Ordering::Acquire);
        if before & 1 == 1 {
            return None;
        }
        let tempo = self.tempo.load(Ordering::Relaxed);
        let signature = self.signature.load(Ordering::Relaxed);
        let flags = self.flags.load(Ordering::Relaxed);
        let volume = self.volume.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != before {
            return None;
        }
        Some(Params {
            tempo_tenths: tempo,
            numerator: signature & 0xFF,
            denominator: (signature >> 8) & 0xFF,
            accent: flags & 1 == 1,
            subdivision: Subdivision::from_index((flags >> 1) & 0b111),
            sound: Sound::from_index(((flags >> 4) & 0xF) as usize),
            volume_dbfs: f64::from_bits(volume),
        })
    }
}

/// A stretch of steady tempo: where its first tick is, and the tick spacing as a fraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    start: u64,
    numerator: u128,
    denominator: u128,
}

impl Grid {
    pub fn new(start: u64, rate: u64, params: &Params) -> Grid {
        let denominator = u128::from(params.tempo_tenths.max(1)) * u128::from(params.denominator.max(1)) * u128::from(params.subdivision.per_beat());
        Grid { start, numerator: u128::from(rate) * 2400, denominator }
    }

    /// Where tick `tick` of this stretch falls, in samples: from the tick's number, every time.
    pub fn onset(&self, tick: u64) -> u64 {
        self.start + (u128::from(tick) * self.numerator / self.denominator) as u64
    }
}

/// What happens once a count-in has counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    /// Nothing: the click carries on.
    Carry = 0,
    /// A take starts on the downbeat after it.
    Record = 1,
    /// The click stops there: a preview.
    Stop = 2,
}

impl Then {
    fn from_bits(bits: u64) -> Then {
        match bits {
            1 => Then::Record,
            2 => Then::Stop,
            _ => Then::Carry,
        }
    }
}

/// A count-in that has counted, as the callback reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counted {
    /// The downbeat after the count-in, as a position in the session.
    pub downbeat: u64,
    /// The start of the block in which the count-in was asked for: when Record was pressed.
    pub pressed: u64,
    pub then: Then,
}

/// What one block came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Events {
    /// The session position of the block's first sample.
    pub block_start: u64,
    /// The count-in that counted in this block, if one did.
    pub counted: Option<Counted>,
    /// Clicks that started in this block.
    pub onsets: u32,
}

/// Where the click is, as the pages read it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Beat {
    pub running: bool,
    /// A click is still ringing out, whether or not the metronome is running.
    pub sounding: bool,
    /// The session position the generator has reached.
    pub position: u64,
    /// The last beat played, as a session position, if one has been.
    pub beat_at: Option<u64>,
    /// Its place in its bar, from zero.
    pub beat: u32,
    /// Its bar, from zero, counted since the click started.
    pub bar: u64,
    pub beats_per_bar: u32,
    /// How long a beat is now, in samples.
    pub beat_samples: f64,
    /// A count-in under way: the bar it counts to, and how many bars it is.
    pub counting_to: Option<(u64, u32)>,
}

/// One click sounding.
#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    active: bool,
    which: usize,
    at: usize,
    gain: f32,
}

#[derive(Clone, Copy, Debug)]
struct CountIn {
    target: u64,
    bars: u32,
    pressed: u64,
    then: Then,
}

/// The callback's own state.
struct Run {
    position: u64,
    running: bool,
    /// The grid in force.
    params: Params,
    /// A grid change waiting for the next beat.
    pending: Option<Params>,
    /// The latest of everything: sound, accent and volume are taken at every click.
    latest: Params,
    grid: Grid,
    tick: u64,
    tick_in_beat: u32,
    beat_in_bar: u32,
    bar: u64,
    next_onset: u64,
    voices: [Voice; VOICES],
    count_in: Option<CountIn>,
    quiet: bool,
    seen_count_in: u64,
    seen_cancel: u64,
}

/// **The metronome's audio**: made at session start at the session's rate, filled from the callback,
/// steered from anywhere through atomics.
pub struct Generator {
    rate: u64,
    bank: Bank,
    params: ParamCell,
    want_running: AtomicBool,
    /// The latest count-in asked for: a number that goes up with each, the quiet flag, what follows,
    /// and how many bars.
    count_in: AtomicU64,
    cancel: AtomicU64,
    requests: AtomicU64,
    running: AtomicBool,
    sounding: AtomicBool,
    position: AtomicU64,
    beat_at: AtomicU64,
    beat: AtomicU32,
    bar: AtomicU64,
    beats_per_bar: AtomicU32,
    beat_samples: AtomicU64,
    counting_to: AtomicU64,
    counting_bars: AtomicU32,
    run: UnsafeCell<Run>,
    scratch: UnsafeCell<Box<[f32]>>,
}

// `run` and `scratch` are the callback's alone (see `fill`); everything else is atomic.
unsafe impl Sync for Generator {}
unsafe impl Send for Generator {}

impl Generator {
    /// Render the sounds and allocate everything, off the audio thread, for a session at `rate` with
    /// blocks of `block` samples.
    pub fn new(rate: f64, block: usize, params: Params) -> Generator {
        Generator::starting_at(rate, block, params, 0)
    }

    /// As [`Generator::new`], with the session already at `position`: for the tests, which start
    /// hours in.
    pub fn starting_at(rate: f64, block: usize, params: Params, position: u64) -> Generator {
        let whole = if rate.is_finite() && rate >= 1.0 { rate.round() as u64 } else { 48_000 };
        let run = Run {
            position,
            running: false,
            params,
            pending: None,
            latest: params,
            grid: Grid::new(position, whole, &params),
            tick: 0,
            tick_in_beat: 0,
            beat_in_bar: 0,
            bar: 0,
            next_onset: position,
            voices: [Voice::default(); VOICES],
            count_in: None,
            quiet: false,
            seen_count_in: 0,
            seen_cancel: 0,
        };
        Generator {
            rate: whole,
            bank: Bank::render(whole as f64),
            params: ParamCell::new(params),
            want_running: AtomicBool::new(false),
            count_in: AtomicU64::new(0),
            cancel: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            running: AtomicBool::new(false),
            sounding: AtomicBool::new(false),
            position: AtomicU64::new(position),
            beat_at: AtomicU64::new(NONE),
            beat: AtomicU32::new(0),
            bar: AtomicU64::new(0),
            beats_per_bar: AtomicU32::new(params.numerator),
            beat_samples: AtomicU64::new(params.beat_samples(whole as f64).to_bits()),
            counting_to: AtomicU64::new(NONE),
            counting_bars: AtomicU32::new(0),
            run: UnsafeCell::new(run),
            scratch: UnsafeCell::new(vec![0.0f32; block.max(1)].into_boxed_slice()),
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate
    }

    // ---------------------------------------------------------------------------------------------
    // Any thread: what the person asks for.
    // ---------------------------------------------------------------------------------------------

    /// New musical choices: sound, accent and volume at the next click, the grid at the next beat.
    pub fn set_params(&self, params: Params) {
        self.params.store(params);
    }

    /// Start at the next block, on a downbeat.
    pub fn start(&self) {
        self.want_running.store(true, Ordering::Release);
    }

    /// Stop at the next block. What is sounding rings out.
    pub fn stop(&self) {
        self.want_running.store(false, Ordering::Release);
    }

    /// Count `bars` bars from the next downbeat (starting the click on one if it is not running),
    /// then do `then` on the downbeat after them. `quiet` plays them under the volume.
    pub fn count_in(&self, bars: u32, then: Then, quiet: bool) {
        let number = self.requests.fetch_add(1, Ordering::AcqRel) + 1;
        let packed = (number << 16) | (u64::from(quiet) << 12) | ((then as u64) << 8) | u64::from(bars.min(255));
        self.count_in.store(packed, Ordering::Release);
        // After the count-in, so a callback that sees the start sees the count-in with it.
        self.start();
    }

    /// Forget any count-in under way.
    pub fn cancel_count_in(&self) {
        self.cancel.fetch_add(1, Ordering::AcqRel);
    }

    /// Where things stand.
    pub fn beat(&self) -> Beat {
        let beat_at = self.beat_at.load(Ordering::Acquire);
        let counting = self.counting_to.load(Ordering::Acquire);
        Beat {
            running: self.running.load(Ordering::Acquire),
            sounding: self.sounding.load(Ordering::Acquire),
            position: self.position.load(Ordering::Acquire),
            beat_at: (beat_at != NONE).then_some(beat_at),
            beat: self.beat.load(Ordering::Relaxed),
            bar: self.bar.load(Ordering::Relaxed),
            beats_per_bar: self.beats_per_bar.load(Ordering::Relaxed),
            beat_samples: f64::from_bits(self.beat_samples.load(Ordering::Relaxed)),
            counting_to: (counting != NONE).then(|| (counting, self.counting_bars.load(Ordering::Relaxed))),
        }
    }

    /// Whether the person asked for it to run.
    pub fn wants_running(&self) -> bool {
        self.want_running.load(Ordering::Acquire)
    }

    // ---------------------------------------------------------------------------------------------
    // The audio thread.
    // ---------------------------------------------------------------------------------------------

    /// **One block of mono audio**, into `out`, starting at the session position after the last.
    ///
    /// Nothing here allocates, locks, logs or waits.
    ///
    /// # Safety
    ///
    /// One caller at a time: the callback, or a test standing in for it.
    pub unsafe fn fill(&self, out: &mut [f32]) -> Events {
        // Safety: the caller promises this is the only thread in here.
        let run = unsafe { &mut *self.run.get() };
        let length = out.len() as u64;
        let start = run.position;
        let mut events = Events { block_start: start, ..Events::default() };
        out.fill(0.0);

        if let Some(latest) = self.params.load() {
            run.latest = latest;
            run.pending = if latest.same_grid(&run.params) { None } else { Some(latest) };
        }
        let want = self.want_running.load(Ordering::Acquire);
        if !want && run.running {
            run.running = false;
            run.count_in = None;
            run.quiet = false;
        }
        if want && !run.running {
            run.running = true;
            if let Some(pending) = run.pending.take() {
                run.params = pending;
            }
            run.grid = Grid::new(start, self.rate, &run.params);
            run.tick = 0;
            run.tick_in_beat = 0;
            run.beat_in_bar = 0;
            run.bar = 0;
            run.next_onset = start;
            run.quiet = false;
        }
        let cancel = self.cancel.load(Ordering::Acquire);
        if cancel != run.seen_cancel {
            run.seen_cancel = cancel;
            run.count_in = None;
            run.quiet = false;
        }
        let request = self.count_in.load(Ordering::Acquire);
        let number = request >> 16;
        if number != run.seen_count_in {
            run.seen_count_in = number;
            let bars = (request & 0xFF) as u32;
            if run.running && bars > 0 {
                let next_downbeat = if run.tick_in_beat == 0 && run.beat_in_bar == 0 { run.bar } else { run.bar + 1 };
                let quiet = (request >> 12) & 1 == 1;
                run.count_in = Some(CountIn { target: next_downbeat + u64::from(bars), bars, pressed: start, then: Then::from_bits((request >> 8) & 0xF) });
                run.quiet = quiet;
            }
        }

        let mut cursor = 0usize;
        while run.running && run.next_onset < start + length {
            let at = (run.next_onset - start) as usize;
            self.render(run, out, cursor, at);
            cursor = at;
            if run.tick_in_beat == 0 {
                if let Some(pending) = run.pending.take() {
                    let new_bar = pending.numerator != run.params.numerator || pending.denominator != run.params.denominator;
                    run.params = pending;
                    run.grid = Grid::new(run.next_onset, self.rate, &run.params);
                    run.tick = 0;
                    if new_bar && run.beat_in_bar != 0 {
                        run.bar += 1;
                        run.beat_in_bar = 0;
                    }
                }
            }
            let downbeat = run.tick_in_beat == 0 && run.beat_in_bar == 0;
            if downbeat {
                if let Some(count) = run.count_in.filter(|count| count.target == run.bar) {
                    events.counted = Some(Counted { downbeat: run.next_onset, pressed: count.pressed, then: count.then });
                    run.count_in = None;
                    run.quiet = false;
                    if count.then == Then::Stop {
                        run.running = false;
                        self.want_running.store(false, Ordering::Release);
                        break;
                    }
                }
            }
            let subdivision = run.tick_in_beat != 0;
            let accent = downbeat && run.latest.accent;
            let under = if subdivision {
                SUBDIVISION_UNDER_DB
            } else if run.latest.accent && !downbeat {
                BEAT_UNDER_ACCENT_DB
            } else {
                0.0
            } + if run.quiet { PREVIEW_UNDER_DB } else { 0.0 };
            let gain = run.latest.gain() * gain_of(-under);
            self.voice(run, run.latest.sound.index() * 2 + usize::from(accent), gain);
            events.onsets += 1;
            if run.tick_in_beat == 0 {
                self.beat_at.store(run.next_onset, Ordering::Relaxed);
                self.beat.store(run.beat_in_bar, Ordering::Relaxed);
                self.bar.store(run.bar, Ordering::Relaxed);
                self.beats_per_bar.store(run.params.numerator, Ordering::Relaxed);
                self.beat_samples.store(run.params.beat_samples(self.rate as f64).to_bits(), Ordering::Relaxed);
            }
            run.tick += 1;
            run.tick_in_beat += 1;
            if run.tick_in_beat >= run.params.subdivision.per_beat() {
                run.tick_in_beat = 0;
                run.beat_in_bar += 1;
                if run.beat_in_bar >= run.params.numerator {
                    run.beat_in_bar = 0;
                    run.bar += 1;
                }
            }
            run.next_onset = run.grid.onset(run.tick);
        }
        self.render(run, out, cursor, out.len());

        // The ceiling, on every sample, whatever piled up.
        let ceiling = gain_of(CEILING_DBFS);
        for sample in out.iter_mut() {
            *sample = sample.clamp(-ceiling, ceiling);
        }
        run.position = start + length;
        self.position.store(run.position, Ordering::Release);
        self.running.store(run.running, Ordering::Release);
        self.sounding.store(run.voices.iter().any(|voice| voice.active), Ordering::Release);
        match run.count_in {
            Some(count) => {
                self.counting_bars.store(count.bars, Ordering::Relaxed);
                self.counting_to.store(count.target, Ordering::Release);
            }
            None => self.counting_to.store(NONE, Ordering::Release),
        }
        events
    }

    /// Start a click, on a free voice or the one that has sounded longest.
    fn voice(&self, run: &mut Run, which: usize, gain: f32) {
        let slot = match run.voices.iter().position(|voice| !voice.active) {
            Some(free) => free,
            None => run.voices.iter().enumerate().max_by_key(|(_, voice)| voice.at).map_or(0, |(index, _)| index),
        };
        run.voices[slot] = Voice { active: gain > 0.0, which, at: 0, gain };
    }

    /// Every voice sounding, into `out[from..to]`.
    fn render(&self, run: &mut Run, out: &mut [f32], from: usize, to: usize) {
        if from >= to {
            return;
        }
        for voice in run.voices.iter_mut().filter(|voice| voice.active) {
            let sound = self.bank.get(Sound::from_index(voice.which / 2), voice.which % 2 == 1);
            let count = (to - from).min(sound.len() - voice.at);
            for (into, &sample) in out[from..from + count].iter_mut().zip(&sound[voice.at..voice.at + count]) {
                *into += sample * voice.gain;
            }
            voice.at += count;
            if voice.at >= sound.len() {
                voice.active = false;
            }
        }
    }

    /// **One block, into the aggregate's output buffers**: the same click on every one of them.
    ///
    /// # Safety
    ///
    /// The callback only, as for [`Generator::fill`]; every pointer is a buffer of `block` samples of
    /// `i32` the aggregate made, alive from createBuffers to disposeBuffers, or null.
    pub unsafe fn fill_outputs(&self, outputs: &[[*mut c_void; 2]], half: usize, block: usize) -> Events {
        // Safety: the callback's alone, as `run` is.
        let scratch = unsafe { &mut *self.scratch.get() };
        let length = block.min(scratch.len());
        let events = unsafe { self.fill(&mut scratch[..length]) };
        for pair in outputs {
            let into = pair[half & 1] as *mut i32;
            if into.is_null() {
                continue;
            }
            // Safety: see above.
            let run = unsafe { std::slice::from_raw_parts_mut(into, length) };
            for (sample, &value) in run.iter_mut().zip(scratch.iter()) {
                *sample = (f64::from(value) * f64::from(i32::MAX)) as i32;
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const RATE: f64 = 48_000.0;

    fn params(tempo: f64) -> Params {
        Params { tempo_tenths: Params::tenths(tempo), ..Params::default() }
    }

    /// Run a generator for `blocks` blocks, calling `each` after every one.
    fn drive(generator: &Generator, block: usize, blocks: usize, mut each: impl FnMut(&[f32], Events)) {
        let mut out = vec![0.0f32; block];
        for _ in 0..blocks {
            let events = unsafe { generator.fill(&mut out) };
            each(&out, events);
        }
    }

    /// Every onset a generator plays, found from its published beats and its blocks' events.
    fn beats(generator: &Generator, block: usize, blocks: usize) -> Vec<u64> {
        let mut seen = Vec::new();
        drive(generator, block, blocks, |_, _| {
            if let Some(at) = generator.beat().beat_at {
                if seen.last() != Some(&at) {
                    seen.push(at);
                }
            }
        });
        seen
    }

    #[test]
    fn the_grid_has_no_drift_over_ten_simulated_hours() {
        for (tempo, denominator, subdivision) in [(97.3, 4, Subdivision::None), (120.0, 8, Subdivision::Triplets), (400.0, 16, Subdivision::Sixteenths), (20.0, 2, Subdivision::None), (133.7, 4, Subdivision::Eighths)] {
            for rate in [44_100u64, 48_000, 96_000] {
                let p = Params { tempo_tenths: Params::tenths(tempo), denominator, subdivision, ..Params::default() };
                let grid = Grid::new(1_000, rate, &p);
                let tick_seconds = 240.0 / (tempo * f64::from(denominator) * f64::from(subdivision.per_beat()));
                let ticks = (10.0 * 3600.0 / tick_seconds) as u64;
                let mut last = grid.onset(0);
                assert_eq!(last, 1_000);
                let ideal = tick_seconds * rate as f64;
                for tick in 1..=ticks {
                    let at = grid.onset(tick);
                    let step = (at - last) as f64;
                    assert!((step - ideal).abs() < 1.0, "each step is the ideal spacing to within a sample");
                    last = at;
                }
                // Ten hours on, the last tick is where the ideal clock puts it, to within a sample.
                let expected = 1_000.0 + ticks as f64 * ideal;
                assert!((last as f64 - expected).abs() <= 1.0, "{tempo} BPM at {rate}: {last} against {expected}");
            }
        }
    }

    #[test]
    fn every_beat_lands_exactly_on_the_grid_block_after_block_hours_into_a_session() {
        let hours_in = (3.0 * 3600.0 * RATE) as u64 + 17;
        for (block, position) in [(256usize, 0u64), (97, 12_345), (1024, hours_in)] {
            let p = params(97.3);
            let generator = Generator::starting_at(RATE, block, p, position);
            generator.start();
            let seen = beats(&generator, block, (RATE as usize * 120) / block);
            let grid = Grid::new(position, 48_000, &p);
            assert!(seen.len() > 190, "two minutes of 97.3 BPM");
            for (index, &at) in seen.iter().enumerate() {
                assert_eq!(at, grid.onset(index as u64), "beat {index} with blocks of {block}");
            }
        }
    }

    #[test]
    fn the_output_is_the_same_sample_for_sample_whatever_the_block_size() {
        // A click straddling a block boundary carries on exactly where it left off.
        let p = Params { tempo_tenths: 2_333, subdivision: Subdivision::Triplets, sound: Sound::Cowbell, ..Params::default() };
        let render = |block: usize| {
            let generator = Generator::new(RATE, block, p);
            generator.start();
            let mut all = Vec::new();
            drive(&generator, block, 96_000 / block + 1, |out, _| all.extend_from_slice(out));
            all.truncate(96_000);
            all
        };
        let reference = render(64);
        assert!(reference.iter().any(|&s| s != 0.0));
        for block in [1usize, 97, 256, 1000, 4096] {
            assert_eq!(render(block), reference, "blocks of {block}");
        }
    }

    #[test]
    fn a_tempo_change_takes_effect_at_the_next_beat() {
        let generator = Generator::new(RATE, 128, params(120.0));
        generator.start();
        // 120 BPM: a beat every 24 000 samples. Change half way through the third beat.
        let mut seen = Vec::new();
        let mut out = vec![0.0f32; 128];
        let mut position = 0u64;
        while position < 400_000 {
            if position == 60_032 {
                generator.set_params(params(100.0));
            }
            unsafe { generator.fill(&mut out) };
            position += 128;
            if let Some(at) = generator.beat().beat_at {
                if seen.last() != Some(&at) {
                    seen.push(at);
                }
            }
        }
        assert_eq!(&seen[..4], &[0, 24_000, 48_000, 72_000], "the beat after the change is where the old tempo put it");
        // From there on, 100 BPM: 28 800 samples a beat.
        for (index, pair) in seen[3..].windows(2).enumerate() {
            assert_eq!(pair[1] - pair[0], 28_800, "beat {} at the new tempo", index + 4);
        }
    }

    #[test]
    fn a_signature_change_starts_a_new_bar_at_the_next_beat() {
        let generator = Generator::new(RATE, 100, params(120.0));
        generator.start();
        drive(&generator, 100, 530, |_, _| {});
        // 53 000 samples in: beats 0, 1 and 2 of the first bar have played.
        assert_eq!((generator.beat().bar, generator.beat().beat), (0, 2));
        generator.set_params(Params { numerator: 3, denominator: 8, ..params(120.0) });
        drive(&generator, 100, 250, |_, _| {});
        let beat = generator.beat();
        assert_eq!((beat.bar, beat.beat, beat.beats_per_bar), (1, 0, 3), "a new bar at 72 000, the next beat");
        assert_eq!(beat.beat_at, Some(72_000));
        // In 3/8 at 120 a beat is an eighth: 12 000 samples.
        drive(&generator, 100, 120, |_, _| {});
        assert_eq!(generator.beat().beat_at, Some(84_000));
        assert_eq!(generator.beat().beats_per_bar, 3);
    }

    #[test]
    fn the_volume_is_held_at_the_ceiling_whatever_is_asked_and_nonsense_is_silence() {
        let ceiling = gain_of(CEILING_DBFS);
        assert!(Params { volume_dbfs: 40.0, ..Params::default() }.gain() <= ceiling);
        assert_eq!(Params { volume_dbfs: f64::NAN, ..Params::default() }.gain(), 0.0);
        assert!((Params::default().gain() - gain_of(-18.0)).abs() < 1e-6, "-18 dBFS by default");
        // Everything at once: the loudest sound at an absurd tempo, clicks piling on each other.
        let p = Params { tempo_tenths: 4_000, denominator: 16, subdivision: Subdivision::Sixteenths, sound: Sound::Cowbell, volume_dbfs: 24.0, ..Params::default() };
        let generator = Generator::new(RATE, 512, p);
        generator.start();
        let mut loudest = 0.0f32;
        drive(&generator, 512, 200, |out, _| loudest = out.iter().fold(loudest, |l, s| l.max(s.abs())));
        assert!(loudest <= ceiling, "{loudest} against {ceiling}");
        assert!(loudest > ceiling * 0.9, "and it does play");
    }

    #[test]
    fn the_default_peak_is_minus_18_and_the_click_carries_no_dc() {
        for sound in Sound::ALL {
            let generator = Generator::new(RATE, 480, Params { sound, ..params(120.0) });
            generator.start();
            let mut all = Vec::new();
            drive(&generator, 480, 200, |out, _| all.extend_from_slice(out));
            let peak = all.iter().fold(0.0f32, |l, s| l.max(s.abs()));
            assert!((20.0 * f64::from(peak).log10() - DEFAULT_VOLUME_DBFS).abs() < 0.01, "{sound:?} peaks at -18 dBFS");
            let mean = all.iter().map(|&s| f64::from(s)).sum::<f64>() / all.len() as f64;
            assert!(mean.abs() < 1e-7, "{sound:?}: DC of {mean}");
        }
    }

    #[test]
    fn the_downbeat_is_the_accent_at_the_volume_and_the_other_beats_are_under_it() {
        let generator = Generator::new(RATE, 24_000, params(120.0));
        generator.start();
        let mut peaks = Vec::new();
        drive(&generator, 24_000, 5, |out, _| peaks.push(out.iter().fold(0.0f32, |l, s| l.max(s.abs()))));
        let db = |g: f32| 20.0 * f64::from(g).log10();
        assert!((db(peaks[0]) + 18.0).abs() < 0.01);
        assert!((db(peaks[1]) + 22.0).abs() < 0.01, "{}", db(peaks[1]));
        assert!((db(peaks[4]) + 18.0).abs() < 0.01, "the next bar's downbeat");
        let flat = Generator::new(RATE, 24_000, Params { accent: false, ..params(120.0) });
        flat.start();
        let mut plain = Vec::new();
        drive(&flat, 24_000, 2, |out, _| plain.push(out.iter().fold(0.0f32, |l, s| l.max(s.abs()))));
        assert!((db(plain[0]) - db(plain[1])).abs() < 0.01, "no accent: every beat alike");
    }

    #[test]
    fn a_count_in_from_stopped_starts_the_click_and_counts_whole_bars() {
        let generator = Generator::starting_at(RATE, 256, params(120.0), 5_000);
        drive(&generator, 256, 10, |_, _| {});
        generator.count_in(2, Then::Record, false);
        let mut counted = None;
        drive(&generator, 256, 1_000, |_, events| counted = counted.or(events.counted));
        let counted = counted.expect("the count-in counted");
        let started = 5_000 + 10 * 256;
        assert_eq!(counted.pressed, started as u64);
        assert_eq!(counted.downbeat, started as u64 + 2 * 4 * 24_000, "two bars of 4/4 at 120 after the first click");
        assert_eq!(counted.then, Then::Record);
        assert!(generator.beat().running, "and the click carries on");
        assert_eq!(generator.beat().counting_to, None);
    }

    #[test]
    fn a_count_in_while_running_waits_for_the_next_downbeat() {
        let generator = Generator::new(RATE, 1_000, params(120.0));
        generator.start();
        drive(&generator, 1_000, 30, |_, _| {});
        generator.count_in(1, Then::Record, false);
        let mut counted = None;
        drive(&generator, 1_000, 1, |_, events| counted = counted.or(events.counted));
        assert_eq!(generator.beat().counting_to, Some((2, 1)), "the rest of bar 0, then bar 1, then the downbeat of bar 2");
        drive(&generator, 1_000, 300, |_, events| counted = counted.or(events.counted));
        assert_eq!(counted.unwrap().downbeat, 2 * 96_000);
    }

    #[test]
    fn a_preview_plays_one_bar_quietly_and_stops() {
        let generator = Generator::new(RATE, 500, params(120.0));
        generator.count_in(1, Then::Stop, true);
        let mut all = Vec::new();
        let mut counted = None;
        drive(&generator, 500, 400, |out, events| {
            all.extend_from_slice(out);
            counted = counted.or(events.counted);
        });
        assert_eq!(counted.unwrap().downbeat, 96_000);
        assert!(!generator.beat().running && !generator.wants_running(), "stopped on the next downbeat");
        let peak = all.iter().fold(0.0f32, |l, s| l.max(s.abs()));
        assert!((20.0 * f64::from(peak).log10() - (DEFAULT_VOLUME_DBFS - PREVIEW_UNDER_DB)).abs() < 0.01);
        assert!(all[96_000..].iter().all(|&s| s == 0.0), "nothing after the bar");
    }

    #[test]
    fn a_cancelled_count_in_counts_nothing() {
        let generator = Generator::new(RATE, 500, params(120.0));
        generator.count_in(1, Then::Record, false);
        drive(&generator, 500, 10, |_, _| {});
        generator.cancel_count_in();
        let mut counted = None;
        drive(&generator, 500, 400, |_, events| counted = counted.or(events.counted));
        assert_eq!(counted, None);
        assert!(generator.beat().running, "the click it started is the caller's to stop");
    }

    #[test]
    fn a_stop_lets_the_last_click_ring_out_and_then_it_is_silent() {
        let generator = Generator::new(RATE, 256, Params { sound: Sound::Cowbell, ..params(120.0) });
        generator.start();
        drive(&generator, 256, 1, |_, _| {});
        generator.stop();
        let mut out = vec![0.0f32; 256];
        unsafe { generator.fill(&mut out) };
        assert!(out.iter().any(|&s| s != 0.0), "the cowbell carries on after the stop, not cut off");
        assert!(generator.beat().sounding && !generator.beat().running);
        drive(&generator, 256, 40, |_, _| {});
        assert!(!generator.beat().sounding);
        unsafe { generator.fill(&mut out) };
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn the_musical_choices_never_tear_under_concurrent_changes() {
        // Every value written keeps one rule across its fields, so half of one and half of another
        // would break it.
        let consistent = |k: u32| Params {
            tempo_tenths: 200 + k % 3_800,
            numerator: 1 + k % 16,
            denominator: DENOMINATORS[(k % 4) as usize],
            accent: k.is_multiple_of(2),
            subdivision: Subdivision::ALL[(k % 4) as usize],
            sound: Sound::ALL[(k % 5) as usize],
            volume_dbfs: -f64::from(k % 60),
        };
        let cell = Arc::new(ParamCell::new(consistent(0)));
        let writers: Vec<_> = (0..2)
            .map(|w| {
                let cell = Arc::clone(&cell);
                std::thread::spawn(move || {
                    for k in 0..100_000u32 {
                        cell.store(consistent(k * 2 + w));
                    }
                })
            })
            .collect();
        let mut read = 0;
        while writers.iter().any(|w| !w.is_finished()) {
            if let Some(p) = cell.load() {
                let k = p.tempo_tenths - 200;
                let candidates = (0..=200_000u32).step_by(3_800).map(|base| base + k).filter(|&k| consistent(k) == p);
                assert!(candidates.count() > 0, "a torn read: {p:?}");
                read += 1;
            }
        }
        for writer in writers {
            writer.join().unwrap();
        }
        assert!(read > 0);
        assert!(cell.load().is_some(), "and quiet again once nobody writes");
    }

    #[test]
    fn nonsense_is_refused_in_words() {
        assert!(Params { tempo_tenths: 199, ..Params::default() }.problem().unwrap().contains("between 20"));
        assert!(Params { tempo_tenths: 4_001, ..Params::default() }.problem().is_some());
        assert!(Params { numerator: 17, ..Params::default() }.problem().unwrap().contains("16"));
        assert!(Params { denominator: 3, ..Params::default() }.problem().is_some());
        assert_eq!(Params::default().problem(), None);
        assert_eq!(Params::tenths(97.34), 973);
    }
}
