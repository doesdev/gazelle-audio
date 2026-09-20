use std::time::Duration;

use super::fake::{FakePc, Spec};
use super::*;

fn options() -> Options {
    Options { proceed: true, seconds: 10, only: None, rate: None, set_rate: None }
}

fn both() -> FakePc {
    FakePc::this_pc(Spec::quadro(), Spec::studio())
}

fn both_with(quadro: Spec, studio: Spec) -> FakePc {
    FakePc::this_pc(quadro, studio)
}

fn run_named(name: &str, callbacks: u64, buffer: i32) -> Run {
    Run { name: name.into(), callbacks, buffer_size: buffer }
}

#[test]
fn the_probe_refuses_when_hardware_is_forbidden() {
    let said = refusal(Some("1")).expect("it must refuse");
    assert!(said.contains(NO_HARDWARE), "{said}");
    assert!(said.contains("will not run"), "{said}");
    assert_eq!(refusal(None), None);
    assert_eq!(refusal(Some("0")), None);
    // Only an exact 1 forbids it, as everywhere else in the workspace.
    assert_eq!(refusal(Some("true")), None);
}

#[test]
fn a_run_without_yes_lists_and_opens_nothing() {
    let pc = both();
    let outcome = run(&pc, &Options::default()).expect("the listing must work");
    assert!(outcome.listed_only);
    assert_eq!(outcome.entries.len(), 5);
    assert_eq!(outcome.targets.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), [
        "Zen Quadro Synergy Core",
        "ZenStudioTB ASIO Driver"
    ]);
    assert!(outcome.opened.is_empty());
    assert!(outcome.runs.is_empty());
    assert!(!outcome.all_started());
    // Nothing was created, so nothing was asked of any driver.
    assert!(pc.calls().is_empty(), "{:?}", pc.calls());
}

#[test]
fn both_drivers_open_start_and_stop_in_one_process() {
    let pc = both();
    let outcome = run(&pc, &options()).expect("the run must work");
    assert!(outcome.failure.is_none());
    assert!(outcome.all_started());
    assert_eq!(outcome.opened.len(), 2);
    assert_eq!(outcome.runs.len(), 2);
    let calls = pc.calls();
    // The second driver is created while the first is still open, and both are released.
    let create_second = calls.iter().position(|c| c == "studio: create").expect("the second was created");
    let release_first = calls.iter().position(|c| c == "quadro: release").expect("the first was released");
    assert!(create_second < release_first, "{calls:?}");
    assert!(calls.contains(&"quadro: start".to_string()) && calls.contains(&"studio: start".to_string()));
    assert!(calls.contains(&"quadro: stop".to_string()) && calls.contains(&"studio: stop".to_string()));
    assert!(calls.contains(&"studio: disposeBuffers".to_string()));
}

#[test]
fn a_locked_pair_stays_in_step() {
    let pc = both();
    let outcome = run(&pc, &options()).expect("the run must work");
    let drift = drift(&outcome.runs, outcome.elapsed).expect("two runs");
    assert_eq!(drift.callbacks, 0);
    assert_eq!(drift.samples, 0);
    assert_eq!(drift.verdict, Verdict::InStep);
    // 44.1 kHz in 512 sample buffers is 861 callbacks over the ten seconds, on both.
    assert_eq!(outcome.runs.iter().map(|r| r.callbacks).collect::<Vec<_>>(), [861, 861]);
    assert_eq!(rate_of(&outcome.runs[0], outcome.elapsed), 86.1);
    assert_eq!(outcome.runs[0].samples(), 861 * 512);
}

#[test]
fn a_free_running_pair_drifts_and_the_report_says_by_how_much() {
    // The Studio+ off the cable, running 30 callbacks a second slower over ten seconds.
    let pc = FakePc::this_pc(Spec::quadro().at(100.0), Spec::studio().at(70.0));
    let outcome = run(&pc, &options()).expect("the run must work");
    let drift = drift(&outcome.runs, outcome.elapsed).expect("two runs");
    assert_eq!((drift.a.as_str(), drift.b.as_str()), ("quadro", "studio"));
    assert_eq!(drift.callbacks, 300);
    assert_eq!(drift.callbacks_per_second, 30.0);
    assert_eq!(drift.samples, 300 * 512);
    assert_eq!(drift.samples_per_second, 30.0 * 512.0);
    assert_eq!(drift.verdict, Verdict::Drifting);
}

