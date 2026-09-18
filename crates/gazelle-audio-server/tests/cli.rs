//! The binary as a harness starts it: `--bind 127.0.0.1:0` must report the port it really bound,
//! since that is the only way a separate process (the web client's tests) can find it. Also where
//! it logs, and the windowless build.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gazelle_audio_server::no_hardware;

const BIN: &str = env!("CARGO_BIN_EXE_gazelle-audio-server");
const WINDOWLESS_BIN: &str = env!("CARGO_BIN_EXE_gazelle-audio-serverw");

/// How every server in this file is started. **`--backend loopback` is not optional here**:
/// `--backend` defaults to `usb` (decision `0018`), so a harness that leaves it out opens the
/// devices on the machine running the tests. `GAZELLE_NO_HARDWARE` in the child's environment is
/// the belt to this brace — see [`no_hardware`].
const BASE: [&str; 7] =
    ["--bind", "127.0.0.1:0", "--backend", "loopback", "--no-persist", "--no-web-ui", "--no-tray"];

/// Kills the server however the test ends.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Every output line, from stdout and stderr alike, until the process closes them.
fn lines(child: &mut Child) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let streams: [Box<dyn Read + Send>; 2] = [Box::new(child.stdout.take().unwrap()), Box::new(child.stderr.take().unwrap())];
    for stream in streams {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
    }
    rx
}

/// A fresh folder under the system temp directory, removed again when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("gazelle-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The port in a "listening on" line, if this is one.
fn listening_port(line: &str) -> Option<u16> {
    let rest = line.split("listening on http://127.0.0.1:").nth(1)?;
    rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

fn assert_healthy(port: u16) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(stream, "GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"status\":\"ok\""), "{response}");
}

