use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use gazelle_audio_capture::capture::import::{ImportSource, MemorySource};
use gazelle_audio_capture::capture::pipeline::{DeviceFilter, PayloadPolicy, Pipeline};
use gazelle_audio_capture::capture::usbpcap::{self, UsbPcapConfig, UsbPcapSource, DEFAULT_EXE};
use gazelle_audio_capture::capture::CaptureSource;
use gazelle_audio_capture::panel::{self, security, PanelApp};
use gazelle_audio_capture::session::clock::{Clock, SystemClock};
use gazelle_audio_capture::session::controller::Controller;
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, ProbePlan};
use gazelle_audio_capture::session::step::{RunStatus, StepTiming};
use gazelle_audio_capture::session::store::{SessionInfo, SessionStore};
use gazelle_audio_capture::synth::device::{DeviceModel, SimpleDevice};
use gazelle_audio_capture::synth::frames::device_frames;
use gazelle_audio_capture::synth::session::{generate_session, ScriptedOperator, SynthSpec};
use serde::Deserialize;

#[derive(Parser)]
#[command(name = "gazelle-capture", version, about = "Vendor-neutral USB capture loop for audio interface parameters")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the companion panel and run one probe.
    Serve(ServeArgs),
    /// Decode a .pcap/.pcapng file to JSON lines of UsbEvent.
    Import(ImportArgs),
    /// Generate a synthetic session directory.
    Synth(SynthArgs),
    /// List USBPcap root hubs and attached devices (Windows).
    Hubs {
        #[arg(long, default_value = DEFAULT_EXE)]
        usbpcap_exe: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SourceKind {
    Usbpcap,
    Import,
    Demo,
}

#[derive(Args)]
struct ServeArgs {
    /// Session directory; created when it holds no session.json.
    #[arg(long)]
    session: PathBuf,
    /// Target VID (hex with 0x, or decimal); required when creating the session.
    #[arg(long, value_parser = parse_u16)]
    vid: Option<u16>,
    #[arg(long, value_parser = parse_u16)]
    pid: Option<u16>,
    /// JSON file: {"parameters": [...], "plan": {...}}.
    #[arg(long)]
    plan: PathBuf,
    /// 0 picks a free port.
    #[arg(long, default_value_t = 8430)]
    port: u16,
    #[arg(long, value_enum, default_value_t = SourceKind::Usbpcap)]
    source: SourceKind,
    /// Capture file for --source import.
    #[arg(long)]
    file: Option<PathBuf>,
    /// USBPcap control device, e.g. \\.\USBPcap1; found by VID/PID when omitted.
    #[arg(long)]
    hub: Option<String>,
    #[arg(long, default_value = DEFAULT_EXE)]
    usbpcap_exe: PathBuf,
    #[arg(long)]
    keep_stream_payloads: bool,
    #[arg(long)]
    seed: Option<u64>,
    /// Start the probe immediately instead of waiting for Enter.
    #[arg(long)]
    no_wait: bool,
}

#[derive(Args)]
struct ImportArgs {
    file: PathBuf,
    #[arg(long, value_parser = parse_u16, requires = "pid")]
    vid: Option<u16>,
    #[arg(long, value_parser = parse_u16, requires = "vid")]
    pid: Option<u16>,
    #[arg(long, requires = "device", conflicts_with = "vid")]
    bus: Option<u16>,
    #[arg(long, requires = "bus")]
    device: Option<u16>,
    #[arg(long)]
    keep_stream_payloads: bool,
}

#[derive(Args)]
struct SynthArgs {
    dir: PathBuf,
    /// 249 (USBPcap) or 220 (usbmon).
    #[arg(long, default_value_t = 249)]
    link_type: u32,
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

#[derive(Deserialize)]
struct ProbeFile {
    parameters: Vec<Parameter>,
    plan: ProbePlan,
}

fn parse_u16(s: &str) -> Result<u16, String> {
    let parsed = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u16::from_str_radix(hex, 16),
        None => s.parse(),
    };
    parsed.map_err(|e| format!("{s:?}: {e}"))
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_writer(std::io::stderr).init();
    let result = match Cli::parse().command {
        Command::Serve(args) => serve(args).await,
        Command::Import(args) => import(args),
        Command::Synth(args) => synth(args),
        Command::Hubs { usbpcap_exe } => hubs(&usbpcap_exe),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn open_or_create(args: &ServeArgs) -> Result<SessionStore, BoxError> {
    if args.session.join("session.json").is_file() {
        return Ok(SessionStore::open(&args.session)?);
    }
    let (Some(vid), Some(pid)) = (args.vid, args.pid) else {
        return Err("creating a session needs --vid and --pid".into());
    };
    Ok(SessionStore::create(&args.session, &SessionInfo { vid, pid, ..SessionInfo::default() })?)
}

fn build_source(args: &ServeArgs, vid: u16, pid: u16) -> Result<Box<dyn CaptureSource>, BoxError> {
    Ok(match args.source {
        SourceKind::Usbpcap => {
            let hub = match &args.hub {
                Some(h) => h.clone(),
                None => usbpcap::find_hub(&args.usbpcap_exe, vid, pid)?
                    .ok_or_else(|| format!("no USBPcap root hub has a device {vid:04x}:{pid:04x}"))?,
            };
            Box::new(UsbPcapSource::new(UsbPcapConfig::new(&args.usbpcap_exe, hub)))
        }
        SourceKind::Import => Box::new(ImportSource::new(args.file.clone().ok_or("--source import needs --file")?)),
        SourceKind::Demo => {
            let mut devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(SimpleDevice::new(vid, pid, 1, 5)), Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3))];
            Box::new(MemorySource::new("demo", device_frames(&mut devices, SystemClock.now_ns(), 60_000_000_000)))
        }
    })
}

