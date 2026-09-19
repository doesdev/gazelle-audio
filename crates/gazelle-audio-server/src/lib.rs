//! Multi-device HTTP/WebSocket control server for Antelope Audio interfaces.
//!
//! One server manages **every connected device at once** and exposes the in-scope command
//! surface over HTTP and WebSocket, so a client can present a unified view rather than one
//! panel per device. The stock software binds one control server per device, which is why
//! its panels make you switch between them.
//!
//! # Safety posture
//!
//! The transport backend is the real hardware unless `--backend loopback` asks for the emulator,
//! because driving the interfaces is the whole point of the shipped app; attaching is read-only,
//! and a write is still a deliberate act, with `--dry-run` there to show the bytes instead of
//! sending them. Binding is loopback-only unless explicitly overridden. See
//! [`no_hardware`] for the variable that keeps a test harness off real devices.

/// The build script's icon-resource writer, compiled into the test build so its own tests run.
///
/// `cargo test` never builds a build script as a test target, so a module that only `build.rs`
/// uses would have no tests at all. Including the same file here (and only here, under
/// `cfg(test)`) means the bytes the linker is handed are the bytes a test checked.
#[cfg(test)]
#[allow(dead_code)]
#[path = "../build/resource.rs"]
mod build_resource;

pub mod config;
pub mod device;
pub mod driver;
pub mod error;
pub mod handover;
pub mod http;
pub mod icon;
pub mod install;
pub mod logging;
pub mod no_hardware;
pub mod notice;
pub mod registry_set;
pub mod snapshot;
pub mod tray;
pub mod update;
pub mod value;
pub mod window;
#[cfg(feature = "web-ui")]
pub mod web;
pub mod workspace;
pub mod ws;

use std::sync::Arc;

use crate::device::manager::DeviceManager;
use crate::snapshot::store::SnapshotStore;
use crate::workspace::store::WorkspaceStore;

/// Everything the HTTP and WebSocket layers share.
#[derive(Clone)]
pub struct AppState {
    pub devices: Arc<DeviceManager>,
    pub store: Arc<dyn WorkspaceStore>,
    /// Where snapshots are kept: beside the workspace, one document each.
    pub snapshots: Arc<dyn SnapshotStore>,
    /// When set, every command is non-mutating regardless of per-request options.
    pub force_dry_run: bool,
    /// Whether this server may apply a snapshot to a device at all (`--enable-recall`). Off by
    /// default, and off is not the whole guard: the request must ask as well, and applying is not
    /// built (`http::recall`).
    pub enable_recall: bool,
    pub backend: String,
    /// Where user theme JSON files for the web UI live; `None` lists no user themes.
    pub themes_dir: Option<std::path::PathBuf>,
    /// Brings the desktop window to the front. `None` on a headless server (no window feature,
    /// `--no-window`, or a window that could not be created), and a second launch is told so
    /// rather than left wondering (`handover`).
    pub show_window: Option<ShowWindow>,
}

/// Asks the desktop window to show itself. Called from an HTTP handler on a worker thread, so the
/// window's own thread is reached through whatever the implementation posts to it.
pub type ShowWindow = Arc<dyn Fn() + Send + Sync>;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
