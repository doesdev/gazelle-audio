//! Effect parameter commands (`refs/schemas/afx_parameters.json`, generated from both panels'
//! bytecode by `refs/tools/scripts/afx_parameters.py`).
//!
//! Each effect type has its own set and get. The ground-truth suite checks every command's default
//! request against the panels' `Payload` logic; these tests pin what that suite cannot see: the
//! read's addressing as the panel sends it (the Quadro names one instance with `ext3` bit 31 and an
//! `id` byte; the Studio+ reads every instance), bytes for real values, and that the schemas and the
//! editor's catalogue describe the same commands.

use gazelle_audio_protocol::field::Field;
use gazelle_audio_protocol::payload::PayloadValues;
use gazelle_audio_protocol::registry::{from_json_doc, Registry};
use gazelle_audio_protocol::{Command, Value};
use serde_json::Value as Json;

fn registry(path: &str) -> Registry {
    let doc: Json = serde_json::from_str(&std::fs::read_to_string(path).expect("read schema")).expect("parse schema");
    from_json_doc(&doc).expect("load registry")
}

fn catalogue() -> Json {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../refs/schemas/afx_parameters.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("read afx_parameters.json")).expect("parse afx_parameters.json")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn a_quadro_effect_read_names_one_instance_in_ext3_bit_31_and_an_id_byte() {
    let reg = registry(gazelle_audio_protocol::QUADRO_COMMANDS_PATH);
    let get = reg.get("get_powergate_conf").expect("get_powergate_conf");
    let bytes = get.build_request(&PayloadValues::default().with_scalar("id", 2)).unwrap();
    // cmd 0x74, seq 17 (header + the id byte, no payload header), ext2 7, ext3 0x80000027 (PowerGate is 39).
    assert_eq!(hex(&bytes), "74000000110000000700000027000080".to_string() + "02");
}

#[test]
fn a_quadro_effect_set_carries_type_and_instance_then_every_parameter_in_description_order() {
    let reg = registry(gazelle_audio_protocol::QUADRO_COMMANDS_PATH);
    let set = reg.get("set_powergate_conf").expect("set_powergate_conf");
    let values = [("type_id", 39), ("inst_id", 2), ("threshold", 90), ("range", 3), ("attack", 100), ("decay", 50), ("hold", 700), ("gain", 232)]
        .iter()
        .fold(PayloadValues::default(), |v, (name, value)| v.with_scalar(name, *value));
    let bytes = set.build_request(&values).unwrap();
    // 11 parameter bytes: payload id 21 with nparams 3 (0xd5), nbytes 11, then the fields; seq 29.
    // The gain of -24 goes out as its byte, 0xe8, as the panel's ctypes uint8 does.
    assert_eq!(hex(&bytes), "700000001d0000000000000000000000d50b2702".to_string() + "5a0364003200bc02e8");
}

#[test]
fn a_studio_effect_read_takes_no_instance_and_answers_every_one() {
    let reg = registry(gazelle_audio_protocol::STUDIO_COMMANDS_PATH);
    let get = reg.get("get_powergate_configs").expect("get_powergate_configs");
    assert_eq!(hex(&get.build_request(&PayloadValues::default()).unwrap()), "74000000100000000700000027000000");
    match get.returns.as_slice() {
        [Field::StructArray { name, count, fields }] => {
            assert_eq!((name.as_str(), *count), ("entries", 16), "entry k is instance k");
            let names: Vec<&str> = fields.iter().map(Field::name).collect();
            assert_eq!(names, ["enabled", "threshold", "range", "attack", "decay", "hold", "gain", "linked"]);
        }
        other => panic!("reply layout {other:?}"),
    }
}

/// The value a reply field declares as its default, if any.
fn default_of(field: &Field) -> Option<i64> {
    match field {
        Field::Scalar { default, .. } => *default,
        _ => None,
    }
}

/// A parameter's default as the device holds it: a negative value in an unsigned field is its two's complement.
fn wire_default(parameter: &Json) -> Option<i64> {
    let value = parameter["default"].as_i64()?;
    let bits = match parameter["wire"].as_str().unwrap() {
        "u8" | "i8" => 8,
        "u16" | "i16" => 16,
        _ => 32,
    };
    Some(if value < 0 && parameter["wire"].as_str().unwrap().starts_with('u') { value + (1 << bits) } else { value })
}

