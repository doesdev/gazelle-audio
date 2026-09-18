//! Generates a complete session directory (parameters, marks, one capture) by running the
//! real step state machine against simulated devices and a scripted operator on virtual time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::capture::decode::{usbmon, usbpcap, ByteOrder};
use crate::capture::event::UsbEvent;
use crate::capture::pipeline::{DeviceFilter, PayloadPolicy, Pipeline};
use crate::capture::writer::CaptureWriter;
use crate::capture::{CaptureError, RawFrame};
use crate::session::authority::OperatorAuthority;
use crate::session::marks::{Mark, PacketClock};
use crate::session::model::{Parameter, ProbePlan};
use crate::session::plan::StepKind;
use crate::session::step::{OperatorCommand, ProbeRun, RunStatus, StepState, StepTiming};
use crate::session::store::{hex, SessionError, SessionInfo, SessionStore};

use super::device::DeviceModel;

/// How the simulated operator behaves.
#[derive(Debug, Clone)]
pub struct ScriptedOperator {
    /// Delay after the pre-roll before touching the vendor UI.
    pub react_ns: u64,
    /// Delay after acting before pressing Done.
    pub press_after_ns: u64,
    /// Steps redone once, just before their first Done.
    pub redo_once: Vec<usize>,
    /// Steps skipped when ready, with the reason.
    pub skip: Vec<(usize, String)>,
    /// Actual values recorded right after acting.
    pub actual_values: Vec<(usize, String)>,
}

impl Default for ScriptedOperator {
    fn default() -> Self {
        Self { react_ns: 300_000_000, press_after_ns: 400_000_000, redo_once: vec![], skip: vec![], actual_values: vec![] }
    }
}

pub struct SynthSpec {
    pub parameters: Vec<Parameter>,
    pub plan: ProbePlan,
    pub seed: u64,
    pub timing: StepTiming,
    pub start_ns: u64,
    pub operator: ScriptedOperator,
    /// 249 (USBPcap) or 220 (usbmon) frames in the stored capture.
    pub link_type: u32,
    pub policy: PayloadPolicy,
}

pub struct SynthResult {
    pub store: SessionStore,
    pub probe_id: String,
    pub capture: PathBuf,
    /// Events of the target device as stored, `packet_index` = position in the capture.
    pub events: Vec<UsbEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum SynthError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error("the first device is the target and is required")]
    NoDevices,
    #[error("unsupported link type {0}")]
    LinkType(u32),
}

/// Virtual-time resolution of the simulation.
pub const SIM_STEP_NS: u64 = 10_000_000;
/// Traffic kept after the probe completes.
pub const TAIL_NS: u64 = 500_000_000;

