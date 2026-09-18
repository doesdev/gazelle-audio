//! Changing the buffer size and Safe Mode: one call, `SetASIOBufferPreferredSize`, built from what
//! the driver reports now for anything not being changed, then read back. Every test here uses the
//! fake driver; none touches a real one.

use std::sync::Arc;
use std::time::Duration;

use super::asio::tests::{quadro, quadro_in_use, quadro_safe_mode_off};
use super::fake::{FakeDll, FakePc};
use super::*;

const QUADRO: &str = "1000000000001";
const STUDIO: &str = "1000000000003";

fn both() -> (Arc<FakeDll>, Arc<FakeDll>, FakePc) {
    let quadro = Arc::new(FakeDll::quadro(QUADRO));
    let studio = Arc::new(FakeDll::studio(STUDIO));
    let pc = FakePc::both(quadro.clone(), studio.clone());
    (quadro, studio, pc)
}

fn quadro_with(bytes: Vec<u8>) -> (Arc<FakeDll>, FakePc) {
    let quadro = Arc::new(FakeDll::quadro(QUADRO).with_asio(bytes));
    let pc = FakePc::both(quadro.clone(), Arc::new(FakeDll::studio(STUDIO)));
    (quadro, pc)
}

fn change(buffer_size: Option<u32>, safe_mode: Option<bool>) -> DriverChange {
    DriverChange { buffer_size, safe_mode, force: false }
}

fn call(asio_instance: u32, reference_sample_rate: u32, preferred_size: u32, options: u32) -> SetterCall {
    SetterCall { asio_instance, reference_sample_rate, preferred_size, options }
}

fn refusal(result: Result<DriverWrite, WriteRefusal>) -> WriteRefusal {
    match result {
        Err(refusal) => refusal,
        Ok(write) => panic!("expected a refusal, got {write:?}"),
    }
}

fn written(result: Result<DriverWrite, WriteRefusal>) -> DriverWrite {
    match result {
        Ok(write) => write,
        Err(refusal) => panic!("expected a write, got {refusal:?}"),
    }
}

fn read_back_asio(write: &DriverWrite) -> AsioInstance {
    match &write.read_back {
        DriverAnswer::Read(DriverSettings { asio: Reading::Read { value }, .. }) => value.clone(),
        other => panic!("no read-back: {other:?}"),
    }
}

#[test]
fn changing_only_the_buffer_keeps_safe_mode_as_it_is() {
    // Safe Mode on (the Quadro as read): the buffer changes, the Safe Mode bit is sent again.
    let (quadro, _, pc) = both();
    let write = written(write_device(&pc, QUADRO, &change(Some(256), None), false));
    assert_eq!(write.call, Some(call(0, 44100, 256, 0x10000)));
    assert_eq!(quadro.sets(), vec![call(0, 44100, 256, 0x10000)]);

    // Safe Mode off: the same kind of change sends options 0.
    let (quadro, pc) = quadro_with(quadro_safe_mode_off());
    written(write_device(&pc, QUADRO, &change(Some(512), None), false));
    assert_eq!(quadro.sets(), vec![call(0, 44100, 512, 0)]);
}

#[test]
fn changing_only_safe_mode_keeps_the_buffer_as_it_is() {
    let (quadro, studio, pc) = both();
    written(write_device(&pc, QUADRO, &change(None, Some(false)), false));
    assert_eq!(quadro.sets(), vec![call(0, 44100, 512, 0)]);

    let (quadro_off, pc_off) = quadro_with(quadro_safe_mode_off());
    written(write_device(&pc_off, QUADRO, &change(None, Some(true)), false));
    assert_eq!(quadro_off.sets(), vec![call(0, 44100, 256, 0x10000)]);

    // The Studio+ (no instance count in its API) is set through its own DLL, instance 0.
    written(write_device(&pc, STUDIO, &change(None, Some(false)), false));
    assert_eq!(studio.sets(), vec![call(0, 44100, 512, 0)]);
}

#[test]
fn both_at_once_is_one_call() {
    let (quadro, _, pc) = both();
    written(write_device(&pc, QUADRO, &change(Some(128), Some(false)), false));
    assert_eq!(quadro.sets(), vec![call(0, 44100, 128, 0)]);
}

#[test]
fn the_reference_rate_is_the_structures_offset_4() {
    let mut bytes = quadro();
    bytes[4..8].copy_from_slice(&48000u32.to_le_bytes());
    let (quadro, pc) = quadro_with(bytes);
    written(write_device(&pc, QUADRO, &change(Some(256), None), false));
    assert_eq!(quadro.sets(), vec![call(0, 48000, 256, 0x10000)]);
}

