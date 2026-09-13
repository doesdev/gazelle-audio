use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, PlanError, ProbePlan};
use gazelle_audio_capture::session::plan::{expand, Block, StepKind};

fn param(id: &str, kind: ParameterKind, values: &[&str]) -> Parameter {
    Parameter {
        id: id.into(),
        label: id.replace('_', " "),
        kind,
        domain: ParameterDomain { values: values.iter().map(|v| v.to_string()).collect(), unit: None },
        location: String::new(),
    }
}

fn params() -> Vec<Parameter> {
    vec![
        param("monitor_level", ParameterKind::Continuous, &[]),
        param("mute", ParameterKind::Toggle, &["off", "on"]),
        param("source", ParameterKind::Discrete, &["mic", "line", "inst"]),
    ]
}

fn plan(value_b: &[&str], sweep: &[&str], repeats: u32) -> ProbePlan {
    ProbePlan {
        parameter: "monitor_level".into(),
        value_a: "0 dB".into(),
        value_b: value_b.iter().map(|s| s.to_string()).collect(),
        sweep: sweep.iter().map(|s| s.to_string()).collect(),
        repeats,
        control_parameter: "mute".into(),
    }
}

#[test]
fn plan_json_defaults_repeats_to_three() {
    let p: ProbePlan = serde_json::from_str(
        r#"{"parameter":"monitor_level","value_a":"0 dB","value_b":["-6 dB"],"control_parameter":"mute"}"#,
    )
    .unwrap();
    assert_eq!((p.repeats, p.sweep.len()), (3, 0));
}

#[test]
fn one_value_b_gives_the_seven_step_repeat() {
    type Shape<'a> = (StepKind, Option<&'a str>, Option<&'a str>, Option<&'a str>);
    let steps = expand(&plan(&["-6 dB"], &[], 1), 1);
    let shape: Vec<Shape> = steps
        .iter()
        .map(|s| (s.kind, s.parameter.as_deref(), s.from.as_deref(), s.to.as_deref()))
        .collect();
    let lvl = Some("monitor_level");
    assert_eq!(
        shape,
        vec![
            (StepKind::Idle, None, None, None),
            (StepKind::Set, lvl, None, Some("0 dB")),
            (StepKind::NoOp, lvl, Some("0 dB"), Some("0 dB")),
            (StepKind::Set, lvl, Some("0 dB"), Some("-6 dB")),
            (StepKind::Set, lvl, Some("-6 dB"), Some("0 dB")),
            (StepKind::Control, Some("mute"), None, None),
            (StepKind::Idle, None, None, None),
        ]
    );
    assert_eq!(steps[3].progress(), "repeat 1 of 1 · step 4 of 7");
}

#[test]
fn repeats_and_sweep_are_numbered_and_chained() {
    let steps = expand(&plan(&["-6 dB"], &["-12 dB", "-18 dB"], 3), 1);
    assert_eq!(steps.len(), 3 * 7 + 4);
    assert!(steps.iter().enumerate().all(|(i, s)| s.index == i));
    assert_eq!(steps[7].block, Block::Repeat { number: 2, of: 3 });
    assert_eq!(steps[8].from.as_deref(), Some("0 dB"), "later repeats start from A");
    assert_eq!(steps[21].progress(), "sweep · step 1 of 4");
    assert_eq!((steps[21].from.as_deref(), steps[21].to.as_deref()), (Some("0 dB"), Some("-12 dB")));
    assert_eq!(steps[22].kind, StepKind::Idle);
    assert_eq!(steps[23].from.as_deref(), Some("-12 dB"));
}

#[test]
fn value_b_order_is_shuffled_per_repeat_and_reproducible() {
    let values = ["-3 dB", "-6 dB", "-9 dB", "-12 dB", "-15 dB"];
    let p = plan(&values, &[], 4);
    let order = |seed| -> Vec<Vec<String>> {
        expand(&p, seed)
            .chunks(5 + 2 * values.len())
            .map(|block| block.iter().skip(3).step_by(2).take(values.len()).map(|s| s.to.clone().unwrap()).collect())
            .collect()
    };
    let a = order(42);
    assert_eq!(a, order(42), "same seed, same order");
    for block in &a {
        let mut sorted = block.clone();
        sorted.sort();
        let mut want: Vec<String> = values.iter().map(|v| v.to_string()).collect();
        want.sort();
        assert_eq!(sorted, want, "each repeat uses every value once");
    }
    assert!(a.windows(2).any(|w| w[0] != w[1]), "orders differ between repeats");
}

#[test]
fn validation_rejects_bad_plans() {
    let ps = params();
    assert_eq!(plan(&["-6 dB"], &[], 3).validate(&ps), Ok(()));
    let mut p = plan(&["-6 dB"], &[], 3);
    p.parameter = "gain".into();
    assert_eq!(p.validate(&ps), Err(PlanError::UnknownParameter("gain".into())));
    let mut p = plan(&["-6 dB"], &[], 3);
    p.control_parameter = "monitor_level".into();
    assert_eq!(p.validate(&ps), Err(PlanError::ControlIsProbed));
    assert_eq!(plan(&[], &[], 3).validate(&ps), Err(PlanError::BadValueB));
    assert_eq!(plan(&["0 dB"], &[], 3).validate(&ps), Err(PlanError::BadValueB));
    assert_eq!(plan(&["-6 dB", "-6 dB"], &[], 3).validate(&ps), Err(PlanError::BadValueB));
    assert_eq!(plan(&["-6 dB"], &[], 0).validate(&ps), Err(PlanError::BadRepeats));
    assert_eq!(plan(&["-6 dB"], &[], 21).validate(&ps), Err(PlanError::BadRepeats));
    let toggle = ProbePlan { parameter: "mute".into(), value_a: "off".into(), value_b: vec!["on".into(), "off2".into()], sweep: vec![], repeats: 3, control_parameter: "source".into() };
    assert_eq!(toggle.validate(&ps), Err(PlanError::ToggleValueB));
    let discrete = ProbePlan { parameter: "source".into(), value_a: "mic".into(), value_b: vec!["hi-z".into()], sweep: vec![], repeats: 3, control_parameter: "mute".into() };
    assert_eq!(
        discrete.validate(&ps),
        Err(PlanError::ValueNotInDomain { parameter: "source".into(), value: "hi-z".into() })
    );
}

#[test]
fn parameter_validation() {
    assert_eq!(param("monitor_level", ParameterKind::Continuous, &[]).validate(), Ok(()));
    assert_eq!(
        param("Monitor Level", ParameterKind::Continuous, &[]).validate(),
        Err(PlanError::BadParameterId("Monitor Level".into()))
    );
    assert_eq!(
        param("mute", ParameterKind::Toggle, &["a", "b", "c"]).validate(),
        Err(PlanError::ToggleDomain("mute".into()))
    );
    let mut p = param("x", ParameterKind::Discrete, &[]);
    p.label = " ".into();
    assert_eq!(p.validate(), Err(PlanError::MissingLabel("x".into())));
}
