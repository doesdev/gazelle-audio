//! Field map (spec §8, versioned JSON) and its Markdown report, built from one probe.

use serde::Serialize;

use super::attribute::{attribute_commands, attribute_readback, Attribution, Field};
use super::channel::{ChannelKey, Channels};
use super::encoding::{fit, Encoding, Model};
use super::noise::{noise_model, ByteClass};
use super::segment::segments;
use crate::capture::event::{Direction, TransferType, UsbEvent};
use crate::session::plan::StepKind;
use crate::session::step::RunStatus;
use crate::session::timeline::ProbeTimeline;

pub const SCHEMA_VERSION: u32 = 1;
/// Fewer distinct values than this leave the bit range and encoding underdetermined: two
/// points always fit a line.
pub const MIN_VALUES_FOR_ENCODING: usize = 3;

/// Everything one probe's analysis needs, already loaded.
pub struct ProbeInput<'a> {
    pub timeline: &'a ProbeTimeline,
    /// The target's events from the probe's capture, sorted by `ts_ns`.
    pub events: &'a [UsbEvent],
    /// Capture path as cited in evidence, relative to the session directory.
    pub capture: String,
    pub vid: u16,
    pub pid: u16,
    pub descriptor_hex: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldMap {
    pub schema_version: u32,
    pub parameter: String,
    pub device: DeviceRef,
    pub command: Option<FieldEntry>,
    pub readback: Option<FieldEntry>,
    pub shared_with: Vec<SharedField>,
    pub evidence: Vec<Evidence>,
    pub caveats: Vec<String>,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceRef {
    pub vid: u16,
    pub pid: u16,
    pub descriptor_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelRef {
    pub endpoint: String,
    pub direction: Direction,
    pub transfer: TransferType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<String>,
}

impl From<ChannelKey> for ChannelRef {
    fn from(c: ChannelKey) -> Self {
        Self {
            endpoint: format!("0x{:02x}", c.endpoint),
            direction: c.direction,
            transfer: c.transfer,
            request: c.request.map(|r| format!("0x{r:02x}")),
            index: c.index.map(|i| format!("0x{i:04x}")),
            discriminator: c.discriminator.map(|d| format!("0x{d:02x}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FieldPosition {
    pub byte: usize,
    pub bits: [u8; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldEntry {
    pub channel: ChannelRef,
    pub template: String,
    pub field: FieldPosition,
    pub encoding: Encoding,
    /// Raw byte per UI value observed on Set steps.
    pub values: Vec<(String, u8)>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SharedField {
    /// The parameter whose Control step also changed the field.
    pub parameter: String,
    pub channel: ChannelRef,
    pub field: FieldPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Evidence {
    pub probe: String,
    pub steps: Vec<usize>,
    pub capture: String,
    pub packet_indices: Vec<u64>,
}

/// FNV-1a 64 over the lowercase descriptor hex: stable, dependency-free, not cryptographic.
pub fn descriptor_hash(hex: &str) -> String {
    let hash = hex.to_ascii_lowercase().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
    format!("fnv1a64:{hash:016x}")
}

pub fn field_map(input: &ProbeInput<'_>, parameter: &str) -> FieldMap {
    let timeline = input.timeline;
    let segs = segments(timeline, input.events);
    let channels = Channels::detect(input.events);
    let noise = noise_model(&segs, &channels);
    let commands = attribute_commands(&segs, &channels, parameter);
    let readback = attribute_readback(&segs, &channels, &noise, parameter);
    let confidence = confidence(timeline, parameter);
    let control = timeline.plan.control_parameter.clone();

    let mut caveats = Vec::new();
    let mut recommendations = Vec::new();
    let command = primary(&commands, "command", confidence, &mut caveats);
    let readback_entry = primary(&readback, "readback", confidence, &mut caveats);
    if command.is_none() {
        caveats.push(format!("no command field was attributed to {parameter}"));
    }
    if !readback.masked.is_empty() {
        let listed: Vec<String> = readback.masked.iter().take(8).map(|m| format!("{} byte {} ({})", channel_text(&m.channel), m.byte, class_text(m.class))).collect();
        let more = readback.masked.len().saturating_sub(listed.len());
        let suffix = if more > 0 { format!(", and {more} more") } else { String::new() };
        caveats.push(format!("{} readback byte(s) vary while idle and were masked: {}{suffix}", readback.masked.len(), listed.join(", ")));
    }
    if timeline.outcome != Some(RunStatus::Completed) {
        caveats.push("the probe did not complete".into());
    }

    let shared_with: Vec<SharedField> =
        commands.shared.iter().chain(&readback.shared).map(|f| SharedField { parameter: control.clone(), channel: f.channel.into(), field: position(f) }).collect();
    if !shared_with.is_empty() {
        recommendations.push(format!("The Control step ({control}) also changed {} field(s) of {parameter}: declare and probe {control} to separate them", shared_with.len()));
    }
    if timeline.plan.repeats < 3 {
        recommendations.push(format!("Only {} repeat(s): probe again with repeats ≥ 3", timeline.plan.repeats));
    }
    let fewest_values = command.iter().chain(&readback_entry).map(|e| e.values.len()).min();
    if let Some(n) = fewest_values.filter(|&n| n < MIN_VALUES_FOR_ENCODING) {
        caveats.push(format!("bit range and encoding rest on only {n} distinct values"));
        recommendations.push(format!("Add a sweep of {parameter} over more values to pin down its bit range and encoding"));
    }
    if confidence < 1.0 && (command.is_some() || readback_entry.is_some()) {
        recommendations.push("Confidence is reduced by redos, differing actual values or clock-suspect steps: repeat ×5".into());
    }

    FieldMap {
        schema_version: SCHEMA_VERSION,
        parameter: parameter.to_string(),
        device: DeviceRef { vid: input.vid, pid: input.pid, descriptor_hash: input.descriptor_hex.map(descriptor_hash) },
        command,
        readback: readback_entry,
        shared_with,
        evidence: evidence(input, &commands, &readback),
        caveats,
        recommendations,
    }
}

fn position(f: &Field) -> FieldPosition {
    FieldPosition { byte: f.byte, bits: [f.bits.0, f.bits.1] }
}

/// The first attributed field; any others are listed as a caveat.
fn primary(attribution: &Attribution, what: &str, confidence: f64, caveats: &mut Vec<String>) -> Option<FieldEntry> {
    let first = attribution.fields.first()?;
    if attribution.fields.len() > 1 {
        let others: Vec<String> = attribution.fields[1..].iter().map(|f| format!("{} byte {}", channel_text(&f.channel), f.byte)).collect();
        caveats.push(format!("{} {what} bytes satisfy the rules; the first is reported, the others are: {}", attribution.fields.len(), others.join(", ")));
    }
    Some(FieldEntry { channel: first.channel.into(), template: first.template.clone(), field: position(first), encoding: fit(&first.values), values: first.values.clone(), confidence })
}

/// 1.0 reduced ×0.9 per clock-suspect Set step of the parameter, ×0.9 per such step whose
/// recorded actual value differs from the requested one, and ×0.95 per redo of such a step.
pub fn confidence(timeline: &ProbeTimeline, parameter: &str) -> f64 {
    let mut c: f64 = 1.0;
    for w in &timeline.windows {
        let Some(spec) = timeline.steps.get(w.step) else {
            continue;
        };
        if spec.kind != StepKind::Set || spec.parameter.as_deref() != Some(parameter) {
            continue;
        }
        if w.clock_suspect {
            c *= 0.9;
        }
        if w.actual_value.is_some() && w.actual_value != spec.to {
            c *= 0.9;
        }
        c *= 0.95f64.powi(timeline.redos.get(&w.step).copied().unwrap_or(0) as i32);
    }
    c.clamp(0.0, 1.0)
}

fn evidence(input: &ProbeInput<'_>, commands: &Attribution, readback: &Attribution) -> Vec<Evidence> {
    let mut steps = Vec::new();
    let mut packet_indices = Vec::new();
    for field in commands.fields.first().into_iter().chain(readback.fields.first()) {
        for &(step, packet) in &field.evidence {
            steps.push(step);
            packet_indices.push(packet);
        }
    }
    if steps.is_empty() {
        return Vec::new();
    }
    steps.sort_unstable();
    steps.dedup();
    packet_indices.sort_unstable();
    packet_indices.dedup();
    vec![Evidence { probe: input.timeline.probe.clone(), steps, capture: input.capture.clone(), packet_indices }]
}

fn direction_text(d: Direction) -> &'static str {
    match d {
        Direction::In => "in",
        Direction::Out => "out",
    }
}

fn transfer_text(t: TransferType) -> &'static str {
    match t {
        TransferType::Isochronous => "isochronous",
        TransferType::Interrupt => "interrupt",
        TransferType::Control => "control",
        TransferType::Bulk => "bulk",
    }
}

fn class_text(class: ByteClass) -> &'static str {
    match class {
        ByteClass::Constant => "constant",
        ByteClass::Counter { .. } => "counter",
        ByteClass::Derived { .. } => "checksum",
        ByteClass::Noisy => "noisy",
    }
}

/// `ep 0x01 out interrupt disc 0x70`, `ep 0x00 out control req 0x01 idx 0x0001`.
pub fn channel_text(c: &ChannelKey) -> String {
    channel_ref_text(&ChannelRef::from(*c))
}

fn channel_ref_text(c: &ChannelRef) -> String {
    let mut text = format!("ep {} {} {}", c.endpoint, direction_text(c.direction), transfer_text(c.transfer));
    for (label, value) in [("req", &c.request), ("idx", &c.index), ("disc", &c.discriminator)] {
        if let Some(v) = value {
            text.push_str(&format!(" {label} {v}"));
        }
    }
    text
}

fn encoding_text(e: &Encoding) -> String {
    let model = match &e.model {
        Model::Linear { scale, offset } => format!("linear: raw = {scale:.4} × value + {offset:.4}"),
        Model::Signed { scale, offset } => format!("signed 8-bit: raw = {scale:.4} × value + {offset:.4}"),
        Model::Db { scale, offset } => format!("dB: raw = {scale:.4} × 10^(value/20) + {offset:.4}"),
        Model::MonotonicTable { entries } => format!("monotonic table of {} values", entries.len()),
        Model::Table { entries } => format!("table of {} values", entries.len()),
    };
    match e.model {
        Model::Linear { .. } | Model::Signed { .. } | Model::Db { .. } => format!("{model} (max residual {:.3})", e.residual_max),
        _ => model,
    }
}

/// Shortest run of identical trailing bytes worth collapsing in the report.
const COLLAPSE_RUN: usize = 8;

/// A template for people: a trailing run of at least [`COLLAPSE_RUN`] identical bytes becomes
/// `… N × xx`. `70 ?? 01 ?? 00 00 00 00 00 00 00 00` → `70 ?? 01 ?? … 8 × 00`.
pub fn short_template(template: &str) -> String {
    let bytes: Vec<&str> = template.split(' ').collect();
    let Some(&last) = bytes.last() else {
        return template.to_string();
    };
    let run = bytes.iter().rev().take_while(|b| **b == last).count();
    if run < COLLAPSE_RUN || last == "??" {
        return template.to_string();
    }
    let head = &bytes[..bytes.len() - run];
    let tail = format!("… {run} × {last}");
    if head.is_empty() {
        tail
    } else {
        format!("{} {tail}", head.join(" "))
    }
}

fn entry_lines(title: &str, entry: &Option<FieldEntry>, lines: &mut Vec<String>) {
    lines.push(format!("## {title}"));
    lines.push(String::new());
    let Some(e) = entry else {
        lines.push("None attributed.".into());
        lines.push(String::new());
        return;
    };
    lines.push(format!("- Channel: {}", channel_ref_text(&e.channel)));
    lines.push(format!("- Template: `{}`", short_template(&e.template)));
    lines.push(format!("- Field: byte {}, bits {}–{}", e.field.byte, e.field.bits[0], e.field.bits[1]));
    lines.push(format!("- Encoding: {}", encoding_text(&e.encoding)));
    lines.push(format!("- Confidence: {:.2}", e.confidence));
    lines.push(String::new());
    lines.push("| UI value | Raw |".into());
    lines.push("|---|---|".into());
    for (value, raw) in &e.values {
        lines.push(format!("| {value} | 0x{raw:02x} ({raw}) |"));
    }
    lines.push(String::new());
}

/// Markdown rendering of a field map for people.
pub fn report(map: &FieldMap) -> String {
    let mut lines = vec![format!("# Field map: {}", map.parameter), String::new()];
    let hash = map.device.descriptor_hash.as_deref().unwrap_or("no descriptor recorded");
    lines.push(format!("Device {:04x}:{:04x} ({hash}), schema version {}.", map.device.vid, map.device.pid, map.schema_version));
    lines.push(String::new());
    entry_lines("Command", &map.command, &mut lines);
    entry_lines("Readback", &map.readback, &mut lines);
    if !map.shared_with.is_empty() {
        lines.push("## Shared fields".into());
        lines.push(String::new());
        for s in &map.shared_with {
            lines.push(format!("- {} byte {}, also changed by {}", channel_ref_text(&s.channel), s.field.byte, s.parameter));
        }
        lines.push(String::new());
    }
    for (title, items) in [("Caveats", &map.caveats), ("Recommendations", &map.recommendations)] {
        if !items.is_empty() {
            lines.push(format!("## {title}"));
            lines.push(String::new());
            lines.extend(items.iter().map(|i| format!("- {i}")));
            lines.push(String::new());
        }
    }
    lines.push("## Evidence".into());
    lines.push(String::new());
    if map.evidence.is_empty() {
        lines.push("None.".into());
    }
    for e in &map.evidence {
        lines.push(format!("- {} ({}): steps {:?}, {} packets", e.probe, e.capture, e.steps, e.packet_indices.len()));
    }
    lines.push(String::new());
    lines.join("\n")
}
