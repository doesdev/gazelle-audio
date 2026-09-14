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
        // Kept sorted, so eviction from the front stays correct if the clock steps backwards
        // (the wall clock is not monotonic); the common in-order case is a push.
        if self.times.back().is_some_and(|last| now_ns < *last) {
            let at = self.times.partition_point(|t| *t <= now_ns);
            self.times.insert(at, now_ns);
        } else {
            self.times.push_back(now_ns);
        }
        self.evict(now_ns);
    }

    pub fn per_second(&mut self, now_ns: u64) -> f64 {
        self.evict(now_ns);
        // Records dated after `now` (left behind by a backwards clock step) are not in the
        // window yet; the deque is sorted, so they are a suffix.
        let in_window = self.times.partition_point(|t| *t <= now_ns);
        in_window as f64 * 1e9 / self.window_ns as f64
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
