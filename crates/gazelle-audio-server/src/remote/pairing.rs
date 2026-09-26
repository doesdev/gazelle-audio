//! The one pairing code at a time, and the limit on wrong tries.
//!
//! A code is started from this computer only, lives [`CODE_LIFETIME_MS`], and is used once:
//! pairing with it, starting another, cancelling, running out of time or running out of tries all
//! end it. Wrong tries are limited three ways, any one of which is enough on its own to make
//! guessing a 40 bit code hopeless (`token`):
//!
//! - the code itself dies after [`WRONG_TRIES_PER_CODE`] wrong tries, from anywhere;
//! - one address may fail [`PEER_FAILURES`] times a minute;
//! - everyone together may fail [`ALL_FAILURES`] times a minute, which is what holds against a
//!   guesser spread over many addresses.
//!
//! Past either rate, a try is refused before its code is even looked at (HTTP 429). A try with no
//! pairing running counts as a wrong one, so probing costs the same whether or not a code exists.

use std::collections::VecDeque;
use std::net::IpAddr;

/// How long a code lives: 5 minutes.
pub const CODE_LIFETIME_MS: u64 = 5 * 60 * 1000;
/// Wrong tries before the code is thrown away and pairing has to be started again.
pub const WRONG_TRIES_PER_CODE: u32 = 10;
/// The window the rates below are counted over.
pub const WINDOW_MS: u64 = 60 * 1000;
/// Failed tries allowed from one address in [`WINDOW_MS`].
pub const PEER_FAILURES: usize = 5;
/// Failed tries allowed from everyone in [`WINDOW_MS`].
pub const ALL_FAILURES: usize = 20;

/// The code running now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    /// As shown: `XXXX-XXXX`.
    pub code: String,
    pub expires_ms: u64,
    pub wrong: u32,
}

impl Pairing {
    pub fn new(code: String, now_ms: u64) -> Self {
        Self { code, expires_ms: now_ms + CODE_LIFETIME_MS, wrong: 0 }
    }

    pub fn expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_ms
    }
}

/// Recent failed tries, for the two rates.
#[derive(Debug, Default)]
pub struct Limiter {
    failures: VecDeque<(u64, IpAddr)>,
}

impl Limiter {
    fn forget_old(&mut self, now_ms: u64) {
        while self.failures.front().is_some_and(|(at, _)| now_ms.saturating_sub(*at) >= WINDOW_MS) {
            self.failures.pop_front();
        }
    }

    /// Whether a try from `peer` may be looked at now.
    pub fn allows(&mut self, peer: IpAddr, now_ms: u64) -> bool {
        self.forget_old(now_ms);
        self.failures.len() < ALL_FAILURES && self.failures.iter().filter(|(_, from)| *from == peer).count() < PEER_FAILURES
    }

    /// A try from `peer` failed. Only tries that were allowed are counted, so the list never holds
    /// more than [`ALL_FAILURES`].
    pub fn failed(&mut self, peer: IpAddr, now_ms: u64) {
        self.failures.push_back((now_ms, peer));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn a_code_lives_five_minutes() {
        let p = Pairing::new("ABCD-EFGH".into(), 1_000);
        assert!(!p.expired(1_000 + CODE_LIFETIME_MS - 1));
        assert!(p.expired(1_000 + CODE_LIFETIME_MS));
    }

    #[test]
    fn one_address_is_stopped_after_its_share_of_failures_and_let_back_a_minute_later() {
        let mut limiter = Limiter::default();
        let (a, b) = (ip("192.168.1.50"), ip("192.168.1.51"));
        for i in 0..PEER_FAILURES as u64 {
            assert!(limiter.allows(a, i));
            limiter.failed(a, i);
        }
        assert!(!limiter.allows(a, 10), "{PEER_FAILURES} failures in a minute is the limit");
        assert!(limiter.allows(b, 10), "another address is not held to the first one's count");
        assert!(limiter.allows(a, WINDOW_MS + PEER_FAILURES as u64), "a minute on, it may try again");
    }

    #[test]
    fn everyone_together_is_stopped_too() {
        let mut limiter = Limiter::default();
        for i in 0..ALL_FAILURES {
            let peer = ip(&format!("10.0.{}.{}", i / 200, i % 200 + 1));
            assert!(limiter.allows(peer, 0));
            limiter.failed(peer, 0);
        }
        assert!(!limiter.allows(ip("10.9.9.9"), 1), "a fresh address is refused once the total is reached");
        assert!(limiter.allows(ip("10.9.9.9"), WINDOW_MS));
    }
}
