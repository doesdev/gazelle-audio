//! One instance: a second launch hands its window to the one already running and exits.
//!
//! The server's own port is the rendezvous, rather than a named mutex and a pipe of our own: it
//! already exists, it is already the thing the second launch collides with, and the handover is
//! then an ordinary request anyone can make by hand or from a test. The endpoint
//! (`POST /api/v1/window/show`) is refused to anything but a loopback peer, so a server bound to
//! every interface does not let the network raise a window on someone's desk.
//!
//! The request is written by hand over a `TcpStream`. It is a dozen lines against a server we
//! wrote, on the same machine, and it keeps an HTTP client out of the dependency tree.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// How long the second launch waits for the running one. Both are on this machine: a running
/// server answers in milliseconds, and a wait longer than this would be a hang the person is
/// watching, not patience worth having.
const TIMEOUT: Duration = Duration::from_secs(2);

/// What the running instance said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Its window is now in front.
    Shown,
    /// It is running without a window (`--no-window`, or a build without the feature), so there
    /// was nothing to show. The string is its own words.
    Headless(String),
}

/// The address a client on this machine can actually reach: a wildcard bind listens everywhere but
/// cannot be connected to, so it becomes the loopback address of the same family.
pub fn reachable(address: SocketAddr) -> SocketAddr {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, address.port())
}

/// Ask the server on `address` to show its window.
///
/// An error means there is no Gazelle there: nothing listening, something else listening, or a
/// server too old to know the request. The caller then reports the port as taken, as before.
pub fn hand_over(address: SocketAddr) -> Result<Outcome, String> {
    let address = reachable(address);
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT).map_err(|e| format!("connecting to {address}: {e}"))?;
    stream.set_read_timeout(Some(TIMEOUT)).and_then(|()| stream.set_write_timeout(Some(TIMEOUT))).map_err(|e| e.to_string())?;
    // `Connection: close` makes the end of the body the end of the stream, so the reply needs no
    // chunked or content-length handling to read to the end.
    let request = format!(
        "POST /api/v1/window/show HTTP/1.1\r\nHost: {address}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).map_err(|e| format!("asking {address} to show its window: {e}"))?;
    let mut reply = Vec::new();
    // A reply this long is not ours; reading unbounded from an unknown listener is not on.
    stream.take(64 * 1024).read_to_end(&mut reply).map_err(|e| format!("reading the answer from {address}: {e}"))?;
    parse_reply(&reply)
}

/// Read the reply of a server that knows the request, or say why it is not one.
fn parse_reply(reply: &[u8]) -> Result<Outcome, String> {
    let text = String::from_utf8_lossy(reply);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("the answer was not an HTTP message")?;
    let status = head.split_whitespace().nth(1).unwrap_or_default();
    if status != "200" {
        return Err(format!("the server at that port answered {} to the handover", if status.is_empty() { "nothing" } else { status }));
    }
    let json: serde_json::Value = serde_json::from_str(body.trim()).map_err(|_| "the answer was not the one Gazelle gives".to_string())?;
    match json.get("shown").and_then(serde_json::Value::as_bool) {
        Some(true) => Ok(Outcome::Shown),
        Some(false) => Ok(Outcome::Headless(
            json.get("reason").and_then(serde_json::Value::as_str).unwrap_or("it is running without a window").to_string(),
        )),
        None => Err("the answer was not the one Gazelle gives".into()),
    }
}

/// Whether a peer may ask for the window. Only this machine: the endpoint moves a window on
/// someone's desk, and a server bound to every interface must not hand that to the network.
///
/// A request with no peer address is refused. That never happens over a socket; it is what an
/// in-process call looks like, and nothing in the server makes one.
pub fn allowed(peer: Option<SocketAddr>) -> bool {
    peer.is_some_and(|peer| peer.ip().is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: &str, body: &str) -> Vec<u8> {
        format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\r\n{body}").into_bytes()
    }

    #[test]
    fn a_shown_window_and_a_headless_server_are_told_apart() {
        assert_eq!(parse_reply(&reply("200 OK", r#"{"shown":true}"#)), Ok(Outcome::Shown));
        assert_eq!(
            parse_reply(&reply("200 OK", r#"{"shown":false,"reason":"no window here"}"#)),
            Ok(Outcome::Headless("no window here".into()))
        );
        // Without a reason of its own, the answer still says which it was.
        assert!(matches!(parse_reply(&reply("200 OK", r#"{"shown":false}"#)), Ok(Outcome::Headless(_))));
    }

    #[test]
    fn anything_that_is_not_gazelle_is_an_error_rather_than_a_handover() {
        // Some other program on the port.
        assert!(parse_reply(b"hello?").is_err());
        assert!(parse_reply(&reply("404 Not Found", "")).is_err(), "an older Gazelle, or another server");
        assert!(parse_reply(&reply("403 Forbidden", r#"{"shown":false}"#)).is_err(), "refused, not headless");
        assert!(parse_reply(&reply("200 OK", "<html></html>")).is_err());
        assert!(parse_reply(&reply("200 OK", r#"{"ok":true}"#)).is_err());
        assert!(parse_reply(b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n").is_err(), "no body at all");
    }

    #[test]
    fn a_wildcard_bind_is_reached_on_the_loopback_of_its_own_family() {
        assert_eq!(reachable("127.0.0.1:8420".parse().unwrap()), "127.0.0.1:8420".parse().unwrap());
        assert_eq!(reachable("0.0.0.0:8420".parse().unwrap()), "127.0.0.1:8420".parse().unwrap());
        assert_eq!(reachable("[::]:8420".parse().unwrap()), "[::1]:8420".parse().unwrap());
        assert_eq!(reachable("192.168.1.5:80".parse().unwrap()), "192.168.1.5:80".parse().unwrap());
    }

    #[test]
    fn only_this_machine_may_raise_the_window() {
        assert!(allowed(Some("127.0.0.1:51234".parse().unwrap())));
        assert!(allowed(Some("[::1]:51234".parse().unwrap())));
        assert!(!allowed(Some("192.168.1.9:51234".parse().unwrap())));
        assert!(!allowed(None));
    }
}
