//! Gazelle control server.
//!
//! Safe by default: the transport backend is the hardware-free loopback and the listener
//! binds to localhost unless told otherwise. Driving real hardware is a deliberate act.

use clap::{Parser, ValueEnum};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use gazelle_audio_server::config::{default_log_dir, default_themes_dir, default_workspace_path};
use gazelle_audio_server::device::hotplug::{self, HotPlug, Scanner};
use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::device::usb;
use gazelle_audio_server::registry_set::RegistrySet;
use gazelle_audio_server::workspace::store::{JsonFileStore, MemoryStore, WorkspaceStore};
use gazelle_audio_server::tray::{self, boot::BootArgs};
use gazelle_audio_server::update::{self, settings::Settings, Updater};
use gazelle_audio_server::{http, logging, AppState};
use tokio::sync::Notify;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Hardware-free emulator. The default.
    Loopback,
    /// Real USB devices, over the OS HID stack.
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

    /// Directory of user theme JSON files for the web UI. Defaults to `themes` in the config
    /// directory, beside the workspace file.
    #[arg(long)]
    themes_dir: Option<PathBuf>,

    /// Make each loopback device also push its cyclic reports every MS milliseconds, with a
    /// moving test pattern, so clients see state and meter traffic without hardware.
    #[arg(long, value_name = "MS")]
    loopback_cyclic_ms: Option<u64>,

    /// Serve only the API, not the embedded web UI.
    #[arg(long)]
    no_web_ui: bool,

    /// Run without the tray icon, as test harnesses and services do. A server that cannot
    /// create one (no desktop session) also runs without it, and says so.
    #[arg(long)]
    no_tray: bool,

    /// Never look for, or offer, an update. The update settings in the config directory
    /// (`update.json`) decide the rest: the channel, and whether checks happen unasked.
    #[arg(long)]
    no_update: bool,

    /// Also log to a size-capped file in DIR. Tray runs do by default, in the platform's state
    /// folder (on Windows `%LOCALAPPDATA%\gazelle\logs`); `--no-tray` runs only when given this.
    #[arg(long, value_name = "DIR")]
    log_dir: Option<PathBuf>,
}

/// Public so the windowless build (`bin/gazelle-audio-serverw.rs`) runs this same server.
pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let log_dir = logging::init(logging::file_log_dir(!args.no_tray, args.log_dir.clone(), || {
        default_log_dir(|k| std::env::var(k).ok())
    }));
    // The error is printed to the console on return anyway; a log file needs it too, since a
    // server with no terminal (the likeliest reason: its port is taken) leaves nothing else.
    run(&args, log_dir.clone()).inspect_err(|e| {
        if log_dir.is_some() {
            tracing::error!("the server stopped: {e}");
        }
    })
}

fn run(args: &Args, log_dir: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    // Before anything else: a binary an earlier update displaced is nothing but clutter now.
    update::clean_up_after_previous_update();

    let runtime = tokio::runtime::Runtime::new()?;
    let updater = updater(args);
    let (listener, app, devices, hotplug) = runtime.block_on(prepare(args, updater.clone()))?;
    let address = listener.local_addr()?;
    // Set when the tray asks to restart into a staged update; acted on once the server is down.
    let restart = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Quit in the tray and Ctrl-C both end the server the same way.
    let quit = Arc::new(Notify::new());

    // The tray's message loop needs the thread that created the icon, so it takes this one and
    // the server runs on the runtime's workers.
    let tray = if args.no_tray {
        None
    } else {
        let mut context = tray_context(args, address, &devices, &quit, log_dir);
        context.update = updater.clone();
        let asked = restart.clone();
        context.restart = Some(Box::new(move || asked.store(true, std::sync::atomic::Ordering::SeqCst)));
        context.rescan = hotplug.as_ref().map(|hotplug| {
            let rescan = hotplug.rescan();
            Box::new(move || rescan.now()) as Box<dyn Fn()>
        });
        match tray::start(context) {
            Ok(tray) => {
                tracing::info!("tray icon added (--no-tray to run without one)");
                Some(tray)
            }
            Err(e) => {
                tracing::warn!("no tray icon, running headless: {e}");
                None
            }
        }
    };

    if let Some(updater) = updater {
        update::spawn_background_checks(updater);
    }

    let closer = tray.as_ref().map(tray::Tray::closer);
    let server = runtime.spawn(async move {
        let result = serve(listener, app, devices, hotplug, quit).await;
        // Stopped by Ctrl-C or an error rather than the tray: take the icon down too.
        if let Some(closer) = closer {
            closer.close();
        }
        result
    });
    if let Some(tray) = tray {
        tray.run();
    }
    runtime.block_on(server)??;
    if restart.load(std::sync::atomic::Ordering::SeqCst) {
        update::relaunch();
    }
    Ok(())
}

