//! Packets-per-second over a sliding window, for the panel indicator.

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct RateMeter {
    window_ns: u64,
    times: VecDeque<u64>,
}

impl RateMeter {
    pub fn new(window_ns: u64) -> Self {
        assert!(window_ns > 0, "window must be positive");
        Self { window_ns, times: VecDeque::new() }
    }

    pub fn record(&mut self, now_ns: u64) {
        self.times.push_back(now_ns);
        self.evict(now_ns);
    }

    pub fn per_second(&mut self, now_ns: u64) -> f64 {
        self.evict(now_ns);
        self.times.len() as f64 * 1e9 / self.window_ns as f64
    }

    fn evict(&mut self, now_ns: u64) {
        // The window is (now - window, now]; nothing is old enough before one full window.
        let Some(floor) = now_ns.checked_sub(self.window_ns) else {
            return;
        };
        while self.times.front().is_some_and(|t| *t <= floor) {
            self.times.pop_front();
        }
    }
}
