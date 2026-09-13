//! The synchronous device worker.
//!
//! One OS thread per device owns the [`Device`], its [`ResponseCorrelator`] and its
//! [`Registry`]. This is the sync side of the sync/async boundary: nothing here knows about
//! tokio, so `gazelle-audio-protocol` and `gazelle-audio-transport` stay runtime-free.
//!
//! Owning the correlator on a single thread makes the device's **one-outstanding-request**
//! rule structural rather than a lock, matching the recovered manager server.

use gazelle_audio_protocol::payload::{PayloadValues, Value};
use gazelle_audio_protocol::registry::Registry;
use gazelle_audio_protocol::wire::Header;
use gazelle_audio_transport::correlation::{Correlation, ResponseCorrelator};
use gazelle_audio_transport::{segment_report, Device, RawPacket, Report};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::device::descriptor::DeviceId;
use crate::error::ServerError;

/// The device's request timeout, from the recovered `HWDevice.request`.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the worker waits for inbound traffic before re-checking its command queue.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A completed command.
#[derive(Debug, Clone)]
pub struct CommandOutcome {
    /// The exact bytes sent (or that would have been sent, when dry-run).
    pub sent: Vec<u8>,
    /// Decoded response fields, when a correlated response arrived and the command
    /// declares `returns`.
    pub response: Option<HashMap<String, Value>>,
    /// Why the response could not be decoded, when it could not be.
    ///
    /// Distinguishes "this command returns nothing" from "a response arrived and we
    /// failed to read it" — collapsing both into `None` hides real decode failures.
    pub response_error: Option<String>,
    pub dry_run: bool,
}

/// An event published by a device worker.
#[derive(Debug, Clone)]
pub enum DeviceEvent {
    /// A cyclic (device-initiated) report.
    Cyclic {
        device_id: DeviceId,
        report_id: u32,
        fields: HashMap<String, Value>,
    },
    /// A report that matched no outstanding request and no known cyclic layout.
    Undecoded { device_id: DeviceId, report_id: u32, len: usize },
}

/// Work sent to a device worker.
pub enum WorkerCommand {
    Request {
        name: String,
        values: PayloadValues,
        dry_run: bool,
        respond: Box<dyn FnOnce(Result<CommandOutcome, ServerError>) + Send>,
    },
    Shutdown,
}

/// Everything the worker loop needs.
pub struct WorkerContext {
    pub device_id: DeviceId,
    pub device: Box<dyn Device + Send>,
    pub registry: Option<Arc<Registry>>,
    pub events: Box<dyn Fn(DeviceEvent) + Send>,
}

