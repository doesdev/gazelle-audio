//! Request/response correlation.
//!
//! # Recovered, not inferred
//!
//! `HWDevice.request` and `HWDevice._sanitize_response` were **never recovered by
//! uncompyle6** — both are stubbed with `Parse error at or near ...` in
//! `refs/decompiled/manager/antelope_dev_base.py`. An earlier version of this module
//! matched responses by `seq`, which the project notes asserted but nothing verified.
//!
//! That was wrong. The logic below is read directly from the bytecode with
//! `refs/tools/scripts/pyc_dis.py`, which decodes 3.5-3.8 opcodes without needing the
//! original interpreter. `_sanitize_response` validates a response like this:
//!
//! ```text
//! resp = input_queue.get(timeout=request_timeout)      # the NEXT report, not a search
//! resp_structure = req_proto.process_response(resp, len(resp))
//! if resp_structure is None: return (False, None)
//! h = resp_structure.header
//! if mode is DeviceMode.APP and _do_sanitize is True:
//!     if h.cmd & 0x80000000 == 0:
//!         valid |= (h.cmd == 255)
//!         valid |= (h.cmd == r_structure.report_id + 1)
//!         valid &= (h.ext2 == r_structure.ext2)
//! else:
//!     valid = True
//! ```
//!
//! So a response is accepted when
//!
//! * the top bit of `cmd` is clear, **and**
//! * `cmd` is either `255` or the request's `report_id + 1`, **and**
//! * `ext2` equals the request's `ext2`.
//!
//! `seq` plays no part. On a request `seq` carries the payload length and on a cyclic
//! report it carries a CRC32, so it never was a usable key.
//!
//! Two further behaviours come from the same reading and are modelled here:
//!
//! * **The queue is drained before sending** (`_drain_input_queue`), so stale reports
//!   cannot satisfy the next request. The original logs
//!   `=== WARNING: CLEANING SINGLE RESPONSE QUEUE ===` when it discards anything.
//! * **Exactly one report is consumed per request.** The device takes the next item off
//!   the queue and validates it; it does not keep reading until something matches. A
//!   non-matching report therefore *fails* the request rather than being skipped.
//!
//! # Refusals: from traffic, not bytecode
//!
//! `_sanitize_response` only says a reply with the top bit of `cmd` set is not valid. What
//! such a reply looks like comes from hardware session 2, where a Studio+ panel start-up got
//! one among 67 replies (fixture `tests/fixtures/studio-live-replies.hex`):
//!
//! ```text
//! cmd 0x80000075   seq 0x10   ext2 0x80000011   ext3 0   contents all zero
//! ```
//!
//! That is the reply header a `get_*` with `ext2` 17 would get, with the top bit set in `cmd`
//! **and** in `ext2`, `ext3` not echoed and nothing in the contents. So a refusal is tied to the
//! outstanding request by the same two fields as an answer, each flagged: `cmd` is the request's
//! `report_id + 1 | 0x80000000` and `ext2` is its `ext2 | 0x80000000`. That completes the request
//! at once as [`Correlation::Refused`] rather than leaving it to time out. The rule is kept to
//! the one form seen: a flagged `255`, or a flagged `cmd` with a plain `ext2`, stays unmatched,
//! so a request can only ever be failed by a refusal naming it, never by one it merely resembles.

use crate::Report;
use std::time::{Duration, Instant};

/// The request timeout, from `self.request_timeout = 3.0` in `HWDevice.__init__`.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// `cmd` value the device accepts as a generic acknowledgement, whatever was asked.
pub const ACK_CMD: u32 = 255;

/// A response whose `cmd` has this bit set is never valid. A refusal sets it in `ext2` too.
const CMD_INVALID_MASK: u32 = 0x8000_0000;

/// A request awaiting its response.
#[derive(Clone, Debug)]
pub struct PendingRequest {
    /// The request's `report_id` (its `cmd` word). A valid response carries this **plus one**.
    pub report_id: u32,
    /// The request's `ext2` selector. A valid response echoes it exactly.
    pub ext2: u32,
    pub sent_at: Instant,
    /// Caller-supplied label (the command name) for diagnostics.
    pub label: String,
}

