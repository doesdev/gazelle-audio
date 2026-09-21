//! What the devices answered plus what the configuration asked for, turned into one device.
//!
//! Nothing here knows how many devices there are or which ones they are. It takes a list of
//! descriptions in configuration order and works out the aggregate's channel list and their names,
//! the buffer sizes every device can take, the one latency figure each way, and how far each
//! device has to be held back so that the channels line up.

use crate::config::{Alignment, Config, DeviceConfig, Labels, PhaseConfig};
use crate::sub::Description;
use gazelle_audio_stream_abi::sample;

/// The longest name the interface carries for a channel: thirty one characters and a terminator.
pub const MAX_NAME: usize = 31;

/// How one device's phase is measured, once the channels the file named have been found among the
/// ones actually opened.
///
/// **Both channels are the driver's, not the DAW's.** They are opened at the devices, because the
/// measurement needs them, and they are kept out of the channel list the DAW is given, so nothing a
/// DAW plays can land on the measurement channel and the measurement can never be heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhasePlan {
    /// The master's own output channel number the cable leaves from.
    pub master_output: i32,
    /// This device's own input channel number the cable arrives on.
    pub input: i32,
    /// Where that output sits among the master's opened outputs.
    pub master_slot: usize,
    /// Where that input sits among this device's opened inputs.
    pub input_slot: usize,
}

/// One device, as the aggregate will use it.
#[derive(Clone, Debug, PartialEq)]
pub struct DevicePlan {
    /// What its channels are called, from the configuration or from its registry key.
    pub name: String,
    /// What the driver called itself.
    pub driver_name: String,
    /// The device's own input channel numbers, in the order they are exposed.
    pub inputs: Vec<i32>,
    pub outputs: Vec<i32>,
    pub input_type: i32,
    pub output_type: i32,
    pub input_width: usize,
    pub output_width: usize,
    pub latency_in: i32,
    pub latency_out: i32,
    /// Samples this device's inputs are held back by so that every device lines up.
    pub pad_in: i32,
    /// Samples this device's outputs are held back by.
    pub pad_out: i32,
    /// Whether this device's audio crosses a ring buffer, which is true of all but the master.
    pub buffered: bool,
    /// How this device's capture phase is measured, when the file says.
    pub phase: Option<PhasePlan>,
    /// Slots of [`DevicePlan::inputs`] the driver opened for itself: the DAW is never given them.
    pub reserved_inputs: Vec<usize>,
    /// The same for [`DevicePlan::outputs`].
    pub reserved_outputs: Vec<usize>,
}

/// One channel of the aggregate: which device it is on, and which of that device's exposed
/// channels it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelRef {
    pub device: usize,
    pub slot: usize,
}

/// The whole aggregate, worked out.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub devices: Vec<DevicePlan>,
    pub master: usize,
    pub alignment: Alignment,
    pub inputs: Vec<ChannelRef>,
    pub outputs: Vec<ChannelRef>,
    pub input_names: Vec<String>,
    pub output_names: Vec<String>,
    /// The buffer sizes the aggregate offers, which is what every device can take.
    pub min: i32,
    pub max: i32,
    pub preferred: i32,
    pub granularity: i32,
    /// The block the plan's latencies were worked out for.
    pub block: i32,
    pub input_latency: i32,
    pub output_latency: i32,
}

/// One device's description with the configuration that found it.
#[derive(Clone, Debug)]
pub struct Found {
    /// What to call it, already settled: the configured name or the registry key.
    pub name: String,
    pub description: Description,
    pub wanted_inputs: Option<Vec<i32>>,
    pub wanted_outputs: Option<Vec<i32>>,
    /// Samples to add to what this device's driver says its input latency is, from the file.
    pub input_trim: i32,
    /// The same for its outputs.
    pub output_trim: i32,
    /// What the person calls this device's inputs, by the device's own channel numbering.
    pub input_labels: Labels,
    /// The same for its outputs.
    pub output_labels: Labels,
    /// Where the cable that this device's phase is measured over runs, from the file.
    pub phase: Option<PhaseConfig>,
}

/// The channels of a device that are actually exposed: what was asked for, with anything the
/// device does not have dropped, or all of them when nothing was asked for.
fn selected(wanted: &Option<Vec<i32>>, available: i32) -> Vec<i32> {
    match wanted {
        Some(list) => {
            let mut chosen: Vec<i32> = list.iter().copied().filter(|&c| c >= 0 && c < available).collect();
            chosen.dedup();
            chosen
        }
        None => (0..available).collect(),
    }
}