async fn serve(args: ServeArgs) -> Result<(), BoxError> {
    let store = open_or_create(&args)?;
    if args.keep_stream_payloads {
        store.update_info(|i| i.keep_stream_payloads = true)?;
    }
    let probe_file: ProbeFile = serde_json::from_slice(&std::fs::read(&args.plan)?)?;
    let existing = store.parameters()?;
    for p in probe_file.parameters {
        match existing.iter().find(|e| e.id == p.id) {
            Some(e) if *e == p => {}
            Some(_) => return Err(format!("parameter {} is already declared differently", p.id).into()),
            None => store.declare_parameter(p)?,
        }
    }
    let info = store.info()?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    // is_elevated() is independent of USBPcap hub discovery (it just inspects the process
    // token), so the panel always shows the correct elevation state even if `build_source`
    // below fails or is never reached, and regardless of `--source`/`--hub`.
    let controller = Controller::new(store, Arc::clone(&clock), StepTiming::default(), usbpcap::is_elevated())?;
    let planned = controller.plan_probe(probe_file.plan, args.seed.unwrap_or_else(|| clock.now_ns()))?;

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", args.port)).await?;
    let port = listener.local_addr()?.port();
    let token = security::generate_token();
    if let Some(path) = security::token_path(|k| std::env::var(k).ok()) {
        if let Err(e) = security::write_token(&path, &token) {
            tracing::warn!(error = %e, path = %path.display(), "could not write token file");
        }
    }
    println!("panel: http://127.0.0.1:{port}/");
    println!("token: {token}");
    println!("probe {}: {} steps", planned.probe_id, planned.steps.len());
    std::io::stdout().flush()?;
    let app = PanelApp { controller: controller.clone(), token: token.into(), port };
    tokio::spawn(panel::serve(listener, app));
    panel::spawn_ticker(controller.clone(), Duration::from_millis(100));

    let source = build_source(&args, info.vid, info.pid)?;
    if !args.no_wait {
        println!("open the panel, then press Enter to start the probe");
        tokio::task::spawn_blocking(|| std::io::stdin().read_line(&mut String::new())).await??;
    }
    // `Controller::start_probe` starts the capture thread and can block briefly on file I/O
    // while the controller's inner lock is held; run it on a blocking thread so it cannot
    // stall this async task (or, on a current-thread runtime, every other task), and surface
    // a JoinError with `?` rather than swallow it.
    let probe_id = planned.probe_id.clone();
    let start_controller = controller.clone();
    tokio::task::spawn_blocking(move || start_controller.start_probe(&probe_id, source)).await??;
    println!("probe {} started", planned.probe_id);

    let mut rx = controller.subscribe();
    let finished = async {
        loop {
            if rx.changed().await.is_err() {
                return None;
            }
            let status = rx.borrow_and_update().probe.as_ref().map(|p| p.status);
            if let Some(s @ (RunStatus::Completed | RunStatus::Abandoned)) = status {
                return Some(s);
            }
        }
    };
    tokio::select! {
        status = finished => {
            println!("probe {} {:?}; press Ctrl-C to exit", planned.probe_id, status);
            tokio::signal::ctrl_c().await?;
        }
        _ = tokio::signal::ctrl_c() => {
            // `Controller::abandon_probe` can also join the capture thread while holding the
            // controller's inner lock; run it off the async runtime thread too.
            let abandon_controller = controller.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || abandon_controller.abandon_probe()).await {
                tracing::warn!(error = %e, "abandon_probe task panicked");
            }
        }
    }
    Ok(())
}

