//! **Recording the aggregate's inputs**, with a pre-roll held in memory while armed.
//!
//! Gazelle hosts Gazelle Aggregate in its own process, exactly as a measurement does, so channels
//! from both interfaces go into one recording lined up by the measured trims and phase. A preset
//! chooses the channels, the folder, the file names, the bit depth and how much memory the
//! pre-roll takes.
//!
//! # How it is put together
//!
//! - [`host`] opens the aggregate and runs its callback, with the calibration's own machinery and
//!   under the calibration's own turn, so there is one owner of the aggregate in Gazelle.
//! - [`capture`] is the ring the callback copies every block into. While armed it is the pre-roll;
//!   Record starts a take at the oldest sample it still holds, and the take runs on into live audio
//!   with no gap, because the pre-roll and the live audio are one run of positions in one ring.
//! - [`sizing`] works out how big the ring is, from the memory free at Arm.
//! - [`writer`] drains a take into one Broadcast WAV per channel ([`wav`]), keeps its log, and
//!   watches the disk.
//! - [`recorder`] is the states (Off, Armed, Recording) and the two threads behind them.
//! - [`env`] is the PC: this one, or the loopback's, made of data ([`sim`]).
//!
//! # The audio thread
//!
//! The callback copies each recorded channel's block into the ring and does nothing else: it does
//! not allocate, lock, log, touch a file or wait. The ring was allocated, committed and touched page
//! by page at Arm, so it cannot fail later or wait for memory to be brought in.

pub mod capture;
pub mod cubase;
pub mod engine;
pub mod env;
pub mod host;
pub mod latency;
pub mod metronome;
pub mod names;
pub mod recorder;
pub mod sim;
pub mod sizing;
pub mod sounds;
pub mod system;
pub mod wav;
pub mod writer;

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

pub use recorder::{Environment, Preset, Recorder, Status};
pub use wav::SampleFormat;
