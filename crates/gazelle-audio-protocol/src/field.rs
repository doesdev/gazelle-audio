//! The field grammar for request payloads and cyclic-report responses.
//!
//! Recovered from `refs/decompiled/manager/antelope_dev_reports.py` (`Field`,
//! `Payload`, `Request`). The wire layout is a tightly packed, little-endian
//! struct with `_pack_ = 1`. Field types as they appear in the recovered
//! `REPORT_FORMAT` JSON:
//!
//! * scalar string: `"ubyte"`, `"byte"`, `"short"`, `"int32"`, `"uint16"`,
//!   `"uint32"`, `"uint8"`, `"int8"` — optionally with a bit width, e.g.
//!   `["density","ubyte",8,100]`.
//! * inline array string: `"ubyte * 2"` / `"ubyte*300"` — `count` elements of
//!   a fixed-width scalar, laid out contiguously.
//! * nested struct: `{"fields": [[name, type], ...], "count": N}` — `N`
//!   consecutive copies of a packed sub-struct.
//! * elem array: `{"elem_type": "ubyte * 2", "count": 32}` — 32 consecutive
//!   copies of a packed element (no per-element struct wrapper).
//!
//! The payload header (present when `payload_id` is not null) is:
//! `payload_id` (6 bits) | `nparams` (2 bits) | [`nbytes` (8 bits)] | fields.

use crate::wire::WireError;
use serde_json::Value;

/// A fixed-width scalar wire type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scalar {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
}

impl Scalar {
    /// Parse a scalar type name as it appears in the recovered format.
    pub fn parse(name: &str) -> Option<Scalar> {
        Some(match name.trim() {
            "ubyte" | "uint8" | "u8" => Scalar::U8,
            "byte" | "int8" | "i8" => Scalar::I8,
            "uint16" | "u16" | "ushort" => Scalar::U16,
            // The reference resolves type names via `ctypes.c_{name}`, so bare "short"
            // is c_short — *signed*. Grouping it with uint16 made negative values decode
            // as large positives.
            "short" | "int16" | "i16" => Scalar::I16,
            "uint32" | "u32" => Scalar::U32,
            "int32" | "i32" => Scalar::I32,
            _ => return None,
        })
    }

    /// Size in bytes of the full-width scalar.
    pub fn size(&self) -> usize {
        match self {
            Scalar::U8 | Scalar::I8 => 1,
            Scalar::U16 | Scalar::I16 => 2,
            Scalar::U32 | Scalar::I32 => 4,
        }
    }

    /// Number of usable bits (a bit-packed scalar uses fewer than its storage).
    pub fn bit_width(&self) -> u32 {
        match self {
            Scalar::U8 | Scalar::I8 => 8,
            Scalar::U16 | Scalar::I16 => 16,
            Scalar::U32 | Scalar::I32 => 32,
        }
    }
}

/// A single field within a payload or response struct.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Field {
    /// A scalar, optionally bit-packed (bit_width < full width).
    Scalar {
        name: String,
        ty: Scalar,
        bit_width: Option<u32>,
        /// Default value applied when a request omits this field (mirrors the
        /// `default` in the recovered format, e.g. `density` = 100).
        default: Option<i64>,
    },
    /// A contiguous array of fixed-width scalars: `"ubyte * 2"`.
    Array {
        name: String,
        elem: Scalar,
        count: usize,
    },
    /// A contiguous array of packed sub-structs: `{"fields":[...], "count": N}`.
    StructArray {
        name: String,
        fields: Vec<Field>,
        count: usize,
    },
    /// A contiguous array of packed elements: `{"elem_type": ..., "count": N}`.
    ElemArray {
        name: String,
        elem: Box<Field>,
        count: usize,
    },
}

impl Field {
    /// Total bit width of this field.
    ///
    /// Bit-packed scalars contribute their `bit_width`; full-width scalars
    /// contribute their storage width; arrays and structs contribute the total
    /// bit width of all their bytes.
    pub fn bit_len(&self) -> usize {
        match self {
            Field::Scalar { bit_width, ty, .. } => match bit_width {
                Some(w) => *w as usize,
                None => ty.size() * 8,
            },
            Field::Array { elem, count, .. } => elem.size() * count * 8,
            Field::StructArray { fields, count, .. } => {
                let bits: usize = fields.iter().map(Field::bit_len).sum();
                bits * count
            }
            Field::ElemArray { elem, count, .. } => elem.bit_len() * count,
        }
    }

    /// Total serialized size of this field, in bytes.
    ///
    /// Bit-packed scalars occupy `(bit_width + 7) / 8` bytes so a struct array
    /// of them packs correctly (e.g. five 1-4 bit fields sum to 8 bits = 1
    /// byte per element).
    pub fn size(&self) -> usize {
        match self {
            Field::Scalar { ty, bit_width, .. } => match bit_width {
                Some(w) => w.div_ceil(8) as usize,
                None => ty.size(),
            },
            Field::Array { elem, count, .. } => elem.size() * count,
            Field::StructArray { fields, count, .. } => {
                // A struct element packs its fields bit-by-bit, so its size is
                // the sum of each field's bit width divided into whole bytes,
                // not the sum of the fields' byte sizes. Five 1-4 bit fields
                // sum to 8 bits = 1 byte, not 5.
                let bits: usize = fields.iter().map(Field::bit_len).sum();
                let one = bits.div_ceil(8);
                one * count
            }
            Field::ElemArray { elem, count, .. } => elem.size() * count,
        }
    }