/// The result of checking a received report against the outstanding request.
#[derive(Clone, Debug)]
pub enum Correlation {
    /// The report satisfies the outstanding request.
    Matched { report: Report },
    /// The device refused the outstanding request; see [`ResponseCorrelator::is_refusal`].
    /// The request is no longer outstanding.
    Refused { report: Report },
    /// The report is not a valid response: cyclic traffic, noise, or a reply to something
    /// else. The request it was checked against remains outstanding.
    Unmatched { report: Report },
    /// A report arrived with no request outstanding. Always device-initiated.
    Unsolicited { report: Report },
}

/// Correlates responses with the single outstanding request.
pub struct ResponseCorrelator {
    pending: Option<PendingRequest>,
    /// Mirrors `_do_sanitize`. When false the device accepts any report as the response,
    /// which the original does for boot mode and for one device quirk.
    sanitize: bool,
}

impl Default for ResponseCorrelator {
    fn default() -> Self {
        ResponseCorrelator::new()
    }
}

impl ResponseCorrelator {
    pub fn new() -> Self {
        ResponseCorrelator { pending: None, sanitize: true }
    }

    /// Build a correlator that accepts any report as a response.
    ///
    /// Models `_do_sanitize = False`, and boot mode, where `_sanitize_response` sets
    /// `valid = True` without inspecting the header.
    pub fn without_sanitizing() -> Self {
        ResponseCorrelator { pending: None, sanitize: false }
    }

    /// Record an outgoing request. `report_id` and `ext2` come from its header.
    ///
    /// The device services one request at a time, so this replaces any previous request.
    pub fn record_request(&mut self, report_id: u32, ext2: u32, label: impl Into<String>) {
        self.pending = Some(PendingRequest {
            report_id,
            ext2,
            sent_at: Instant::now(),
            label: label.into(),
        });
    }

    /// Whether a report would be accepted as the response to `req`.
    ///
    /// Mirrors `_sanitize_response` exactly; see the module docs.
    pub fn is_valid_response(report: &Report, req: &PendingRequest, sanitize: bool) -> bool {
        if !sanitize {
            return true;
        }
        let cmd = report.cmd();
        if cmd & CMD_INVALID_MASK != 0 {
            return false;
        }
        let cmd_ok = cmd == ACK_CMD || cmd == req.report_id.wrapping_add(1);
        cmd_ok && report.ext2() == req.ext2
    }

    /// Whether a report is the device refusing `req`: the reply `cmd` and the request's `ext2`,
    /// each with the top bit set. See "Refusals" in the module docs.
    pub fn is_refusal(report: &Report, req: &PendingRequest) -> bool {
        report.cmd() == req.report_id.wrapping_add(1) | CMD_INVALID_MASK
            && report.ext2() == req.ext2 | CMD_INVALID_MASK
    }

