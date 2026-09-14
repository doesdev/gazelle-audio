//! Request types and the operation table. Responses are JSON values built by the service; the
//! request schemas are what agents are shown.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::model::{Parameter, ProbePlan};

/// Select the device under study: open an existing session directory, or create one for a
/// VID/PID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpen {
    /// Session directory. Created when it holds no session yet.
    pub path: String,
    /// Target USB vendor id; required when creating a session.
    pub vid: Option<u16>,
    /// Target USB product id; required when creating a session.
    pub pid: Option<u16>,
}

/// Capture state, elevation, USBPcap attachment, the running probe and flagged steps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionStatus {}

/// Load a recorded capture for offline analysis as the session's next probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportCapture {
    /// A `.pcap` or `.pcapng` file (USBPcap or usbmon link type).
    pub capture: String,
    /// The `marks.jsonl` recorded with the capture. Without marks there are no step windows to
    /// analyse.
    pub marks: Option<String>,
}

/// Declare a parameter in the vendor UI's own terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeclareParameter {
    pub parameter: Parameter,
}

/// List the declared parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListParameters {}

/// Submit a probe plan; returns the fixed step sequence the operator will follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanProbe {
    pub plan: ProbePlan,
    /// Shuffle seed; chosen by the helper when omitted.
    pub seed: Option<u64>,
}

/// Start capturing and guiding the operator through a planned probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StartProbe {
    pub probe_id: String,
}

/// Cancel the running probe.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AbandonProbe {}

/// Wait until the running probe completes, a step is flagged, or the timeout passes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AwaitProgress {
    /// Longest wait in milliseconds; 30000 when omitted.
    pub timeout_ms: Option<u64>,
}

/// Analyse a finished probe: field map, confidence and recommendations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnalyzeProbe {
    /// Probe id such as `p1`; the latest probe when omitted.
    pub probe_id: Option<String>,
}

/// The stored field map for a parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetFieldMap {
    pub parameter: String,
}

/// The evidence behind a parameter's field map; raw packet bytes only when asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetEvidence {
    pub parameter: String,
    /// Include hex excerpts of the cited packets.
    #[serde(default)]
    pub include_packets: bool,
}

/// One operation as agents see it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OperationSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema of the request object.
    pub input_schema: Value,
}

fn spec<T: JsonSchema>(name: &'static str, description: &'static str) -> OperationSpec {
    let input_schema = serde_json::to_value(schemars::schema_for!(T)).expect("schemas serialise");
    OperationSpec { name, description, input_schema }
}

/// Every operation, in the spec's order.
pub fn operations() -> Vec<OperationSpec> {
    vec![
        spec::<SessionOpen>("session_open", "Select the device under study by opening or creating a session directory."),
        spec::<SessionStatus>("session_status", "Capture state, elevation, USBPcap attachment, the running probe and flagged steps."),
        spec::<ImportCapture>("import_capture", "Load a recorded .pcap/.pcapng (with its marks) as the next probe, for offline analysis."),
        spec::<DeclareParameter>("declare_parameter", "Declare a parameter using the vendor UI's own label, kind and values."),
        spec::<ListParameters>("list_parameters", "List the declared parameters."),
        spec::<PlanProbe>("plan_probe", "Submit a probe plan and get back the fixed step sequence the operator will follow."),
        spec::<StartProbe>("start_probe", "Start capturing and guiding the operator through a planned probe."),
        spec::<AbandonProbe>("abandon_probe", "Cancel the running probe."),
        spec::<AwaitProgress>("await_progress", "Wait until the probe completes, a step is flagged, or the timeout passes."),
        spec::<AnalyzeProbe>("analyze_probe", "Analyse a finished probe: field map, confidence and recommendations."),
        spec::<GetFieldMap>("get_field_map", "The stored field map for a parameter."),
        spec::<GetEvidence>("get_evidence", "Evidence behind a parameter's field map; raw packet excerpts only on request."),
    ]
}
