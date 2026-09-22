//! What each device model has, for checking the indexes a workspace names: channel counts per
//! input and output type from the embedded topologies (`refs/schemas/*_topology.json`, the panels'
//! own lists), and the output ids `set_volume` takes, which are not in the topology.

use std::sync::OnceLock;

use crate::device::routing_loopback::{QUADRO_TOPOLOGY, STUDIO_TOPOLOGY};

/// A model's routing groups as `(type, channels)`, by side.
struct Model {
    inputs: Vec<(String, u32)>,
    outputs: Vec<(String, u32)>,
    /// Destination group ids (`MIXER_IN0`, `SPDIF_OUT0`) in topology order; a group's place in this
    /// list is its wire position, which `get_routing` takes as its `ext3`.
    output_ids: Vec<String>,
    /// Every source group whole, in topology order, which is the order a routing slot numbers them.
    source_groups: Vec<Group>,
    /// Every destination group whole, in topology order.
    destination_groups: Vec<Group>,
    /// The source group each mix plays out of, by mix: `MIXER_OUT0` for mix 1 and so on.
    mix_outputs: Vec<String>,
}

/// One routing group as the topology lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// `COM_REC0`, `PREAMP0`, …
    pub id: String,
    /// `COM_REC`, `PREAMP`, …
    pub kind: String,
    /// What the panels, and Gazelle's Routing page, call it: `USB A REC`, `PREAMP`, …
    pub name: String,
    pub channels: u32,
}