/// Where a channel sits among the ones a device has opened, opening it if it is not there yet.
/// The measurement needs its channel whether or not the file asked for it to be exposed.
fn place(channels: &mut Vec<i32>, channel: i32) -> usize {
    match channels.iter().position(|&already| already == channel) {
        Some(at) => at,
        None => {
            channels.push(channel);
            channels.len() - 1
        }
    }
}

/// **Hold every device back to the longest path**, so that every channel of every device is the
/// same distance from the converter, and answer what that distance is.
///
/// `pads` is written in place and nothing is allocated, because this is worked out again on the
/// audio path when a measurement moves a device.
pub fn hold_back(paths: &[i32], aligned: bool, pads: &mut [i32]) -> i32 {
    let longest = paths.iter().copied().max().unwrap_or(0);
    for (pad, &here) in pads.iter_mut().zip(paths) {
        *pad = if aligned { longest - here } else { 0 };
    }
    longest
}

/// A channel's name, as short as the interface carries one: "Quadro 1", "Studio+ 12".
pub fn channel_name(device: &str, number: usize) -> String {
    let tail = format!(" {number}");
    format!("{}{tail}", cut(device, MAX_NAME.saturating_sub(tail.len())))
}

/// A channel's name when the person has given it one: "Vocal mic (Quadro 1)".
///
/// Their own name comes first and the automatic one stays with it in brackets, so that a channel is
/// both the thing they call it and the interface and socket it is on. When the two together will
/// not fit, their name alone is what is kept: half a bracket reads as a name that was cut off,
/// which is worse than no reference at all.
pub fn labelled_name(device: &str, number: usize, label: Option<&str>) -> String {
    let automatic = channel_name(device, number);
    let Some(label) = label.map(str::trim).filter(|label| !label.is_empty()) else {
        return automatic;
    };
    let both = format!("{label} ({automatic})");
    if both.chars().count() <= MAX_NAME && both.len() <= MAX_NAME {
        return both;
    }
    cut(label, MAX_NAME)
}

/// As much of a name as fits: whole characters, never half of one, and never more bytes than the
/// interface's buffer carries.
fn cut(text: &str, room: usize) -> String {
    let mut kept: String = text.chars().take(room).collect();
    while kept.len() > room {
        kept.pop();
    }
    kept.trim_end().to_string()
}

