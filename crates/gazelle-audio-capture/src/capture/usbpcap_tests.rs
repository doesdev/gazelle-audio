//! Unit tests for the platform-neutral plumbing used by `platform::start`/`find_hub` on Windows:
//! `exit_tool_error`, `Supervised` and `select_hub`. None of this spawns a process; the
//! Windows-only parts (`Child::kill`/`wait`, actually running USBPcapCMD) are out of reach here
//! and are checked by hand against a real capture.

use std::cell::Cell;

use super::*;
use crate::capture::decode::ByteOrder;

fn dummy_frame(index: u64) -> RawFrame {
    RawFrame { ts_ns: 0, link_type: 249, index, orig_len: 0, byte_order: ByteOrder::Little, data: Vec::new() }
}

/// Stand-in for the `IoError(UnexpectedEof)` `pcap-file` yields on a truncated record: the
/// shape of error a killed USBPcapCMD leaves behind mid-write.
fn truncated() -> CaptureError {
    CaptureError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "truncated record"))
}

fn frame_iter(items: Vec<Result<RawFrame, CaptureError>>) -> FrameIter {
    Box::new(items.into_iter())
}

fn msg(e: &CaptureError) -> String {
    e.to_string()
}

// --- exit_tool_error ---

#[test]
fn exit_tool_error_suppresses_a_requested_stop() {
    assert!(exit_tool_error(true, Some(1)).is_none());
}

#[test]
fn exit_tool_error_ignores_a_clean_exit() {
    assert!(exit_tool_error(false, Some(0)).is_none());
}

#[test]
fn exit_tool_error_ignores_an_unreapable_exit() {
    assert!(exit_tool_error(false, None).is_none());
}

#[test]
fn exit_tool_error_reports_a_bad_exit() {
    let err = exit_tool_error(false, Some(3)).expect("a non-zero unrequested exit must report");
    assert!(msg(&err).contains('3'), "{err}");
}

// --- Supervised ---

#[test]
fn supervised_ends_cleanly_on_a_trailing_error_after_stop() {
    let inner = frame_iter(vec![Ok(dummy_frame(0)), Err(truncated())]);
    let stopped = Arc::new(AtomicBool::new(true));
    let mut s = Supervised { inner, stopped, reap: || None, reaped: false };
    assert!(matches!(s.next(), Some(Ok(_))));
    assert!(s.next().is_none(), "a trailing error after a requested stop must end cleanly");
}

#[test]
fn supervised_propagates_a_trailing_error_without_stop() {
    let inner = frame_iter(vec![Ok(dummy_frame(0)), Err(truncated())]);
    let stopped = Arc::new(AtomicBool::new(false));
    let mut s = Supervised { inner, stopped, reap: || None, reaped: false };
    assert!(matches!(s.next(), Some(Ok(_))));
    assert!(matches!(s.next(), Some(Err(_))), "an error with no stop requested must still propagate");
}

#[test]
fn supervised_reacts_to_a_stop_flipped_mid_stream() {
    // Mirrors the real StopHandle: the flag flips (by the stop closure) between reads of the
    // underlying stream, not necessarily before the very first frame.
    let inner = frame_iter(vec![Ok(dummy_frame(0)), Err(truncated())]);
    let stopped = Arc::new(AtomicBool::new(false));
    let mut s = Supervised { inner, stopped: Arc::clone(&stopped), reap: || None, reaped: false };
    assert!(matches!(s.next(), Some(Ok(_))));
    stopped.store(true, Ordering::Relaxed);
    assert!(s.next().is_none());
}

#[test]
fn supervised_reports_a_bad_exit_once_the_stream_ends_on_its_own() {
    let inner = frame_iter(vec![]);
    let stopped = Arc::new(AtomicBool::new(false));
    let mut s = Supervised { inner, stopped, reap: || Some(1), reaped: false };
    match s.next() {
        Some(Err(e)) => assert!(msg(&e).contains('1'), "{e}"),
        other => panic!("expected a final tool error, got {other:?}"),
    }
    assert!(s.next().is_none(), "the stream must end after the one synthetic error");
}

#[test]
fn supervised_reports_nothing_for_a_requested_stop_even_with_a_bad_exit_code() {
    let inner = frame_iter(vec![]);
    let stopped = Arc::new(AtomicBool::new(true));
    let mut s = Supervised { inner, stopped, reap: || Some(1), reaped: false };
    assert!(s.next().is_none());
}

#[test]
fn supervised_reports_nothing_for_a_clean_exit() {
    let inner = frame_iter(vec![]);
    let stopped = Arc::new(AtomicBool::new(false));
    let mut s = Supervised { inner, stopped, reap: || Some(0), reaped: false };
    assert!(s.next().is_none());
}

#[test]
fn supervised_only_reaps_once() {
    // `reap` must be `Send` (it is boxed into a `FrameIter`), so the counter is an atomic rather
    // than a `Cell`.
    use std::sync::atomic::AtomicU32;
    let calls = AtomicU32::new(0);
    let inner = frame_iter(vec![]);
    let stopped = Arc::new(AtomicBool::new(false));
    let mut s = Supervised {
        inner,
        stopped,
        reap: || {
            calls.fetch_add(1, Ordering::Relaxed);
            None
        },
        reaped: false,
    };
    assert!(s.next().is_none());
    assert!(s.next().is_none());
    assert_eq!(calls.load(Ordering::Relaxed), 1, "reap must run exactly once even across repeated polls past the end");
}

// --- select_hub ---

fn hub(n: u16) -> HubInterface {
    HubInterface { value: format!("h{n}"), display: format!("H{n}") }
}

#[test]
fn select_hub_returns_the_first_hub_that_finds_the_target() {
    let hubs = vec![hub(1), hub(2)];
    let result = select_hub(hubs, |h| Ok(h.value == "h1"));
    assert_eq!(result.unwrap(), Some("h1".to_string()));
}

#[test]
fn select_hub_skips_a_failing_hub_and_finds_the_target_on_the_next() {
    let hubs = vec![hub(1), hub(2)];
    let result = select_hub(hubs, |h| if h.value == "h1" { Err(CaptureError::Tool("boom".into())) } else { Ok(true) });
    assert_eq!(result.unwrap(), Some("h2".to_string()));
}

#[test]
fn select_hub_returns_the_last_error_when_none_match() {
    let hubs = vec![hub(1), hub(2)];
    let result = select_hub(hubs, |h| Err(CaptureError::Tool(format!("bad {}", h.value))));
    let err = result.unwrap_err();
    assert!(msg(&err).contains("bad h2"), "{err}");
}

#[test]
fn select_hub_returns_ok_none_when_no_hub_matches_or_errors() {
    let hubs = vec![hub(1), hub(2)];
    let result = select_hub(hubs, |_| Ok(false));
    assert_eq!(result.unwrap(), None);
}

#[test]
fn select_hub_does_not_probe_after_a_match() {
    let hubs = vec![hub(1), hub(2)];
    let calls = Cell::new(0u32);
    let result = select_hub(hubs, |h| {
        calls.set(calls.get() + 1);
        Ok(h.value == "h1")
    });
    assert_eq!(result.unwrap(), Some("h1".to_string()));
    assert_eq!(calls.get(), 1, "must not probe hubs after the target is found");
}
