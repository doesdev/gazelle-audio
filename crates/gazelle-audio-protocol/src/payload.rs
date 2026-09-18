//! Serialization of request payloads to their on-the-byte form.
//!
//! Mirrors `Payload` in `refs/decompiled/manager/antelope_dev_reports.py`. When a
//! payload has a `payload_id`, the serialized body is:
//!
//! ```text
//! byte 0: payload_id (bits 0..6) | nparams (bits 6..8)
//! byte 1: nbytes                 (only when user_bytes >= 4)
//!        ... user fields ...
//! ```
//!
//! When `payload_id` is null (e.g. `set_tb_latency`), the body is just the
//! user fields. The `nparams` field is `3` when present, otherwise
//! `user_bytes - 1` (the `<4` branch adds no `nbytes` byte).

use crate::field::{Field, Scalar};
use crate::wire::WireError;

/// A serialized request payload: the user fields plus the optional header.
#[derive(Clone, Debug)]
pub struct Payload {
    /// The payload id, or `None` for commands without a payload header.
    pub payload_id: Option<u32>,
    /// The user-supplied fields (the payload body).
    pub fields: Vec<Field>,
    /// Total serialized size including the optional header bytes.
    pub bytesize: usize,
    /// Size of the user payload body, in bytes (excludes header bytes).
    pub user_bytes: usize,
}

/// Emit any buffered sub-byte bits as a whole byte, LSB-first, and reset the accumulator.
///
/// Called before every byte-oriented field and once at the end, so bit-packed fields never
/// overtake the full-width fields that follow them.
fn flush_bits(out: &mut Vec<u8>, bit_acc: &mut u32, bit_fill: &mut u32) {
    if *bit_fill > 0 {
        out.push((*bit_acc & 0xFF) as u8);
        *bit_acc = 0;
        *bit_fill = 0;
    }
}

impl Payload {
    pub fn new(payload_id: Option<u32>, fields: Vec<Field>) -> Result<Payload, WireError> {
        // The reference (`Payload.__init__` in the decompiled antelope_dev_reports.py)
        // sums each field's *bit* length, requires the total to be byte-complete, and
        // divides by 8. Summing per-field byte sizes instead rounds every sub-byte field
        // up independently, so e.g. five 1-bit fields would report 5 bytes rather than 1
        // and write a wrong `nbytes` into the payload header.
        let total_bits: usize = fields.iter().map(Field::bit_len).sum();
        // `%` rather than `is_multiple_of`, which is only stable since 1.87 (the floor is 1.82).
        if total_bits % 8 != 0 {
            return Err(WireError::FieldOverflow);
        }
        let user_bytes: usize = total_bits / 8;
        let mut bytesize = user_bytes;
        if payload_id.is_some() {
            // payload_id (6 bits) + nparams (2 bits) share one byte.
            if user_bytes < 4 {
                bytesize += 1;
            } else {
                bytesize += 2;
            }
        }
        Ok(Payload {
            payload_id,
            fields,
            bytesize,
            user_bytes,
        })
    }

