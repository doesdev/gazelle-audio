//! Two switches for the web app's end to end tests, **compiled into debug builds only**.
//!
//! Those tests drive a real server through a real browser, and the rule is that no test listens on
//! anything but loopback. So a test cannot turn phones on and have the server listen on every
//! interface, and it cannot connect from anywhere but this machine. These let it act the rest out:
//!
//! - `GAZELLE_TEST_PHONE_LISTEN=127.0.0.1` makes the phone listener bind that address on a port
//!   of its own choosing instead of every interface on the server's port. Only a loopback address
//!   is taken; anything else is ignored and the listener binds as it always would.
//! - `GAZELLE_TEST_PEER_HEADER=1` makes the server believe an `x-gazelle-test-peer` header about
//!   where a request came from, so a test on this machine can be a phone. It is believed only on a
//!   connection that really is from this machine: it can make a local request look remote, never
//!   the other way round.
//!
//! In a release build (`--release`, which is how every shipped binary is built) both functions
//! below are constant: the variables are never read and the header means nothing. A release
//! cannot be talked into either, whatever its environment says.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::http::HeaderMap;

pub const LISTEN_VAR: &str = "GAZELLE_TEST_PHONE_LISTEN";
pub const PEER_VAR: &str = "GAZELLE_TEST_PEER_HEADER";
pub const PEER_HEADER: &str = "x-gazelle-test-peer";

/// Where the phone listener binds for a server whose own port is `port`.
pub fn phone_listen(port: u16) -> SocketAddr {
    #[cfg(debug_assertions)]
    if let Some(ip) = std::env::var(LISTEN_VAR).ok().and_then(|v| v.trim().parse::<IpAddr>().ok()) {
        if ip.is_loopback() {
            return SocketAddr::new(ip, 0);
        }
    }
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)
}

/// Whether this build honours the test peer header, read once.
#[cfg(debug_assertions)]
fn peer_header_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var(PEER_VAR).is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// The peer a test says this request came from, when the build and the environment allow it.
/// An address with or without a port; without one, port 0.
pub fn test_peer(headers: &HeaderMap) -> Option<SocketAddr> {
    #[cfg(debug_assertions)]
    if peer_header_on() {
        let value = headers.get(PEER_HEADER)?.to_str().ok()?.trim();
        return value.parse::<SocketAddr>().ok().or_else(|| value.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, 0)));
    }
    let _ = headers;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests of this crate never set the variable, so the listener binds as it would for real.
    #[test]
    fn without_the_variable_phones_listen_on_every_interface_at_the_servers_port() {
        if std::env::var(LISTEN_VAR).is_err() {
            assert_eq!(phone_listen(8420), "0.0.0.0:8420".parse().unwrap());
        }
    }

    #[test]
    fn without_the_variable_the_header_means_nothing() {
        if std::env::var(PEER_VAR).is_err() {
            let mut headers = HeaderMap::new();
            headers.insert(PEER_HEADER, "192.168.1.9:5000".parse().unwrap());
            assert_eq!(test_peer(&headers), None);
        }
    }
}
