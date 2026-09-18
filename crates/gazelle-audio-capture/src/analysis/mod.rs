//! Analysis pipeline (sub-project 2): pure functions over `UsbEvent`s and step timelines, no
//! I/O, except `run`, which loads a stored probe and writes its results. Spec §8; plan
//! `plans/2026-09-14-capture-2-analysis.md`.

pub mod attribute;
pub mod channel;
pub mod encoding;
pub mod fieldmap;
pub mod group;
pub mod noise;
pub mod run;
pub mod segment;
