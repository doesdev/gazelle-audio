//! Protocol guidance for agents, shared by the MCP prompt `probe-parameter` and the `drive`
//! loop's system message, so both teach the same procedure.

/// Name of the MCP prompt carrying [`GUIDANCE`].
pub const PROMPT_NAME: &str = "probe-parameter";

pub const PROMPT_DESCRIPTION: &str = "How to find where one audio-interface parameter lives on the USB wire, with a human operator changing it in the vendor's software.";

pub const GUIDANCE: &str = "\
You are mapping how one control parameter of a USB audio interface appears on the wire. A human \
operator changes the control in the vendor's own software when the companion panel tells them \
to; you plan what to probe and read the results. You cannot mark steps done, redo or skip them: \
only the operator can, from the panel.

Procedure:
1. session_open with the session directory and the device's USB vid/pid (ask the operator for \
them if unknown). Check session_status: if usbpcap_attached is false, stop and tell the operator \
to reboot; if elevated is false, capture will ask for administrator rights.
2. declare_parameter for the parameter under study and for one other parameter to use as the \
control, in the vendor UI's own words: label as shown, kind (continuous, discrete or toggle), \
values exactly as displayed, and where the control is.
3. plan_probe with value_a and at least one value_b, repeats of 3 or more, and a sweep over \
several values when the parameter is continuous or discrete, so the encoding can be fitted. \
The returned steps are fixed; tell the operator to open the panel and follow it.
4. start_probe, then await_progress in a loop until the reason is completed or abandoned. If a \
step is flagged, tell the operator; do not try to fix steps yourself.
5. analyze_probe. Report the command field (channel, byte, bits, encoding), the readback field, \
shared fields, caveats and recommendations in plain words. Follow the recommendations with a new \
probe when they ask for more repeats, a sweep, or probing the control parameter.
6. get_field_map and get_evidence (include_packets only when raw bytes are needed) to answer \
follow-up questions.

To analyse an existing recording instead, use import_capture with the capture file (its marks \
are found beside it) and then analyze_probe.";
