//! What the devices answered plus what the configuration asked for, turned into one device.
//!
//! Nothing here knows how many devices there are or which ones they are. It takes a list of
//! descriptions in configuration order and works out the aggregate's channel list and their names,
//! the buffer sizes every device can take, the one latency figure each way, and how far each
//! device has to be held back so that the channels line up.

use crate::config::{Alignment, Config, DeviceConfig};
use crate::sub::Description;
use gazelle_audio_stream_abi::sample;

/// The longest name the interface carries for a channel: thirty one characters and a terminator.
pub const MAX_NAME: usize = 31;

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

/// A channel's name, as short as the interface carries one: "Quadro 1", "Studio+ 12".
pub fn channel_name(device: &str, number: usize) -> String {
    let tail = format!(" {number}");
    let room = MAX_NAME.saturating_sub(tail.len());
    let mut head: String = device.chars().take(room).collect();
    while head.len() > room {
        head.pop();
    }
    format!("{}{tail}", head.trim_end())
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
            latency_in: device.description.latency_in,
            latency_out: device.description.latency_out,
            pad_in: 0,
            pad_out: 0,
            buffered: index != master,
        });
    }

    if devices.iter().all(|d| d.inputs.is_empty() && d.outputs.is_empty()) {
        return Err("the configuration exposes no channels at all".to_string());
    }

    // A device that is not the master hands its audio over a buffer, which costs one block each
    // way. Everything else is the device's own reported latency.
    let path_in: Vec<i32> = devices.iter().map(|d| d.latency_in + if d.buffered { block } else { 0 }).collect();
    let path_out: Vec<i32> = devices.iter().map(|d| d.latency_out + if d.buffered { block } else { 0 }).collect();
    let input_latency = path_in.iter().copied().max().unwrap_or(0);
    let output_latency = path_out.iter().copied().max().unwrap_or(0);

    if config.alignment == Alignment::Aligned {
        // Hold the earlier devices back to the slowest path, so every channel of every device is
        // the same distance from the converter.
        for (device, (&here_in, &here_out)) in devices.iter_mut().zip(path_in.iter().zip(path_out.iter())) {
            device.pad_in = input_latency - here_in;
            device.pad_out = output_latency - here_out;
        }
    }

    let mut inputs = Vec::new();
    let mut input_names = Vec::new();
    let mut outputs = Vec::new();
    let mut output_names = Vec::new();
    for (index, device) in devices.iter().enumerate() {
        for (slot, channel) in device.inputs.iter().enumerate() {
            inputs.push(ChannelRef { device: index, slot });
            input_names.push(channel_name(&device.name, *channel as usize + 1));
        }
        for (slot, channel) in device.outputs.iter().enumerate() {
            outputs.push(ChannelRef { device: index, slot });
            output_names.push(channel_name(&device.name, *channel as usize + 1));
        }
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
        }
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
}