    /// Parse a field from the recovered format's field entry.
    ///
    /// Two entry shapes appear in the recovered data:
    /// * list form: `[name, "type"]`, `[name, "ubyte * N"]`, or a bit-packed
    ///   scalar `[name, "ubyte", 3]` (optionally followed by a default value,
    ///   `[name, "ubyte", 3, 100]`)
    /// * dict form: `{"name": ..., "type": ..., "bit_width": ..., "default": ...}`
    /// * dict-form array/struct: `[name, {"fields": [...], "count": N}]` or
    ///   `[name, {"elem_type": ..., "count": N}]`
    ///
    /// where `type` is a scalar string (`"ubyte"`), an inline array string
    /// (`"ubyte * 2"`), or a nested struct dict
    /// (`{"fields": [[n,t],...], "count": N}` / `{"elem_type": ..., "count": N}`).
    pub fn parse(entry: &serde_json::Value) -> Result<Field, WireError> {
        // Dict form: {"name": ..., "type": ..., "bit_width": ..., "default": ...}
        if let Some(obj) = entry.as_object() {
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let ty = obj
                .get("type")
                .ok_or(WireError::FieldOverflow)?;
            let bit_width = obj
                .get("bit_width")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            let default = obj
                .get("default")
                .and_then(default_as_i64);
            return Field::parse_type_spec(&name, ty, bit_width, default);
        }
        // List form: [name, type[, bit_width[, default]]]
        let name = entry[0].as_str().unwrap_or_default().to_string();
        let spec = &entry[1];
        let bit_width = entry.get(2).and_then(|v| v.as_u64()).map(|v| v as u32);
        let default = entry.get(3).and_then(default_as_i64);
        Field::parse_type_spec(&name, spec, bit_width, default)
    }

    /// Build a named field from its `type` spec.
    ///
    /// The spec may be a scalar/array string, or a dict with `fields` (nested
    /// struct) or `elem_type` (elem array). `name` may be empty when the spec
    /// is a bare type (e.g. a nested `elem_type` struct).
    fn parse_type_spec(
        name: &str,
        spec: &serde_json::Value,
        bit_width: Option<u32>,
        default: Option<i64>,
    ) -> Result<Field, WireError> {
        if let Some(s) = spec.as_str() {
            return Field::parse_scalar_string(name, s, bit_width, default);
        }
        if let Some(obj) = spec.as_object() {
            if let Some(fields) = obj.get("fields") {
                let count = obj.get("count").and_then(|c| c.as_u64()).unwrap_or(1) as usize;
                let mut fs = Vec::new();
                if let Some(arr) = fields.as_array() {
                    for f in arr {
                        fs.push(Field::parse(f)?);
                    }
                }
                return Ok(Field::StructArray {
                    name: name.to_string(),
                    fields: fs,
                    count,
                });
            }
            if let Some(elem) = obj.get("elem_type") {
                let count = obj.get("count").and_then(|c| c.as_u64()).unwrap_or(1) as usize;
                // `elem_type` may be a scalar/array string or a nested struct
                // dict; parse it recursively as a field.
                let f = Field::parse_type_spec("", elem, None, None)?;
                return Ok(Field::ElemArray {
                    name: name.to_string(),
                    elem: Box::new(f),
                    count,
                });
            }
        }
        Err(WireError::FieldOverflow)
    }

    fn parse_scalar_string(
        name: &str,
        s: &str,
        bit_width: Option<u32>,
        default: Option<i64>,
    ) -> Result<Field, WireError> {
        // The recovered format stores nested struct / elem-array types as a
        // JSON *string* inside the `type` field, e.g.
        //   "{\"fields\": [[\"in_periph_id\", \"uint8\"], ...], \"count\": 64}"
        // Try to parse that first; fall back to a plain scalar/array string.
        if let Ok(inner) = serde_json::from_str::<serde_json::Value>(s) {
            return Field::parse_type_spec(name, &inner, bit_width, default);
        }
        // Inline array, either spelling. The reference's `_parse_ctype` accepts both
        // "ubyte * 2" (type first) and "20 * int8" (count first); only the former was
        // handled here, so a count-first type failed registry loading outright.
        if s.contains('*') {
            let parts: Vec<&str> = s.split('*').map(str::trim).collect();
            if parts.len() == 2 {
                let (elem, count) = match (Scalar::parse(parts[0]), parts[0].parse::<usize>()) {
                    // "ubyte * 2"
                    (Some(e), _) => (
                        e,
                        parts[1].parse::<usize>().map_err(|_| WireError::FieldOverflow)?,
                    ),
                    // "20 * int8"
                    (None, Ok(n)) => (
                        Scalar::parse(parts[1]).ok_or(WireError::FieldOverflow)?,
                        n,
                    ),
                    (None, Err(_)) => return Err(WireError::FieldOverflow),
                };
                return Ok(Field::Array {
                    name: name.to_string(),
                    elem,
                    count,
                });
            }
        }
        let ty = Scalar::parse(s).ok_or(WireError::FieldOverflow)?;
        Ok(Field::Scalar {
            name: name.to_string(),
            ty,
            bit_width,
            default,
        })
    }
}

/// Read a JSON number as `i64`, accepting both signed and unsigned JSON values.
fn default_as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_u64().map(|x| x as i64))
}