/// The updater, when this server should have one.
///
/// Off with `--no-update` or when the settings say not to check, and **only on a loopback
/// bind**: an update check and a download belong to the machine running the server, not to
/// whoever can reach it over the network, so a server bound anywhere else offers neither the
/// tray items nor the HTTP routes.
fn updater(args: &Args) -> Option<Arc<Updater>> {
    if args.no_update || !args.bind.ip().is_loopback() {
        return None;
    }
    let path = update::settings::default_settings_path(|k| std::env::var(k).ok());
    let (settings, warning) = Settings::load(&path);
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
    }
    if !settings.check {
        tracing::info!("update checks are off in {}", path.display());
        return None;
    }
    match Updater::for_this_build(settings) {
        Ok(updater) => Some(Arc::new(updater)),
        Err(e) => {
            tracing::warn!("no update checks: {e}");
            None
        }
    }
}

/// Everything up to a bound listener: devices attached, workspace store chosen, routes built. For
/// the USB backend, also the scanner that keeps attaching and detaching devices from then on.
async fn prepare(
    args: &Args,
    updater: Option<Arc<Updater>>,
) -> Result<(tokio::net::TcpListener, axum::Router, Arc<DeviceManager>, Option<HotPlug>), Box<dyn std::error::Error>> {
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
    let hotplug = if args.backend == Backend::Usb {
        Some(attach_usb_devices(&devices)?)
    } else {
        match args.loopback_cyclic_ms {
            Some(ms) => devices.attach_cyclic_loopbacks(&pids, 64, std::time::Duration::from_millis(ms.max(1))),
            None => devices.attach_loopbacks(&pids, 64),
        }
        None
    };

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
        themes_dir: Some(args.themes_dir.clone().unwrap_or_else(|| default_themes_dir(|k| std::env::var(k).ok()))),
    };

    let app = http::router(state);
    let app = match updater {
        Some(updater) => app.merge(http::update::routes(updater)),
        None => app,
    };
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

    Ok((listener, app, devices, hotplug))
}

/// Serve until Ctrl-C or Quit, then stop the device workers.
async fn serve(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    devices: Arc<DeviceManager>,
    hotplug: Option<HotPlug>,
    quit: Arc<Notify>,
) -> std::io::Result<()> {
    let shutdown = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = quit.notified() => {}
        }
        tracing::info!("shutting down, stopping device workers");
        // Scanning stops first, so nothing is attached behind the shutdown.
        if let Some(hotplug) = hotplug {
            hotplug.stop();
        }
        devices.shutdown_all();
    };
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await
}

/// What the tray is told about this server, including the arguments a boot entry repeats.
fn tray_context(
    args: &Args,
    address: SocketAddr,
    devices: &Arc<DeviceManager>,
    quit: &Arc<Notify>,
    log_dir: Option<PathBuf>,
) -> tray::Context {
    let backend = format!("{:?}", args.backend).to_lowercase();
    let boot_args = BootArgs {
        bind: args.bind,
        backend: backend.clone(),
        dry_run: args.dry_run,
        workspace: args.workspace.clone(),
        themes_dir: args.themes_dir.clone(),
        log_dir: args.log_dir.clone(),
        loopback_models: args.loopback_models.clone(),
        loopback_cyclic_ms: args.loopback_cyclic_ms,
        no_web_ui: args.no_web_ui,
    };
    let boot_args = match std::env::current_dir() {
        Ok(cwd) => boot_args.absolute(&cwd),
        Err(_) => boot_args,
    };
    let quit = quit.clone();
    tray::Context {
        address,
        backend,
        dry_run: args.dry_run,
        web_ui: cfg!(feature = "web-ui") && !args.no_web_ui,
        devices: devices.clone(),
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gazelle-audio-server")),
        boot_args,
        log_dir,
        quit: Box::new(move || quit.notify_one()),
        rescan: None,
        update: None,
        restart: None,
    }
}

/// Attaches every Antelope control interface the HID stack can open now, then keeps scanning.
///
/// Finding none, or none that opens, no longer stops the server: Antelope's Manager Service holds
/// the devices exclusively while it runs (hardware session 2), and they attach within a scan of it
/// being stopped. The scanner's log says so. Only a HID stack that cannot start at all is fatal.
fn attach_usb_devices(devices: &Arc<DeviceManager>) -> Result<HotPlug, Box<dyn std::error::Error>> {
    let api = hidapi::HidApi::new().map_err(|e| format!("opening the HID stack: {e}"))?;
    let mut scanner = Scanner::new(usb::HidEnumerator::new(api));
    for change in scanner.scan(devices) {
        change.log();
    }
    Ok(HotPlug::spawn(scanner, devices.clone(), hotplug::RESCAN_INTERVAL))
}