/// `devices[0]` is the target; the others share its bus and must be filtered out.
pub fn generate_session(root: &Path, spec: SynthSpec, mut devices: Vec<Box<dyn DeviceModel>>) -> Result<SynthResult, SynthError> {
    if devices.is_empty() {
        return Err(SynthError::NoDevices);
    }
    if spec.link_type != usbpcap::LINKTYPE_USBPCAP && spec.link_type != usbmon::LINKTYPE_USB_LINUX_MMAPPED {
        return Err(SynthError::LinkType(spec.link_type));
    }
    let (vid, pid) = devices[0].vid_pid();
    let store = SessionStore::create(
        root,
        &SessionInfo { vid, pid, keep_stream_payloads: spec.policy.keep_stream_payloads, ..SessionInfo::default() },
    )?;
    for p in &spec.parameters {
        store.declare_parameter(p.clone())?;
    }
    spec.plan.validate(&spec.parameters).map_err(SessionError::from)?;
    let probe_id = store.next_probe_id()?;
    let mut sim = Sim {
        writer: CaptureWriter::new(std::io::BufWriter::new(store.create_capture(&probe_id)?))?,
        pipeline: Pipeline::new(DeviceFilter::Target { vid, pid }, spec.policy),
        link_type: spec.link_type,
        events: Vec::new(),
        clock: None,
        index: 0,
    };

    let mut t = spec.start_ns;
    let mut emitted = Vec::new();
    for d in devices.iter_mut() {
        d.enumerate(t, &mut emitted);
    }
    sim.store_events(&mut emitted)?;
    let mut next_tick: Vec<u64> = devices.iter().map(|d| t + d.tick_ns()).collect();
    let mut current_values: HashMap<String, String> = spec
        .parameters
        .iter()
        .map(|p| (p.id.clone(), p.domain.values.first().cloned().unwrap_or_default()))
        .collect();

    let authority = OperatorAuthority::grant();
    let (mut run, marks) = ProbeRun::start(&probe_id, &spec.plan, spec.seed, spec.timing, t, sim.clock);
    store.append_marks(&marks)?;
    // (step, attempt) the operator has already acted on.
    let mut acted: Option<(usize, u32)> = None;
    let mut redone: Vec<usize> = Vec::new();
    let mut end_ns = None;

    loop {
        t += SIM_STEP_NS;
        for (d, next) in devices.iter_mut().zip(next_tick.iter_mut()) {
            while *next <= t {
                d.tick(*next, &mut emitted);
                *next += d.tick_ns();
            }
        }
        let mut marks: Vec<Mark> = Vec::new();
        if let Some(step) = run.current().cloned() {
            let i = step.spec.index;
            if let StepState::Armed { armed_ns } = step.state {
                let ready = armed_ns + spec.timing.pre_roll_ns;
                let act_at = ready + spec.operator.react_ns;
                let press_at = act_at + spec.operator.press_after_ns;
                if let Some((_, reason)) = spec.operator.skip.iter().find(|(s, _)| *s == i) {
                    if t >= ready {
                        marks.extend(op(&mut run, &authority, OperatorCommand::Skip { reason: reason.clone() }, t, sim.clock));
                    }
                } else if step.spec.kind != StepKind::Idle {
                    if t >= act_at && acted != Some((i, step.attempt)) {
                        acted = Some((i, step.attempt));
                        let parameter = step.spec.parameter.clone().unwrap_or_default();
                        match step.spec.kind {
                            StepKind::Set => {
                                let to = step.spec.to.clone().unwrap_or_default();
                                devices[0].change(t, &parameter, &to, &mut emitted);
                                current_values.insert(parameter, to);
                            }
                            StepKind::NoOp => devices[0].touch(t, &parameter, &mut emitted),
                            StepKind::Control => {
                                let other = other_value(&spec.parameters, &parameter, current_values.get(&parameter));
                                devices[0].change(t, &parameter, &other, &mut emitted);
                                current_values.insert(parameter, other);
                            }
                            StepKind::Idle => {}
                        }
                        if let Some((_, value)) = spec.operator.actual_values.iter().find(|(s, _)| *s == i) {
                            marks.extend(op(&mut run, &authority, OperatorCommand::ActualValue { value: value.clone() }, t, sim.clock));
                        }
                    }
                    if t >= press_at {
                        let command = if spec.operator.redo_once.contains(&i) && !redone.contains(&i) {
                            redone.push(i);
                            OperatorCommand::Redo
                        } else {
                            OperatorCommand::Done
                        };
                        marks.extend(op(&mut run, &authority, command, t, sim.clock));
                    }
                }
            }
        }
        sim.store_events(&mut emitted)?;
        marks.extend(run.tick(t, sim.clock));
        store.append_marks(&marks)?;
        if run.status() != RunStatus::Running && end_ns.is_none() {
            end_ns = Some(t + TAIL_NS);
        }
        if end_ns.is_some_and(|end| t >= end) {
            break;
        }
    }

    let (bus, dev) = devices[0].address();
    if let Some(d) = sim.pipeline.device_map().get(bus, dev) {
        let descriptor = hex(&d.descriptor);
        store.update_info(|i| i.device_descriptor_hex = Some(descriptor))?;
    }
    sim.writer.finish()?;
    let capture = store.capture_path(&probe_id);
    Ok(SynthResult { store, probe_id, capture, events: sim.events })
}

fn op(run: &mut ProbeRun, authority: &OperatorAuthority, command: OperatorCommand, t: u64, clock: Option<PacketClock>) -> Vec<Mark> {
    run.operator(authority, command, t, clock).expect("scripted operator issues only valid commands")
}

/// "Any other value": the next domain value after the current one, or the current value
/// with a prime when the domain is open.
fn other_value(parameters: &[Parameter], id: &str, current: Option<&String>) -> String {
    let current = current.cloned().unwrap_or_default();
    let values = parameters.iter().find(|p| p.id == id).map(|p| p.domain.values.clone()).unwrap_or_default();
    match values.iter().position(|v| *v == current) {
        Some(i) if values.len() > 1 => values[(i + 1) % values.len()].clone(),
        _ => format!("{current}'"),
    }
}

struct Sim<W: std::io::Write> {
    writer: CaptureWriter<W>,
    pipeline: Pipeline,
    link_type: u32,
    events: Vec<UsbEvent>,
    clock: Option<PacketClock>,
    index: u64,
}

impl<W: std::io::Write> Sim<W> {
    /// Encodes, filters and stores emitted events exactly as the live path would.
    fn store_events(&mut self, emitted: &mut Vec<UsbEvent>) -> Result<(), SynthError> {
        emitted.sort_by_key(|e| e.ts_ns);
        for ev in emitted.drain(..) {
            let data = if self.link_type == usbpcap::LINKTYPE_USBPCAP { usbpcap::encode(&ev) } else { usbmon::encode(&ev) };
            let frame = RawFrame {
                ts_ns: ev.ts_ns,
                link_type: self.link_type,
                index: self.index,
                orig_len: data.len() as u32,
                byte_order: ByteOrder::Little,
                data,
            };
            self.index += 1;
            self.clock = Some(PacketClock { ts_ns: ev.ts_ns, host_ns: ev.ts_ns });
            for (frame, mut stored) in self.pipeline.process(frame).map_err(CaptureError::from)? {
                stored.packet_index = self.writer.write(&frame)?;
                self.events.push(stored);
            }
        }
        Ok(())
    }
}
