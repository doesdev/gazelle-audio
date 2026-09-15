//! The extracted topology (`refs/schemas/*_topology.json`, written by
//! `refs/tools/scripts/extract_topology.py` from the panels' bytecode) must agree with the command
//! schemas the mixer will drive, and must say where every value came from.

use serde_json::Value;

const QUADRO_TOPOLOGY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../refs/schemas/quadro_topology.json");
const STUDIO_TOPOLOGY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../refs/schemas/studio_topology.json");

fn load(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap()
}

fn families() -> [(&'static str, Value, Value, &'static str, &'static str); 2] {
    [
        ("quadro", load(QUADRO_TOPOLOGY), load(gazelle_audio_protocol::QUADRO_COMMANDS_PATH), "set_mixer", "3.8"),
        ("studio", load(STUDIO_TOPOLOGY), load(gazelle_audio_protocol::STUDIO_COMMANDS_PATH), "set_mixer_cfg", "3.5"),
    ]
}

#[test]
fn mixers_agree_with_the_command_schema() {
    for (family, topology, commands, command, _) in families() {
        let mixers = &topology["mixers"];
        assert_eq!(topology["family"], family);
        assert_eq!(mixers["command"], command, "{family}");
        let params: Vec<&str> = commands["commands"][command]["params"].as_array().unwrap_or_else(|| panic!("{family} has no {command}")).iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(&params[..3], &["mixer_id", "channel", "level"], "{family} {command}");

        let channels = mixers["channels"].as_u64().unwrap();
        let count = mixers["count"].as_u64().unwrap();
        assert_eq!((count, channels), (4, 32), "{family}: four mixers of 32 channels, as the panels build them");
        assert!(channels < 256, "master plus channels fit the channel byte");

        let peaks = commands["cyclic_reports"]["0x73"]["fields"].as_array().unwrap().iter().find(|f| f["name"] == "peaks_mixer").unwrap_or_else(|| panic!("{family} 0x73 has peaks_mixer"));
        assert_eq!(peaks["size"].as_u64(), Some(channels), "{family}: one peak byte per mixer channel");

        let ids = |groups: &Value, kind: &str| -> Vec<String> { groups.as_array().unwrap().iter().filter(|g| g["type"] == kind).map(|g| g["id"].as_str().unwrap().to_string()).collect() };
        let mixer_inputs = ids(&topology["outputs"], "MIXER_IN");
        let mixer_outputs = ids(&topology["inputs"], "MIXER_OUT");
        assert_eq!(mixers["inputGroups"], serde_json::json!(mixer_inputs), "{family}: mixer inputs are the MIXER_IN routing outputs");
        assert_eq!(mixers["outputGroups"], serde_json::json!(mixer_outputs), "{family}: mixer outputs are the MIXER_OUT routing inputs");
        assert_eq!(mixer_inputs.len() as u64, count);
        for group in topology["outputs"].as_array().unwrap().iter().filter(|g| g["type"] == "MIXER_IN") {
            assert_eq!(group["channels"].as_u64(), Some(channels), "{family} {}", group["id"]);
        }
    }
}

#[test]
fn groups_are_well_formed() {
    for (family, topology, _, _, _) in families() {
        for kind in ["inputs", "outputs"] {
            let groups = topology[kind].as_array().unwrap();
            assert!(!groups.is_empty(), "{family} {kind}");
            let mut ids: Vec<&str> = groups.iter().map(|g| g["id"].as_str().unwrap()).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), groups.len(), "{family} {kind}: group ids are unique");
            for group in groups {
                let colour = group["color"].as_str().unwrap();
                assert!(colour.len() == 7 && colour.starts_with('#') && colour[1..].bytes().all(|b| b.is_ascii_hexdigit()), "{family} {group}");
                assert!(group["channels"].as_u64().unwrap() > 0, "{family} {group}");
                assert!(group["name"].as_str().is_some_and(|n| !n.is_empty()), "{family} {group}");
            }
        }
    }
}

#[test]
fn provenance_names_every_blob_a_value_came_from() {
    for (family, topology, _, _, bytecode) in families() {
        let source = &topology["source"];
        assert_eq!(source["tool"], "refs/tools/scripts/extract_topology.py");
        assert_eq!(source["bytecode"], bytecode, "{family}");
        let blobs = source["blobs"].as_array().unwrap();
        for blob in blobs {
            let digest = blob["sha256"].as_str().unwrap();
            assert!(digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()), "{family} {blob}");
        }
        let reads: Vec<String> = blobs.iter().flat_map(|b| b["read"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string())).collect();
        for expected in ["RoutingInputGroupType", "RoutingOutputGroupType", "INPUT_SPEC"] {
            assert!(reads.iter().any(|r| r.contains(expected)), "{family}: {expected} is attributed to a blob: {reads:?}");
        }
        let link = if family == "quadro" { "STEREO_LINK_ID" } else { "STEREO_LINK_PERIPH_ID" };
        assert!(reads.iter().any(|r| r.contains(link)), "{family}: the mixer stereo link constant is attributed to a blob: {reads:?}");
        assert!(topology["assumptions"].as_array().is_some_and(|a| !a.is_empty()), "{family}: assumptions are recorded");
    }
}