fn import(args: ImportArgs) -> Result<(), BoxError> {
    let filter = match (args.vid, args.pid, args.bus, args.device) {
        (Some(vid), Some(pid), _, _) => DeviceFilter::Target { vid, pid },
        (_, _, Some(bus), Some(device)) => DeviceFilter::Address { bus, device },
        _ => DeviceFilter::All,
    };
    let mut pipeline = Pipeline::new(filter, PayloadPolicy { keep_stream_payloads: args.keep_stream_payloads });
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let mut errors = 0u64;
    for frame in ImportSource::new(&args.file).start()?.frames {
        match pipeline.process(frame?) {
            Ok(stored) => {
                for (_, ev) in stored {
                    serde_json::to_writer(&mut out, &ev)?;
                    out.write_all(b"\n")?;
                }
            }
            Err(_) => errors += 1,
        }
    }
    out.flush()?;
    if errors > 0 {
        eprintln!("{errors} frames could not be decoded");
    }
    Ok(())
}

fn synth(args: SynthArgs) -> Result<(), BoxError> {
    let levels = ["0 dB", "-6 dB", "-12 dB"];
    let parameters = vec![
        Parameter { id: "monitor_level".into(), label: "Monitor level".into(), kind: ParameterKind::Discrete, domain: ParameterDomain { values: levels.iter().map(|s| s.to_string()).collect(), unit: Some("dB".into()) }, location: "Output section".into() },
        Parameter { id: "mute".into(), label: "Mute".into(), kind: ParameterKind::Toggle, domain: ParameterDomain { values: vec!["off".into(), "on".into()], unit: None }, location: "Output section".into() },
    ];
    let spec = SynthSpec {
        parameters,
        plan: ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into()], sweep: vec!["-12 dB".into()], repeats: 3, control_parameter: "mute".into() },
        seed: args.seed,
        timing: StepTiming::default(),
        start_ns: SystemClock.now_ns(),
        operator: ScriptedOperator::default(),
        link_type: args.link_type,
        policy: PayloadPolicy::default(),
    };
    let devices: Vec<Box<dyn DeviceModel>> = vec![
        Box::new(SimpleDevice::new(0x1234, 0xABCD, 1, 5).with_parameter("monitor_level", &levels).with_parameter("mute", &["off", "on"])),
        Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3)),
    ];
    let result = generate_session(&args.dir, spec, devices)?;
    println!("{}", result.capture.display());
    println!("{} events, probe {}", result.events.len(), result.probe_id);
    Ok(())
}

fn hubs(exe: &Path) -> Result<(), BoxError> {
    for (hub, devices) in usbpcap::list_hubs(exe)? {
        println!("{} ({})", hub.value, hub.display);
        for d in devices {
            println!("  [{}] {}", d.address, d.display);
        }
    }
    Ok(())
}
