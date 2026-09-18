//! Expansion of a probe plan into the fixed step sequence the agent cannot alter.

use serde::{Deserialize, Serialize};

use super::model::ProbePlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Operator does nothing for the idle window.
    Idle,
    /// Operator sets `parameter` from `from` to `to`.
    Set,
    /// Operator touches `parameter` and leaves it at `to`.
    NoOp,
    /// Operator changes `parameter` (the control parameter) to any other value.
    Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Block {
    Repeat { number: u32, of: u32 },
    Sweep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepSpec {
    /// Zero-based position in the whole probe.
    pub index: usize,
    pub block: Block,
    /// One-based position within the block.
    pub position: usize,
    pub block_len: usize,
    pub kind: StepKind,
    /// Parameter id the operator acts on; `None` for idle steps.
    pub parameter: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

impl StepSpec {
    /// Progress text for the panel, e.g. `repeat 2 of 3 · step 4 of 7`.
    pub fn progress(&self) -> String {
        match self.block {
            Block::Repeat { number, of } => {
                format!("repeat {number} of {of} · step {} of {}", self.position, self.block_len)
            }
            Block::Sweep => format!("sweep · step {} of {}", self.position, self.block_len),
        }
    }
}

/// SplitMix64: a tiny deterministic generator so a seed reproduces the shuffle exactly.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Fisher-Yates shuffle.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

struct Builder {
    steps: Vec<StepSpec>,
}

impl Builder {
    fn push(&mut self, block: Block, kind: StepKind, parameter: Option<&str>, from: Option<&str>, to: Option<&str>) {
        self.steps.push(StepSpec {
            index: self.steps.len(),
            block,
            position: 0,
            block_len: 0,
            kind,
            parameter: parameter.map(str::to_string),
            from: from.map(str::to_string),
            to: to.map(str::to_string),
        });
    }

    fn close_block(&mut self, start: usize) {
        let len = self.steps.len() - start;
        for (i, s) in self.steps[start..].iter_mut().enumerate() {
            s.position = i + 1;
            s.block_len = len;
        }
    }
}

/// Expands a validated plan. Per repeat, with `value_b` shuffled by `seed`:
/// `Idle, Set A, No-op A, (Set b, Set A) for each b, Control, Idle`; then per sweep value
/// `Set v, Idle`.
pub fn expand(plan: &ProbePlan, seed: u64) -> Vec<StepSpec> {
    let mut rng = SplitMix64::new(seed);
    let mut b = Builder { steps: Vec::new() };
    let p = Some(plan.parameter.as_str());
    let a = Some(plan.value_a.as_str());
    let mut current: Option<String> = None;
    for number in 1..=plan.repeats {
        let block = Block::Repeat { number, of: plan.repeats };
        let start = b.steps.len();
        let mut order = plan.value_b.clone();
        rng.shuffle(&mut order);
        b.push(block, StepKind::Idle, None, None, None);
        b.push(block, StepKind::Set, p, current.as_deref(), a);
        b.push(block, StepKind::NoOp, p, a, a);
        for value in &order {
            b.push(block, StepKind::Set, p, a, Some(value));
            b.push(block, StepKind::Set, p, Some(value), a);
        }
        b.push(block, StepKind::Control, Some(&plan.control_parameter), None, None);
        b.push(block, StepKind::Idle, None, None, None);
        b.close_block(start);
        current = Some(plan.value_a.clone());
    }
    let start = b.steps.len();
    for value in &plan.sweep {
        b.push(Block::Sweep, StepKind::Set, p, current.as_deref(), Some(value));
        b.push(Block::Sweep, StepKind::Idle, None, None, None);
        current = Some(value.clone());
    }
    b.close_block(start);
    b.steps
}
