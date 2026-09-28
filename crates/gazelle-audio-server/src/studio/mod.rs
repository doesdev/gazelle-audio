//! The studio hub: this PC as a place that is ready to record at any moment.
//!
//! - [`settings`]: auto-arm and starting in the recording hub, kept in `recording.json` in the
//!   config directory.
//! - [`metronome`]: the metronome's settings, kept in `metronome.json` beside it.
//! - [`auto_arm`]: the rules auto-arm keeps, as plain data. `crate::recording` acts on them.
//!
//! The recording widget and the recording hub themselves are windows of the desktop app
//! (`crate::window`) over the web app's `#/widget` and `#/hub` routes.

pub mod auto_arm;
pub mod metronome;
pub mod settings;

pub use settings::{Studio, StudioSettings};
