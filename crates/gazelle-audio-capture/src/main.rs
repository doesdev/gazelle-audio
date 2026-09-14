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
use gazelle_audio_capture::session::controller::{ControlError, Controller};
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, ProbePlan};
use gazelle_audio_capture::session::step::{RunStatus, StepError, StepTiming};
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

/// A Ctrl-C listener, created once and kept alive for the whole of `serve`. Both
/// `tokio::signal::unix::signal` and `tokio::signal::windows::ctrl_c` install the OS-level
/// handler synchronously when called — unlike the `tokio::signal::ctrl_c()` convenience
/// wrapper, which is an async fn and (per its docs) only registers on its returned future's
/// first poll — and both can be `recv()`-ed more than once. Creating one of these up front,
/// before any of the blocking work below runs, means a signal arriving during that work is
/// captured (recorded by the OS-level handler this installs) instead of falling through to the
/// default disposition, which would kill the process outright.
struct SigintListener(
    #[cfg(unix)] tokio::signal::unix::Signal,
    #[cfg(windows)] tokio::signal::windows::CtrlC,
);

#[cfg(unix)]
impl SigintListener {
    fn new() -> std::io::Result<Self> {
        Ok(Self(tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?))
    }
}

#[cfg(windows)]
impl SigintListener {
    fn new() -> std::io::Result<Self> {
        Ok(Self(tokio::signal::windows::ctrl_c()?))
    }
}

impl SigintListener {
    async fn recv(&mut self) {
        self.0.recv().await;
    }
}

async fn serve(args: ServeArgs) -> Result<(), BoxError> {
    // Registered before any of the blocking work below (hub discovery, the Enter wait) runs —
    // see `SigintListener`'s doc comment for why that ordering matters.
    let mut ctrl_c = SigintListener::new()?;
    let args = Arc::new(args);
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

    // From here on a Ctrl-C can arrive at any point: while a hub is being discovered, while
    // waiting for Enter, while `start_probe` is already spawning the capture thread, or once
    // the probe is running. Before this point nothing has been started (no capture thread, no
    // marks appended), so there is nothing yet to clean up — but `ctrl_c` (registered above,
    // before any of that blocking work ran) has already installed the OS-level handler, so a
    // Ctrl-C here is captured and queued, not left to the default disposition (which would kill
    // the process); it is just not *observed* (via `ctrl_c.recv()`) until Phase 1a's `select!`,
    // below.

    // Phase 1a: prepare the capture source. `build_source` (specifically USBPcap hub discovery)
    // runs on a blocking thread rather than directly in this async task — partly so it cannot
    // stall the runtime, but importantly so `select!` can actually observe `ctrl_c` while it
    // runs instead of being stuck inside `build_source`'s own, fully synchronous call stack.
    // Discovery has its own bounded (~3s-per-hub) watchdog and, on Windows, its own child
    // process to clean up (in its own process group, so it won't see a console Ctrl-C itself);
    // a Ctrl-C here therefore waits for the `JoinHandle` (bounded by that watchdog, and itself
    // interruptible by a second Ctrl-C via `wait_or_force_exit`) rather than abandoning it and
    // orphaning that child.
    let args_for_source = Arc::clone(&args);
    let (vid, pid) = (info.vid, info.pid);
    let mut build_handle = tokio::task::spawn_blocking(move || build_source(&args_for_source, vid, pid));
    let source = tokio::select! {
        result = &mut build_handle => result??,
        _ = ctrl_c.recv() => {
            eprintln!("interrupted while preparing the capture source; waiting for it to finish");
            let _ = wait_or_force_exit(&mut build_handle, &mut ctrl_c).await;
            println!("interrupted before the probe started; nothing to stop");
            std::process::exit(0);
        }
    };

    // Phase 1b: wait for the operator to press Enter (skipped with `--no-wait`). Unlike phase
    // 1a, reading stdin can block indefinitely (a helper launched with no terminal, a
    // held-open pipe, ...), so there is nothing here worth waiting for: a Ctrl-C exits at once
    // rather than risk hanging on `tokio::main`'s runtime teardown, which — since dropping the
    // runtime waits for outstanding `spawn_blocking` tasks — would otherwise wait forever for
    // that blocked read to return.
    if !args.no_wait {
        println!("open the panel, then press Enter to start the probe");
        let mut read_handle = tokio::task::spawn_blocking(|| std::io::stdin().read_line(&mut String::new()));
        let n = tokio::select! {
            result = &mut read_handle => result??,
            _ = ctrl_c.recv() => {
                println!("interrupted before the probe started; nothing to stop");
                std::process::exit(0);
            }
        };
        if n == 0 {
            // EOF: read_line returns Ok(0) immediately for closed/redirected stdin (a helper
            // launched with no terminal, `/dev/null`, a finished pipe). Treat that as an error
            // rather than silently starting the probe unattended.
            return Err("stdin closed; use --no-wait".into());
        }
    }

    // `Controller::start_probe` starts the capture thread and can block briefly on file I/O
    // while the controller's inner lock is held; run it on a blocking thread so it cannot
    // stall this async task (or, on a current-thread runtime, every other task). Keep the
    // `JoinHandle` in a local so a losing Ctrl-C branch below can still wait on it afterward:
    // a blocking task can't be cancelled once spawned, only waited out.
    let probe_id = planned.probe_id.clone();
    let start_controller = controller.clone();
    let mut start_handle = tokio::task::spawn_blocking(move || start_controller.start_probe(&probe_id, source));
    tokio::select! {
        result = &mut start_handle => result??,
        _ = ctrl_c.recv() => {
            eprintln!("interrupted while starting; waiting for it to finish, then stopping it");
            abandon_after_start(&mut start_handle, &controller, &mut ctrl_c).await?;
            return Ok(());
        }
    }
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
            ctrl_c.recv().await;
        }
        _ = ctrl_c.recv() => {
            abandon(&controller, &mut ctrl_c).await?;
        }
    }
    Ok(())
}

