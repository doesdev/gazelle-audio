//! Encoding fits (spec §8 step 6): linear, signed, dB and table models.

use gazelle_audio_capture::analysis::encoding::{fit, ui_number, Model};

fn pairs(items: &[(&str, u8)]) -> Vec<(String, u8)> {
    items.iter().map(|(v, r)| (v.to_string(), *r)).collect()
}

#[test]
fn ui_numbers_are_read_from_labels() {
    assert_eq!(ui_number("-6 dB"), Some(-6.0));
    assert_eq!(ui_number("+3.5 dB"), Some(3.5));
    assert_eq!(ui_number("10"), Some(10.0));
    assert_eq!(ui_number("Line 2"), Some(2.0));
    assert_eq!(ui_number("off"), None);
}

#[test]
fn positions_fit_a_linear_model() {
    let e = fit(&pairs(&[("0 dB", 0), ("-6 dB", 1), ("-12 dB", 2)]));
    match e.model {
        Model::Linear { scale, offset } => {
            assert!((scale + 1.0 / 6.0).abs() < 1e-9 && offset.abs() < 1e-9, "{scale} {offset}");
        }
        other => panic!("{other:?}"),
    }
    assert!(e.residual_max < 1e-9);
}

#[test]
fn live_preamp_gain_fits_signed_twos_complement() {
    // Channel 8 gain from the 2026-09-14 capture: -6 dB was sent as 250.
    let e = fit(&pairs(&[("10 dB", 10), ("-6 dB", 250), ("-5 dB", 251), ("0 dB", 0), ("-1 dB", 255)]));
    assert_eq!(e.model, Model::Signed { scale: 1.0, offset: 0.0 });
}

#[test]
fn amplitude_steps_fit_a_db_model() {
    // raw = round(100 · 10^(dB/20)): 100, 50, 25, 10.
    let e = fit(&pairs(&[("0 dB", 100), ("-6 dB", 50), ("-12 dB", 25), ("-20 dB", 10)]));
    match e.model {
        Model::Db { scale, offset } => assert!((scale - 100.0).abs() < 1.5 && offset.abs() < 1.5, "{scale} {offset}"),
        other => panic!("{other:?}"),
    }
    assert!(e.residual_max <= 0.5);
}

#[test]
fn a_curve_no_formula_fits_becomes_a_monotonic_table() {
    let e = fit(&pairs(&[("0", 0), ("1", 1), ("2", 4), ("3", 9), ("4", 16)]));
    assert!(matches!(e.model, Model::MonotonicTable { ref entries } if entries.len() == 5), "{:?}", e.model);
}

#[test]
fn words_and_unordered_values_become_a_table() {
    assert!(matches!(fit(&pairs(&[("off", 0), ("on", 1)])).model, Model::Table { .. }));
    assert!(matches!(fit(&pairs(&[("1", 5), ("2", 1), ("3", 9)])).model, Model::Table { .. }));
}