fn parse(json: &str) -> Model {
    let topology: serde_json::Value = serde_json::from_str(json).expect("the embedded topology is JSON");
    let side = |name: &str| {
        topology[name]
            .as_array()
            .map(|groups| groups.iter().filter_map(|g| Some((g["type"].as_str()?.to_owned(), u32::try_from(g["channels"].as_u64()?).ok()?))).collect())
            .unwrap_or_default()
    };
    let output_ids = topology["outputs"]
        .as_array()
        .map(|groups| groups.iter().filter_map(|g| Some(g["id"].as_str()?.to_owned())).collect())
        .unwrap_or_default();
    let whole = |name: &str| -> Vec<Group> {
        topology[name]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .filter_map(|g| {
                        Some(Group {
                            id: g["id"].as_str()?.to_owned(),
                            kind: g["type"].as_str()?.to_owned(),
                            name: g["name"].as_str()?.to_owned(),
                            channels: u32::try_from(g["channels"].as_u64()?).ok()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let mix_outputs = topology["mixers"]["outputGroups"]
        .as_array()
        .map(|ids| ids.iter().filter_map(|id| Some(id.as_str()?.to_owned())).collect())
        .unwrap_or_default();
    Model { inputs: side("inputs"), outputs: side("outputs"), output_ids, source_groups: whole("inputs"), destination_groups: whole("outputs"), mix_outputs }
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

/// Every routing destination group of a model, as `(topology id, wire position)`.
///
/// A snapshot keys routing by the topology id rather than the position, so it survives a topology
/// re-extraction that reorders groups.
pub fn destination_groups(family: &str) -> Option<Vec<(String, u32)>> {
    let m = model(family)?;
    Some(m.output_ids.iter().enumerate().map(|(at, id)| (id.clone(), at as u32)).collect())
}

/// Every routing source group of a model, whole, in the order a routing slot numbers them: a slot's
/// source is its group's place in this list.
pub fn source_groups(family: &str) -> Option<&'static [Group]> {
    model(family).map(|m| m.source_groups.as_slice())
}

/// Every routing destination group of a model, whole, in wire order.
pub fn destination_groups_whole(family: &str) -> Option<&'static [Group]> {
    model(family).map(|m| m.destination_groups.as_slice())
}

/// The source group each of a model's mixes plays out of, by mix, as topology ids.
pub fn mix_outputs(family: &str) -> Option<&'static [String]> {
    model(family).map(|m| m.mix_outputs.as_slice())
}

/// How many outputs `set_volume` addresses: MONITOR, HP1, HP2, LINE OUT, and the Studio+'s REAMP.
pub fn output_ids(family: &str) -> Option<u32> {
    match family {
        "quadro" => Some(4),
        "studio" => Some(5),
        _ => None,
    }
}

/// Channels of a digital port (`SPDIF_OUT`, `ADAT_OUT`, `SPDIF_IN`, `ADAT_IN`) on a model: outputs
/// from the routing destinations, inputs from the sources.
pub fn port_channels(family: &str, port: &str) -> Option<u32> {
    if port.ends_with("_OUT") {
        output_channels(family, port)
    } else {
        input_channels(family, port)
    }
}

/// Channels one cable or port strip carries: a stereo pair for S/PDIF, eight for an ADAT port.
pub fn port_width(port: &str) -> u32 {
    if port.starts_with("ADAT") {
        8
    } else {
        2
    }
}

/// The clock sources a model offers, in `set_sync_source`'s index order, as each panel lists them
/// (`app/ui/cpanel.py` and `zenstudiotb/ui/widgets/comboboxes.py`). The Studio+'s internal clock
/// is its oven-controlled oscillator, and it alone has a word clock input. The web client carries
/// the same two lists.
pub fn clock_sources(family: &str) -> Option<&'static [&'static str]> {
    match family {
        "quadro" => Some(&["Internal", "ADAT x1", "ADAT x2", "ADAT x4", "S/PDIF", "USB"]),
        "studio" => Some(&["Oven", "Word clock", "ADAT", "ADAT x2", "ADAT x4", "S/PDIF", "USB"]),
        _ => None,
    }
}

/// The clock source a model should be on to follow a cable arriving at `port` (`SPDIF_IN` or
/// `ADAT_IN`), as `(index, name)`.
///
/// This is the one check phase 0 said nothing in the code would have suggested: a device left on
/// its internal clock silently becomes USB clocked the moment a DAW opens it, so an aggregate is
/// only sample locked when each following device was put on its cable's input beforehand. The
/// ADAT answer is the plain `x1` entry: a device taking a higher multiple is following the same
/// cable, so that is accepted too (`follows_port`).
pub fn clock_source_for_port(family: &str, port: &str) -> Option<(u32, &'static str)> {
    let wanted = if port.starts_with("ADAT") { "ADAT" } else { "S/PDIF" };
    let sources = clock_sources(family)?;
    sources
        .iter()
        .position(|source| *source == wanted || (wanted == "ADAT" && *source == "ADAT x1"))
        .map(|at| (at as u32, sources[at]))
}

/// Whether a clock source index is one that follows a cable arriving at `port`: the plain entry
/// or any of its multiples.
pub fn follows_port(family: &str, port: &str, source: u32) -> bool {
    let Some(sources) = clock_sources(family) else { return false };
    let wanted = if port.starts_with("ADAT") { "ADAT" } else { "S/PDIF" };
    sources.get(source as usize).is_some_and(|name| name.starts_with(wanted))
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

    #[test]
    fn destination_groups_are_named_and_positioned_as_the_topology_lists_them() {
        let quadro = destination_groups("quadro").expect("quadro topology");
        assert_eq!(quadro.len(), 12, "the Quadro has twelve destination groups");
        assert_eq!(quadro[0], ("LINE_OUT0".to_string(), 0));
        assert!(quadro.iter().any(|(id, _)| id == "SPDIF_OUT0"));
        assert_eq!(destination_groups("studio").expect("studio topology").len(), 14);
        assert!(destination_groups("zen").is_none());
    }

    #[test]
    fn groups_come_whole_with_the_names_the_routing_page_shows() {
        let sources = source_groups("quadro").expect("quadro topology");
        assert_eq!(sources[0], Group { id: "PREAMP0".into(), kind: "PREAMP".into(), name: "PREAMP".into(), channels: 4 });
        assert_eq!(sources[1].name, "USB 1 PLAY");
        assert_eq!(sources[1].channels, 16);
        let destinations = destination_groups_whole("studio").expect("studio topology");
        assert!(destinations.iter().any(|g| g.id == "USB_REC0" && g.name == "USB REC" && g.channels == 24));
        assert_eq!(mix_outputs("quadro").expect("quadro mixes"), ["MIXER_OUT0", "MIXER_OUT1", "MIXER_OUT2", "MIXER_OUT3"]);
        assert!(source_groups("zen").is_none());
    }

    #[test]
    fn a_cable_names_the_clock_source_the_device_at_its_end_should_be_on() {
        assert_eq!(clock_source_for_port("quadro", "SPDIF_IN"), Some((4, "S/PDIF")));
        assert_eq!(clock_source_for_port("studio", "SPDIF_IN"), Some((5, "S/PDIF")));
        assert_eq!(clock_source_for_port("quadro", "ADAT_IN"), Some((1, "ADAT x1")));
        assert_eq!(clock_source_for_port("studio", "ADAT_IN"), Some((2, "ADAT")));
        assert_eq!(clock_source_for_port("zen", "SPDIF_IN"), None);
    }

    #[test]
    fn a_device_on_a_multiple_of_its_cables_input_is_still_following_it() {
        assert!(follows_port("studio", "ADAT_IN", 2), "ADAT");
        assert!(follows_port("studio", "ADAT_IN", 4), "ADAT x4 follows the same cable");
        assert!(!follows_port("studio", "ADAT_IN", 5), "S/PDIF is a different cable");
        assert!(follows_port("studio", "SPDIF_IN", 5));
        assert!(!follows_port("studio", "SPDIF_IN", 0), "the oven follows nothing");
        assert!(!follows_port("studio", "SPDIF_IN", 6), "and USB is the trap, not the answer");
        assert!(!follows_port("studio", "SPDIF_IN", 99), "a source the model does not have");
    }
}
