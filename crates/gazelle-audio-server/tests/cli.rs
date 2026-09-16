//! The binary as a harness starts it: `--bind 127.0.0.1:0` must report the port it really bound,
//! since that is the only way a separate process (the web client's tests) can find it. Also where
//! it logs, and the windowless build.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_gazelle-audio-server");
const WINDOWLESS_BIN: &str = env!("CARGO_BIN_EXE_gazelle-audio-serverw");

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
        .args(["--bind", "127.0.0.1:0", "--no-persist", "--no-web-ui", "--no-tray"])
        .args(extra)
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
        .args(["--bind", "127.0.0.1:0", "--no-persist", "--no-web-ui", "--no-tray"])
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
