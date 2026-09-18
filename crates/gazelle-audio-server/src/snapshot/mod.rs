//! Snapshots: a named record of a whole setup, so it can be compared with the present and, later,
//! recalled part by part (`specs/2026-09-16-workspace-snapshots-and-cross-device-mixer.md` §2).
//!
//! **A snapshot is a record of reads. Nothing here writes to a device**: capture only asks, and
//! `compare` only asks. Recall (phase 6) is a separate, hardware-gated piece of work and attaches
//! at [`capture::capture`] (to read fresh before writing) and [`diff::diff`] (to know what would
//! change), which is why both are plain functions over documents rather than page logic.
//!
//! Snapshots live beside the workspace, one JSON file each, behind [`store::SnapshotStore`]
//! (decisions 0010 and 0011, spec §2.5 option B): a rename must not re-send every captured value,
//! and two clients capturing at once must not overwrite each other.

pub mod capture;
pub mod diff;
pub mod model;
pub mod store;
pub mod time;
