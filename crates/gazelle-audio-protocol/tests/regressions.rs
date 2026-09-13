//! Regression tests for defects found in the 2026-09-12 review of the protocol crate.
//!
//! Each test names the defect it pins down. These paths were not covered by the
//! ground-truth suite, which compares *request* bytes for the 63 in-scope commands and is
//! therefore silent about response decoding and about field shapes no in-scope command uses.

use gazelle_audio_protocol::field::{Field, Scalar};
use gazelle_audio_protocol::payload::{Payload, PayloadValues};
use gazelle_audio_protocol::registry::{from_json_doc, Registry};

fn registry() -> Registry {
    let doc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(gazelle_audio_protocol::QUADRO_COMMANDS_PATH).unwrap(),
    )
    .unwrap();
    from_json_doc(&doc).unwrap()
}

/// A `count: 0` struct array must decode to an empty list, not panic.
///
/// No in-scope command currently declares `count: 0` in either schema — `get_afx_order.slots`
/// and `get_afx_strip_order.slots` were once thought to (their count was assumed to come from
/// `afx_pool.PHY_AFX_STRIP_SIZE`, which was never decompiled), but both schemas actually carry
/// count 8 for them, and have since the first commit. The decoder's count-0 branch still
/// exists, so this test exercises it directly with a synthetic field list instead of relying
/// on a real command.
#[test]
fn zero_count_struct_array_does_not_panic() {
    use gazelle_audio_protocol::payload::Value;
    use gazelle_audio_protocol::Command;

    let fields = vec![Field::StructArray {
        name: "slots".into(),
        fields: vec![
            Field::Scalar { name: "type".into(), ty: Scalar::U8, bit_width: None, default: None },
            Field::Scalar { name: "inst".into(), ty: Scalar::U8, bit_width: None, default: None },
        ],
        count: 0,
    }];

    let out = Command::parse_field_list(&fields, &[])
        .unwrap_or_else(|e| panic!("count-0 struct array must decode, got {e:?}"));
    match out.get("slots") {
        Some(Value::List(items)) => {
            assert!(items.is_empty(), "count 0 must decode to an empty list")
        }
        other => panic!("expected an empty list, got {other:?}"),
    }
}

/// Every element of a struct array must decode, not just the first.
///
/// `get_routing` returns 64 routing slots; the decoder used to return slot 0 only, and the
/// cyclic ground truth read element 0 only, so both sides agreed on the wrong answer.
#[test]
fn struct_arrays_decode_every_element() {
    use gazelle_audio_protocol::payload::Value;
    let r = registry();
    let c = r.get("get_routing").expect("get_routing");

    let mut buf = vec![0u8; 16];
    buf.push(1); // bank_idx
    for i in 0..64u8 {
        buf.extend_from_slice(&[i, i.wrapping_mul(2)]);
    }
    let out = c.parse_response(&buf).expect("decode");

    let items = match out.get("bank_configs") {
        Some(Value::List(items)) => items,
        other => panic!("bank_configs: expected a list, got {other:?}"),
    };
    assert_eq!(items.len(), 64);
    match &items[63] {
        Value::Struct(s) => {
            assert!(matches!(s.get("in_periph_id"), Some(Value::U64(63))));
            assert!(matches!(s.get("in_chann"), Some(Value::U64(126))));
        }
        other => panic!("element 63: expected struct, got {other:?}"),
    }
}

/// Full-width signed scalars must sign-extend.
///
/// Live on `get_tb_latency`, whose latency/buffer adjustments are genuinely negative.
#[test]
fn signed_full_width_fields_sign_extend() {
    use gazelle_audio_protocol::payload::Value;
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

/// Sub-byte fields declared with the 3-tuple form must pack into shared bytes.
///
/// `set_mixer` declares `pan`(6 bits), `mute`(1) and `solo`(1) — one byte between them,
/// not three. The extractor previously read a bit width only from the 4-element field
/// form, so the 3-element form silently lost it and every sub-byte field widened to a
/// full byte. That made the command's wire layout wrong and, because no field in the
/// registry then had a bit width, left the entire bit-packing path untested.
#[test]
fn set_mixer_packs_pan_mute_solo_into_one_byte() {
    use gazelle_audio_protocol::payload::PayloadValues;
    let r = registry();
    let c = r.get("set_mixer").expect("set_mixer");

    // mixer_id, channel, level are whole bytes; pan/mute/solo share the fourth.
    let bytes = c
        .build_request(
            &PayloadValues::default()
                .with_scalar("mixer_id", 0)
                .with_scalar("channel", 1)
                .with_scalar("level", 0x40)
                .with_scalar("pan", 0b10_1010)
                .with_scalar("mute", 1)
                .with_scalar("solo", 0),
        )
        .expect("build");

    // 16-byte header, then payload_id|nparams, nbytes, then 4 user bytes.
    let body = &bytes[16..];
    assert_eq!(body[1], 4, "user payload is 4 bytes, not 6");
    let packed = body[5];
    assert_eq!(packed & 0b0011_1111, 0b10_1010, "pan occupies the low 6 bits");
    assert_eq!((packed >> 6) & 1, 1, "mute is bit 6");
    assert_eq!((packed >> 7) & 1, 0, "solo is bit 7");
}

/// A command whose fields are all sub-byte collapses to a single-byte payload.
#[test]
fn set_sine_gen_is_one_user_byte() {
    let r = registry();
    let c = r.get("set_sine_gen").expect("set_sine_gen");
    let bytes = c.build_request(&Default::default()).expect("build");
    // 2+2+2+1+1 = 8 bits = 1 byte, so the short-payload layout applies:
    // one header byte, no nbytes byte.
    assert_eq!(bytes.len(), 16 + 2, "header + packed byte + one user byte");
}