/// A server started as the harnesses start it, plus `extra`, with every folder its log could
/// default to pointed at `home`.
fn start(bin: &str, extra: &[&str], home: &Path) -> Child {
    Command::new(bin)
        .args(BASE)
        .args(extra)
        .env(no_hardware::VAR, "1")
        .env("LOCALAPPDATA", home)
        .env("XDG_STATE_HOME", home)
        .env("HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
fn port_zero_reports_the_bound_port() {
    let mut child = Command::new(BIN)
        .args(BASE)
        .env(no_hardware::VAR, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let rx = lines(&mut child);
    let _server = Server(child);

    let port = loop {
        let line = rx.recv_timeout(Duration::from_secs(20)).expect("the server logs where it listens");
        if let Some(rest) = line.split("listening on http://127.0.0.1:").nth(1) {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            break digits.parse::<u16>().unwrap();
        }
    };
    assert_ne!(port, 0, "the log names the requested port 0, not the bound one");

    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(stream, "GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"status\":\"ok\""), "{response}");
}

/// Waits for the "listening on" line on the console and returns its port.
fn console_port(rx: &mpsc::Receiver<String>) -> u16 {
    loop {
        let line = rx.recv_timeout(Duration::from_secs(20)).expect("the server logs where it listens");
        if let Some(port) = listening_port(&line) {
            return port;
        }
    }
}

#[test]
fn a_no_tray_run_writes_no_log_file() {
    let home = TempDir::new("no-tray");
    let mut child = start(BIN, &[], &home.0);
    let rx = lines(&mut child);
    let _server = Server(child);
    assert_healthy(console_port(&rx));
    let left: Vec<_> = std::fs::read_dir(&home.0).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(left.is_empty(), "a --no-tray run left {left:?} where logs go by default");
}

#[test]
fn log_dir_writes_the_log_to_a_file_as_well() {
    let home = TempDir::new("log-dir");
    let logs = home.0.join("my logs");
    let mut child = start(BIN, &["--log-dir", logs.to_str().unwrap()], &home.0);
    let rx = lines(&mut child);
    let _server = Server(child);
    let port = console_port(&rx);
    let text = std::fs::read_to_string(logs.join("gazelle.log")).unwrap();
    assert_eq!(text.lines().find_map(listening_port), Some(port), "{text}");
    assert!(!text.contains('\x1b'), "no colour codes in the file: {text:?}");
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 1, "only the named folder is written");
}

/// The subsystem field of a Windows executable's PE header: 2 is a windowed program, 3 a console
/// one.
#[cfg(windows)]
fn pe_subsystem(path: &str) -> u16 {
    let bytes = std::fs::read(path).unwrap();
    let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    // The optional header follows the 4-byte signature and the 20-byte file header; the subsystem
    // is at offset 68 in it, for PE32 and PE32+ alike.
    let at = pe + 24 + 68;
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

#[cfg(windows)]
#[test]
fn the_windowless_build_is_a_windowed_program_and_the_server_a_console_one() {
    const WINDOWS_GUI: u16 = 2;
    const WINDOWS_CUI: u16 = 3;
    assert_eq!(pe_subsystem(WINDOWLESS_BIN), WINDOWS_GUI, "Windows would give it a console window");
    assert_eq!(pe_subsystem(BIN), WINDOWS_CUI, "the harnesses and a terminal read the console build's output");
}

#[test]
fn the_windowless_build_runs_the_same_server_and_logs_to_its_file() {
    let home = TempDir::new("windowless");
    let logs = home.0.join("logs");
    let mut child = start(WINDOWLESS_BIN, &["--log-dir", logs.to_str().unwrap()], &home.0);
    // Drained so a full pipe never stalls it; the test reads only the file.
    let _output = lines(&mut child);
    let _server = Server(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    let port = loop {
        let text = std::fs::read_to_string(logs.join("gazelle.log")).unwrap_or_default();
        if let Some(port) = text.lines().find_map(listening_port) {
            break port;
        }
        assert!(Instant::now() < deadline, "the windowless server logged no port to its file: {text}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_healthy(port);
}

/// The whole response to a GET, as text, so a test can read a status line and a body.
fn http_get(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

/// A config directory holding update settings that point at a **closed local port**, so the
/// server's own start-up check fails at once and nothing in this test can reach the real
/// release source.
fn config_with_dead_release_source(home: &Path) -> PathBuf {
    let config = home.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("update.json"),
        r#"{"check":true,"interval_hours":0,"api_base":"http://127.0.0.1:1","repo":"gazelle/none"}"#,
    )
    .unwrap();
    config
}

fn start_with_config(extra: &[&str], home: &Path, config: &Path) -> Child {
    Command::new(BIN)
        .args(BASE)
        .args(extra)
        .env(no_hardware::VAR, "1")
        .env("GAZELLE_CONFIG_DIR", config)
        .env("LOCALAPPDATA", home)
        .env("XDG_STATE_HOME", home)
        .env("HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Proves `main.rs` really merges the update routes on a loopback bind — the only test that
/// exercises the wiring rather than the module.
#[test]
fn a_loopback_server_serves_the_update_endpoint_and_names_its_version() {
    let home = TempDir::new("update-endpoint");
    let config = config_with_dead_release_source(&home.0);
    let mut child = start_with_config(&[], &home.0, &config);
    let rx = lines(&mut child);
    let _server = Server(child);
    let port = console_port(&rx);

    let response = http_get(port, "/api/v1/update");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains(&format!("\"version\":\"{}\"", env!("CARGO_PKG_VERSION"))), "{response}");
    assert!(response.contains("\"channel\":\"stable\""), "{response}");
}

#[test]
fn no_update_leaves_the_endpoint_unserved() {
    let home = TempDir::new("no-update");
    let config = config_with_dead_release_source(&home.0);
    let mut child = start_with_config(&["--no-update"], &home.0, &config);
    let rx = lines(&mut child);
    let _server = Server(child);
    let port = console_port(&rx);

    assert!(http_get(port, "/api/v1/update").starts_with("HTTP/1.1 404"));
    // The server itself is fine, and still says what version it is.
    assert_healthy(port);
}

/// What a guarded run did: whether it ever served, whether it exited unhappily, and what it said.
struct Guarded {
    served: bool,
    failed: bool,
    printed: String,
}

/// Starts the server with `extra` on top of a **backend-free** command line, under
/// `GAZELLE_NO_HARDWARE`, and waits for it to stop — or to announce that it is serving, which is
/// the failure these tests are about. Either way the child is not left behind.
///
/// Deliberately not passing `--backend`: this is how a forgetful harness would start it, and the
/// point is that such a run never reaches a device. The variable is what makes running these on a
/// machine with hardware attached safe.
fn guarded_run(extra: &[&str]) -> Guarded {
    let mut child = Command::new(BIN)
        .args(["--bind", "127.0.0.1:0", "--no-persist", "--no-web-ui", "--no-tray"])
        .args(extra)
        .env(no_hardware::VAR, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let rx = lines(&mut child);
    let mut printed = String::new();
    // The streams close when the process ends, so a disconnect is "it stopped". A "listening on"
    // line, or silence for long enough, means it did not.
    let served = loop {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(line) => {
                let listening = line.contains("listening on");
                printed.push_str(&line);
                printed.push('\n');
                if listening {
                    break true;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break true,
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
        }
    };
    if served {
        let _ = child.kill();
    }
    let failed = !child.wait().unwrap().success();
    Guarded { served, failed, printed }
}

/// The default is `usb` (decision `0018`), asserted **without opening a device**: under
/// `GAZELLE_NO_HARDWARE` a run with no `--backend` is refused, and the refusal is the USB one.
/// A default of `loopback` would serve happily instead, and fail this test.
#[test]
fn the_default_backend_is_usb_and_the_guard_stops_it() {
    let run = guarded_run(&[]);
    assert!(!run.served, "a defaulted run under the guard must not serve: {}", run.printed);
    assert!(run.failed, "and must exit unhappily, so a harness notices: {}", run.printed);
    assert!(run.printed.contains(no_hardware::VAR), "the refusal names the variable: {}", run.printed);
    assert!(run.printed.contains("--backend loopback"), "the refusal says what to do: {}", run.printed);
}

/// The same refusal when the flag is there and says `usb`, so the guard is about the backend
/// rather than about the flag being absent.
#[test]
fn an_explicit_usb_backend_is_refused_under_the_guard_too() {
    let run = guarded_run(&["--backend", "usb"]);
    assert!(!run.served, "{}", run.printed);
    assert!(run.failed, "{}", run.printed);
    assert!(run.printed.contains(no_hardware::VAR), "{}", run.printed);
}

/// And the guard is not a general ban on running: the loopback is what the harnesses ask for, and
/// it serves exactly as before.
#[test]
fn the_loopback_backend_still_runs_under_the_guard() {
    let home = TempDir::new("guard-loopback");
    let mut child = start(BIN, &[], &home.0);
    let rx = lines(&mut child);
    let _server = Server(child);
    let port = console_port(&rx);
    assert_healthy(port);
    let response = http_get(port, "/api/v1/health");
    assert!(response.contains("\"backend\":\"loopback\""), "{response}");
}