/// Run the worker loop until shutdown or the command channel closes.
pub fn run(mut ctx: WorkerContext, rx: Receiver<WorkerCommand>) {
    let mut correlator = ResponseCorrelator::new();
    loop {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(WorkerCommand::Shutdown) => return,
            Ok(WorkerCommand::Request { name, values, dry_run, respond }) => {
                let result = handle_request(&mut ctx, &mut correlator, &name, &values, dry_run);
                respond(result);
            }
            Err(RecvTimeoutError::Timeout) => {
                // Idle: still drain anything the device pushed at us.
                drain_events(&mut ctx, &mut correlator);
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn handle_request(
    ctx: &mut WorkerContext,
    correlator: &mut ResponseCorrelator,
    name: &str,
    values: &PayloadValues,
    dry_run: bool,
) -> Result<CommandOutcome, ServerError> {
    // Clone the Arc up front: the worker mutates `ctx.device` below, so it cannot keep a
    // borrow of `ctx.registry` alive across that.
    let registry = ctx
        .registry
        .clone()
        .ok_or_else(|| ServerError::NoRegistry(ctx.device_id.to_string()))?;

    let command = registry
        .get(name)
        .ok_or_else(|| ServerError::UnknownCommand {
            device: ctx.device_id.to_string(),
            command: name.to_string(),
        })?
        .clone();

    let bytes = registry
        .build_request(name, values)
        .map_err(|e| ServerError::Protocol(format!("building '{name}': {e:?}")))?;

    // Dry-run stops here: the bytes are reported, nothing is written to the device.
    if dry_run {
        return Ok(CommandOutcome {
            sent: bytes,
            response: None,
            response_error: None,
            dry_run: true,
        });
    }

    let header = Header::from_bytes(&bytes)
        .map_err(|e| ServerError::Protocol(format!("built '{name}' has no valid header: {e:?}")))?;

    // Discard stale inbound traffic before sending, exactly as `HWDevice.request` does
    // via `_drain_input_queue`. Without this, a cyclic report that arrived before the
    // request could be taken as its response. The original logs a warning when it
    // discards anything; we do the same.
    let stale = ctx.device.poll_reports();
    if !stale.is_empty() {
        tracing::warn!(
            device = %ctx.device_id,
            count = stale.len(),
            "cleaning stale reports from the input queue before request"
        );
        for report in stale {
            publish(ctx, report);
        }
    }

    // Correlation is on `cmd` and `ext2`, per the device's own `_sanitize_response`.
    correlator.record_request(header.cmd, header.ext2, name);

    let segments = segment_report(
        header.cmd,
        header.seq,
        header.ext2,
        header.ext3,
        &bytes[gazelle_audio_protocol::wire::HEADER_SIZE..],
        ctx.device.max_packet_size(),
    );

    for seg in &segments {
        let report = Report {
            header: Header::from_bytes(seg)
                .map_err(|e| ServerError::Protocol(format!("segment header: {e:?}")))?,
            contents: seg[gazelle_audio_protocol::wire::HEADER_SIZE..].to_vec(),
        };
        ctx.device
            .send(&report)
            .map_err(|e| ServerError::Protocol(format!("sending '{name}': {e:?}")))?;
        // The transport delivers what it was given; a real adapter writes to the wire here.
        ctx.device.on_received_data(RawPacket { bytes: seg.clone() });
    }

    // Wait for the correlated response, publishing any cyclic traffic seen meanwhile.
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(matched) = pump(ctx, correlator) {
            let (response, response_error) = if command.returns.is_empty() {
                (None, None)
            } else {
                match decode_returns(&command, &matched) {
                    Ok(fields) => (Some(fields), None),
                    Err(e) => (None, Some(e)),
                }
            };
            return Ok(CommandOutcome {
                sent: bytes,
                response,
                response_error,
                dry_run: false,
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    Err(ServerError::Timeout {
        device: ctx.device_id.to_string(),
        command: name.to_string(),
    })
}

/// Drain inbound reports, returning the one that satisfied the outstanding request.
///
/// Reports that are not valid responses are published as device-initiated events rather
/// than discarded. Note this is slightly more forgiving than the original, which consumes
/// exactly one queued report per request and fails if it does not validate; here a cyclic
/// report arriving mid-request does not kill the request. That is a deliberate divergence,
/// recorded in `.agent/reference/decompilation-fidelity.md`.
fn pump(ctx: &mut WorkerContext, correlator: &mut ResponseCorrelator) -> Option<Report> {
    let mut matched = None;
    for report in ctx.device.poll_reports() {
        match correlator.correlate(report) {
            Correlation::Matched { report } => matched = Some(report),
            Correlation::Unmatched { report } | Correlation::Unsolicited { report } => {
                publish(ctx, report)
            }
        }
    }
    matched
}

fn drain_events(ctx: &mut WorkerContext, correlator: &mut ResponseCorrelator) {
    for report in ctx.device.poll_reports() {
        match correlator.correlate(report) {
            // With no request outstanding everything is device-initiated.
            Correlation::Matched { report }
            | Correlation::Unmatched { report }
            | Correlation::Unsolicited { report } => publish(ctx, report),
        }
    }
}

fn publish(ctx: &WorkerContext, report: Report) {
    let fields = ctx
        .registry
        .as_ref()
        .and_then(|r| decode_cyclic(r, &report));
    let event = match fields {
        Some(fields) => DeviceEvent::Cyclic {
            device_id: ctx.device_id.clone(),
            report_id: report.cmd(),
            fields,
        },
        None => DeviceEvent::Undecoded {
            device_id: ctx.device_id.clone(),
            report_id: report.cmd(),
            len: report.contents.len(),
        },
    };
    (ctx.events)(event);
}

/// Decode a device-initiated report against its declared cyclic layout.
///
/// Returns `None` when the device's registry declares no layout for this report id, in
/// which case the caller emits an `Undecoded` event rather than inventing fields.
fn decode_cyclic(registry: &Registry, report: &Report) -> Option<HashMap<String, Value>> {
    let layout = registry.cyclic(report.cmd())?;
    match layout.parse_contents(&report.contents) {
        Ok(fields) => Some(fields),
        Err(e) => {
            // A declared layout that does not fit the bytes is worth surfacing: it means
            // either a truncated report or a layout mismatch, both of which matter.
            tracing::debug!(
                report_id = format!("0x{:X}", report.cmd()),
                len = report.contents.len(),
                "cyclic report did not match its declared layout: {e:?}"
            );
            None
        }
    }
}

fn decode_returns(
    command: &gazelle_audio_protocol::Command,
    report: &Report,
) -> Result<HashMap<String, Value>, String> {
    let mut buf = Vec::with_capacity(gazelle_audio_protocol::wire::HEADER_SIZE + report.contents.len());
    buf.extend_from_slice(&report.header.to_bytes());
    buf.extend_from_slice(&report.contents);
    command.parse_response(&buf).map_err(|e| {
        format!(
            "could not decode {} bytes of response for '{}': {:?}",
            report.contents.len(),
            command.name,
            e
        )
    })
}
