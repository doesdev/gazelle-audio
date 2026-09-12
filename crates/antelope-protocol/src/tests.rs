//! Ground-truth tests: request bytes are asserted against the output of the
//! real decompiled `Payload`/`Request` classes in
//! `refs/decompiled/manager/antelope_dev_reports.py`.

use super::*;
use crate::field::Field;
use crate::payload::{PayloadValues, Value};
use std::collections::HashMap;

fn pv(pairs: Vec<(&str, Value)>) -> PayloadValues {
    let mut h: HashMap<String, Value> = HashMap::new();
    for (k, v) in pairs {
        h.insert(k.to_string(), v);
    }
    PayloadValues::new(h)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn scalar(name: &str, _v: u64) -> Field {
    Field::Scalar {
        name: name.to_string(),
        ty: Scalar::U8,
        bit_width: None,
        default: None,
    }
}

#[test]
fn set_mixer_matches_reference() {
    // Reference (py38): 70000000180000000000000000000000d406000102030405
    let cmd = Command {
        name: "set_mixer".into(),
        report_id: 0x70,
        ext2: 0,
        ext3: 0,
        payload_id: Some(20),
        params: vec![
            scalar("mixer_id", 0),
            scalar("channel", 0),
            scalar("level", 0),
            scalar("pan", 0),
            scalar("mute", 0),
            scalar("solo", 0),
        ],
        returns: vec![],
        auto_send_notification: false,
    };
    let v = pv(vec![
        ("mixer_id", Value::U64(0)),
        ("channel", Value::U64(1)),
        ("level", Value::U64(2)),
        ("pan", Value::U64(3)),
        ("mute", Value::U64(4)),
        ("solo", Value::U64(5)),
    ]);
    let out = cmd.build_request(&v).unwrap();
    assert_eq!(hex(&out), "70000000180000000000000000000000d406000102030405");
}

#[test]
fn set_pre_type_no_nbytes_byte() {
    // Reference (py38): 700000001300000000000000000000004f0703
    // 3 user bytes (< 4) => no nbytes byte; nparams = bytesize - 1 = 2.
    let cmd = Command {
        name: "set_pre_type".into(),
        report_id: 0x70,
        ext2: 0,
        ext3: 0,
        payload_id: Some(15),
        params: vec![scalar("id", 0), scalar("pretype", 0)],
        returns: vec![],
        auto_send_notification: false,
    };
    let v = pv(vec![("id", Value::U64(7)), ("pretype", Value::U64(3))]);
    let out = cmd.build_request(&v).unwrap();
    assert_eq!(hex(&out), "700000001300000000000000000000004f0703");
}

#[test]
fn set_tb_latency_no_payload_id() {
    // Reference (py38): e000000024000000 + 20 zero bytes.
    let cmd = Command {
        name: "set_tb_latency".into(),
        report_id: 0xE0,
        ext2: 0,
        ext3: 0,
        payload_id: None,
        params: vec![
            Field::Scalar { name: "mode".into(), ty: Scalar::I32, bit_width: None, default: None },
            Field::Scalar { name: "buffer_adjust_in".into(), ty: Scalar::I32, bit_width: None, default: None },
            Field::Scalar { name: "buffer_adjust_out".into(), ty: Scalar::I32, bit_width: None, default: None },
            Field::Scalar { name: "latency_adjust_in".into(), ty: Scalar::I32, bit_width: None, default: None },
            Field::Scalar { name: "latency_adjust_out".into(), ty: Scalar::I32, bit_width: None, default: None },
        ],
        returns: vec![],
        auto_send_notification: false,
    };
    let v = PayloadValues::new(HashMap::from([
        ("mode".into(), Value::I64(1)),
        ("buffer_adjust_in".into(), Value::I64(2)),
        ("buffer_adjust_out".into(), Value::I64(3)),
        ("latency_adjust_in".into(), Value::I64(4)),
        ("latency_adjust_out".into(), Value::I64(5)),
    ]));
    let out = cmd.build_request(&v).unwrap();
    assert_eq!(hex(&out), "e00000002400000000000000000000000100000002000000030000000400000005000000");
}

#[test]
fn header_roundtrip() {
    let h = Header::new(0x70, 24, 0, 0);
    let bytes = h.to_bytes();
    assert_eq!(hex(&bytes), "70000000180000000000000000000000");
    let back = Header::from_bytes(&bytes).unwrap();
    assert_eq!(back, h);
}

#[test]
fn field_sizes() {
    let arr = Field::parse(&serde_json::json!(["bank_configs", "ubyte * 2"])).unwrap();
    assert_eq!(arr.size(), 2);
    let big = Field::parse(&serde_json::json!(["pkg_data", "ubyte*300"])).unwrap();
    assert_eq!(big.size(), 300);
    let elem = Field::parse(&serde_json::json!(["bank_configs", {"elem_type": "ubyte * 2", "count": 32}])).unwrap();
    assert_eq!(elem.size(), 64);
    let nested = Field::parse(&serde_json::json!(["bank_configs", {"fields": [["in_periph_id", "uint8"], ["in_chann", "uint8"]], "count": 64}])).unwrap();
    assert_eq!(nested.size(), 128);
}

#[test]
fn payload_id_nparams_packing() {
    // payload_id=20 (0b010100), nparams=3 (0b11) => 0b11010100 = 0xd4
    let cmd = Command {
        name: "set_mixer".into(),
        report_id: 0x70,
        ext2: 0,
        ext3: 0,
        payload_id: Some(20),
        params: vec![scalar("mixer_id", 0), scalar("channel", 0),
                     scalar("level", 0), scalar("pan", 0),
                     scalar("mute", 0), scalar("solo", 0)],
        returns: vec![],
        auto_send_notification: false,
    };
    let v = pv(vec![
        ("mixer_id", Value::U64(0)),
        ("channel", Value::U64(0)),
        ("level", Value::U64(0)),
        ("pan", Value::U64(0)),
        ("mute", Value::U64(0)),
        ("solo", Value::U64(0)),
    ]);
    let out = cmd.build_request(&v).unwrap();
    // byte 16 = payload_id|nparams, byte 17 = nbytes
    assert_eq!(out[16], 0xd4);
    assert_eq!(out[17], 6);
}
