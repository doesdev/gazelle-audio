//! Reads the panels declare with a top-level `count` reply with that many elements: the manager
//! builds the reply as `Field("contents", returns)`, a ctypes array when counted, and hands the
//! panel a list (bytecode research, 2026-09). The extractor used to drop the count, which made
//! `get_mixer` one strip instead of 33 and every links read one pair.

use gazelle_audio_protocol::payload::Value;
use gazelle_audio_protocol::registry::from_json_doc;
use serde_json::Value as Json;

fn schema(path: &str) -> Json {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap()
}

#[test]
fn counted_replies_are_lists_of_that_many_entries() {
    let families = [
        ("quadro", gazelle_audio_protocol::QUADRO_COMMANDS_PATH, vec![("get_mixer", 33, 2), ("get_mixer_links", 64, 1), ("get_preamps_links", 1, 1), ("get_adats_links", 8, 1), ("get_spdifs_links", 1, 1)]),
        ("studio", gazelle_audio_protocol::STUDIO_COMMANDS_PATH, vec![("get_mixer", 33, 3), ("get_mixer_links", 64, 1), ("get_preamps_links", 6, 1), ("get_lines_links", 4, 1), ("get_adats_links", 8, 1), ("get_spdifs_links", 1, 1)]),
    ];
    for (family, path, reads) in families {
        let doc = schema(path);
        for (name, count, element_bytes) in reads {
            let returns = doc["commands"][name]["returns"].as_array().unwrap_or_else(|| panic!("{family} {name} has returns"));
            assert_eq!(returns.len(), 1, "{family} {name}: one list field, got {returns:?}");
            assert_eq!(returns[0]["name"], "entries", "{family} {name}");
            let spec: Json = serde_json::from_str(returns[0]["type"].as_str().unwrap()).unwrap();
            assert_eq!(spec["count"], count, "{family} {name}: entry count");
            assert_eq!(returns[0]["size"], count * element_bytes, "{family} {name}: reply bytes");
        }
    }
}

#[test]
fn a_quadro_get_mixer_reply_decodes_to_33_strips_master_first() {
    let registry = from_json_doc(&schema(gazelle_audio_protocol::QUADRO_COMMANDS_PATH)).unwrap();
    let command = registry.get("get_mixer").unwrap();
    let mut reply = vec![0u8; 16];
    for strip in 0..33u8 {
        // level, then pan (bits 0-5) | mute << 6 | solo << 7
        let pan = strip % 60 + 2;
        let byte = pan | if strip == 32 { 1 << 6 } else { 0 } | if strip == 1 { 1 << 7 } else { 0 };
        reply.extend_from_slice(&[strip, byte]);
    }
    let fields = command.parse_response(&reply).expect("decode");
    let Some(Value::List(entries)) = fields.get("entries") else { panic!("entries: {fields:?}") };
    assert_eq!(entries.len(), 33);
    let field = |i: usize, name: &str| match &entries[i] {
        Value::Struct(s) => match s.get(name) {
            Some(Value::U64(v)) => *v,
            other => panic!("entry {i} {name}: {other:?}"),
        },
        other => panic!("entry {i}: {other:?}"),
    };
    assert_eq!((field(0, "level"), field(0, "pan")), (0, 2), "entry 0 is the master");
    assert_eq!((field(1, "solo"), field(1, "mute")), (1, 0));
    assert_eq!((field(32, "level"), field(32, "pan"), field(32, "mute"), field(32, "solo")), (32, 34, 1, 0));
}
