//! The operation set: the names it promises, well-formed schemas, and no way for an
//! agent to act for the operator.

use gazelle_audio_capture::ops::types::operations;

const SPEC_NAMES: &[&str] = &[
    "session_open",
    "session_status",
    "import_capture",
    "declare_parameter",
    "list_parameters",
    "plan_probe",
    "start_probe",
    "abandon_probe",
    "await_progress",
    "analyze_probe",
    "get_field_map",
    "get_evidence",
];

#[test]
fn the_operation_set_is_exactly_the_specs() {
    let names: Vec<&str> = operations().iter().map(|o| o.name).collect();
    assert_eq!(names, SPEC_NAMES);
}

#[test]
fn names_are_snake_case_and_descriptions_present() {
    for op in operations() {
        assert!(op.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{}", op.name);
        assert!(!op.description.is_empty() && op.description.ends_with('.'), "{}: {:?}", op.name, op.description);
    }
}

#[test]
fn every_input_schema_is_an_object_schema() {
    for op in operations() {
        assert_eq!(op.input_schema["type"], "object", "{}: {}", op.name, op.input_schema);
    }
    let declare = operations().into_iter().find(|o| o.name == "declare_parameter").unwrap();
    let text = declare.input_schema.to_string();
    for needle in ["\"parameter\"", "\"label\"", "\"kind\"", "continuous"] {
        assert!(text.contains(needle), "declare_parameter schema lacks {needle}: {text}");
    }
}

#[test]
fn no_operation_acts_for_the_operator() {
    for op in operations() {
        for forbidden in ["done", "redo", "skip", "operator", "actual_value"] {
            assert!(!op.name.contains(forbidden), "{} must not let an agent act for the operator", op.name);
        }
    }
}
