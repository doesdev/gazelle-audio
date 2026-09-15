//! Antelope Audio Synergy Core USB wire protocol.
//!
//! This crate is a transport-agnostic, protocol-correct reimplementation of the
//! command model recovered from the Antelope panel/server decompilation. It is
//! deliberately free of any USB/HID/OS dependency so it can be unit-tested
//! against ground-truth byte vectors (see the `tests` module) before any real
//! hardware is attached.
//!
//! The recovered command sets live in `refs/schemas/quadro_commands.json` (63:
//! the shared 35 + 28 Quadro-only) and `refs/schemas/studio_commands.json` (43:
//! the shared 35 + 8 Studio+-only). Each command carries a `report_id`,
//! `ext2`/`ext3` selectors, an optional `payload_id`, and a list of `Field`s
//! for its request params and/or its response `returns`.

/// Absolute path to the recovered command registry, resolved at compile time.
///
/// Single source of truth for this path — tests in every crate reference this
/// constant rather than re-deriving `../..` chains, which were previously
/// inconsistent and cwd-fragile.
pub const QUADRO_COMMANDS_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../refs/schemas/quadro_commands.json");

/// Absolute path to the Studio+ command registry, resolved at compile time.
pub const STUDIO_COMMANDS_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../refs/schemas/studio_commands.json");

pub mod crc32;
pub mod field;
pub mod payload;
pub mod registry;
pub mod wire;

pub use field::{Field, Scalar};
pub use payload::{Payload, PayloadValues, Value};
pub use wire::{Header, WireError};

/// Read `bits` bits LSB-first starting at bit position `start` within `data`.
///
/// Mirrors the ctypes bitfield packing used by the device's cyclic reports:
/// the first field occupies the least significant bits of the first byte.
fn read_bits_lsb_first(data: &[u8], start: usize, bits: usize) -> u64 {
    let mut out: u64 = 0;
    for i in 0..bits {
        let bit_pos = start + i;
        let byte = data[bit_pos / 8];
        let bit = (byte >> (bit_pos % 8)) & 1;
        out |= (bit as u64) << i;
    }
    out
}

/// Sign-extend a `bits`-bit unsigned value to a signed integer.
fn sign_extend(value: u64, bits: usize) -> i64 {
    if bits >= 64 || (value & (1 << (bits - 1))) == 0 {
        value as i64
    } else {
        // Set all high bits to mark a negative number, then reinterpret.
        (value | !((1u64 << bits) - 1)) as i64
    }
}

/// Read a little-endian unsigned integer of `n` bytes (1, 2, or 4) from `bytes`.
fn read_le_int(bytes: &[u8], n: usize) -> u64 {
    bytes[..n]
        .iter()
        .enumerate()
        .fold(0u64, |acc, (i, &b)| acc | (b as u64) << (8 * i))
}

use std::collections::HashMap;

/// A single device command (request + optional response layout).
#[derive(Clone, Debug)]
pub struct Command {
    /// Command name, e.g. `set_mixer`.
    pub name: String,
    /// Report/command id (the `cmd` header word).
    pub report_id: u32,
    /// ext2 selector (command type / bank index / per-field selector).
    pub ext2: u32,
    /// ext3 selector.
    pub ext3: u32,
    /// Optional payload id (null for commands without a payload header).
    pub payload_id: Option<u32>,
    /// Request parameter fields (empty for pure responses).
    pub params: Vec<Field>,
    /// Response `returns` fields (empty for pure commands).
    pub returns: Vec<Field>,
    /// Whether sending this command also pushes a notification to other
    /// clients (mirrors `auto_send_notification` in the recovered format).
    pub auto_send_notification: bool,
}

/// Commands whose header `ext3` is a per-request selector rather than a fixed value.
///
/// The recovered schemas give every command a constant `ext3`, but the panels vary it per call
/// for these (bytecode research, 2026-09):
/// * `get_routing`: `ext3` is the destination group index; `get_device_data` asks once per group.
/// * `get_mixer`: `ext3` is the mixer id; one call returns one mixer.
///
/// Every other command must keep its schema `ext3`: overriding it would silently change what
/// the device is asked, so callers are refused rather than trusted.
pub const EXT3_SELECTOR_COMMANDS: &[&str] = &["get_routing", "get_mixer"];

