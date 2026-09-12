//! Conversion between protocol [`Value`]s and JSON.
//!
//! The mapping is deliberately symmetric: whatever shape a field decodes to is the shape
//! accepted when setting it.
//!
//! | Protocol | JSON |
//! |---|---|
//! | `U64` / `I64` | number |
//! | `Bytes` | lowercase hex **string** |
//! | `Struct` | object |
//!
//! Byte arrays are hex rather than arrays of numbers because in-scope commands carry arrays
//! up to 300 bytes (`get_sonarworks_ir`), where a JSON number array is both unreadable and
//! several times larger.

use antelope_protocol::payload::{PayloadValues, Value};
use serde_json::{Map, Value as Json};
use std::collections::HashMap;

use crate::error::ServerError;

/// Render a protocol value as JSON.
pub fn value_to_json(v: &Value) -> Json {
    match v {
        Value::U64(n) => Json::from(*n),
        Value::I64(n) => Json::from(*n),
        Value::Bytes(b) => Json::String(to_hex(b)),
        Value::Struct(fields) => {
            let mut m = Map::new();
            for (k, fv) in fields {
                m.insert(k.clone(), value_to_json(fv));
            }
            Json::Object(m)
        }
    }
}

/// Render a decoded report as a JSON object.
pub fn fields_to_json(fields: &HashMap<String, Value>) -> Json {
    let mut m = Map::new();
    for (k, v) in fields {
        m.insert(k.clone(), value_to_json(v));
    }
    Json::Object(m)
}

/// Parse a single JSON value into a protocol value.
///
/// Numbers become `U64` when non-negative and `I64` otherwise, so a signed field given a
/// positive number still round-trips. Strings are treated as hex byte arrays.
pub fn json_to_value(name: &str, j: &Json) -> Result<Value, ServerError> {
    match j {
        Json::Number(n) => {
            if let Some(u) = n.as_u64() {
                Ok(Value::U64(u))
            } else if let Some(i) = n.as_i64() {
                Ok(Value::I64(i))
            } else {
                Err(ServerError::BadValue(format!(
                    "field '{name}': {n} is not an integer; protocol fields are integral"
                )))
            }
        }
        Json::Bool(b) => Ok(Value::U64(u64::from(*b))),
        Json::String(s) => from_hex(s)
            .map(Value::Bytes)
            .map_err(|e| ServerError::BadValue(format!("field '{name}': {e}"))),
        Json::Object(map) => {
            let mut fields = HashMap::new();
            for (k, v) in map {
                fields.insert(k.clone(), json_to_value(k, v)?);
            }
            Ok(Value::Struct(fields))
        }
        Json::Array(items) => {
            // An array of small integers is accepted as a byte array, so clients that
            // prefer arrays to hex are not locked out.
            let mut bytes = Vec::with_capacity(items.len());
            for (i, it) in items.iter().enumerate() {
                let n = it.as_u64().ok_or_else(|| {
                    ServerError::BadValue(format!(
                        "field '{name}'[{i}]: arrays must contain byte values 0-255"
                    ))
                })?;
                if n > 255 {
                    return Err(ServerError::BadValue(format!(
                        "field '{name}'[{i}]: {n} exceeds 255"
                    )));
                }
                bytes.push(n as u8);
            }
            Ok(Value::Bytes(bytes))
        }
        Json::Null => Err(ServerError::BadValue(format!(
            "field '{name}': null is not a value; omit the field to use its default"
        ))),
    }
}

/// Build [`PayloadValues`] from a JSON object of field values.
///
/// Omitted fields are left out entirely so the protocol layer applies each field's declared
/// default, then zero.
pub fn json_to_payload_values(body: &Json) -> Result<PayloadValues, ServerError> {
    let map = match body {
        Json::Object(m) => m,
        Json::Null => return Ok(PayloadValues::default()),
        _ => {
            return Err(ServerError::BadValue(
                "request body must be a JSON object of field values".into(),
            ))
        }
    };
    let mut values = HashMap::new();
    for (k, v) in map {
        values.insert(k.clone(), json_to_value(k, v)?);
    }
    Ok(PayloadValues::new(values))
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    let t = s.strip_prefix("0x").unwrap_or(s);
    // `%` rather than `is_multiple_of`, which is only stable since 1.87 and would
    // raise this crate's MSRV for no benefit.
    if t.len() % 2 != 0 {
        return Err(format!("hex string has odd length ({})", t.len()));
    }
    (0..t.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&t[i..i + 2], 16)
                .map_err(|_| format!("'{}' is not valid hex", &t[i..i + 2]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrips() {
        let b = vec![0x00, 0x4f, 0xd4, 0xff];
        assert_eq!(to_hex(&b), "004fd4ff");
        assert_eq!(from_hex("004fd4ff").unwrap(), b);
        assert_eq!(from_hex("0x004fd4ff").unwrap(), b);
    }

    #[test]
    fn hex_rejects_malformed() {
        assert!(from_hex("abc").is_err());
        assert!(from_hex("zz").is_err());
    }

    #[test]
    fn scalars_map_both_ways() {
        assert_eq!(value_to_json(&Value::U64(64)), Json::from(64));
        assert_eq!(value_to_json(&Value::I64(-3)), Json::from(-3));
        assert!(matches!(
            json_to_value("x", &Json::from(64)).unwrap(),
            Value::U64(64)
        ));
        assert!(matches!(
            json_to_value("x", &Json::from(-3)).unwrap(),
            Value::I64(-3)
        ));
    }

    #[test]
    fn bytes_are_hex_strings() {
        assert_eq!(
            value_to_json(&Value::Bytes(vec![1, 2, 255])),
            Json::String("0102ff".into())
        );
        match json_to_value("x", &Json::String("0102ff".into())).unwrap() {
            Value::Bytes(b) => assert_eq!(b, vec![1, 2, 255]),
            other => panic!("expected bytes, got {other:?}"),
        }
    }

    #[test]
    fn byte_arrays_are_also_accepted() {
        match json_to_value("x", &serde_json::json!([1, 2, 255])).unwrap() {
            Value::Bytes(b) => assert_eq!(b, vec![1, 2, 255]),
            other => panic!("expected bytes, got {other:?}"),
        }
        assert!(json_to_value("x", &serde_json::json!([256])).is_err());
    }

    #[test]
    fn null_is_rejected_with_guidance() {
        let e = json_to_value("lvl", &Json::Null).unwrap_err();
        assert!(e.to_string().contains("omit the field"), "{e}");
    }

    #[test]
    fn omitted_fields_yield_empty_values() {
        let v = json_to_payload_values(&serde_json::json!({})).unwrap();
        // Nothing set: the protocol layer will apply declared defaults, then zero.
        assert!(v.get_bytes("anything").is_err());
    }
}