/// Work the whole aggregate out, or say why it cannot be one device.
pub fn plan(found: &[Found], config: &Config, block: Option<i32>) -> Result<Plan, String> {
    if found.is_empty() {
        return Err("no devices to aggregate: nothing in the configuration matched a driver on this PC".to_string());
    }

    // Every device must speak the same sample rate, and the copy path must know both its types.
    for device in found {
        for (what, code) in [("inputs", device.description.input_type), ("outputs", device.description.output_type)] {
            if !sample::convertible(code) {
                return Err(format!("{}'s {what} are {}, which this driver will not convert", device.name, sample::name(code)));
            }
        }
    }

    let min = found.iter().map(|d| d.description.min).max().unwrap_or(0);
    let max = found.iter().map(|d| d.description.max).min().unwrap_or(0);
    if min > max && max > 0 {
        return Err(format!(
            "these devices share no buffer size: the smallest any of them will take is {min} and the largest is {max}"
        ));
    }
    let granularity = if found.iter().all(|d| d.description.granularity == -1) {
        -1
    } else {
        found.iter().map(|d| d.description.granularity).max().unwrap_or(0)
    };
    let agreed = found.iter().map(|d| d.description.preferred).max().unwrap_or(0);
    let preferred = config.buffer_size.unwrap_or(agreed).clamp(min.max(1), max.max(1));
    let block = block.unwrap_or(preferred);

    let master = master_of(found, config)?;

    let mut devices = Vec::new();
    for (index, device) in found.iter().enumerate() {
        let inputs = selected(&device.wanted_inputs, device.description.inputs);
        let outputs = selected(&device.wanted_outputs, device.description.outputs);
        devices.push(DevicePlan {
            name: device.name.clone(),
            driver_name: device.description.name.clone(),
            inputs,
            outputs,
            input_type: device.description.input_type,
            output_type: device.description.output_type,
            input_width: sample::width(device.description.input_type).unwrap_or(4),
            output_width: sample::width(device.description.output_type).unwrap_or(4),
            latency_in: device.description.latency_in + device.input_trim,
            latency_out: device.description.latency_out + device.output_trim,
            pad_in: 0,
            pad_out: 0,
            buffered: index != master,
            phase: None,
            reserved_inputs: Vec::new(),
            reserved_outputs: Vec::new(),
        });
    }

    // The channels the measurement runs over. They are opened at the devices, because the
    // measurement needs them, and they never reach the channel list the DAW is given.
    for (index, device) in found.iter().enumerate() {
        let Some((master_output, input)) = device.phase.as_ref().and_then(PhaseConfig::channels) else { continue };
        if index == master {
            return Err(format!(
                "{} drives the callback, and a phase is measured against the device that drives the callback, so it cannot be measured against itself",
                device.name
            ));
        }
        let master_name = &found[master].name;
        if master_output >= found[master].description.outputs {
            return Err(format!(
                "{}'s phase is measured over {master_name}'s output {master_output}, and {master_name} has {} outputs, numbered from zero",
                device.name, found[master].description.outputs
            ));
        }
        if input >= device.description.inputs {
            return Err(format!(
                "{}'s phase is measured on its own input {input}, and it has {} inputs, numbered from zero",
                device.name, device.description.inputs
            ));
        }
        let master_slot = place(&mut devices[master].outputs, master_output);
        if !devices[master].reserved_outputs.contains(&master_slot) {
            devices[master].reserved_outputs.push(master_slot);
        }
        let input_slot = place(&mut devices[index].inputs, input);
        if !devices[index].reserved_inputs.contains(&input_slot) {
            devices[index].reserved_inputs.push(input_slot);
        }
        devices[index].phase = Some(PhasePlan { master_output, input, master_slot, input_slot });
    }

    // A device that is not the master hands its audio over a buffer, which costs one block each
    // way. Everything else is the device's own reported latency.
    let path_in: Vec<i32> = devices.iter().map(|d| d.latency_in + if d.buffered { block } else { 0 }).collect();
    let path_out: Vec<i32> = devices.iter().map(|d| d.latency_out + if d.buffered { block } else { 0 }).collect();
    let aligned = config.alignment == Alignment::Aligned;
    let mut pads = vec![0i32; devices.len()];
    let input_latency = hold_back(&path_in, aligned, &mut pads);
    for (device, pad) in devices.iter_mut().zip(&pads) {
        device.pad_in = *pad;
    }
    let output_latency = hold_back(&path_out, aligned, &mut pads);
    for (device, pad) in devices.iter_mut().zip(&pads) {
        device.pad_out = *pad;
    }

    let mut inputs = Vec::new();
    let mut input_names = Vec::new();
    let mut outputs = Vec::new();
    let mut output_names = Vec::new();
    for (index, (device, configured)) in devices.iter().zip(found.iter()).enumerate() {
        for (slot, channel) in device.inputs.iter().enumerate() {
            if device.reserved_inputs.contains(&slot) {
                continue;
            }
            inputs.push(ChannelRef { device: index, slot });
            let label = configured.input_labels.get(*channel as u32);
            input_names.push(labelled_name(&device.name, *channel as usize + 1, label));
        }
        for (slot, channel) in device.outputs.iter().enumerate() {
            if device.reserved_outputs.contains(&slot) {
                continue;
            }
            outputs.push(ChannelRef { device: index, slot });
            let label = configured.output_labels.get(*channel as u32);
            output_names.push(labelled_name(&device.name, *channel as usize + 1, label));
        }
    }

    if inputs.is_empty() && outputs.is_empty() {
        return Err("the configuration exposes no channels at all".to_string());
    }

    Ok(Plan {
        devices,
        master,
        alignment: config.alignment,
        inputs,
        outputs,
        input_names,
        output_names,
        min,
        max,
        preferred,
        granularity,
        block,
        input_latency,
        output_latency,
    })
}

#[cfg(test)]
impl Plan {
    /// A plan of nothing, for a test that needs the shape of one rather than a real one.
    pub fn empty_for_tests() -> Plan {
        Plan {
            devices: Vec::new(),
            master: 0,
            alignment: Alignment::Aligned,
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_names: Vec::new(),
            output_names: Vec::new(),
            min: 0,
            max: 0,
            preferred: 0,
            granularity: -1,
            block: 0,
            input_latency: 0,
            output_latency: 0,
        }
    }
}

/// Which device drives the callback: the one the configuration names, or the first.
fn master_of(found: &[Found], config: &Config) -> Result<usize, String> {
    let Some(asked) = config.callback_master.as_deref() else { return Ok(0) };
    let wanted = asked.trim().to_ascii_lowercase();
    found
        .iter()
        .position(|d| {
            d.name.to_ascii_lowercase() == wanted
                || d.description.name.to_ascii_lowercase() == wanted
                || d.name.to_ascii_lowercase().contains(&wanted)
        })
        .ok_or_else(|| {
            let names: Vec<&str> = found.iter().map(|d| d.name.as_str()).collect();
            format!("callback_master is \"{}\", which is none of the devices: {}", asked.trim(), names.join(", "))
        })
}

