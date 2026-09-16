//! The binary as a harness starts it: `--bind 127.0.0.1:0` must report the port it really bound,
//! since that is the only way a separate process (the web client's tests) can find it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_gazelle-audio-server");

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