#[test]
fn an_implausible_reference_rate_sends_nothing() {
    let mut bytes = quadro();
    bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
    let (quadro, pc) = quadro_with(bytes);
    let refused = refusal(write_device(&pc, QUADRO, &change(Some(256), None), false));
    assert_eq!(refused.code, RefusalCode::Unreadable);
    assert!(quadro.sets().is_empty());
}

#[test]
fn a_size_the_driver_does_not_offer_is_refused_and_nothing_is_sent() {
    let (quadro, _, pc) = both();
    let refused = refusal(write_device(&pc, QUADRO, &change(Some(500), None), false));
    assert_eq!(refused.code, RefusalCode::NotOffered);
    assert!(refused.message.contains("500"), "{}", refused.message);
    assert!(refused.message.contains("8, 16, 32, 64, 128, 256, 512, 1024, 2048"), "{}", refused.message);
    assert!(quadro.sets().is_empty());
}

#[test]
fn a_request_that_changes_nothing_is_refused_or_answered_without_a_call() {
    let (quadro, _, pc) = both();
    assert_eq!(refusal(write_device(&pc, QUADRO, &change(None, None), false)).code, RefusalCode::NothingToChange);
    // What the driver already has: no call, so no program's audio restarts for nothing.
    let write = written(write_device(&pc, QUADRO, &change(Some(512), Some(true)), false));
    assert_eq!(write.outcome, WriteOutcome::Unchanged);
    assert_eq!(write.call, None);
    assert!(quadro.sets().is_empty());
}

#[test]
fn the_answer_is_what_the_driver_reports_after_the_call() {
    let (_, _, pc) = both();
    let write = written(write_device(&pc, QUADRO, &change(Some(256), Some(false)), false));
    assert_eq!(write.outcome, WriteOutcome::Applied, "{}", write.message);
    let after = read_back_asio(&write);
    assert_eq!((after.buffer_size, after.safe_mode), (256, false));
    // The fake's latencies for 256 samples with Safe Mode off, as the Quadro reported them.
    assert_eq!((after.input_latency, after.output_latency), (315, 191));
}

