//! What each device model has, for checking the indexes a workspace names: channel counts per
//! input and output type from the embedded topologies (`refs/schemas/*_topology.json`, the panels'
//! own lists), and the output ids `set_volume` takes, which are not in the topology.

use std::sync::OnceLock;

use crate::device::routing_loopback::{QUADRO_TOPOLOGY, STUDIO_TOPOLOGY};

/// A model's routing groups as `(type, channels)`, by side.
struct Model {
    inputs: Vec<(String, u32)>,
    outputs: Vec<(String, u32)>,
}

fn parse(json: &str) -> Model {
    let topology: serde_json::Value = serde_json::from_str(json).expect("the embedded topology is JSON");
    let side = |name: &str| {
        topology[name]
            .as_array()
            .map(|groups| groups.iter().filter_map(|g| Some((g["type"].as_str()?.to_owned(), u32::try_from(g["channels"].as_u64()?).ok()?))).collect())
            .unwrap_or_default()
    };
    Model { inputs: side("inputs"), outputs: side("outputs") }
}

fn model(family: &str) -> Option<&'static Model> {
    static QUADRO: OnceLock<Model> = OnceLock::new();
    static STUDIO: OnceLock<Model> = OnceLock::new();
    match family {
        "quadro" => Some(QUADRO.get_or_init(|| parse(QUADRO_TOPOLOGY))),
        "studio" => Some(STUDIO.get_or_init(|| parse(STUDIO_TOPOLOGY))),
        _ => None,
    }
}

/// Channels of a routing source type (`PREAMP`, `ADAT_IN`, …) on a model: `None` for an unknown
/// model, `Some(0)` for a type the model does not have.
pub fn input_channels(family: &str, kind: &str) -> Option<u32> {
    model(family).map(|m| m.inputs.iter().filter(|(t, _)| t == kind).map(|(_, n)| n).sum())
}

/// Channels of a routing destination type (`SPDIF_OUT`, `ADAT_OUT`, …) on a model, as for inputs.
pub fn output_channels(family: &str, kind: &str) -> Option<u32> {
    model(family).map(|m| m.outputs.iter().filter(|(t, _)| t == kind).map(|(_, n)| n).sum())
}

/// How many outputs `set_volume` addresses: MONITOR, HP1, HP2, LINE OUT, and the Studio+'s REAMP.
pub fn output_ids(family: &str) -> Option<u32> {
    match family {
        "quadro" => Some(4),
        "studio" => Some(5),
        _ => None,
    }
}

/// The topology type of a workspace input kind (`preamp`, `line`, `adat`, `spdif`).
pub fn input_type(kind: &str) -> Option<&'static str> {
    match kind {
        "preamp" => Some("PREAMP"),
        "line" => Some("LINE_IN"),
        "adat" => Some("ADAT_IN"),
        "spdif" => Some("SPDIF_IN"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_come_from_each_models_topology() {
        assert_eq!(input_channels("quadro", "PREAMP"), Some(4));
        assert_eq!(input_channels("studio", "PREAMP"), Some(12));
        assert_eq!(input_channels("quadro", "LINE_IN"), Some(0));
        assert_eq!(input_channels("studio", "ADAT_IN"), Some(16));
        assert_eq!(output_channels("quadro", "ADAT_OUT"), Some(0));
        assert_eq!(output_channels("studio", "ADAT_OUT"), Some(16));
        assert_eq!(output_channels("quadro", "SPDIF_OUT"), Some(2));
        assert_eq!(input_channels("zen", "PREAMP"), None);
        assert_eq!(output_ids("studio"), Some(5));
    }
}
