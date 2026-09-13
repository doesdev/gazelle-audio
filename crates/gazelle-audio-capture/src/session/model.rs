//! Parameters and probe plans as declared by the agent, in the vendor UI's terms.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterKind {
    Continuous,
    Discrete,
    Toggle,
}

/// Values and unit exactly as the vendor UI shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterDomain {
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
    pub id: String,
    pub label: String,
    pub kind: ParameterKind,
    #[serde(default)]
    pub domain: ParameterDomain,
    /// Free-text hint where the control is in the vendor UI.
    #[serde(default)]
    pub location: String,
}

fn default_repeats() -> u32 {
    3
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbePlan {
    pub parameter: String,
    pub value_a: String,
    pub value_b: Vec<String>,
    #[serde(default)]
    pub sweep: Vec<String>,
    #[serde(default = "default_repeats")]
    pub repeats: u32,
    pub control_parameter: String,
}

pub const MAX_REPEATS: u32 = 20;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("parameter id {0:?} must be 1-64 characters of a-z, 0-9 or _")]
    BadParameterId(String),
    #[error("parameter {0:?} needs a label")]
    MissingLabel(String),
    #[error("toggle parameter {0:?} must list exactly two values or none")]
    ToggleDomain(String),
    #[error("parameter {0:?} is already declared")]
    DuplicateParameter(String),
    #[error("unknown parameter {0:?}")]
    UnknownParameter(String),
    #[error("control parameter must differ from the probed parameter")]
    ControlIsProbed,
    #[error("value_b must list at least one value, without duplicates or value_a")]
    BadValueB,
    #[error("a toggle probe takes exactly one value_b")]
    ToggleValueB,
    #[error("repeats must be between 1 and {MAX_REPEATS}")]
    BadRepeats,
    #[error("value {value:?} is not in the domain of {parameter:?}")]
    ValueNotInDomain { parameter: String, value: String },
}

impl Parameter {
    pub fn validate(&self) -> Result<(), PlanError> {
        let id_ok = !self.id.is_empty()
            && self.id.len() <= 64
            && self.id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
        if !id_ok {
            return Err(PlanError::BadParameterId(self.id.clone()));
        }
        if self.label.trim().is_empty() {
            return Err(PlanError::MissingLabel(self.id.clone()));
        }
        if self.kind == ParameterKind::Toggle && !matches!(self.domain.values.len(), 0 | 2) {
            return Err(PlanError::ToggleDomain(self.id.clone()));
        }
        Ok(())
    }
}

impl ProbePlan {
    /// Checks the plan against the declared parameters.
    pub fn validate(&self, parameters: &[Parameter]) -> Result<(), PlanError> {
        let find = |id: &str| {
            parameters
                .iter()
                .find(|p| p.id == id)
                .ok_or_else(|| PlanError::UnknownParameter(id.to_string()))
        };
        let probed = find(&self.parameter)?;
        find(&self.control_parameter)?;
        if self.control_parameter == self.parameter {
            return Err(PlanError::ControlIsProbed);
        }
        let mut seen = std::collections::HashSet::new();
        if self.value_b.is_empty()
            || self.value_b.iter().any(|v| *v == self.value_a || !seen.insert(v.as_str()))
        {
            return Err(PlanError::BadValueB);
        }
        if probed.kind == ParameterKind::Toggle && self.value_b.len() != 1 {
            return Err(PlanError::ToggleValueB);
        }
        if self.repeats == 0 || self.repeats > MAX_REPEATS {
            return Err(PlanError::BadRepeats);
        }
        if probed.kind != ParameterKind::Continuous && !probed.domain.values.is_empty() {
            let all = std::iter::once(&self.value_a).chain(&self.value_b).chain(&self.sweep);
            for v in all {
                if !probed.domain.values.contains(v) {
                    return Err(PlanError::ValueNotInDomain {
                        parameter: probed.id.clone(),
                        value: v.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}
