//! Integration test: load the real recovered `in_scope_commands.json` and
//! exercise the registry end-to-end.

use antelope_protocol::crc32::crc32;
use antelope_protocol::field::Field;
use antelope_protocol::payload::Value;
use antelope_protocol::registry::{from_json_doc, Registry};
use antelope_protocol::payload::PayloadValues;
use antelope_protocol::Command;
fn load_registry() -> Registry {
    let path = antelope_protocol::IN_SCOPE_COMMANDS_PATH;
    let doc = std::fs::read_to_string(path).expect("read in_scope_commands.json");
    let parsed: serde_json::Value = serde_json::from_str(&doc).expect("parse json");
    from_json_doc(&parsed).expect("load registry")
}

/// Load the ground-truth request bytes (name -> hex) generated from the
/// decompiled `Payload`/`Request` classes with default/zero values.
fn load_ground_truth() -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/ground_truth.json");
    let doc = std::fs::read_to_string(path).expect("read ground_truth.json");
    serde_json::from_str(&doc).expect("parse ground_truth.json")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Every command's default-valued request must match the decompiled reference
/// byte-for-byte. This is the automated regression guard for the serialization
/// rules (payload header layout, null payload_id, bit-packing, zero defaults).
#[test]
fn all_commands_match_ground_truth() {
    let reg = load_registry();
    let gt = load_ground_truth();
    let gt_cmds = gt.as_object().expect("ground_truth.json is an object");
    assert_eq!(
        reg.len(),
        gt_cmds.len(),
        "command count mismatch: registry={} ground_truth={}",
        reg.len(),
        gt_cmds.len()
    );
    for name in reg.names() {
        let values = PayloadValues::default();
        let out = reg
            .build_request(name, &values)
            .unwrap_or_else(|e| panic!("failed to build request for {name}: {e}"));
        let expected = gt_cmds[name]
            .as_str()
            .unwrap_or_else(|| panic!("ground_truth entry for {name} is not a hex string"));
        assert_eq!(
            hex(&out),
            expected,
            "{name}: Rust != ground truth\n  got:      {}\n  expected: {}",
            hex(&out),
            expected
        );
    }
}

#[test]
fn loads_all_in_scope_commands() {
    let reg = load_registry();
    // The recovered in-scope surface is 63 commands (shared 35 + Quadro-only 28).
    assert_eq!(reg.len(), 63, "expected 63 in-scope commands");
}

/// Load the ground-truth 0x73 cyclic report bytes and expected decoded values.
fn load_cyclic_ground_truth() -> (Vec<u8>, serde_json::Value) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/cyclic_gt.json");
    let doc = std::fs::read_to_string(path).expect("read cyclic_gt.json");
    let parsed: serde_json::Value = serde_json::from_str(&doc).expect("parse cyclic_gt.json");
    let report = hex_to_bytes(parsed["report"].as_str().expect("report is hex"));
    (report, parsed)
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex byte"))
        .collect()
}

/// Build a Command whose `returns` layout is the 0x73 cyclic report layout.
fn build_cyclic_command(gt: &serde_json::Value) -> Command {
    let layout = gt["layout"].as_array().expect("layout is array");
    let returns: Vec<Field> = layout
        .iter()
        .map(|e| Field::parse(e).expect("parse returns field"))
        .collect();
    Command {
        name: "get_cyclic_0x73".to_string(),
        report_id: 0x73,
        ext2: 0,
        ext3: 0,
        payload_id: None,
        params: Vec::new(),
        returns,
        auto_send_notification: false,
    }
}

/// Recursively compare decoded values against the ground truth, converting
/// Python ints/arrays to the Rust Value enum.
fn assert_values_match(
    got: &std::collections::HashMap<String, Value>,
    expected: &serde_json::Value,
    path: &str,
) {
    for (key, exp_val) in expected.as_object().expect("expected is object") {
        let got_val = got.get(key).unwrap_or_else(|| panic!("missing field {path}.{key}"));
        match exp_val {
            serde_json::Value::Number(n) => {
                let exp = n.as_u64().unwrap_or_else(|| n.as_i64().unwrap_or(0) as u64);
                match got_val {
                    Value::U64(v) => assert_eq!(*v, exp, "{path}.{key}"),
                    Value::I64(v) => assert_eq!(*v as u64, exp, "{path}.{key}"),
                    _ => panic!("{path}.{key}: expected number, got {got_val:?}"),
                }
            }
            serde_json::Value::Array(arr) => {
                let got_bytes = match got_val {
                    Value::Bytes(b) => b.clone(),
                    _ => panic!("{path}.{key}: expected bytes, got {got_val:?}"),
                };
                // Preserve the byte pattern: signed JSON numbers (e.g. -1) must
                // map to their two's-complement byte (0xFF), not 0.
                let exp_bytes: Vec<u8> = arr
                    .iter()
                    .map(|v| v.as_i64().unwrap_or(0) as u8)
                    .collect();
                assert_eq!(
                    got_bytes,
                    exp_bytes,
                    "{path}.{key}: bytes mismatch"
                );
            }
            serde_json::Value::Object(_obj) => {
                let got_struct = match got_val {
                    Value::Struct(s) => s,
                    _ => panic!("{path}.{key}: expected struct, got {got_val:?}"),
                };
                assert_values_match(got_struct, exp_val, &format!("{path}.{key}"));
            }
            _ => panic!("{path}.{key}: unsupported expected value"),
        }
    }
}

/// The 0x73 cyclic report must decode to the ground-truth field values, and its
/// `seq` header must equal the CRC32 of the contents.
#[test]
fn cyclic_report_0x73_decodes_to_ground_truth() {
    let (report, gt) = load_cyclic_ground_truth();
    assert_eq!(report.len(), 258, "0x73 report length");

    let cmd = build_cyclic_command(&gt);
    let decoded = cmd.parse_response(&report).expect("parse 0x73 report");

    // The seq header (bytes 4..8, little-endian) holds the CRC32 of the
    // contents (bytes 16..end).
    let seq = u32::from_le_bytes([report[4], report[5], report[6], report[7]]);
    let contents = &report[16..];
    assert_eq!(seq, crc32(contents), "seq must equal CRC32 of contents");

    assert_values_match(&decoded, &gt["expected"], "0x73");
}

#[test]
fn every_command_builds_a_request() {
    let reg = load_registry();
    for name in reg.names() {
        // An empty value set must still build a request: payload headers are
        // synthesized and user fields default to zero.
        let values = PayloadValues::default();
        let out = reg
            .build_request(name, &values)
            .unwrap_or_else(|e| panic!("failed to build request for {name}: {e}"));
        // Every request has at least the 16-byte header.
        assert!(out.len() >= 16, "{name}: request shorter than header");
        // The header cmd word must match the command's report_id.
        let report_id = reg.get(name).unwrap().report_id;
        assert_eq!(&out[0..4], &report_id.to_le_bytes(), "{name}: report_id mismatch");
    }
}