#[test]
fn a_read_back_that_does_not_match_is_reported_as_a_mismatch() {
    let quadro = Arc::new(FakeDll { ignores_set: true, ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    let write = written(write_device(&pc, QUADRO, &change(Some(256), None), false));
    assert_eq!(write.outcome, WriteOutcome::Mismatch);
    assert!(write.message.contains("256"), "{}", write.message);
    assert!(write.message.contains("512"), "{}", write.message);
    assert_eq!(read_back_asio(&write).buffer_size, 512, "the read-back is shown as it is");

    let write = written(write_device(&pc, QUADRO, &change(None, Some(false)), false));
    assert_eq!(write.outcome, WriteOutcome::Mismatch);
    assert!(write.message.contains("Safe Mode"), "{}", write.message);
}

#[test]
fn a_setter_that_answers_a_status_is_reported_with_the_read_back() {
    let quadro = Arc::new(FakeDll { failing: [("TUSBAUDIO_SetASIOBufferPreferredSize", 0xee00_0005)].into(), ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    let write = written(write_device(&pc, QUADRO, &change(Some(256), None), false));
    assert_eq!(write.outcome, WriteOutcome::Failed);
    assert!(write.message.contains("TSTATUS_INVALID_PARAMETER"), "{}", write.message);
    assert_eq!(read_back_asio(&write).buffer_size, 512);
}

#[test]
fn a_read_back_that_cannot_be_read_is_unconfirmed() {
    let quadro = Arc::new(FakeDll { fails_after_set: true, ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    let write = written(write_device(&pc, QUADRO, &change(Some(256), None), false));
    assert_eq!(write.outcome, WriteOutcome::Unconfirmed, "{}", write.message);
}

#[test]
fn a_program_using_asio_refuses_the_change_unless_it_is_forced() {
    // One DAW recording on the Quadro counted 4 (2026-09-18), so the message gives no count.
    const IN_USE: &str = "The driver's ASIO interface is in use (by a DAW, most likely), and changing the buffer or Safe Mode restarts its audio. Nothing was sent; ask again with force to change it anyway.";
    let (quadro, pc) = quadro_with(quadro_in_use(1));
    let refused = refusal(write_device(&pc, QUADRO, &change(Some(256), None), false));
    assert_eq!(refused.code, RefusalCode::AsioInUse);
    assert_eq!(refused.message, IN_USE);
    assert!(quadro.sets().is_empty());

    let (_, pc2) = quadro_with(quadro_in_use(4));
    assert_eq!(refusal(write_device(&pc2, QUADRO, &change(None, Some(false)), false)).message, IN_USE);

    let forced = DriverChange { force: true, ..change(Some(256), None) };
    let write = written(write_device(&pc, QUADRO, &forced, false));
    assert_eq!(write.outcome, WriteOutcome::Applied);
    assert_eq!(quadro.sets(), vec![call(0, 44100, 256, 0x10000)]);
}

#[test]
fn a_size_not_offered_is_refused_even_when_forced() {
    let (quadro, _, pc) = both();
    let forced = DriverChange { force: true, ..change(Some(3), None) };
    assert_eq!(refusal(write_device(&pc, QUADRO, &forced, false)).code, RefusalCode::NotOffered);
    assert!(quadro.sets().is_empty());
}

#[test]
fn a_dry_run_builds_the_call_and_sends_nothing() {
    let (quadro, _, pc) = both();
    let write = written(write_device(&pc, QUADRO, &change(Some(256), None), true));
    assert_eq!(write.outcome, WriteOutcome::DryRun);
    assert_eq!(write.call, Some(call(0, 44100, 256, 0x10000)));
    assert!(quadro.sets().is_empty());
}

#[test]
fn a_device_with_no_reading_cannot_be_changed() {
    let (quadro, studio, pc) = both();
    assert_eq!(refusal(write_device(&pc, "999", &change(Some(256), None), false)).code, RefusalCode::Unavailable);
    let unread = Arc::new(FakeDll { missing: vec!["TUSBAUDIO_GetASIOInstanceInfo"], ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(unread.clone(), studio);
    assert_eq!(refusal(write_device(&pc, QUADRO, &change(Some(256), None), false)).code, RefusalCode::Unreadable);
    assert!(quadro.sets().is_empty() && unread.sets().is_empty());
}

#[test]
fn a_write_calls_nothing_but_the_reads_and_the_one_setter() {
    let (quadro, studio, pc) = both();
    written(write_device(&pc, QUADRO, &change(Some(256), Some(false)), false));
    written(write_device(&pc, STUDIO, &change(Some(1024), None), false));
    for call in quadro.calls().iter().chain(studio.calls().iter()) {
        assert!(READ_EXPORTS.contains(&call.as_str()) || WRITE_EXPORTS.contains(&call.as_str()), "{call} was called");
    }
    assert!(quadro.calls().contains(&"TUSBAUDIO_SetASIOBufferPreferredSize".to_string()));
}

#[test]
fn the_one_setter_is_the_only_write_the_module_may_resolve() {
    assert_eq!(WRITE_EXPORTS, &["TUSBAUDIO_SetASIOBufferPreferredSize"]);
    // Every name the Windows side mentions is a read or that setter: no Load, Dfu, Start, Enable,
    // Disable or any other Set can be reached by a literal added there.
    let source = include_str!("windows.rs");
    let mut seen = 0;
    for (at, _) in source.match_indices("\"TUSBAUDIO_") {
        let name = &source[at + 1..at + 1 + source[at + 1..].find('"').unwrap()];
        assert!(READ_EXPORTS.contains(&name) || WRITE_EXPORTS.contains(&name), "windows.rs names {name}");
        seen += 1;
    }
    assert!(seen >= READ_EXPORTS.len(), "the scan found the names ({seen})");
    // And the resolver takes its names from the two lists and nowhere else.
    assert!(source.contains("for &name in READ_EXPORTS.iter().chain(WRITE_EXPORTS)"), "the resolver's list changed");
}

#[test]
fn a_write_leaves_the_read_back_in_the_cache() {
    let (_, _, pc) = both();
    let service = DriverService::new(Arc::new(pc), Duration::from_secs(60));
    service.read("serial:x", QUADRO, false);
    let report = service.write("serial:x", QUADRO, &change(Some(256), None), false).unwrap();
    assert_eq!(report.outcome, WriteOutcome::Applied);
    assert!(!report.read_back.cached);
    let again = service.read("serial:x", QUADRO, false);
    let DriverAnswer::Read(DriverSettings { asio: Reading::Read { value }, .. }) = again.answer else { panic!() };
    assert_eq!(value.buffer_size, 256, "not the answer from before the write");
}

#[test]
fn a_write_report_serialises_with_its_outcome_call_and_read_back() {
    let (_, _, pc) = both();
    let service = DriverService::new(Arc::new(pc), CACHE_FOR);
    let json = serde_json::to_value(service.write("serial:x", QUADRO, &change(Some(256), None), false).unwrap()).unwrap();
    assert_eq!(json["device_id"], "serial:x");
    assert_eq!(json["outcome"], "applied");
    assert_eq!(json["call"], serde_json::json!({ "asio_instance": 0, "reference_sample_rate": 44100, "preferred_size": 256, "options": 65536 }));
    assert_eq!(json["read_back"]["state"], "read");
    assert_eq!(json["read_back"]["asio"]["value"]["buffer_size"], 256);
    let refused = serde_json::to_value(WriteRefusal { code: RefusalCode::AsioInUse, message: "m".into() }).unwrap();
    assert_eq!(refused, serde_json::json!({ "code": "asio_in_use", "message": "m" }));
}
