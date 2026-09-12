//! Antelope control server.
//!
//! Safe by default: the transport backend is the hardware-free loopback and the listener
//! binds to localhost unless told otherwise. Driving real hardware is a deliberate act.

use clap::{Parser, ValueEnum};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use antelope_server::device::manager::DeviceManager;
use antelope_server::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
use antelope_server::workspace::store::{JsonFileStore, MemoryStore, WorkspaceStore};
use antelope_server::{http, AppState};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Hardware-free emulator. The default.
    Loopback,
    /// Real USB devices. Not implemented yet.
    Usb,
}

#[derive(Parser, Debug)]
#[command(name = "antelope-server", version, about = "Antelope Audio control server")]
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
}

fn default_workspace_path() -> PathBuf {
    // Honour XDG on Linux, fall back to the home directory, then the current directory.
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("antelope").join("workspace.json");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("antelope")
            .join("workspace.json");
    }
    PathBuf::from("workspace.json")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "antelope_server=info,tower_http=info".into()),
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
    let devices = DeviceManager::new(registries);

    let pids: Vec<u16> = args
        .loopback_models
        .iter()
        .filter_map(|m| match m.as_str() {
            "quadro" => Some(PID_QUADRO),
            "studio" => Some(PID_STUDIO),
            other => {
                tracing::warn!("unknown loopback model '{other}', skipping");
                None
            }
        })
        .collect();
    devices.attach_loopbacks(&pids, 64);

    let store: Arc<dyn WorkspaceStore> = if args.no_persist {
        Arc::new(MemoryStore::default())
    } else {
        let path = args.workspace.clone().unwrap_or_else(default_workspace_path);
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
    let listener = tokio::net::TcpListener::bind(args.bind).await?;

    tracing::info!(
        "listening on http://{} — backend={:?} devices={} dry_run={}",
        args.bind,
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