impl Command {
    /// Whether this command takes a per-request `ext3` selector (see [`EXT3_SELECTOR_COMMANDS`]).
    pub fn takes_ext3_selector(&self) -> bool {
        EXT3_SELECTOR_COMMANDS.contains(&self.name.as_str())
    }

    /// Build the request with `ext3` replaced by a per-request selector, when one is given.
    ///
    /// Fails with [`WireError::FieldOverflow`] if a selector is given for a command that does not
    /// take one.
    pub fn build_request_with_ext3(
        &self,
        values: &PayloadValues,
        ext3: Option<u32>,
    ) -> Result<Vec<u8>, WireError> {
        let mut bytes = self.build_request(values)?;
        if let Some(selector) = ext3 {
            if !self.takes_ext3_selector() {
                return Err(WireError::FieldOverflow);
            }
            bytes[12..16].copy_from_slice(&selector.to_le_bytes());
        }
        Ok(bytes)
    }

    /// Build the serialized request bytes for this command.
    ///
    /// `values` maps each request field name to its value. The payload header
    /// (`payload_id`/`nparams`/`nbytes`) is synthesized from `self.payload_id`
    /// and the sizes of `self.params`. Returns the full 16-byte header +
    /// payload. For commands without a payload (`payload_id` is null), the
    /// header `seq` is just the header size (16).
    pub fn build_request(
        &self,
        values: &PayloadValues,
    ) -> Result<Vec<u8>, WireError> {
        let body = if let Some(pid) = self.payload_id {
            let p = Payload::new(Some(pid), self.params.clone())?;
            p.to_bytes(values)?
        } else if self.params.is_empty() {
            Vec::new()
        } else {
            // No payload header (e.g. set_tb_latency), but the user fields
            // still serialize directly after the header.
            let p = Payload::new(None, self.params.clone())?;
            p.to_bytes(values)?
        };
        let seq = (16 + body.len()) as u32;
        let header = Header::new(self.report_id, seq, self.ext2, self.ext3);
        let mut out = header.to_bytes().to_vec();
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Parse a response buffer into a map of field name -> decoded values,
    /// using this command's `returns` layout.
    ///
    /// Cyclic reports (e.g. `0x73`) pack sub-byte scalars LSB-first into the
    /// contents, so bit-packed fields are read from a running bit position
    /// rather than sliced as whole bytes. Full-width scalars, arrays, and
    /// struct arrays are read as raw bytes.
    pub fn parse_response(&self, buf: &[u8]) -> Result<HashMap<String, Value>, WireError> {
        if buf.len() < 16 {
            return Err(WireError::TruncatedHeader);
        }
        let contents = &buf[16..];
        Self::parse_contents(&self.returns, contents)
    }

    /// Parse the contents (payload after the 16-byte header) of a report into a
    /// map of field name -> decoded value. Bit-packed scalars are read LSB-first
    /// from a running bit position; full-width scalars are read as little-endian
    /// integers. Arrays and elem-arrays decode to raw bytes; struct arrays recurse
    /// so their nested fields are decoded too.
    /// Decode an arbitrary field list against a buffer.
    ///
    /// Public so cyclic report layouts, which are not commands, can use the same decoder.
    pub fn parse_field_list(
        fields: &[Field],
        contents: &[u8],
    ) -> Result<HashMap<String, Value>, WireError> {
        Self::parse_contents(fields, contents)
    }

    fn parse_contents(
        fields: &[Field],
        contents: &[u8],
    ) -> Result<HashMap<String, Value>, WireError> {
        let mut out = HashMap::new();
        let mut bit_pos: usize = 0;
        for f in fields {
            match f {
                Field::Scalar { bit_width: Some(w), ty, name, .. } => {
                    // Bit-packed scalar: read `w` bits LSB-first from bit_pos.
                    if bit_pos + *w as usize > contents.len() * 8 {
                        return Err(WireError::TruncatedPayload);
                    }
                    let raw = read_bits_lsb_first(contents, bit_pos, *w as usize);
                    let signed = matches!(ty, Scalar::I8 | Scalar::I16 | Scalar::I32);
                    let value = if signed {
                        Value::I64(sign_extend(raw, *w as usize))
                    } else {
                        Value::U64(raw)
                    };
                    out.insert(name.clone(), value);
                    bit_pos += *w as usize;
                }
                Field::Scalar { ty, name, .. } => {
                    // Full-width scalar: read a little-endian integer of the
                    // scalar's width. Signed types sign-extend.
                    let n = ty.size();
                    // Byte-oriented fields resume at the next byte boundary: truncating a
                    // mid-byte bit position would re-read bytes a preceding bit
                    // field already consumed.
                    bit_pos = bit_pos.div_ceil(8) * 8;
                    let start = bit_pos / 8;
                    if start + n > contents.len() {
                        return Err(WireError::TruncatedPayload);
                    }
                    let raw = read_le_int(&contents[start..start + n], n);
                    let value = if matches!(ty, Scalar::I8 | Scalar::I16 | Scalar::I32) {
                        // `read_le_int` zero-extends, so a signed field must be
                        // sign-extended from its own width. Without this an i8 of 0xFF
                        // decodes as 255 and an i32 of -1 as 4294967295 — which is live
                        // for get_tb_latency's latency/buffer adjust fields.
                        Value::I64(sign_extend(raw, n * 8))
                    } else {
                        Value::U64(raw)
                    };
                    out.insert(name.clone(), value);
                    bit_pos += n * 8;
                }
                Field::Array { .. } | Field::ElemArray { .. } => {
                    // Array: read raw bytes.
                    let n = f.size();
                    // Byte-oriented fields resume at the next byte boundary: truncating a
                    // mid-byte bit position would re-read bytes a preceding bit
                    // field already consumed.
                    bit_pos = bit_pos.div_ceil(8) * 8;
                    let start = bit_pos / 8;
                    if start + n > contents.len() {
                        return Err(WireError::TruncatedPayload);
                    }
                    out.insert(
                        f.name().to_string(),
                        Value::Bytes(contents[start..start + n].to_vec()),
                    );
                    bit_pos += n * 8;
                }
                Field::StructArray { fields, count, name } => {
                    // Struct array: each element is a packed sub-struct. Parse
                    // every element recursively (bit-packed fields included) into a
                    // list of structs. Advance past the whole
                    // array, not just one element. The element size is the total
                    // bit width of its fields divided into whole bytes, not the
                    // sum of each field's byte size.
                    let elem_bits: usize = fields.iter().map(Field::bit_len).sum();
                    let elem_size = elem_bits.div_ceil(8);
                    let array_size = elem_size * count;
                    // Byte-oriented fields resume at the next byte boundary: truncating a
                    // mid-byte bit position would re-read bytes a preceding bit
                    // field already consumed.
                    bit_pos = bit_pos.div_ceil(8) * 8;
                    let start = bit_pos / 8;
                    if start + array_size > contents.len() {
                        return Err(WireError::TruncatedPayload);
                    }
                    // Every element, in wire order. `count: 0` (get_afx_order.slots, whose real
                    // count comes from the never-decompiled afx_pool.PHY_AFX_STRIP_SIZE) yields
                    // an empty list rather than slicing past the buffer.
                    let mut elems = Vec::with_capacity(*count);
                    for i in 0..*count {
                        let off = start + i * elem_size;
                        let elem = Self::parse_contents(fields, &contents[off..off + elem_size])?;
                        elems.push(Value::Struct(elem));
                    }
                    out.insert(name.clone(), Value::List(elems));
                    bit_pos += array_size * 8;
                }
            }
        }
        Ok(out)
    }
}

/// The set of in-scope commands, indexed by name.
#[derive(Clone, Debug)]
pub struct CommandRegistry {
    by_name: HashMap<String, Command>,
}

impl CommandRegistry {
    pub fn new(commands: Vec<Command>) -> Self {
        let mut by_name = HashMap::new();
        for c in commands {
            by_name.insert(c.name.clone(), c);
        }
        CommandRegistry { by_name }
    }

    pub fn get(&self, name: &str) -> Option<&Command> {
        self.by_name.get(name)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Build the request bytes for a named command.
    pub fn build_request(
        &self,
        name: &str,
        values: &PayloadValues,
    ) -> Result<Vec<u8>, WireError> {
        self.get(name)
            .ok_or(WireError::FieldOverflow)?
            .build_request(values)
    }
}

#[cfg(test)]
mod tests;