    /// Check a received report against the outstanding request.
    ///
    /// Note the original consumes exactly one report per request: a report that fails
    /// validation does not get skipped in favour of the next one, it fails the request.
    /// Callers decide whether to keep waiting; this only classifies.
    pub fn correlate(&mut self, report: Report) -> Correlation {
        let Some(req) = self.pending.as_ref() else {
            return Correlation::Unsolicited { report };
        };
        if Self::is_valid_response(&report, req, self.sanitize) {
            self.pending = None;
            Correlation::Matched { report }
        } else if Self::is_refusal(&report, req) {
            self.pending = None;
            Correlation::Refused { report }
        } else {
            Correlation::Unmatched { report }
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn pending(&self) -> Option<&PendingRequest> {
        self.pending.as_ref()
    }

    /// Drop the outstanding request, e.g. after a timeout.
    pub fn drop_pending(&mut self) -> Option<PendingRequest> {
        self.pending.take()
    }

    /// Whether the outstanding request has exceeded the device's 3.0 s timeout.
    pub fn timed_out(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|r| r.sent_at.elapsed() >= REQUEST_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gazelle_audio_protocol::wire::Header;

    fn report(cmd: u32, ext2: u32) -> Report {
        Report { header: Header::new(cmd, 0, ext2, 0), contents: vec![0; 4] }
    }

    #[test]
    fn response_is_report_id_plus_one_with_matching_ext2() {
        let mut c = ResponseCorrelator::new();
        c.record_request(0x74, 4, "get_mixer");
        assert!(matches!(c.correlate(report(0x75, 4)), Correlation::Matched { .. }));
    }

    #[test]
    fn ack_cmd_255_is_accepted() {
        let mut c = ResponseCorrelator::new();
        c.record_request(0x70, 0, "set_mixer");
        assert!(matches!(c.correlate(report(255, 0)), Correlation::Matched { .. }));
    }

    #[test]
    fn mismatched_ext2_is_rejected() {
        let mut c = ResponseCorrelator::new();
        c.record_request(0x74, 4, "get_mixer");
        assert!(matches!(c.correlate(report(0x75, 9)), Correlation::Unmatched { .. }));
        assert!(c.has_pending(), "a rejected report leaves the request outstanding");
    }

    #[test]
    fn wrong_cmd_is_rejected() {
        let mut c = ResponseCorrelator::new();
        c.record_request(0x74, 4, "get_mixer");
        // 0x73 is the cyclic report, not a response to 0x74.
        assert!(matches!(c.correlate(report(0x73, 4)), Correlation::Unmatched { .. }));
    }

    #[test]
    fn high_bit_in_cmd_is_never_valid() {
        let mut c = ResponseCorrelator::new();
        c.record_request(0x74, 4, "get_mixer");
        assert!(matches!(
            c.correlate(report(0x8000_0000 | 0x75, 4)),
            Correlation::Unmatched { .. }
        ));
    }

    #[test]
    fn a_refusal_of_the_outstanding_request_completes_it_as_refused() {
        let mut c = ResponseCorrelator::new();
        c.record_request(0x74, 0x11, "get_feature_mask");
        // As captured in hardware session 2: the reply id and the request's ext2, both with the top bit set.
        assert!(matches!(c.correlate(report(0x8000_0075, 0x8000_0011)), Correlation::Refused { .. }));
        assert!(!c.has_pending(), "a refused request is no longer outstanding");
    }

    #[test]
    fn a_refusal_of_something_else_is_not_taken_for_this_request() {
        let mut c = ResponseCorrelator::new();
        c.record_request(0x74, 0x11, "get_feature_mask");
        for (cmd, ext2) in [
            (0x8000_0075, 0x8000_0004), // another selector refused
            (0x8000_0071, 0x8000_0011), // another command's reply id refused
            (0x8000_0075, 0x11),        // ext2 without the top bit: not the captured form
            (0x8000_00FF, 0x8000_0011), // a refused acknowledgement has never been seen
        ] {
            assert!(matches!(c.correlate(report(cmd, ext2)), Correlation::Unmatched { .. }), "cmd {cmd:#X} ext2 {ext2:#X}");
        }
        assert!(c.has_pending());
    }

    #[test]
    fn a_refusal_with_no_request_outstanding_is_unsolicited() {
        let mut c = ResponseCorrelator::new();
        assert!(matches!(c.correlate(report(0x8000_0075, 0x8000_0011)), Correlation::Unsolicited { .. }));
    }

    #[test]
    fn reports_with_no_request_are_unsolicited() {
        let mut c = ResponseCorrelator::new();
        assert!(matches!(c.correlate(report(0x73, 0)), Correlation::Unsolicited { .. }));
    }

    #[test]
    fn without_sanitizing_accepts_anything() {
        let mut c = ResponseCorrelator::without_sanitizing();
        c.record_request(0x74, 4, "get_mixer");
        assert!(matches!(c.correlate(report(0x00, 99)), Correlation::Matched { .. }));
    }

    #[test]
    fn recording_a_new_request_replaces_the_old() {
        let mut c = ResponseCorrelator::new();
        c.record_request(0x70, 0, "a");
        c.record_request(0x74, 4, "b");
        assert_eq!(c.pending().unwrap().label, "b");
        // The abandoned request must not still match.
        assert!(matches!(c.correlate(report(0x71, 0)), Correlation::Unmatched { .. }));
    }
}
