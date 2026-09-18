//! Multi-device HTTP/WebSocket control server for Antelope Audio interfaces.
//!
//! One server manages **every connected device at once** and exposes the in-scope command
//! surface over HTTP and WebSocket, so a client can present a unified view rather than one
//! panel per device. The stock software binds one control server per device, which is why
//! its panels make you switch between them.
//!
//! # Safety posture
//!
//! The default transport backend is the hardware-free loopback, and binding is loopback-only
//! unless explicitly overridden. Driving real hardware is a deliberate act. See
//! `.agent/decisions/0012-loopback-default-and-dry-run.md`.

pub mod config;
pub mod device;
pub mod error;
pub mod handover;
pub mod http;
pub mod icon;
pub mod logging;
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
    /// Where snapshots are kept: beside the workspace, one document each (decision 0011).
    pub snapshots: Arc<dyn SnapshotStore>,
    /// When set, every command is non-mutating regardless of per-request options.
    pub force_dry_run: bool,
    pub backend: String,
    /// Where user theme JSON files for the web UI live; `None` lists no user themes.
    pub themes_dir: Option<std::path::PathBuf>,
    /// Brings the desktop window to the front. `None` on a headless server — no window feature,
    /// `--no-window`, or a window that could not be created — and a second launch is told so
    /// rather than left wondering (`handover`).
    pub show_window: Option<ShowWindow>,
}

/// Asks the desktop window to show itself. Called from an HTTP handler on a worker thread, so the
/// window's own thread is reached through whatever the implementation posts to it.
pub type ShowWindow = Arc<dyn Fn() + Send + Sync>;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
