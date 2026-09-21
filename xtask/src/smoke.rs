//! Start the built binaries and check what they say, before anything is signed.
//!
//! The release checklist's "how it is checked" column, run against the files that will ship:
//!
//! - `GET /api/v1/health` names the version being released;
//! - `GET /api/v1/update` answers (so the updater is compiled in and offered), reports
//!   `"can_verify": true` (so a release signing key is baked in) and the target the assets are
//!   named for;
//! - `GET /` is the web app, not the "Web UI not built" notice a build before `pnpm build` embeds;
//! - with `--pubkey`, the binary carries **that** key, not merely some key;
//! - on Windows, the binary carries the aggregate driver cargo built beside it, byte for byte, so
//!   a release can never ship a Gazelle whose Aggregate page has no driver to offer.
//!
//! Each server runs on the loopback backend with `GAZELLE_NO_HARDWARE=1`, `--no-tray` and
//! `--no-persist`, on a spare port on 127.0.0.1, with its config directory pointed at a scratch
//! one whose `update.json` sends the start-up update check to a closed local port: a smoke test
//! never opens a device, never touches the user's own settings, and never asks GitHub anything.
//! Both binaries are checked; the windowless one has no console to print its port on, which is
//! why the port is chosen here rather than read from the log.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gazelle_audio_server::update::release::from_hex;
use gazelle_audio_server::update::BINARIES;

const START_TIMEOUT: Duration = Duration::from_secs(60);

/// The aggregate driver, as cargo builds it into the same directory as the binaries.
pub const DRIVER: &str = "gazelle_aggregate.dll";

/// The notice page `build.rs` embeds when the web UI was not built first.
const NOT_BUILT: &str = "Web UI not built";

pub fn smoke(bin_dir: &Path, version: &str, pubkey: Option<&str>, target: &str) -> Result<(), String> {
    let pubkey = match pubkey.map(|k| k.trim().to_ascii_lowercase()) {
        Some(key) if from_hex(&key).is_none() => return Err(format!("--pubkey {key:?} is not 64 hex digits")),
        other => other,
    };
    let carries_driver = target.contains("windows");
    for stem in BINARIES {
        let exe = bin_dir.join(crate::dist::built_name(stem, target));
        if !exe.is_file() {
            return Err(format!("{} is missing; build the release first", exe.display()));
        }
        let bytes = std::fs::read(&exe).map_err(|e| format!("reading {}: {e}", exe.display()))?;
        if let Some(key) = &pubkey {
            if !carries(&bytes, key) {
                return Err(format!(
                    "{} does not carry the public key {key}; build it with GAZELLE_UPDATE_PUBKEY set to that",
                    exe.display()
                ));
            }
        }
        if carries_driver {
            check_driver(&exe, &bytes, &bin_dir.join(DRIVER))?;
        }
        let answers = run_one(&exe)?;
        let problems = check(&answers, version, target);
        if !problems.is_empty() {
            return Err(format!("{}:\n  {}", exe.display(), problems.join("\n  ")));
        }
        println!(
            "{}: version {version}, target {target}, can_verify{}{}, web UI served",
            exe.display(),
            if pubkey.is_some() { ", carries the expected key" } else { "" },
            if carries_driver { ", carries the aggregate driver" } else { "" }
        );
    }
    Ok(())
}

/// Whether the key's hex digits appear in the binary. `option_env!` compiles the string in as
/// it was given, so a binary built with the key holds it verbatim.
fn carries(bytes: &[u8], key: &str) -> bool {
    let needle = key.as_bytes();
    bytes.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

/// Refuse `exe` unless it holds the driver at `driver` byte for byte: a build that was never
/// told to carry it, or one that carries a stale copy from an earlier build of the driver.
fn check_driver(exe: &Path, exe_bytes: &[u8], driver: &Path) -> Result<(), String> {
    let wanted = std::fs::read(driver).map_err(|e| {
        format!("{} could not be read ({e}); build the driver first: cargo build --release -p gazelle-audio-aggregate", driver.display())
    })?;
    if !holds(exe_bytes, &wanted) {
        return Err(format!(
            "{} does not carry the aggregate driver {}; build it with GAZELLE_AGGREGATE_DLL set to that file",
            exe.display(),
            driver.display()
        ));
    }
    Ok(())
}

/// Whether `needle` appears in `haystack` exactly. `include_bytes!` puts the driver in the
/// binary as it is, in one piece.
fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// A response: status and body.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct Answers {
    pub health: Answer,
    pub update: Answer,
    pub index: Answer,
}