#[test]
fn one_callback_of_skew_is_not_drift_but_two_are() {
    let elapsed = Duration::from_secs(10);
    let in_step = [run_named("quadro", 861, 512), run_named("studio", 860, 512)];
    assert_eq!(drift(&in_step, elapsed).expect("two runs").verdict, Verdict::InStep);
    let drifting = [run_named("quadro", 862, 512), run_named("studio", 860, 512)];
    let drifting = drift(&drifting, elapsed).expect("two runs");
    assert_eq!(drifting.verdict, Verdict::Drifting);
    assert_eq!(drifting.callbacks_per_second, 0.2);
    // A driver behind the other reports a negative difference, not an underflowed number.
    let behind = [run_named("quadro", 860, 512), run_named("studio", 900, 512)];
    assert_eq!(drift(&behind, elapsed).expect("two runs").callbacks, -40);
    assert_eq!(drift(&[run_named("quadro", 1, 512)], elapsed), None);
}

#[test]
fn a_second_driver_that_refuses_to_be_created_names_the_step_and_what_was_open() {
    let pc = both().refusing("studio", "class not registered");
    let outcome = run(&pc, &options()).expect("the run must report");
    let failure = outcome.failure.clone().expect("it must fail");
    assert_eq!(failure.driver, "studio");
    assert_eq!(failure.step, Step::Create);
    assert_eq!(failure.step.call(), "CoCreateInstance");
    assert_eq!(failure.message, "class not registered");
    assert_eq!(failure.already_open, 1);
    assert!(!outcome.all_started());
    assert!(outcome.runs.is_empty());
    // The first driver was still cleaned up and released.
    assert!(pc.calls().contains(&"quadro: release".to_string()), "{:?}", pc.calls());
}

#[test]
fn a_second_driver_that_fails_at_init_while_the_first_is_open_is_reported_and_released() {
    let pc = FakePc::this_pc(Spec::quadro(), Spec::studio().failing(Step::Init, "the device is already in use"));
    let outcome = run(&pc, &options()).expect("the run must report");
    let failure = outcome.failure.expect("it must fail");
    assert_eq!((failure.driver.as_str(), failure.step), ("studio", Step::Init));
    assert_eq!(failure.message, "the device is already in use");
    assert_eq!(failure.already_open, 1);
    // Only the driver that got through init was read, and neither was started.
    assert_eq!(outcome.opened.len(), 1);
    let calls = pc.calls();
    assert!(!calls.iter().any(|c| c.ends_with(": start")), "{calls:?}");
    assert!(calls.contains(&"studio: release".to_string()) && calls.contains(&"quadro: release".to_string()));
}

#[test]
fn a_driver_that_fails_at_start_while_the_other_runs_is_reported_and_both_are_stopped() {
    let pc = FakePc::this_pc(Spec::quadro(), Spec::studio().failing(Step::Start, "could not lock to the clock"));
    let outcome = run(&pc, &options()).expect("the run must report");
    let failure = outcome.failure.expect("it must fail");
    assert_eq!((failure.driver.as_str(), failure.step), ("studio", Step::Start));
    assert_eq!(failure.message, "could not lock to the clock");
    // One driver was already running when the second refused.
    assert_eq!(failure.already_open, 1);
    assert!(outcome.runs.is_empty());
    let calls = pc.calls();
    assert!(calls.contains(&"quadro: stop".to_string()), "{calls:?}");
    assert!(calls.contains(&"quadro: disposeBuffers".to_string()) && calls.contains(&"studio: disposeBuffers".to_string()));
}

#[test]
fn a_driver_that_fails_at_create_buffers_is_reported_before_anything_starts() {
    let pc = FakePc::this_pc(Spec::quadro(), Spec::studio().failing(Step::CreateBuffers, "no free buffers"));
    let outcome = run(&pc, &options()).expect("the run must report");
    let failure = outcome.failure.expect("it must fail");
    assert_eq!((failure.driver.as_str(), failure.step), ("studio", Step::CreateBuffers));
    assert_eq!(failure.step.call(), "createBuffers");
    assert!(!pc.calls().iter().any(|c| c.ends_with(": start")), "{:?}", pc.calls());
}

#[test]
fn mismatched_preferred_buffer_sizes_are_reported_and_the_run_goes_on() {
    let pc = FakePc::this_pc(Spec::quadro(), Spec::studio().preferring(256));
    let outcome = run(&pc, &options()).expect("the run must work");
    assert!(outcome.all_started(), "a mismatch must not stop the run");
    let mismatch = buffer_mismatch(&outcome.opened).expect("the sizes differ");
    assert_eq!(mismatch, vec![("quadro".to_string(), 512), ("studio".to_string(), 256)]);
    // Each driver is opened at its own preferred size, not at the other's.
    let calls = pc.calls();
    assert!(calls.contains(&"quadro: createBuffers 2 in, 2 out, 512".to_string()), "{calls:?}");
    assert!(calls.contains(&"studio: createBuffers 2 in, 2 out, 256".to_string()), "{calls:?}");
    // Matching sizes say nothing.
    assert_eq!(buffer_mismatch(&run(&both(), &options()).expect("a matched run").opened), None);
}