/// Waits for `handle`, but exits the process immediately (rather than swallow a second signal)
/// if another Ctrl-C arrives first: a blocking task can't be cancelled, only waited out, and
/// the operator should not be stuck if it — or the capture source it is stopping — hangs.
async fn wait_or_force_exit<T>(handle: &mut tokio::task::JoinHandle<T>, ctrl_c: &mut SigintListener) -> Result<T, tokio::task::JoinError> {
    tokio::select! {
        result = handle => result,
        _ = ctrl_c.recv() => {
            eprintln!("cleanup interrupted; exiting immediately");
            std::process::exit(130);
        }
    }
}

/// Ctrl-C arrived while `start_probe` was already spawning the capture thread: there is no way
/// to cancel that blocking call, only wait for it, then stop what it started (a probe is now
/// running only if it succeeded — if it failed there is nothing to abandon, and its error is
/// surfaced as-is).
async fn abandon_after_start(
    start_handle: &mut tokio::task::JoinHandle<Result<(), ControlError>>,
    controller: &Controller,
    ctrl_c: &mut SigintListener,
) -> Result<(), BoxError> {
    wait_or_force_exit(start_handle, ctrl_c).await??;
    abandon(controller, ctrl_c).await
}

/// Cancels the running probe. `Controller::abandon_probe` now always runs `finish_capture` —
/// stopping the source and joining the capture thread — once the run is no longer `Running`,
/// even when appending the abandon mark itself fails (final review F2), so a returned `Err`
/// here means only that the mark, or `finish_capture`'s own descriptor-metadata update, failed
/// to land — not that the capture was left unstopped or unjoined. That's still surfaced (rather
/// than swallowed), along with a task panic, since either means the shutdown was not fully clean
/// and `serve` should exit non-zero so the operator can see it.
///
/// `StepError::NotRunning` is not such a failure: it means Ctrl-C raced the probe's own natural
/// completion (final review F4) — `finished` (in `serve`, above) already observed a terminal
/// status through the same controller, so there is nothing left to abandon. Treat that as a
/// clean exit rather than reporting a shutdown failure.
async fn abandon(controller: &Controller, ctrl_c: &mut SigintListener) -> Result<(), BoxError> {
    let controller = controller.clone();
    let mut handle = tokio::task::spawn_blocking(move || controller.abandon_probe());
    match wait_or_force_exit(&mut handle, ctrl_c).await {
        Ok(Ok(())) | Ok(Err(ControlError::Step(StepError::NotRunning))) => Ok(()),
        Ok(Err(e)) => {
            eprintln!("error: could not abandon the probe cleanly: {e}");
            Err(e.into())
        }
        Err(e) => {
            eprintln!("error: abandon task panicked: {e}");
            Err(e.into())
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn controller_with_a_finished_probe(dir: &std::path::Path) -> Controller {
        let store = SessionStore::create(dir, &SessionInfo { vid: 0x1234, pid: 0xABCD, ..SessionInfo::default() }).unwrap();
        store
            .declare_parameter(Parameter { id: "monitor_level".into(), label: "Monitor level".into(), kind: ParameterKind::Continuous, domain: ParameterDomain::default(), location: String::new() })
            .unwrap();
        store
            .declare_parameter(Parameter {
                id: "mute".into(),
                label: "Mute".into(),
                kind: ParameterKind::Toggle,
                domain: ParameterDomain { values: vec!["off".into(), "on".into()], unit: None },
                location: String::new(),
            })
            .unwrap();
        let controller = Controller::new(store, Arc::new(SystemClock), StepTiming::default(), Some(false)).unwrap();
        let plan = ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into()], sweep: vec![], repeats: 1, control_parameter: "mute".into() };
        let planned = controller.plan_probe(plan, 1).unwrap();
        controller.start_probe(&planned.probe_id, Box::new(MemorySource::new("memory", Vec::new()))).unwrap();
        // Abandoning it "for real" leaves the run in the same left-`Running` state that a
        // natural completion (`RunStatus::Completed`) would also leave: `abandon_probe` is only
        // reachable while `Running`.
        controller.abandon_probe().unwrap();
        controller
    }

    /// Final review F4: if Ctrl-C wins `serve`'s `select!` right as the probe finishes on its
    /// own, `abandon_probe` sees a run that has already left `Running` and returns
    /// `StepError::NotRunning` — not a real shutdown failure. `abandon` (above) must treat that
    /// specific error as a clean exit, matched on the actual `ControlError` variant, rather than
    /// reporting "could not abandon the probe cleanly" and exiting non-zero.
    ///
    /// This drives the exact code path the race would hit (`abandon_probe` called when the run
    /// is no longer `Running`) without needing to reproduce the `select!` race's timing from
    /// outside the process, which an external, process-level test (`tests/cli.rs`) cannot do
    /// deterministically — see the final fix report for why no such test is added there.
    #[tokio::test]
    async fn abandon_treats_a_not_running_probe_as_a_clean_exit() {
        let dir = tempfile::tempdir().unwrap();
        let controller = controller_with_a_finished_probe(dir.path());
        let mut ctrl_c = SigintListener::new().unwrap();
        assert!(abandon(&controller, &mut ctrl_c).await.is_ok());
    }
}