/// Everything wrong with what a server answered, as sentences; empty when it is right.
pub fn check(answers: &Answers, version: &str, target: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let json = |answer: &Answer, route: &str, problems: &mut Vec<String>| -> serde_json::Value {
        if answer.status != 200 {
            problems.push(format!("GET {route} answered {}", answer.status));
            return serde_json::Value::Null;
        }
        serde_json::from_str(&answer.body).unwrap_or_else(|e| {
            problems.push(format!("GET {route} is not JSON: {e}"));
            serde_json::Value::Null
        })
    };

    let health = json(&answers.health, "/api/v1/health", &mut problems);
    if !health.is_null() {
        if health["version"] != version {
            problems.push(format!("/api/v1/health reports version {}, not {version}", health["version"]));
        }
        if health["backend"] != "loopback" {
            problems.push(format!("/api/v1/health reports backend {}, not loopback", health["backend"]));
        }
    }

    let update = json(&answers.update, "/api/v1/update", &mut problems);
    if !update.is_null() {
        if update["can_verify"] != true {
            problems.push("/api/v1/update reports can_verify false: no release signing key is built in (GAZELLE_UPDATE_PUBKEY)".into());
        }
        if update["version"] != version {
            problems.push(format!("/api/v1/update reports version {}, not {version}", update["version"]));
        }
        if update["target"] != target {
            problems.push(format!("/api/v1/update reports target {}, but the assets are named for {target}", update["target"]));
        }
    }

    if answers.index.status != 200 {
        problems.push(format!("GET / answered {}", answers.index.status));
    } else if answers.index.body.contains(NOT_BUILT) {
        problems.push("GET / is the \"Web UI not built\" notice; run pnpm -C web build, then rebuild".into());
    } else if !answers.index.body.contains("type=\"module\"") {
        problems.push("GET / does not look like the built web app (no module script)".into());
    }
    problems
}