#[test]
fn only_runs_the_one_driver_named() {
    let pc = both();
    let mut options = options();
    options.only = Some("Studio".into());
    let outcome = run(&pc, &options).expect("the run must work");
    assert_eq!(outcome.targets.len(), 1);
    assert_eq!(outcome.runs.len(), 1);
    assert_eq!(outcome.runs[0].name, "studio");
    assert!(drift(&outcome.runs, outcome.elapsed).is_none());
    assert!(!pc.calls().iter().any(|c| c.starts_with("quadro")), "{:?}", pc.calls());
    // A name that is not there leaves nothing to open, and the probe says so by opening nothing.
    let mut nowhere = options.clone();
    nowhere.only = Some("mixbus".into());
    let outcome = run(&both(), &nowhere).expect("the run must work");
    assert!(outcome.targets.is_empty());
    assert!(outcome.runs.is_empty());
}

#[test]
fn a_rate_is_asked_about_and_never_set() {
    let pc = both();
    let mut options = options();
    options.rate = Some(96_000.0);
    let outcome = run(&pc, &options).expect("the run must work");
    assert_eq!(outcome.opened.iter().map(|o| o.can_rate).collect::<Vec<_>>(), [Some(true), Some(true)]);
    options.rate = Some(192_000.0);
    let outcome = run(&both(), &options).expect("the run must work");
    assert_eq!(outcome.opened.iter().map(|o| o.can_rate).collect::<Vec<_>>(), [Some(false), Some(false)]);
    // The rate is asked about, and the run still reports the rate each driver is actually on.
    assert_eq!(outcome.opened[0].description.rate, 44_100.0);
    assert!(pc.calls().contains(&"quadro: canSampleRate".to_string()));
    // With no rate asked for, the question is never put.
    let plain = run(&both(), &Options { proceed: true, ..Options::default() }).expect("the run must work");
    assert_eq!(plain.opened.iter().map(|o| o.can_rate).collect::<Vec<_>>(), [None, None]);
}

#[test]
fn only_the_two_antelope_entries_are_ever_opened() {
    let pc = both();
    let outcome = run(&pc, &options()).expect("the run must work");
    assert_eq!(outcome.entries.len(), 5);
    assert_eq!(outcome.targets.len(), 2);
    for target in &outcome.targets {
        assert!(target.target().is_some(), "{target:?}");
    }
    assert!(!pc.calls().iter().any(|c| c.starts_with("Realtek") || c.starts_with("Yamaha")), "{:?}", pc.calls());
}

#[test]
fn class_ids_match_whatever_their_case_or_braces() {
    assert!(same_clsid(QUADRO_CLSID, &QUADRO_CLSID.to_ascii_lowercase()));
    assert!(same_clsid(QUADRO_CLSID, QUADRO_CLSID.trim_matches(|c| c == '{' || c == '}')));
    assert!(!same_clsid(QUADRO_CLSID, STUDIO_CLSID));
    let entries = both().entries;
    let chosen = chosen(&entries, None);
    assert_eq!(chosen.len(), 2);
    // The order is the probe's, not the registry's: the clock master first.
    assert_eq!(chosen[0].target(), Some("quadro"));
    assert_eq!(chosen[1].target(), Some("studio"));
}

#[test]
fn set_rate_moves_every_driver_before_any_buffer_is_made() {
    // The drivers idle at whatever Windows last used (44.1 kHz here), and a DAW moves them when it
    // opens. The probe does the same with --set-rate, so drift can be measured at the rate the user
    // records at. Every driver moves before any of them makes buffers, as the aggregate will do.
    let pc = both();
    let outcome = run(&pc, &Options { set_rate: Some(96_000.0), ..options() }).expect("the run must work");
    assert_eq!(outcome.opened.iter().map(|o| o.description.rate).collect::<Vec<_>>(), [96_000.0, 96_000.0]);
    let calls = pc.calls();
    let first_buffers = calls.iter().position(|c| c.contains("createBuffers")).expect("buffers were made");
    let last_set = calls.iter().rposition(|c| c.contains("set_rate")).expect("the rate was set");
    assert!(last_set < first_buffers, "every rate is set before the first buffer: {calls:?}");
    // At 96 kHz in 512 sample buffers the callbacks come about twice as often as at 44.1.
    assert_eq!(rate_of(&outcome.runs[0], outcome.elapsed), 187.5);
}