    /// Serialize this payload to bytes.
    ///
    /// `values` maps field names to their values. Scalars are provided as
    /// `u64`/`i64`; arrays/structs are provided as `Vec<u8>` of the raw bytes.
    pub fn to_bytes(&self, values: &PayloadValues) -> Result<Vec<u8>, WireError> {
        let mut out = Vec::with_capacity(self.bytesize);

        // Field defaults, so omitted scalars fall back to their declared
        // default (e.g. `density` = 100) rather than always zero.
        let defaults: std::collections::HashMap<String, i64> = self
            .fields
            .iter()
            .filter_map(|f| match f {
                Field::Scalar { default: Some(d), name, .. } => Some((name.clone(), *d)),
                _ => None,
            })
            .collect();

        // Optional payload header. Ground-truth bytes from the decompiled
        // Payload show two layouts:
        //   user_bytes < 4: 1 header byte, nparams = user_bytes - 1, no nbytes
        //   user_bytes >= 4: 2 header bytes, nparams = 3, nbytes = user_bytes
        if let Some(pid) = self.payload_id {
            if self.user_bytes < 4 {
                let nparams = self.user_bytes.saturating_sub(1) as u32;
                let header_byte = (pid & 0x3F) | ((nparams & 0x03) << 6);
                out.push(header_byte as u8);
            } else {
                let header_byte = (pid & 0x3F) | (3 << 6);
                out.push(header_byte as u8);
                out.push(self.user_bytes as u8);
            }
        }

        let mut bit_acc: u32 = 0;
        let mut bit_fill: u32 = 0;
        for f in &self.fields {
            match f {
                Field::Scalar { bit_width: Some(w), .. } => {
                    // Pack several sub-byte scalars into one byte, LSB-first:
                    // the first field occupies the least significant bits
                    // (e.g. payload_id in bits 0..6, nparams in bits 6..8).
                    let raw = values.get_scalar(f.name(), &defaults)?;
                    if raw >= (1u64 << *w) {
                        return Err(WireError::FieldOverflow);
                    }
                    bit_acc |= (raw as u32) << bit_fill;
                    bit_fill += w;
                    while bit_fill >= 8 {
                        out.push((bit_acc & 0xFF) as u8);
                        bit_acc >>= 8;
                        bit_fill -= 8;
                    }
                }
                Field::Scalar { ty, .. } => {
                    flush_bits(&mut out, &mut bit_acc, &mut bit_fill);
                    let raw = values.get_scalar(f.name(), &defaults)?;
                    match ty {
                        Scalar::U8 | Scalar::I8 => out.push(raw as u8),
                        Scalar::U16 | Scalar::I16 => out.extend_from_slice(&(raw as u16).to_le_bytes()),
                        Scalar::U32 | Scalar::I32 => out.extend_from_slice(&(raw as u32).to_le_bytes()),
                    }
                }
                Field::Array { elem, count, .. } => {
                    flush_bits(&mut out, &mut bit_acc, &mut bit_fill);
                    let expected = elem.size() * count;
                    match values.get_bytes(f.name()) {
                        Ok(bytes) => {
                            if bytes.len() != expected {
                                return Err(WireError::TruncatedPayload);
                            }
                            out.extend_from_slice(bytes);
                        }
                        // Missing array fields default to zero bytes.
                        Err(_) => out.extend(std::iter::repeat_n(0u8, expected)),
                    }
                }
                Field::StructArray { .. } | Field::ElemArray { .. } => {
                    flush_bits(&mut out, &mut bit_acc, &mut bit_fill);
                    let expected = f.size();
                    match values.get_bytes(f.name()) {
                        Ok(bytes) => {
                            if bytes.len() != expected {
                                return Err(WireError::TruncatedPayload);
                            }
                            out.extend_from_slice(bytes);
                        }
                        // Missing struct/elem-array fields default to zero bytes.
                        Err(_) => out.extend(std::iter::repeat_n(0u8, expected)),
                    }
                }
            }
        }
        // Flush any remaining bits. LSB-first, matching the in-loop flush above and
        // `read_bits_lsb_first` on the decode side. The previous MSB-aligned shift
        // (`bit_acc << (8 - bit_fill)`) contradicted both, so a trailing partial byte
        // could not be read back by this crate's own reader.
        flush_bits(&mut out, &mut bit_acc, &mut bit_fill);
        Ok(out)
    }
}

/// Field values for serializing a payload.
#[derive(Default)]
pub struct PayloadValues {
    map: std::collections::HashMap<String, Value>,
}

/// A single field value.
#[derive(Clone, Debug)]
pub enum Value {
    U64(u64),
    I64(i64),
    Bytes(Vec<u8>),
    /// A nested struct: one element of a StructArray.
    Struct(std::collections::HashMap<String, Value>),
    /// Every element of a `StructArray` field, in wire order.
    List(Vec<Value>),
}


impl PayloadValues {
    pub fn new(map: std::collections::HashMap<String, Value>) -> Self {
        PayloadValues { map }
    }

    pub fn with_scalar(mut self, name: &str, v: u64) -> Self {
        self.map.insert(name.to_string(), Value::U64(v));
        self
    }

    pub fn with_i64(mut self, name: &str, v: i64) -> Self {
        self.map.insert(name.to_string(), Value::I64(v));
        self
    }

    pub fn with_bytes(mut self, name: &str, v: Vec<u8>) -> Self {
        self.map.insert(name.to_string(), Value::Bytes(v));
        self
    }
}

impl PayloadValues {
    pub fn get_scalar(
        &self,
        name: &str,
        defaults: &std::collections::HashMap<String, i64>,
    ) -> Result<u64, WireError> {
        match self.map.get(name) {
            Some(Value::U64(v)) => Ok(*v),
            Some(Value::I64(v)) => Ok(*v as u64),
            // Missing scalars fall back to the field's declared default, then 0.
            // This mirrors the decompiled Payload, which fills omitted fields
            // from each field's `default` (e.g. `density` = 100).
            _ => Ok(defaults.get(name).copied().unwrap_or(0) as u64),
        }
    }

    pub fn get_bytes(&self, name: &str) -> Result<&[u8], WireError> {
        match self.map.get(name) {
            Some(Value::Bytes(b)) => Ok(b.as_slice()),
            _ => Err(WireError::TruncatedPayload),
        }
    }
}

/// Convenience accessor for a field name.
impl Field {
    pub fn name(&self) -> &str {
        match self {
            Field::Scalar { name, .. }
            | Field::Array { name, .. }
            | Field::StructArray { name, .. }
            | Field::ElemArray { name, .. } => name,
        }
    }
}