/// Whether a registry entry looks like one of the devices a configuration entry asked for.
pub fn matches(entry_key: &str, entry_clsid: &str, description: Option<&str>, wanted: &DeviceConfig) -> bool {
    if let Some(clsid) = &wanted.clsid {
        return gazelle_audio_stream_abi::same_clsid(clsid, entry_clsid);
    }
    let Some(key) = &wanted.key else { return false };
    let key = key.trim().to_ascii_lowercase();
    let entry = entry_key.to_ascii_lowercase();
    entry == key || entry.contains(&key) || description.is_some_and(|d| d.to_ascii_lowercase().contains(&key))
}

/// With no configuration file, this is what the driver opens: everything on the PC that says it is
/// an Antelope interface, in registry order. Matching the maker rather than a list of models is
/// what makes another Antelope interface work without a code change.
pub fn looks_like_ours(key: &str, description: Option<&str>, dll: Option<&str>) -> bool {
    const MAKER: &str = "antelope";
    let hit = |text: &str| text.to_ascii_lowercase().contains(MAKER);
    hit(key) || description.is_some_and(hit) || dll.is_some_and(hit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub::Description;

    fn described(name: &str, inputs: i32, outputs: i32, latency_in: i32, latency_out: i32) -> Description {
        Description {
            name: name.to_string(),
            version: 1,
            inputs,
            outputs,
            min: 32,
            max: 2048,
            preferred: 512,
            granularity: -1,
            rate: 96_000.0,
            latency_in,
            latency_out,
            input_type: sample::INT32_LSB,
            output_type: sample::INT32_LSB,
            output_ready: true,
        }
    }

    fn found(name: &str, inputs: i32, outputs: i32, latency_in: i32, latency_out: i32) -> Found {
        Found {
            name: name.to_string(),
            description: described(name, inputs, outputs, latency_in, latency_out),
            wanted_inputs: None,
            wanted_outputs: None,
            input_trim: 0,
            output_trim: 0,
            input_labels: Labels::default(),
            output_labels: Labels::default(),
            phase: None,
        }
    }

    /// A cable from the master's output to this device's input, as the file declares one.
    fn phase_over(master_output: i32, input: i32) -> PhaseConfig {
        PhaseConfig { master_output: Some(master_output), input: Some(input) }
    }

    /// Labels as the file gives them: the device's own channel number, and what to call it.
    fn labels(pairs: &[(u32, &str)]) -> Labels {
        pairs.iter().map(|(channel, label)| (*channel, (*label).to_string())).collect()
    }

    /// The two devices phase 0 measured, at 96 kHz.
    fn this_pc() -> Vec<Found> {
        vec![found("Quadro", 16, 16, 639, 799), found("Studio+", 24, 24, 636, 700)]
    }

    #[test]
    fn every_channel_of_every_device_is_exposed_in_order_and_named() {
        let plan = plan(&this_pc(), &Config::default(), None).expect("two ordinary devices");
        assert_eq!(plan.inputs.len(), 40);
        assert_eq!(plan.outputs.len(), 40);
        assert_eq!(plan.input_names[0], "Quadro 1");
        assert_eq!(plan.input_names[15], "Quadro 16");
        assert_eq!(plan.input_names[16], "Studio+ 1");
        assert_eq!(plan.input_names[39], "Studio+ 24");
        assert_eq!(plan.inputs[16], ChannelRef { device: 1, slot: 0 });
        assert_eq!(plan.outputs[39], ChannelRef { device: 1, slot: 23 });
    }

    #[test]
    fn a_third_device_needs_no_code_only_a_line_in_the_file() {
        let mut devices = this_pc();
        devices.push(found("Orion", 32, 32, 700, 800));
        let plan = plan(&devices, &Config::default(), None).expect("three devices are no different");
        assert_eq!(plan.inputs.len(), 16 + 24 + 32);
        assert_eq!(plan.input_names.last().unwrap(), "Orion 32");
        assert!(plan.devices[2].buffered);
    }

    #[test]
    fn only_the_channels_the_file_asks_for_are_exposed() {
        let mut devices = this_pc();
        devices[0].wanted_inputs = Some(vec![0, 1]);
        devices[0].wanted_outputs = Some(vec![4, 5]);
        devices[1].wanted_inputs = Some(vec![2, 99]);
        let plan = plan(&devices, &Config::default(), None).expect("a narrowed device is still a device");
        assert_eq!(plan.input_names, vec!["Quadro 1", "Quadro 2", "Studio+ 3"], "a channel the device has not got is dropped");
        assert_eq!(plan.output_names[0], "Quadro 5", "channels are named by the device's own numbering");
        assert_eq!(plan.outputs.len(), 2 + 24);
    }

    #[test]
    fn the_master_is_the_first_device_unless_the_file_says_otherwise() {
        let config = Config::default();
        assert_eq!(plan(&this_pc(), &config, None).unwrap().master, 0);
        let studio = Config { callback_master: Some("Studio+".into()), ..Config::default() };
        assert_eq!(plan(&this_pc(), &studio, None).unwrap().master, 1);
        let by_part = Config { callback_master: Some("studio".into()), ..Config::default() };
        assert_eq!(plan(&this_pc(), &by_part, None).unwrap().master, 1);
    }

    #[test]
    fn a_master_that_is_not_there_is_a_refusal_that_names_the_devices() {
        let config = Config { callback_master: Some("Orion".into()), ..Config::default() };
        let error = plan(&this_pc(), &config, None).expect_err("a master must exist");
        assert!(error.contains("Orion"), "{error}");
        assert!(error.contains("Quadro") && error.contains("Studio+"), "{error}");
    }

    #[test]
    fn lining_up_pads_the_earlier_device_and_reports_one_figure() {
        let plan = plan(&this_pc(), &Config::default(), Some(512)).expect("the default is aligned");
        // The master is direct; the other device crosses a buffer, so its path is a block longer.
        assert_eq!(plan.input_latency, 636 + 512);
        assert_eq!(plan.output_latency, 700 + 512);
        assert_eq!(plan.devices[0].pad_in, 1148 - 639);
        assert_eq!(plan.devices[0].pad_out, 1212 - 799);
        assert_eq!(plan.devices[1].pad_in, 0);
        assert_eq!(plan.devices[1].pad_out, 0);
        // Which is the whole point: every device's path is now the same length.
        for (index, device) in plan.devices.iter().enumerate() {
            let buffered = if index == plan.master { 0 } else { plan.block };
            assert_eq!(device.latency_in + device.pad_in + buffered, plan.input_latency);
            assert_eq!(device.latency_out + device.pad_out + buffered, plan.output_latency);
        }
    }

    #[test]
    fn the_lowest_latency_setting_pads_nothing() {
        let config = Config { alignment: Alignment::LowestLatency, ..Config::default() };
        let plan = plan(&this_pc(), &config, Some(512)).expect("two devices");
        assert!(plan.devices.iter().all(|d| d.pad_in == 0 && d.pad_out == 0));
        // The figure reported is still the longest path, because that is the honest one.
        assert_eq!(plan.input_latency, 636 + 512);
        assert_eq!(plan.output_latency, 700 + 512);
    }

    #[test]
    fn one_device_alone_costs_nothing_at_all() {
        let plan = plan(&this_pc()[..1], &Config::default(), Some(512)).expect("one device is a valid aggregate");
        assert_eq!(plan.input_latency, 639);
        assert_eq!(plan.output_latency, 799);
        assert_eq!(plan.devices[0].pad_in, 0);
        assert!(!plan.devices[0].buffered);
    }

    #[test]
    fn the_buffer_sizes_offered_are_the_ones_every_device_can_take() {
        let mut devices = this_pc();
        devices[0].description.min = 64;
        devices[0].description.max = 1024;
        devices[1].description.min = 32;
        devices[1].description.max = 2048;
        devices[1].description.preferred = 256;
        let plan = plan(&devices, &Config::default(), None).expect("they overlap");
        assert_eq!(plan.min, 64);
        assert_eq!(plan.max, 1024);
        assert_eq!(plan.preferred, 512, "the larger of the two preferred sizes, which both can take");
    }

    #[test]
    fn the_file_can_ask_for_a_buffer_size_and_it_is_held_inside_what_is_possible() {
        let config = Config { buffer_size: Some(128), ..Config::default() };
        assert_eq!(plan(&this_pc(), &config, None).unwrap().preferred, 128);
        let silly = Config { buffer_size: Some(99_999), ..Config::default() };
        assert_eq!(plan(&this_pc(), &silly, None).unwrap().preferred, 2048, "held inside what the devices will take");
    }

    #[test]
    fn devices_that_share_no_buffer_size_are_refused_rather_than_guessed_at() {
        let mut devices = this_pc();
        devices[0].description.min = 1024;
        devices[1].description.max = 512;
        let error = plan(&devices, &Config::default(), None).expect_err("there is no size both will take");
        assert!(error.contains("1024") && error.contains("512"), "{error}");
    }

    #[test]
    fn a_sample_type_the_copy_path_does_not_know_is_refused_by_name() {
        let mut devices = this_pc();
        devices[1].description.output_type = 2; // big endian, which no Windows driver reports
        let error = plan(&devices, &Config::default(), None).expect_err("an unknown type is a refusal");
        assert!(error.contains("Studio+") && error.contains("outputs"), "{error}");
    }

    #[test]
    fn nothing_to_aggregate_is_a_refusal_in_plain_words() {
        let error = plan(&[], &Config::default(), None).expect_err("no devices");
        assert!(error.contains("no devices"), "{error}");
        let mut devices = this_pc();
        for device in &mut devices {
            device.wanted_inputs = Some(Vec::new());
            device.wanted_outputs = Some(Vec::new());
        }
        assert!(plan(&devices, &Config::default(), None).is_err(), "no channels is no device");
    }

    #[test]
    fn a_long_device_name_still_leaves_room_for_the_channel_number() {
        let name = channel_name("A Very Long Interface Name Indeed", 12);
        assert!(name.len() <= MAX_NAME, "{name} is {} characters", name.len());
        assert!(name.ends_with(" 12"));
        assert_eq!(channel_name("Quadro", 1), "Quadro 1");
    }

    #[test]
    fn a_channel_the_person_has_named_keeps_the_automatic_name_in_brackets() {
        let mut devices = this_pc();
        devices[0].input_labels = labels(&[(0, "Vocal mic"), (2, "DI")]);
        devices[0].output_labels = labels(&[(0, "Main L"), (1, "Main R")]);
        let plan = plan(&devices, &Config::default(), None).expect("two ordinary devices");
        assert_eq!(plan.input_names[0], "Vocal mic (Quadro 1)");
        assert_eq!(plan.input_names[2], "DI (Quadro 3)");
        assert_eq!(plan.output_names[0], "Main L (Quadro 1)");
        assert_eq!(plan.output_names[1], "Main R (Quadro 2)");
        assert!(plan.input_names.iter().all(|name| name.chars().count() <= MAX_NAME), "{:?}", plan.input_names);
    }

    #[test]
    fn a_channel_nobody_has_named_is_called_exactly_what_it_always_was() {
        let mut devices = this_pc();
        devices[0].input_labels = labels(&[(0, "Vocal mic")]);
        let plan = plan(&devices, &Config::default(), None).expect("two ordinary devices");
        assert_eq!(plan.input_names[1], "Quadro 2", "the channel beside a named one is untouched");
        assert_eq!(plan.output_names[0], "Quadro 1", "and so is the same channel the other way round");
        assert_eq!(plan.input_names[16], "Studio+ 1", "and so is every channel of a device with no names at all");
    }

    #[test]
    fn a_name_too_long_to_carry_the_reference_keeps_the_persons_own_words() {
        // Half a bracket reads as a name that was cut off, so it is all of the reference or none.
        let name = labelled_name("Quadro", 1, Some("The big valve preamp in the rack"));
        assert!(name.chars().count() <= MAX_NAME, "{name} is {} characters", name.chars().count());
        assert_eq!(name, "The big valve preamp in the rac");
        assert!(!name.contains('('), "{name}");
        // A name that fills the last character the interface carries still keeps the reference.
        let exact = labelled_name("Quadro", 1, Some("Twenty characters ok"));
        assert_eq!(exact, "Twenty characters ok (Quadro 1)");
        assert_eq!(exact.chars().count(), MAX_NAME);
        // One character more, and the reference goes rather than half of it.
        assert_eq!(labelled_name("Quadro", 1, Some("Twenty characters oks")), "Twenty characters oks");
    }

    #[test]
    fn a_name_in_somebody_elses_alphabet_is_cut_between_characters_and_never_inside_one() {
        let long = "\u{3053}".repeat(40);
        let name = labelled_name("Quadro", 1, Some(&long));
        assert!(name.len() <= MAX_NAME, "{name} is {} bytes", name.len());
        assert!(long.starts_with(&name), "it is the beginning of what was asked for: {name}");
        assert_eq!(labelled_name("Quadro", 1, Some("Caf\u{e9}")), "Caf\u{e9} (Quadro 1)");
    }

    #[test]
    fn a_name_for_a_channel_that_is_not_exposed_is_simply_unused() {
        let mut devices = this_pc();
        devices[0].wanted_inputs = Some(vec![0, 1]);
        devices[0].input_labels = labels(&[(0, "Vocal mic"), (9, "A channel nobody asked for")]);
        let plan = plan(&devices, &Config::default(), None).expect("a narrowed device is still a device");
        assert_eq!(plan.input_names[0], "Vocal mic (Quadro 1)");
        assert_eq!(plan.input_names[1], "Quadro 2");
        assert!(!plan.input_names.iter().any(|name| name.contains("nobody asked for")), "{:?}", plan.input_names);
    }

    #[test]
    fn a_name_that_is_empty_or_only_spaces_is_not_a_name() {
        let mut devices = this_pc();
        devices[0].input_labels = labels(&[(0, ""), (1, "   ")]);
        let plan = plan(&devices, &Config::default(), None).expect("two ordinary devices");
        assert_eq!(plan.input_names[0], "Quadro 1");
        assert_eq!(plan.input_names[1], "Quadro 2");
        assert_eq!(labelled_name("Quadro", 1, None), "Quadro 1");
        assert_eq!(labelled_name("Quadro", 1, Some("  Vocal mic  ")), "Vocal mic (Quadro 1)", "the spaces around one are not part of it");
    }

    #[test]
    fn devices_are_matched_by_class_id_first_and_then_by_name() {
        let by_clsid = DeviceConfig { clsid: Some("{AE4A4452-A316-11E5-A113-080027F6C1F4}".into()), ..Default::default() };
        assert!(matches("ZenStudioTB ASIO Driver", "{ae4a4452-a316-11e5-a113-080027f6c1f4}", None, &by_clsid));
        assert!(!matches("Zen Quadro", "{12217625-CB57-11EE-908D-7085C2FB2DD5}", None, &by_clsid));
        let by_key = DeviceConfig { key: Some("zen quadro".into()), ..Default::default() };
        assert!(matches("Zen Quadro Synergy Core", "{1}", None, &by_key));
        assert!(!matches("ZenStudioTB ASIO Driver", "{2}", None, &by_key));
    }

    #[test]
    fn with_no_file_the_makers_own_drivers_are_the_ones_opened() {
        assert!(looks_like_ours("Zen Quadro Synergy Core", None, Some(r"c:\program files\antelope audio\x.dll")));
        assert!(looks_like_ours("Antelope Audio Thunderbolt", None, None));
        assert!(looks_like_ours("Something", Some("Antelope Audio USB"), None));
        assert!(!looks_like_ours("Realtek ASIO", None, Some(r"c:\realtek\rtasio.dll")));
    }

    #[test]
    fn a_device_can_be_trimmed_by_a_measured_number_of_samples() {
        // A device's real converter latency can differ from the figure its driver reports. Measured
        // at the devices on 2026-09-21: with the same source into both, the Studio+'s recording sat
        // about 28 samples behind the Quadro's, and it stayed there when the two microphones were
        // swapped, so it belongs to the device and not to the microphone. A trim nulls it: a device
        // that records late admits to less latency than it has, so its trim is positive and the
        // others are held back to meet it. A trim of the wrong sign doubles the error.
        let mut devices = this_pc();
        devices[1].input_trim = -28;
        let plan = plan(&devices, &Config::default(), None).expect("two ordinary devices");
        let (quadro, studio) = (&plan.devices[0], &plan.devices[1]);
        // The Studio+ is buffered, so its path is 636 + 512 = 1148, less the 28 it is trimmed by;
        // the Quadro's is 639. Everything is held back to the longest, which is still the Studio+.
        assert_eq!(plan.input_latency, 1120, "the trim shortens the longest path, so the DAW is told less");
        assert_eq!(studio.pad_in, 0);
        assert_eq!(quadro.pad_in, 1120 - 639, "and the Quadro waits that much longer");
        // The trim moves only the inputs: an output trim is its own field.
        assert_eq!(plan.output_latency, 1212);
        assert_eq!(studio.pad_out, 0);
        assert_eq!(quadro.pad_out, 1212 - 799);
    }

    /// What a phase measurement costs the DAW: the two channels it runs over, which the DAW never
    /// sees, so nothing it plays can land on the measurement channel.
    #[test]
    fn the_channels_a_phase_is_measured_over_are_kept_out_of_the_daws_list() {
        let mut devices = this_pc();
        devices[1].phase = Some(phase_over(8, 20));
        let plan = plan(&devices, &Config::default(), None).expect("two ordinary devices and a cable");
        // The Quadro's output 8 and the Studio+'s input 20 are opened at the devices...
        assert!(plan.devices[0].outputs.contains(&8));
        assert!(plan.devices[1].inputs.contains(&20));
        let measured = plan.devices[1].phase.expect("the follower is measured");
        assert_eq!((measured.master_output, measured.input), (8, 20));
        assert_eq!(plan.devices[0].outputs[measured.master_slot], 8);
        assert_eq!(plan.devices[1].inputs[measured.input_slot], 20);
        // ...and neither is in the list the DAW is given, nor named in it.
        assert_eq!(plan.outputs.len(), 16 + 24 - 1, "one of the Quadro's outputs is the driver's own");
        assert_eq!(plan.inputs.len(), 16 + 24 - 1);
        assert!(!plan.output_names.contains(&"Quadro 9".to_string()), "{:?}", plan.output_names);
        assert!(!plan.input_names.contains(&"Studio+ 21".to_string()), "{:?}", plan.input_names);
        // The channels beside them are untouched, and still name the sockets they are on.
        assert_eq!(plan.output_names[7], "Quadro 8");
        assert_eq!(plan.output_names[8], "Quadro 10", "the numbering follows the device, not the list");
        assert_eq!(plan.devices[0].phase, None, "the device that drives the callback is not measured");
    }

    /// A channel the file did not ask to expose is still opened when the measurement needs it.
    #[test]
    fn a_measurement_channel_outside_what_the_file_exposes_is_opened_anyway_and_still_kept_back() {
        let mut devices = this_pc();
        devices[0].wanted_outputs = Some(vec![0, 1]);
        devices[1].wanted_inputs = Some(vec![0, 1]);
        devices[1].phase = Some(phase_over(8, 20));
        let plan = plan(&devices, &Config::default(), None).expect("two ordinary devices and a cable");
        assert_eq!(plan.devices[0].outputs, vec![0, 1, 8], "it is opened at the device");
        assert_eq!(plan.devices[1].inputs, vec![0, 1, 20]);
        assert_eq!(plan.output_names.iter().filter(|name| name.starts_with("Quadro")).count(), 2, "and not exposed");
        assert_eq!(plan.input_names.iter().filter(|name| name.starts_with("Studio+")).count(), 2);
    }

    /// Three interfaces, each follower measured against the one that drives the callback.
    #[test]
    fn every_follower_is_measured_against_the_master_on_its_own_channels() {
        let mut devices = this_pc();
        devices.push(found("Orion", 32, 32, 700, 800));
        devices[1].phase = Some(phase_over(8, 20));
        devices[2].phase = Some(phase_over(9, 30));
        let three = plan(&devices, &Config::default(), None).expect("three devices are no different");
        assert_eq!(three.devices[1].phase.unwrap().input, 20);
        assert_eq!(three.devices[2].phase.unwrap().input, 30);
        // Both cables leave the master, on channels of its own, and both are kept back from the DAW.
        assert_eq!(three.devices[0].reserved_outputs.len(), 2);
        assert_eq!(three.outputs.len(), 16 - 2 + 24 + 32);
        assert_eq!(three.inputs.len(), 16 + 24 - 1 + 32 - 1);

        // Two followers may share one of the master's outputs, which is one cable split in two.
        devices[2].phase = Some(phase_over(8, 30));
        let shared = plan(&devices, &Config::default(), None).expect("one output feeding both");
        assert_eq!(shared.devices[0].reserved_outputs.len(), 1, "the same output is kept back once");
        assert_eq!(shared.outputs.len(), 16 - 1 + 24 + 32);
    }

    #[test]
    fn a_phase_measured_over_a_channel_that_is_not_there_is_refused_by_name() {
        let mut devices = this_pc();
        devices[1].phase = Some(phase_over(99, 20));
        let no_output = plan(&devices, &Config::default(), None).expect_err("the Quadro has 16 outputs");
        assert!(no_output.contains("Studio+") && no_output.contains("99") && no_output.contains("16"), "{no_output}");

        devices[1].phase = Some(phase_over(8, 99));
        let no_input = plan(&devices, &Config::default(), None).expect_err("the Studio+ has 24 inputs");
        assert!(no_input.contains("Studio+") && no_input.contains("99") && no_input.contains("24"), "{no_input}");
    }

    #[test]
    fn the_device_that_drives_the_callback_cannot_be_measured_against_itself() {
        let mut devices = this_pc();
        devices[0].phase = Some(phase_over(8, 20));
        let error = plan(&devices, &Config::default(), None).expect_err("there is nothing to measure it against");
        assert!(error.contains("Quadro") && error.contains("drives the callback"), "{error}");
        // The same setting on the device that is not the master is an ordinary measurement.
        let mut moved = this_pc();
        moved[1].phase = Some(phase_over(8, 20));
        assert!(plan(&moved, &Config::default(), None).is_ok());
    }

    #[test]
    fn a_device_with_no_phase_setting_is_planned_exactly_as_it_always_was() {
        let plain = plan(&this_pc(), &Config::default(), Some(512)).expect("two ordinary devices");
        assert!(plain.devices.iter().all(|device| device.phase.is_none()));
        assert!(plain.devices.iter().all(|device| device.reserved_inputs.is_empty() && device.reserved_outputs.is_empty()));
        assert_eq!(plain.inputs.len(), 40);
        assert_eq!(plain.input_latency, 636 + 512);
        assert_eq!(plain.devices[0].pad_in, 1148 - 639);
    }

    #[test]
    fn a_trim_can_hold_a_device_back_as_well_as_bring_it_forward() {
        let mut devices = this_pc();
        devices[0].input_trim = 30;
        devices[0].output_trim = 12;
        let plan = plan(&devices, &Config::default(), None).expect("two ordinary devices");
        assert_eq!(plan.devices[0].pad_in, 1148 - (639 + 30));
        assert_eq!(plan.devices[0].pad_out, 1212 - (799 + 12));
    }
}
