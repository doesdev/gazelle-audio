//! Gazelle control server.
//!
//! Safe by default: the transport backend is the hardware-free loopback and the listener
//! binds to localhost unless told otherwise. Driving real hardware is a deliberate act.

use clap::{Parser, ValueEnum};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use gazelle_audio_server::config::default_workspace_path;
use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::registry_set::RegistrySet;
use gazelle_audio_server::workspace::store::{JsonFileStore, MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Hardware-free emulator. The default.
    Loopback,
    /// Real USB devices. Not implemented yet.
    Usb,
}

#[derive(Parser, Debug)]
#[command(name = "gazelle-audio-server", version, about = "Gazelle control server for Antelope Audio interfaces")]
struct Args {
    /// Address to bind. Defaults to localhost; override only deliberately.
    #[arg(long, default_value = "127.0.0.1:8420")]
    bind: SocketAddr,

    /// Transport backend.
    #[arg(long, value_enum, default_value_t = Backend::Loopback)]
    backend: Backend,

    /// Never write to a device: report the bytes each command would send.
    #[arg(long)]
    dry_run: bool,

    /// Where workspace state is stored. Defaults to a platform config path.
    #[arg(long)]
    workspace: Option<PathBuf>,

    /// Keep workspace state in memory only.
    #[arg(long)]
    no_persist: bool,

    /// Which loopback devices to create, by model.
    #[arg(long, value_delimiter = ',', default_values_t = ["quadro".to_string(), "studio".to_string()])]
    loopback_models: Vec<String>,

    /// Make each loopback device also push its cyclic reports every MS milliseconds, with a
    /// moving test pattern, so clients see state and meter traffic without hardware.
    #[arg(long, value_name = "MS")]
    loopback_cyclic_ms: Option<u64>,

    /// Serve only the API, not the embedded web UI.
    #[cfg_attr(not(feature = "web-ui"), allow(dead_code))]
    #[arg(long)]
    no_web_ui: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gazelle_audio_server=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();

    if args.backend == Backend::Usb {
        // Refuse clearly rather than pretending: no physical adapter exists yet, and
        // silently falling back to loopback would be worse than failing.
        eprintln!(
            "error: the USB backend is not implemented yet.\n\
             \n\
             The protocol and transport layers are validated against ground truth, but no\n\
             physical adapter has been written, and neither device has ever been driven by\n\
             this software. Run with --backend loopback (the default) meanwhile."
        );
        std::process::exit(2);
    }

    let registries = RegistrySet::builtin().map_err(|e| format!("loading registries: {e}"))?;

    let pids: Vec<u16> = args
        .loopback_models
        .iter()
        .filter_map(|m| {
            let pid = registries.models().find(|(_, r)| r.family == m.as_str()).map(|(pid, _)| *pid);
            if pid.is_none() {
                tracing::warn!("unknown loopback model '{m}', skipping");
            }
            pid
        })
        .collect();

    let devices = DeviceManager::new(registries);
    match args.loopback_cyclic_ms {
        Some(ms) => devices.attach_cyclic_loopbacks(&pids, 64, std::time::Duration::from_millis(ms.max(1))),
        None => devices.attach_loopbacks(&pids, 64),
    }

    let store: Arc<dyn WorkspaceStore> = if args.no_persist {
        Arc::new(MemoryStore::default())
    } else {
        let path = args
            .workspace
            .clone()
            .unwrap_or_else(|| default_workspace_path(|k| std::env::var(k).ok()));
        tracing::info!("workspace: {}", path.display());
        Arc::new(JsonFileStore::new(path))
    };

    let state = AppState {
        devices: devices.clone(),
        store,
        force_dry_run: args.dry_run,
        backend: format!("{:?}", args.backend).to_lowercase(),
    };

    let app = http::router(state);
    #[cfg(feature = "web-ui")]
    let app = if args.no_web_ui { app } else { gazelle_audio_server::web::with_ui(app) };
    let listener = tokio::net::TcpListener::bind(args.bind).await?;

    // The bound address, not `args.bind`: with port 0 this log line is how another process (the
    // web client's integration tests) finds the server.
    tracing::info!(
        "listening on http://{} — backend={:?} devices={} dry_run={}",
        listener.local_addr()?,
        args.backend,
        devices.len(),
        args.dry_run
    );
    if !args.bind.ip().is_loopback() {
        tracing::warn!(
            "bound to a non-loopback address ({}); this exposes device control to the network",
            args.bind.ip()
        );
    }

    let shutdown = {
        let devices = devices.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down, stopping device workers");
            devices.shutdown_all();
        }
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
