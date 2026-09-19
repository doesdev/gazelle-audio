//! The operator boundary: only the panel (and the synthetic operator standing in for it)
//! may mark a step done, redo it, skip it or record an actual value.

/// Proof that a step command comes from the operator. It has no public constructor;
/// `tests/operator_boundary.rs` fails if `grant()` is called outside `src/panel/`,
/// `src/synth/`, `src/session/authority.rs` or in-crate unit test files (`*_tests.rs`).
#[derive(Debug)]
pub struct OperatorAuthority {
    _private: (),
}

impl OperatorAuthority {
    // Called by the synthetic operator and the panel; until then only
    // the state machine's unit tests use it.
    pub(crate) fn grant() -> Self {
        Self { _private: () }
    }
}