#[test]
fn a_driver_that_refuses_the_rate_says_so_and_nothing_starts() {
    let pc = both_with(Spec::quadro(), Spec::studio().failing(Step::SetRate, "rate not supported"));
    let outcome = run(&pc, &Options { set_rate: Some(96_000.0), ..options() }).expect("a refusal is an answer");
    let failure = outcome.failure.expect("the refusal is recorded");
    assert_eq!(failure.step, Step::SetRate);
    assert_eq!(failure.driver, "studio");
    assert_eq!(failure.already_open, 1, "the Quadro was open while the Studio+ refused");
    assert!(outcome.runs.is_empty(), "nothing ran");
    assert!(!pc.calls().iter().any(|c| c.contains("start")), "and nothing started: {:?}", pc.calls());
}

#[test]
fn sample_positions_measure_the_two_clocks_far_more_finely_than_callbacks_do() {
    // Counting callbacks quantises to a buffer, so a 20 second run cannot tell a locked pair from a
    // pair that is 50 parts per million apart. getSamplePosition reports an exact sample count with
    // a timestamp, which gives each driver's real rate and so the difference between them.
    let pc = both();
    let outcome = run(&pc, &options()).expect("the run must work");
    let measured = clocks(&outcome.positions).expect("both drivers reported a position");
    assert_eq!(measured.rates, [96_000.0, 96_000.0].map(|_| 44_100.0), "the fake runs both at 44.1 kHz");
    assert_eq!(measured.parts_per_million, 0.0, "one clock, so no difference at all");
}

#[test]
fn a_pair_on_two_crystals_shows_up_in_parts_per_million() {
    // 50 ppm at 44.1 kHz is about 2.2 samples a second: under half a callback over ten seconds, so
    // the callback count would still read "in step" while the positions say otherwise.
    let drifting = Spec::studio().at(44_100.0 * (1.0 - 50e-6) / 512.0).positions_at(44_100.0 * (1.0 - 50e-6));
    let pc = both_with(Spec::quadro(), drifting);
    let outcome = run(&pc, &options()).expect("the run must work");
    assert_eq!(drift(&outcome.runs, outcome.elapsed).expect("two runs").verdict, Verdict::InStep, "callbacks cannot see it");
    let measured = clocks(&outcome.positions).expect("both drivers reported a position");
    assert!((measured.parts_per_million - 50.0).abs() < 1.0, "{} ppm", measured.parts_per_million);
}

#[test]
fn many_readings_fitted_see_through_the_quantising_that_two_endpoints_cannot() {
    // A sample position steps a whole buffer at a time and the timestamps step a millisecond, so
    // comparing the first reading with the last measures the quantising, not the clocks (seen at the
    // devices, 2026-09-20: two locked units read exactly one buffer apart). Reading all through the
    // run and fitting a line through each driver's points averages that out.
    let pc = both();
    let outcome = run(&pc, &options()).expect("the run must work");
    let fitted = fitted_clocks(&outcome.series).expect("both drivers were sampled");
    assert!(fitted.parts_per_million.abs() < 1.0, "one clock: {} ppm", fitted.parts_per_million);
    assert_eq!(fitted.verdict, Verdict::InStep);
    assert!(fitted.readings >= 10, "the run is sampled all the way through, not just at its ends");
}

#[test]
fn a_fitted_pair_on_two_crystals_reads_their_real_difference() {
    let apart = Spec::studio().positions_at(44_100.0 * (1.0 - 44e-6));
    let pc = both_with(Spec::quadro(), apart);
    let outcome = run(&pc, &options()).expect("the run must work");
    let fitted = fitted_clocks(&outcome.series).expect("both drivers were sampled");
    assert!((fitted.parts_per_million - 44.0).abs() < 2.0, "{} ppm", fitted.parts_per_million);
    assert_eq!(fitted.verdict, Verdict::Drifting);
}

#[test]
fn the_gap_between_the_two_counts_grows_only_when_the_clocks_differ() {
    // No timestamps in this one: the two drivers are read together, so their sample counts can be
    // subtracted. Locked, the gap stays where it started; apart, it grows run after run, and that
    // growth is what a long run can see and a short one cannot.
    let locked = gap(&run(&both(), &Options { seconds: 600, ..options() }).expect("the run must work").series).expect("two series");
    assert_eq!(locked.last, 0, "one clock keeps the counts together");
    assert_eq!(locked.widest, 0);
    assert_eq!(locked.samples_per_second, 0.0);

    // 44 parts per million at 44.1 kHz is about 1.9 samples a second: over ten minutes, 1164.
    let apart = Spec::studio().positions_at(44_100.0 * (1.0 - 44e-6));
    let drifting = gap(&run(&both_with(Spec::quadro(), apart), &Options { seconds: 600, ..options() }).expect("the run must work").series).expect("two series");
    assert!(drifting.last > 1100 && drifting.last < 1200, "the gap grew to {}", drifting.last);
    assert!((drifting.samples_per_second - 1.94).abs() < 0.1, "{} samples a second", drifting.samples_per_second);
}