/// Start one binary, ask it the three questions, stop it.
fn run_one(exe: &Path) -> Result<Answers, String> {
    let stem = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("server");
    let scratch = std::env::temp_dir().join(format!("gazelle-smoke-{}-{stem}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| format!("creating {}: {e}", scratch.display()))?;
    let result = run_in(exe, &scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn run_in(exe: &Path, scratch: &Path) -> Result<Answers, String> {
    // The start-up check goes to a port nothing listens on, and fails quietly; `can_verify`
    // does not depend on it.
    let settings = r#"{ "check": true, "interval_hours": 0, "api_base": "http://127.0.0.1:9" }"#;
    std::fs::write(scratch.join("update.json"), settings).map_err(|e| format!("writing the scratch update.json: {e}"))?;
    let log_path = scratch.join("server.log");
    let log = std::fs::File::create(&log_path).map_err(|e| format!("creating {}: {e}", log_path.display()))?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;

    let port = spare_port()?;
    let bind = format!("127.0.0.1:{port}");
    let mut child = Command::new(exe)
        .args(["--backend", "loopback", "--bind", &bind, "--no-tray", "--no-persist", "--no-window"])
        .env("GAZELLE_NO_HARDWARE", "1")
        .env("GAZELLE_CONFIG_DIR", scratch)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .spawn()
        .map_err(|e| format!("starting {}: {e}", exe.display()))?;

    let asked = (|| -> Result<Answers, String> {
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!("it exited ({status}) before answering"));
            }
            if get(port, "/api/v1/health").is_ok() {
                break;
            }
            if started.elapsed() > START_TIMEOUT {
                return Err(format!("it did not answer on {bind} within {} s", START_TIMEOUT.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(Answers { health: get(port, "/api/v1/health")?, update: get(port, "/api/v1/update")?, index: get(port, "/")? })
    })();

    // Killed by its own handle, which is this process's child and nothing else.
    let _ = child.kill();
    let _ = child.wait();
    asked.map_err(|e| {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        format!("{}: {e}\n--- its output ---\n{}", exe.display(), log.trim_end())
    })
}

/// A port nothing is listening on right now.
fn spare_port() -> Result<u16, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| format!("finding a spare port: {e}"))?;
    listener.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// One HTTP/1.0 GET to 127.0.0.1, which the server answers in full and then closes, so no
/// chunked body ever needs undoing.
pub fn get(port: u16, path: &str) -> Result<Answer, String> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).map_err(|e| format!("connecting to {address}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(20))).map_err(|e| e.to_string())?;
    let request = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nAccept: */*\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|e| format!("GET {path}: {e}"))?;
    let mut raw = Vec::new();
    if let Err(e) = stream.read_to_end(&mut raw) {
        // A reset after the whole answer arrived is Windows closing the socket, not a failure.
        if !raw.windows(4).any(|w| w == b"\r\n\r\n") {
            return Err(format!("GET {path}: {e}"));
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| format!("GET {path}: no complete answer"))?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("GET {path}: no status in {:?}", head.lines().next()))?;
    Ok(Answer { status, body: body.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "x86_64-pc-windows-msvc";

    fn answer(status: u16, body: &str) -> Answer {
        Answer { status, body: body.into() }
    }

    fn good() -> Answers {
        Answers {
            health: answer(200, r#"{"status":"ok","version":"0.1.0","backend":"loopback","devices":2}"#),
            update: answer(200, r#"{"version":"0.1.0","target":"x86_64-pc-windows-msvc","can_verify":true,"state":{"state":"idle"}}"#),
            index: answer(200, r#"<!doctype html><title>Gazelle</title><script type="module" crossorigin src="/assets/index-abc.js"></script>"#),
        }
    }

    #[test]
    fn a_release_build_that_answers_rightly_passes() {
        assert_eq!(check(&good(), "0.1.0", TARGET), Vec::<String>::new());
    }

    #[test]
    fn each_wrong_answer_is_named() {
        type Break = fn(&mut Answers);
        let cases: [(Break, &str); 7] = [
            (|a| a.health.body = a.health.body.replace("0.1.0", "0.0.9"), "reports version \"0.0.9\", not 0.1.0"),
            (|a| a.health.body = a.health.body.replace("loopback", "usb"), "backend \"usb\""),
            (|a| a.update.body = a.update.body.replace("true", "false"), "can_verify false"),
            (|a| a.update.status = 404, "GET /api/v1/update answered 404"),
            (|a| a.update.body = a.update.body.replace("x86_64", "aarch64"), "assets are named for"),
            (
                |a| a.index.body = "<html><title>Gazelle</title><h1>Web UI not built</h1></html>".into(),
                "\"Web UI not built\" notice",
            ),
            (|a| a.health.body = "not json".into(), "is not JSON"),
        ];
        for (break_it, expected) in cases {
            let mut answers = good();
            break_it(&mut answers);
            let problems = check(&answers, "0.1.0", TARGET);
            assert!(problems.iter().any(|p| p.contains(expected)), "expected {expected:?} in {problems:?}");
        }
    }

    #[test]
    fn the_key_must_be_in_the_binary_verbatim() {
        let key = "ab".repeat(32);
        let binary = [b"\0\0junk".as_slice(), key.as_bytes(), b"more"].concat();
        assert!(carries(&binary, &key));
        assert!(carries(&binary, &key.to_uppercase()));
        assert!(!carries(&binary, &"ac".repeat(32)));
        assert!(!carries(&binary[..40], &key));
    }

    #[test]
    fn get_reads_status_and_body_from_a_real_socket() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
                request.push(byte[0]);
            }
            let request = String::from_utf8_lossy(&request).into_owned();
            stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\n\r\n{\"a\":\"b c\"}").unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            request
        });
        let got = get(port, "/api/v1/health").unwrap();
        assert_eq!((got.status, got.body.as_str()), (200, "{\"a\":\"b c\"}"));
        assert!(server.join().unwrap().starts_with("GET /api/v1/health HTTP/1.0\r\n"));
    }

    #[test]
    fn a_pubkey_that_is_not_a_key_is_refused_before_anything_starts() {
        let error = smoke(Path::new("nowhere"), "0.1.0", Some("abc"), TARGET).unwrap_err();
        assert!(error.contains("not 64 hex digits"), "{error}");
    }

    #[test]
    fn the_driver_must_be_in_the_binary_whole() {
        let driver = b"MZ the aggregate driver".to_vec();
        let binary = [b"\0\0junk".as_slice(), &driver, b"more"].concat();
        assert!(holds(&binary, &driver));
        assert!(!holds(&binary, b"MZ a different driver"), "a stale copy is not the one built");
        assert!(!holds(&binary[..10], &driver));
        assert!(!holds(&binary, b""), "an empty driver is no driver");
    }

    /// A build that was not told to carry the driver, and one whose driver was never built, are
    /// both refused before anything is started.
    #[test]
    fn a_windows_release_without_the_driver_is_refused() {
        let dir = std::env::temp_dir().join(format!("gazelle-xtask-smoke-driver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for stem in BINARIES {
            std::fs::write(dir.join(format!("{stem}.exe")), b"a build that carries nothing").unwrap();
        }
        let not_built = smoke(&dir, "0.1.0", None, TARGET).unwrap_err();
        std::fs::write(dir.join(DRIVER), b"MZ the aggregate driver").unwrap();
        let not_carried = smoke(&dir, "0.1.0", None, TARGET).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(not_built.contains("cargo build --release -p gazelle-audio-aggregate"), "{not_built}");
        assert!(not_carried.contains("does not carry the aggregate driver") && not_carried.contains("GAZELLE_AGGREGATE_DLL"), "{not_carried}");
    }

    #[test]
    fn a_missing_binary_is_named() {
        let error = smoke(Path::new("nowhere"), "0.1.0", None, TARGET).unwrap_err();
        assert!(error.contains("gazelle-audio-server.exe is missing"), "{error}");
    }
}
