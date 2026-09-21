//! **Measuring what Gazelle Aggregate has left over**, and working out the trim that cancels it.
//!
//! The aggregate lines its interfaces up from the latency figures their drivers report. Those
//! figures are close but not exact, so two interfaces can still record a few tens of samples
//! apart. The cure is a trim per interface in `%APPDATA%\gazelle\aggregate.json`, and until now
//! arriving at one meant recording something and judging two waveforms by eye. This turns it into
//! a measurement.
//!
//! # The idea
//!
//! **Measure through the aggregate, not around it.** Opening the vendor drivers separately and
//! comparing their sample counters compares two things that were never meant to be compared. This
//! opens the aggregate itself, with the configuration the person is actually using, drives it
//! exactly as a DAW does, and reads the aggregate's own input buffers. Everything the aggregate
//! does to line the interfaces up has already happened by then, so whatever offset is left in
//! that buffer **is** the error, in the same coordinates a trim is written in, and adding it to a
//! trim cancels it. It also means no new code has to touch the interfaces: the aggregate already
//! opens them.
//!
//! # How it is put together
//!
//! - [`rig`] is the cabling written down, and everything that can be refused about it.
//! - [`click`] is what is played: a short shaped sweep, not a single sample.
//! - [`measure`] is the arithmetic, and has no hardware anywhere in it: cross correlation between
//!   the captured channels, a parabola through the peak for a fraction of a sample, a median and a
//!   spread across the clicks, how far apart they are allowed to be before their middle means
//!   nothing, and a line through them that catches two interfaces drifting apart.
//! - [`trim`] turns a reading into what to write in the file, and owns the sign rule.
//! - **Witnesses** are extra input channels a rig carries along: recorded and reported like
//!   everything else, and part of no trim. One input per interface is what a trim means, so a
//!   second input on an interface can only ever be an observation, and that is what answers a
//!   question about one interface's own inputs in a single run.
//! - [`session`] is the host: it drives the aggregate, plays the clicks and keeps what came back.
//!
//! # The sign rule
//!
//! **The interface whose copy of the click lands later is recording late, and it takes a positive
//! trim.** [`trim`] says why, and the end to end test proves it by cabling one interface late on
//! purpose and nulling it.
//!
//! # What it refuses
//!
//! `GAZELLE_NO_HARDWARE` set, a rig whose cabling could not measure what it claims to, an
//! interface the aggregate has not got, fewer than two interfaces, a driver that is already open,
//! and anything the aggregate itself refuses, in the aggregate's own words. Nothing is opened and
//! no sample is played until every one of them has passed.
//!
//! # What it will not turn into a trim
//!
//! A reading whose clicks did not agree with each other, one taken while the audio underneath lost
//! a block, one that drifts, and one where nothing arrived at all. Each of them is reported in
//! full, with what happened and what to do about it, and the trim in the file is left exactly
//! where it was.

pub mod click;
pub mod measure;
pub mod rig;
pub mod session;
pub mod trim;

#[cfg(test)]
mod end_to_end;

pub use measure::{ClickLag, Drift, Reading};
pub use rig::{Direction, Rig, Settings, LOUDEST_DBFS};
pub use session::{Outcome, Witness, NO_HARDWARE};
pub use trim::TrimChange;

#[cfg(windows)]
pub use session::measure as measure_here;
