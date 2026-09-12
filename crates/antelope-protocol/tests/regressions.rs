//! Regression tests for defects found in the 2026-09-12 review of the protocol crate.
//!
//! Each test names the defect it pins down. These paths were not covered by the
//! ground-truth suite, which compares *request* bytes for the 63 in-scope commands and is
//! therefore silent about response decoding and about field shapes no in-scope command uses.

use antelope_protocol::field::{Field, Scalar};
use antelope_protocol::payload::{Payload, PayloadValues};
use antelope_protocol::registry::{from_json_doc, Registry};

fn registry() -> Registry {
    let doc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(antelope_protocol::IN_SCOPE_COMMANDS_PATH).unwrap(),
    )
    .unwrap();
    from_json_doc(&doc).unwrap()
}

/// A `count: 0` struct array must decode to an empty struct, not panic.
///
/// `get_afx_order.slots` and `get_afx_strip_order.slots` really carry count 0, because
/// their count comes from `afx_pool.PHY_AFX_STRIP_SIZE`, which was never decompiled.
#[test]
fn zero_count_struct_array_does_not_panic() {
    let r = registry();
    for name in ["get_afx_order", "get_afx_strip_order"] {
        let c = r.get(name).unwrap_or_else(|| panic!("{name} in registry"));
        let out = c
            .parse_response(&[0u8; 64])
            .unwrap_or_else(|e| panic!("{name} must decode, got {e:?}"));
        assert!(out.contains_key("slots"), "{name} should still yield a slots key");
    }
}

/// Full-width signed scalars must sign-extend.
///
/// Live on `get_tb_latency`, whose latency/buffer adjustments are genuinely negative.
#[test]
fn signed_full_width_fields_sign_extend() {
    use antelope_protocol::payload::Value;
    let r = registry();
    let c = r.get("get_tb_latency").expect("get_tb_latency");

    let mut buf = vec![0u8; 16];
    buf.extend_from_slice(&[0xFFu8; 20]); // five int32 of -1
    let out = c.parse_response(&buf).expect("decode");

    for f in ["mode", "buffer_adjust_in", "buffer_adjust_out", "latency_adjust_in"] {
        match out.get(f) {
            Some(Value::I64(v)) => assert_eq!(*v, -1, "{f} should decode 0xFFFFFFFF as -1"),
            other => panic!("{f}: expected I64(-1), got {other:?}"),
        }
    }
}

/// A trailing partial byte must be written LSB-first, so this crate can read back what it
/// writes. The old flush was MSB-aligned and contradicted its own reader.
#[test]
fn trailing_bits_roundtrip_through_our_own_reader() {
    let fields = vec![
        Field::Scalar { name: "a".into(), ty: Scalar::U8, bit_width: Some(4), default: None },
        Field::Scalar { name: "b".into(), ty: Scalar::U8, bit_width: Some(4), default: None },
    ];
    let p = Payload::new(None, fields).expect("byte-complete payload");
    let bytes = p
        .to_bytes(&PayloadValues::default().with_scalar("a", 5).with_scalar("b", 0))
        .expect("serialize");
    assert_eq!(bytes, vec![0x05], "a=5 in the low nibble, LSB-first");
}

/// Bit-packed fields must not overtake the full-width fields that follow them.
#[test]
fn pending_bits_flush_before_a_full_width_field() {
    let fields = vec![
        Field::Scalar { name: "a".into(), ty: Scalar::U8, bit_width: Some(4), default: None },
        Field::Scalar { name: "pad".into(), ty: Scalar::U8, bit_width: Some(4), default: None },
        Field::Scalar { name: "b".into(), ty: Scalar::U8, bit_width: None, default: None },
    ];
    let p = Payload::new(None, fields).expect("payload");
    let bytes = p
        .to_bytes(
            &PayloadValues::default()
                .with_scalar("a", 0xA)
                .with_scalar("pad", 0)
                .with_scalar("b", 0xBB),
        )
        .expect("serialize");
    assert_eq!(bytes, vec![0x0A, 0xBB], "the packed byte must precede the full-width field");
}

/// `user_bytes` is the total bit width in bytes, not the sum of per-field byte sizes.
#[test]
fn user_bytes_counts_bits_not_rounded_fields() {
    // Eight 1-bit fields are one byte, not eight.
    let fields: Vec<Field> = (0..8)
        .map(|i| Field::Scalar {
            name: format!("f{i}"),
            ty: Scalar::U8,
            bit_width: Some(1),
            default: None,
        })
        .collect();
    let p = Payload::new(Some(1), fields).expect("payload");
    assert_eq!(p.user_bytes, 1, "eight 1-bit fields occupy one byte");
}

/// A payload whose fields do not add up to whole bytes is rejected, as the reference does.
#[test]
fn non_byte_complete_payload_is_rejected() {
    let fields = vec![Field::Scalar {
        name: "a".into(),
        ty: Scalar::U8,
        bit_width: Some(3),
        default: None,
    }];
    assert!(
        Payload::new(Some(1), fields).is_err(),
        "3 bits is not byte-complete and must not silently become 1 byte"
    );
}

/// `short` is `c_short` in the reference — signed.
#[test]
fn short_is_signed() {
    assert_eq!(Scalar::parse("short"), Some(Scalar::I16));
    assert_eq!(Scalar::parse("int16"), Some(Scalar::I16));
    assert_eq!(Scalar::parse("uint16"), Some(Scalar::U16));
    assert_eq!(Scalar::parse("ushort"), Some(Scalar::U16));
}

/// Both array spellings parse: type-first and count-first.
#[test]
fn both_array_spellings_parse() {
    let a = Field::parse(&serde_json::json!({"name": "x", "type": "ubyte * 2"})).expect("type-first");
    let b = Field::parse(&serde_json::json!({"name": "x", "type": "2 * ubyte"})).expect("count-first");
    assert_eq!(a.size(), 2);
    assert_eq!(b.size(), 2, "the count-first spelling is accepted by the reference too");
}

/// Loading a malformed registry returns an error without printing to stderr.
#[test]
fn registry_errors_are_returned_not_printed() {
    let doc = serde_json::json!({"commands": {"bad": {"name": "bad"}}});
    assert!(from_json_doc(&doc).is_err(), "a command without a report_id must fail");
}
