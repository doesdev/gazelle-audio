use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gazelle_audio_capture::capture::event::UsbEvent;
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_gazelle-capture");

const DEMO_PLAN: &str = r#"{"parameters":[
    {"id":"monitor_level","label":"Monitor level","kind":"continuous"},
    {"id":"mute","label":"Mute","kind":"toggle","domain":{"values":["off","on"]}}],
  "plan":{"parameter":"monitor_level","value_a":"0 dB","value_b":["-6 dB"],"control_parameter":"mute"}}"#;

fn write_demo_plan(dir: &std::path::Path) -> std::path::PathBuf {
    let plan = dir.join("plan.json");
    std::fs::write(&plan, DEMO_PLAN).unwrap();
    plan
}

/// Waits up to `timeout` for `child` to exit, polling rather than blocking, so a regression
/// that reintroduces a hang fails the assertion instead of hanging the test suite. Kills and
/// reaps the child before returning `None` on timeout.
fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn synth_then_import_prints_target_events() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("demo");
    let out = Command::new(BIN).args(["synth", session.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let capture = session.join("captures/p1.pcapng");
    assert!(capture.is_file());
    let out = Command::new(BIN).args(["import", capture.to_str().unwrap(), "--vid", "0x1234", "--pid", "0xabcd"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let events: Vec<UsbEvent> = String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert!(events.len() > 100);
    assert!(events.iter().all(|e| e.device == 5));
    let out = Command::new(BIN).args(["import", capture.to_str().unwrap(), "--bus", "1", "--device", "9"]).output().unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "no events for an absent address");
}

#[test]
fn synth_then_analyze_writes_a_field_map_and_report() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("demo");
    let out = Command::new(BIN).args(["synth", session.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(BIN).args(["analyze", "--session", session.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("monitor_level.json") && stdout.contains("monitor_level.md"), "{stdout}");

    let map: Value = serde_json::from_slice(&std::fs::read(session.join("analysis/monitor_level.json")).unwrap()).unwrap();
    assert_eq!(map["schema_version"], 1);
    assert_eq!(map["parameter"], "monitor_level");
    assert_eq!(map["command"]["channel"]["transfer"], "control");
    assert_eq!(map["evidence"][0]["capture"], "captures/p1.pcapng");
    let md = std::fs::read_to_string(session.join("analysis/monitor_level.md")).unwrap();
    assert!(md.starts_with("# Field map: monitor_level"), "{md}");

    let out = Command::new(BIN).args(["analyze", "--session", session.to_str().unwrap(), "--probe", "p9"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no probe p9"));
}

fn http_get(port: u16, path: &str, token: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Connection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    s.read_to_string(&mut response).unwrap();
    let status = response[9..12].parse().unwrap();
    let body = response.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn http_post(port: u16, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    s.read_to_string(&mut response).unwrap();
    let status = response[9..12].parse().unwrap();
    let body = response.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

#[test]
fn agent_serves_the_panel_mcp_and_openai_tools_for_one_session() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("session");
    let mut child = Command::new(BIN)
        .args(["agent", "--session", session.to_str().unwrap(), "--vid", "0x1234", "--pid", "0xabcd", "--port", "0", "--source", "demo"])
        .env("LOCALAPPDATA", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let (mut port, mut token, mut mcp) = (0u16, String::new(), false);
    while port == 0 || token.is_empty() || !mcp {
        let line = lines.next().expect("agent printed its addresses and token").unwrap();
        if let Some(url) = line.strip_prefix("panel: http://127.0.0.1:") {
            port = url.trim_end_matches('/').parse().unwrap();
        } else if let Some(t) = line.strip_prefix("token: ") {
            token = t.to_string();
        } else if line.starts_with("mcp: http://127.0.0.1:") && line.ends_with("/mcp") {
            mcp = true;
        }
    }
    assert!(session.join("session.json").is_file());
    assert_eq!(std::fs::read_to_string(dir.path().join("gazelle/capture-token")).unwrap(), token);

    assert_eq!(http_get(port, "/api/state", Some(&token)).0, 200);
    let (status, body) = http_get(port, "/openai/tools", Some(&token));
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["tools"].as_array().unwrap().len(), 12);

    let (status, body) = http_post(port, "/openai/call", Some(&token), r#"{"name":"session_status"}"#);
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["result"]["open"], true);

    let other = serde_json::json!({ "name": "session_open", "arguments": { "path": dir.path().join("elsewhere").display().to_string(), "vid": 1, "pid": 2 } });
    let (status, body) = http_post(port, "/openai/call", Some(&token), &other.to_string());
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("this helper serves"), "{body}");

    assert_eq!(http_post(port, "/mcp", None, "{}").0, 401, "MCP needs the token");
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn serve_demo_starts_a_probe_behind_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let plan = write_demo_plan(dir.path());
    let session = dir.path().join("session");
    let mut child = Command::new(BIN)
        .args(["serve", "--session", session.to_str().unwrap(), "--vid", "0x1234", "--pid", "0xabcd", "--plan", plan.to_str().unwrap(), "--port", "0", "--source", "demo", "--no-wait"])
        .env("LOCALAPPDATA", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut port = 0u16;
    let mut token = String::new();
    while port == 0 || token.is_empty() {
        let line = lines.next().expect("serve printed its address and token").unwrap();
        if let Some(url) = line.strip_prefix("panel: http://127.0.0.1:") {
            port = url.trim_end_matches('/').parse().unwrap();
        } else if let Some(t) = line.strip_prefix("token: ") {
            token = t.to_string();
        }
    }
    assert_eq!(std::fs::read_to_string(dir.path().join("gazelle/capture-token")).unwrap(), token);
    assert_eq!(http_get(port, "/api/state", None).0, 401);
    let deadline = Instant::now() + Duration::from_secs(10);
    let state = loop {
        let (status, body) = http_get(port, "/api/state", Some(&token));
        assert_eq!(status, 200);
        let state: Value = serde_json::from_str(&body).unwrap();
        if state["probe"]["probe_id"] == "p1" && state["capture"]["packets"].as_u64().unwrap() > 0 {
            break state;
        }
        assert!(Instant::now() < deadline, "probe did not start: {state}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(state["probe"]["kind"], "idle");
    assert!(session.join("captures/p1.pcapng").is_file());
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn serve_names_a_bad_plan_file_and_creates_no_session() {
    let dir = tempfile::tempdir().unwrap();
    let plan = dir.path().join("broken-plan.json");
    std::fs::write(&plan, "{ not json").unwrap();
    let session = dir.path().join("session");
    let out = Command::new(BIN)
        .args(["serve", "--session", session.to_str().unwrap(), "--vid", "0x1234", "--pid", "0xabcd", "--plan", plan.to_str().unwrap(), "--port", "0", "--source", "demo", "--no-wait"])
        .env("LOCALAPPDATA", dir.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("broken-plan.json"), "the error names the plan file: {stderr}");
    assert!(!session.join("session.json").exists(), "a bad plan must not leave a session behind");
}

#[test]
fn serve_warns_when_vid_pid_disagree_with_an_existing_session() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("session");
    let plan = write_demo_plan(dir.path());
    // `--source import` without `--file` fails only after the session has been opened or
    // created, so the first run creates a 1234:abcd session and the second reopens it.
    let serve = |vid: &str, pid: &str| {
        Command::new(BIN)
            .args(["serve", "--session", session.to_str().unwrap(), "--vid", vid, "--pid", pid, "--plan", plan.to_str().unwrap(), "--port", "0", "--source", "import", "--no-wait"])
            .env("LOCALAPPDATA", dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let first = serve("0x1234", "0xabcd");
    assert!(session.join("session.json").is_file(), "{}", String::from_utf8_lossy(&first.stderr));
    assert!(!String::from_utf8_lossy(&first.stderr).contains("ignoring --vid/--pid"));
    let out = serve("0x23e5", "0xa100");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("already targets 1234:abcd; ignoring --vid/--pid"), "{stderr}");
    assert!(stderr.contains("--source import needs --file"), "{stderr}");
}

#[test]
fn serve_without_no_wait_fails_fast_when_stdin_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let plan = write_demo_plan(dir.path());
    let session = dir.path().join("session");
    let mut child = Command::new(BIN)
        .args(["serve", "--session", session.to_str().unwrap(), "--vid", "0x1234", "--pid", "0xabcd", "--plan", plan.to_str().unwrap(), "--port", "0", "--source", "demo"])
        .env("LOCALAPPDATA", dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let status = wait_with_timeout(&mut child, Duration::from_secs(10));
    let mut stdout = String::new();
    let mut stderr = String::new();
    let _ = child.stdout.take().map(|mut s| s.read_to_string(&mut stdout));
    let _ = child.stderr.take().map(|mut s| s.read_to_string(&mut stderr));
    let status = status.unwrap_or_else(|| panic!("serve did not exit within 10s of a closed stdin\nstdout: {stdout}\nstderr: {stderr}"));
    assert!(!status.success(), "expected a non-zero exit when stdin is closed without --no-wait\nstdout: {stdout}\nstderr: {stderr}");
    assert!(!session.join("captures/p1.pcapng").is_file(), "no probe should have started");
}

/// Round 2 finding: a Ctrl-C during the "press Enter" wait used to return `Ok(())` from
/// `serve`, letting `main` return and the `#[tokio::main]` runtime start tearing down — which
/// waits indefinitely for the still-blocked `spawn_blocking(stdin().read_line(...))` thread,
/// since a closed/held-open stdin never gives it a line to return. The process would hang
/// forever, and a second Ctrl-C was silently swallowed (the OS default disposition had already
/// been overridden, but nothing was left polling for it). This holds stdin open with a pipe
/// that's never closed or written to, so the fix must exit without waiting on that read.
#[cfg(unix)]
#[test]
fn ctrl_c_during_enter_wait_exits_promptly() {
    let dir = tempfile::tempdir().unwrap();
    let plan = write_demo_plan(dir.path());
    let session = dir.path().join("session");
    let mut child = Command::new(BIN)
        .args(["serve", "--session", session.to_str().unwrap(), "--vid", "0x1234", "--pid", "0xabcd", "--plan", plan.to_str().unwrap(), "--port", "0", "--source", "demo"])
        .env("LOCALAPPDATA", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Held for the rest of the test: an open pipe end that's never written to or closed, so
    // the blocking `read_line` inside `serve` has nothing to read and cannot return on its own.
    let _stdin = child.stdin.take().unwrap();

    // Keep draining stdout (and stderr) for the child's whole life, not just until the prompt
    // line: closing our read end early would make the child's *next* `println!`/`eprintln!`
    // (e.g. the "interrupted..." line printed right after Ctrl-C) fail with a broken pipe,
    // which panics — println!/eprintln! panic on a write error — and, on the resulting unwind,
    // `#[tokio::main]`'s runtime would still be dropped waiting on the very stdin-read task
    // this test is holding open, reproducing a hang from a test-harness bug rather than the
    // one under test.
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut sent_prompt = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !sent_prompt && line.starts_with("open the panel, then press Enter") {
                sent_prompt = true;
                let _ = tx.send(());
            }
        }
    });
    let stderr = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        for _line in BufReader::new(stderr).lines().map_while(Result::ok) {}
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(()) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("serve never printed the Enter prompt");
            }
        }
    }

    let pid = child.id().to_string();
    let status = Command::new("kill").args(["-INT", &pid]).status().unwrap();
    assert!(status.success(), "kill -INT failed to signal the child");

    match wait_with_timeout(&mut child, Duration::from_secs(5)) {
        Some(status) => assert!(status.success(), "expected a clean exit after Ctrl-C, got {status:?}"),
        None => panic!("serve did not exit within 5s of a Ctrl-C during the Enter wait (stdin held open)"),
    }
}