#[test]
fn every_catalogued_effect_has_its_set_and_get_in_the_schema_as_the_catalogue_describes() {
    let doc = catalogue();
    for (family, path) in [("quadro", gazelle_audio_protocol::QUADRO_COMMANDS_PATH), ("studio", gazelle_audio_protocol::STUDIO_COMMANDS_PATH)] {
        let reg = registry(path);
        let effects = doc[family]["effects"].as_array().expect("effects");
        assert!(effects.len() > 30, "{family}: {} effects", effects.len());
        for effect in effects {
            let name = effect["name"].as_str().unwrap();
            let parameters = effect["parameters"].as_array().unwrap();
            let set = reg.get(effect["set"].as_str().unwrap()).unwrap_or_else(|| panic!("{family} {name}: no {}", effect["set"]));
            let set_names: Vec<&str> = set.params.iter().map(Field::name).collect();
            let mut expected = vec!["type_id", "inst_id"];
            expected.extend(parameters.iter().map(|p| p["name"].as_str().unwrap()));
            assert_eq!(set_names, expected, "{family} {name}: set fields");
            assert_eq!((set.report_id, set.ext2, set.ext3), (0x70, 0, 0), "{family} {name}: set header");

            let get: &Command = reg.get(effect["get"].as_str().unwrap()).unwrap_or_else(|| panic!("{family} {name}: no {}", effect["get"]));
            assert_eq!((get.report_id, get.ext2, u64::from(get.ext3)), (0x74, 7, effect["get_ext3"].as_u64().unwrap()), "{family} {name}: get header");
            let instance: Vec<&str> = get.params.iter().map(Field::name).collect();
            let expected_instance: Vec<&str> = effect.get("instance_param").and_then(Json::as_str).into_iter().collect();
            assert_eq!(instance, expected_instance, "{family} {name}: read parameters");
            let [Field::StructArray { name: entries, count, fields }] = get.returns.as_slice() else {
                panic!("{family} {name}: reply is not a list of entries");
            };
            assert_eq!((entries.as_str(), *count as u64), ("entries", effect["reply_count"].as_u64().unwrap()), "{family} {name}: reply count");
            assert_eq!((fields[0].name(), default_of(&fields[0])), ("enabled", Some(1)), "{family} {name}: an effect starts processing");
            for parameter in parameters {
                let field = fields.iter().find(|f| f.name() == parameter["name"].as_str().unwrap()).unwrap_or_else(|| panic!("{family} {name}: reply lacks {}", parameter["name"]));
                assert_eq!(default_of(field), wire_default(parameter), "{family} {name}.{}: reply default", parameter["name"]);
            }
        }
    }
}

/// The Guitar Amp's controls depend on its model (`GuitarAmp.model_classes`, one view per model, each with
/// its own knobs and switches). The catalogue says which parameters each model the panel offers shows,
/// and every other parameter goes back as read.
#[test]
fn the_guitar_amp_lists_each_offered_models_controls_from_its_parameters() {
    let doc = catalogue();
    for family in ["quadro", "studio"] {
        let amp = doc[family]["effects"].as_array().unwrap().iter().find(|e| e["type"] == 3).expect("the Guitar Amp");
        let parameters: Vec<&str> = amp["parameters"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        let layouts = &amp["layouts"];
        assert_eq!(layouts["by"], "model", "{family}: the model picks the layout");
        let model = amp["parameters"].as_array().unwrap().iter().find(|p| p["name"] == "model").unwrap();
        let offered: Vec<String> = model["options"].as_array().unwrap().iter().map(|o| o[0].to_string()).collect();
        let models = layouts["models"].as_object().expect("layouts by model id");
        let mut ids: Vec<&String> = models.keys().collect();
        ids.sort_by_key(|id| id.parse::<u32>().unwrap());
        assert_eq!(ids, offered.iter().collect::<Vec<_>>(), "{family}: a layout for every model the menu offers, and no other");
        for (id, controls) in models {
            for control in controls.as_array().unwrap() {
                let name = control["name"].as_str().unwrap();
                assert!(parameters.contains(&name), "{family} model {id}: {name} is not a parameter");
                assert!(!["model", "level"].contains(&name), "{family} model {id}: the model menu and level are every model's");
            }
        }
        assert_eq!(amp["status"], "full", "{family}: nothing is left without a control on the model that uses it");
    }
    // Darkface 65: four knobs and its bright switch; Tweed Deluxe: two knobs and one switch.
    let quadro = doc["quadro"]["effects"].as_array().unwrap().iter().find(|e| e["type"] == 3).unwrap();
    let names = |id: &str| -> Vec<String> { quadro["layouts"]["models"][id].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect() };
    assert_eq!(names("0"), ["bass", "mid", "treble", "volume", "mode1"]);
    assert_eq!(names("7"), ["mid", "volume", "mode2"]);
}

#[test]
fn a_reply_decodes_per_instance_with_signed_fields_signed() {
    let reg = registry(gazelle_audio_protocol::STUDIO_COMMANDS_PATH);
    let get = reg.get("get_powergate_configs").unwrap();
    let mut contents = Vec::new();
    for inst in 0..16u8 {
        // enabled, threshold, range, attack u16, decay u16, hold u16, gain i8, linked
        contents.extend_from_slice(&[u8::from(inst != 5), 90 + inst, 0, 100, 0, 50, 0, 0, 0, 0xe8, 0]);
    }
    let decoded = Command::parse_field_list(&get.returns, &contents).unwrap();
    let Some(Value::List(entries)) = decoded.get("entries") else { panic!("{decoded:?}") };
    let Value::Struct(fifth) = &entries[5] else { panic!() };
    assert!(matches!(fifth.get("enabled"), Some(Value::U64(0))), "{fifth:?}");
    assert!(matches!(fifth.get("threshold"), Some(Value::U64(95))), "{fifth:?}");
    assert!(matches!(fifth.get("gain"), Some(Value::I64(-24))), "the Studio+ declares gain int8: {fifth:?}");
}
