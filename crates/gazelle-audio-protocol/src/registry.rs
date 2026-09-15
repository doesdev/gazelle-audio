//! Load `Command`s from the recovered `quadro_commands.json` format.
//!
//! The JSON is produced by `refs/tools/scripts/extract_field_layouts.py` from the
//! panel `REPORT_FORMAT`. Each command entry looks like:
//!
//! ```json
//! {
//!   "name": "set_mixer",
//!   "report_id": "0x70",
//!   "ext2": 0,
//!   "ext3": 0,
//!   "payload_id": 20,
//!   "params": [["mixer_id", "ubyte"], ...],
//!   "auto_send_notification": true
//! }
//! ```
//!
//! `payload_id` may be absent/null (commands without a payload header).
//! `params`/`returns` are lists of field entries parsed by `Field::parse`.

use crate::field::Field;
use crate::wire::WireError;
use crate::Command;
use serde_json::Value;
use std::collections::HashMap;

/// Load a command registry from the full `quadro_commands.json` document.
pub fn from_json_doc(doc: &Value) -> Result<Registry, WireError> {
    let commands = doc
        .get("commands")
        .and_then(Value::as_object)
        .ok_or(WireError::FieldOverflow)?;
    let mut by_name = HashMap::new();
    for (name, entry) in commands {
        let cmd = Command::from_json(entry)?;
        by_name.insert(name.clone(), cmd);
    }
    // Cyclic report layouts are optional: an older schema may not carry them, in which
    // case the registry simply has none and callers report reports as undecoded rather
    // than guessing a layout.
    let mut cyclic = HashMap::new();
    if let Some(obj) = doc.get("cyclic_reports").and_then(Value::as_object) {
        for (rid, entry) in obj {
            let report_id = parse_id(&entry["report_id"]).or_else(|_| {
                parse_id(&Value::String(rid.clone()))
            })?;
            let fields = parse_fields(entry.get("fields"))?;
            cyclic.insert(report_id, CyclicReport { report_id, fields });
        }
    }

    Ok(Registry { by_name, cyclic })
}

/// A loaded command registry.
#[derive(Clone, Debug)]
pub struct Registry {
    by_name: HashMap<String, Command>,
    /// Cyclic (device-initiated) report layouts, keyed by report id.
    ///
    /// These are the device -> host direction: unsolicited state pushes such as meters
    /// and transport status. They share the request field grammar but are not commands,
    /// so they are indexed separately, by id rather than by name.
    cyclic: HashMap<u32, CyclicReport>,
}

/// The field layout of one cyclic report.
#[derive(Clone, Debug)]
pub struct CyclicReport {
    /// The report id (the `cmd` header word), e.g. `0x73`.
    pub report_id: u32,
    pub fields: Vec<crate::field::Field>,
}

impl CyclicReport {
    /// Decode a report's contents (everything after the 16-byte header).
    pub fn parse_contents(
        &self,
        contents: &[u8],
    ) -> Result<HashMap<String, crate::payload::Value>, WireError> {
        Command::parse_field_list(&self.fields, contents)
    }
}

impl Registry {
    pub fn get(&self, name: &str) -> Option<&Command> {
        self.by_name.get(name)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.by_name.keys()
    }

    /// The cyclic report layout for a report id, if this device declares one.
    pub fn cyclic(&self, report_id: u32) -> Option<&CyclicReport> {
        self.cyclic.get(&report_id)
    }

    /// Every cyclic report id this device declares.
    pub fn cyclic_ids(&self) -> impl Iterator<Item = &u32> {
        self.cyclic.keys()
    }

    pub fn cyclic_len(&self) -> usize {
        self.cyclic.len()
    }

    /// Build the request bytes for a named command.
    pub fn build_request(
        &self,
        name: &str,
        values: &crate::payload::PayloadValues,
    ) -> Result<Vec<u8>, WireError> {
        self.get(name)
            .ok_or(WireError::FieldOverflow)?
            .build_request(values)
    }

    /// Build the request bytes for a named command with an optional `ext3` selector.
    pub fn build_request_with_ext3(
        &self,
        name: &str,
        values: &crate::payload::PayloadValues,
        ext3: Option<u32>,
    ) -> Result<Vec<u8>, WireError> {
        self.get(name)
            .ok_or(WireError::FieldOverflow)?
            .build_request_with_ext3(values, ext3)
    }
}

impl Command {
    /// Parse a single command entry from the recovered JSON.
    pub fn from_json(entry: &Value) -> Result<Command, WireError> {
        let name = entry["name"]
            .as_str()
            .ok_or(WireError::FieldOverflow)?
            .to_string();
        let report_id = parse_id(&entry["report_id"])?;
        let ext2 = entry["ext2"].as_u64().unwrap_or(0) as u32;
        let ext3 = entry["ext3"].as_u64().unwrap_or(0) as u32;
        let payload_id = match entry.get("payload_id") {
            Some(v) if !v.is_null() => Some(v.as_u64().ok_or(WireError::FieldOverflow)? as u32),
            _ => None,
        };
        let params = parse_fields(entry.get("params"))?;
        let returns = parse_fields(entry.get("returns"))?;
        let auto_send_notification = entry
            .get("auto_send_notification")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(Command {
            name,
            report_id,
            ext2,
            ext3,
            payload_id,
            params,
            returns,
            auto_send_notification,
        })
    }
}

fn parse_id(v: &Value) -> Result<u32, WireError> {
    match v.as_str() {
        Some(s) => u32::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16)
            .map_err(|_| WireError::FieldOverflow),
        None => v
            .as_u64()
            .map(|n| n as u32)
            .ok_or(WireError::FieldOverflow),
    }
}

fn parse_fields(fields: Option<&Value>) -> Result<Vec<Field>, WireError> {
    let Some(arr) = fields.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for f in arr {
        out.push(Field::parse(f)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_mixer_command() {
        let doc: Value = serde_json::json!({
            "commands": {
                "set_mixer": {
                    "name": "set_mixer",
                    "report_id": "0x70",
                    "ext2": 0,
                    "ext3": 0,
                    "payload_id": 20,
                    "params": [["mixer_id", "ubyte"], ["channel", "ubyte"],
                               ["level", "ubyte"], ["pan", "ubyte"],
                               ["mute", "ubyte"], ["solo", "ubyte"]],
                    "auto_send_notification": true
                }
            }
        });
        let reg = from_json_doc(&doc).unwrap();
        assert_eq!(reg.len(), 1);
        let cmd = reg.get("set_mixer").unwrap();
        assert_eq!(cmd.report_id, 0x70);
        assert_eq!(cmd.payload_id, Some(20));
        assert!(cmd.auto_send_notification);
        assert_eq!(cmd.params.len(), 6);
    }

    #[test]
    fn parses_hex_and_null_payload() {
        let doc: Value = serde_json::json!({
            "commands": {
                "set_tb_latency": {
                    "name": "set_tb_latency",
                    "report_id": "0xE0",
                    "ext2": 0,
                    "ext3": 0,
                    "params": [["mode", "int32"]]
                }
            }
        });
        let reg = from_json_doc(&doc).unwrap();
        let cmd = reg.get("set_tb_latency").unwrap();
        assert_eq!(cmd.report_id, 0xE0);
        assert_eq!(cmd.payload_id, None);
    }
}
