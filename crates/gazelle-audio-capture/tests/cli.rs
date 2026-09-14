use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gazelle_audio_capture::capture::event::UsbEvent;
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_gazelle-capture");

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

#[test]
fn serve_demo_starts_a_probe_behind_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let plan = dir.path().join("plan.json");
    std::fs::write(
        &plan,
        r#"{"parameters":[
            {"id":"monitor_level","label":"Monitor level","kind":"continuous"},
            {"id":"mute","label":"Mute","kind":"toggle","domain":{"values":["off","on"]}}],
          "plan":{"parameter":"monitor_level","value_a":"0 dB","value_b":["-6 dB"],"control_parameter":"mute"}}"#,
    )
    .unwrap();
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
