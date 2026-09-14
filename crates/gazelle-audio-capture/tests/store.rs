use gazelle_audio_capture::session::marks::{Mark, MarkKind};
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, PlanError, ProbePlan};
use gazelle_audio_capture::session::store::{SessionError, SessionInfo, SessionStore};

fn info() -> SessionInfo {
    SessionInfo { vid: 0x1234, pid: 0xABCD, vendor_app: Some("Control Panel".into()), ..SessionInfo::default() }
}

fn level() -> Parameter {
    Parameter {
        id: "monitor_level".into(),
        label: "Monitor level".into(),
        kind: ParameterKind::Continuous,
        domain: ParameterDomain { values: vec![], unit: Some("dB".into()) },
        location: "Main window, right".into(),
    }
}

fn started(probe: &str) -> Mark {
    Mark {
        probe: probe.into(),
        step: None,
        attempt: 0,
        wall_ns: 1,
        last_packet: None,
        clock_suspect: false,
        kind: MarkKind::ProbeStarted {
            plan: ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into()], sweep: vec![], repeats: 3, control_parameter: "mute".into() },
            seed: 1,
            steps: vec![],
        },
    }
}

#[test]
fn create_lays_out_the_session_directory() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("s1");
    let store = SessionStore::create(&root, &info()).unwrap();
    for name in ["session.json", "parameters.json", "marks.jsonl"] {
        assert!(root.join(name).is_file(), "{name}");
    }
    assert!(root.join("captures").is_dir() && root.join("analysis").is_dir());
    assert_eq!(store.info().unwrap(), info());
    assert!(matches!(SessionStore::create(&root, &info()), Err(SessionError::Exists(_))));
    assert!(SessionStore::open(&root).is_ok());
    assert!(matches!(SessionStore::open(dir.path()), Err(SessionError::NotASession(_))));
}

#[test]
fn parameters_are_validated_and_unique() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::create(dir.path(), &info()).unwrap();
    store.declare_parameter(level()).unwrap();
    assert!(matches!(
        store.declare_parameter(level()),
        Err(SessionError::Plan(PlanError::DuplicateParameter(id))) if id == "monitor_level"
    ));
    let mut bad = level();
    bad.id = "Bad Id".into();
    assert!(matches!(store.declare_parameter(bad), Err(SessionError::Plan(PlanError::BadParameterId(_)))));
    assert_eq!(SessionStore::open(dir.path()).unwrap().parameters().unwrap(), vec![level()]);
}

#[test]
fn marks_append_and_probe_ids_follow_started_probes() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::create(dir.path(), &info()).unwrap();
    assert_eq!(store.next_probe_id().unwrap(), "p1");
    store.append_marks(&[started("p1")]).unwrap();
    store.append_marks(&[]).unwrap();
    assert_eq!(store.next_probe_id().unwrap(), "p2");
    let text = std::fs::read_to_string(dir.path().join("marks.jsonl")).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains(r#""type":"probe_started""#));
    assert_eq!(store.marks().unwrap(), vec![started("p1")]);
}

#[test]
fn a_torn_final_mark_is_ignored_and_cut_before_the_next_append() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::create(dir.path(), &info()).unwrap();
    store.append_marks(&[started("p1")]).unwrap();
    // A crash mid-write leaves half a JSON line with no newline.
    let path = dir.path().join("marks.jsonl");
    let mut torn = std::fs::read(&path).unwrap();
    torn.extend_from_slice(br#"{"probe":"p2","step":null,"att"#);
    std::fs::write(&path, &torn).unwrap();

    assert_eq!(store.marks().unwrap(), vec![started("p1")], "the torn line is skipped");
    assert_eq!(store.next_probe_id().unwrap(), "p2", "a torn line does not block the next probe");

    store.append_marks(&[started("p2")]).unwrap();
    assert_eq!(store.marks().unwrap(), vec![started("p1"), started("p2")]);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.ends_with('\n') && !text.contains(r#""att{"#), "the fragment was cut, not glued to the new line: {text}");
}

#[test]
fn a_bad_line_that_is_not_the_torn_tail_is_still_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::create(dir.path(), &info()).unwrap();
    let path = dir.path().join("marks.jsonl");
    std::fs::write(&path, "{not json}\n").unwrap();
    assert!(store.marks().is_err());
}

#[test]
fn one_capture_per_probe() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::create(dir.path(), &info()).unwrap();
    store.create_capture("p1").unwrap();
    assert!(store.capture_path("p1").ends_with("captures/p1.pcapng"));
    assert!(matches!(store.create_capture("p1"), Err(SessionError::CaptureExists(p)) if p == "p1"));
}

#[test]
fn update_info_records_the_descriptor() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::create(dir.path(), &info()).unwrap();
    store.update_info(|i| i.device_descriptor_hex = Some("1201".into())).unwrap();
    assert_eq!(store.info().unwrap().device_descriptor_hex.as_deref(), Some("1201"));
}
