//! The configuration file: `%APPDATA%\gazelle\aggregate.json`.
//!
//! Read once, at `init`, and never on a callback thread. Everything in it is optional, and a field
//! this version does not know is ignored rather than refused, so an older driver can read a file
//! written by a newer one. A file that is not valid JSON is a refusal with the file named, because
//! silently ignoring what someone asked for is worse than saying no.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Where the file lives, under the same folder Gazelle keeps its other settings in. Both names
/// come from the shared status crate, so that Gazelle and the driver cannot disagree about them.
pub const FILE_NAME: &str = gazelle_audio_aggregate_status::names::CONFIG_FILE;
pub const FOLDER: &str = gazelle_audio_aggregate_status::names::FOLDER;

/// How the aggregate lines its devices up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
pub enum Alignment {
    /// Every device's inputs and outputs line up with every other's, at the cost of delaying the
    /// master by about one buffer. This is the default: a person recording two interfaces at once
    /// wants the tracks to land together.
    #[default]
    Aligned,
    /// The master's path is direct and the other devices sit about one buffer behind it. Lower
    /// latency on the master, and the tracks do not line up.
    LowestLatency,
}

impl From<String> for Alignment {
    fn from(value: String) -> Self {
        match value.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "lowest_latency" => Alignment::LowestLatency,
            // Anything else, including a word a newer version understands, is the default.
            _ => Alignment::Aligned,
        }
    }
}

impl Alignment {
    pub fn as_str(self) -> &'static str {
        match self {
            Alignment::Aligned => "aligned",
            Alignment::LowestLatency => "lowest_latency",
        }
    }
}

/// What a person calls a device's channels, by the device's own channel number from zero.
///
/// What the file holds is taken as it is written, and any objection to it is kept here rather than
/// raised on the spot: by the time serde is reading this it no longer knows which device it is
/// inside, and a refusal that cannot name the device is one nobody can act on.
#[derive(Clone, Debug, Default)]
pub struct Labels {
    by_channel: BTreeMap<u32, String>,
    problem: Option<String>,
}

impl Labels {
    /// What this device's channel is called, by the device's own number from zero. A name that is
    /// empty or only spaces is nobody's name for anything, so it counts as not given.
    pub fn get(&self, channel: u32) -> Option<&str> {
        self.by_channel.get(&channel).map(|label| label.trim()).filter(|label| !label.is_empty())
    }

    /// What is wrong with the way it was written, if anything is.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.by_channel.is_empty()
    }

    fn refused(why: String) -> Labels {
        Labels { by_channel: BTreeMap::new(), problem: Some(why) }
    }

    /// Read one device's channel names out of whatever the file put there.
    fn read(value: serde_json::Value) -> Labels {
        let serde_json::Value::Object(fields) = value else {
            return Labels::refused(
                "channel names are written as an object of channel number to name, such as {\"0\": \"Vocal mic\"}"
                    .to_string(),
            );
        };
        let mut by_channel = BTreeMap::new();
        for (key, value) in fields {
            let Ok(channel) = key.trim().parse::<u32>() else {
                return Labels::refused(format!(
                    "\"{key}\" is not a channel number, and channel names are keyed by the device's own channel numbers, from zero"
                ));
            };
            let Some(label) = value.as_str() else {
                return Labels::refused(format!("the name for channel {channel} is not text"));
            };
            by_channel.insert(channel, label.to_string());
        }
        Labels { by_channel, problem: None }
    }
}

impl FromIterator<(u32, String)> for Labels {
    fn from_iter<I: IntoIterator<Item = (u32, String)>>(pairs: I) -> Labels {
        Labels { by_channel: pairs.into_iter().collect(), problem: None }
    }
}

impl<'de> Deserialize<'de> for Labels {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Labels, D::Error> {
        serde_json::Value::deserialize(deserializer).map(Labels::read)
    }
}

/// The digital path the driver measures a follower's phase over: which output of the device that
/// drives the callback feeds it, and which of its own inputs the cable arrives on, and the phase
/// that was measured over it while this device's trim was measured.
///
/// Both channels are the **devices' own** channel numbers from zero, the numbering `inputs` and
/// `outputs` use, so the setting survives a change to which channels are exposed. Leaving the whole
/// thing out means this interface is not phase measured, which is what every session did before
/// this existed.
///
/// **Three numbers, not one.** The trim is the constant a person measured once with a click. The
/// reference is the phase measured in the same session as that trim. The phase is what each
/// session measures. Every session is lined up by the reference minus the phase, which puts it
/// back in the state the trim was measured in, and then the trim applies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct PhaseConfig {
    /// The callback master's output channel the cable leaves from.
    #[serde(default)]
    pub master_output: Option<i32>,
    /// This device's input channel the cable arrives on.
    #[serde(default)]
    pub input: Option<i32>,
    /// The phase measured in the session this device's trim was measured in, in samples. Left out
    /// means there is nothing to line a session up to yet: each session is measured and left on
    /// the drivers' figures, and the log says the interfaces need measuring once. A calibration
    /// run writes it beside the trim it measured.
    #[serde(default)]
    pub reference: Option<i32>,
}

impl PhaseConfig {
    /// Both channels, when both were given. Anything else is refused when the file is read, so by
    /// the time a plan is made this is the only shape there is.
    pub fn channels(&self) -> Option<(i32, i32)> {
        match (self.master_output, self.input) {
            (Some(output), Some(input)) => Some((output, input)),
            _ => None,
        }
    }

    /// What is wrong with the way it was written, if anything is.
    fn problem(&self) -> Option<&'static str> {
        match (self.master_output, self.input) {
            (Some(output), Some(input)) if output >= 0 && input >= 0 => None,
            (None, _) => Some(
                "phase needs master_output, which is the output channel of the device that drives the callback that the cable leaves from",
            ),
            (_, None) => Some("phase needs input, which is this device's own input channel the cable arrives on"),
            _ => Some("phase names a channel below zero, and channels are numbered from zero"),
        }
    }
}

/// One sub-device, and how to find it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct DeviceConfig {
    /// The registry key name the vendor driver registers itself under. Matched without case, and
    /// as a part of the name when nothing matches it whole.
    #[serde(default)]
    pub key: Option<String>,
    /// The vendor driver's class id, which is the sure way to name one.
    #[serde(default)]
    pub clsid: Option<String>,
    /// What to call this device's channels. Defaults to the registry key name.
    #[serde(default)]
    pub name: Option<String>,
    /// Which of the device's inputs to expose, by the device's own numbering from zero. Left out
    /// means all of them.
    #[serde(default)]
    pub inputs: Option<Vec<i32>>,
    /// Which of the device's outputs to expose. Left out means all of them.
    #[serde(default)]
    pub outputs: Option<Vec<i32>>,
    /// What the person calls this device's inputs, by the device's own channel numbering from
    /// zero, which is the numbering `inputs` uses. A name for a channel that is not exposed is
    /// simply unused.
    #[serde(default)]
    pub input_names: Labels,
    /// The same for the device's outputs.
    #[serde(default)]
    pub output_names: Labels,
    /// Samples to add to this device's reported input latency, when the figure its driver gives is
    /// not the whole truth. A device that records **late** has a longer path than it admits, so it
    /// takes a **positive** trim, and everything else is then held back to match it; a device that
    /// records early takes a negative one. Measured by recording one source into both devices and
    /// comparing, which the README describes.
    #[serde(default)]
    pub input_trim: Option<i32>,
    /// The same for the device's outputs.
    #[serde(default)]
    pub output_trim: Option<i32>,
    /// How to measure this device's capture phase at the start of a session, over the digital
    /// cable that already locks it to the others. Left out means it is not measured, and the
    /// session runs on the figures the drivers report, exactly as it did before.
    #[serde(default)]
    pub phase: Option<PhaseConfig>,
}

impl DeviceConfig {
    /// How this entry reads in a message.
    pub fn described(&self) -> String {
        match (&self.name, &self.key, &self.clsid) {
            (Some(name), _, _) => name.clone(),
            (_, Some(key), _) => key.clone(),
            (_, _, Some(clsid)) => clsid.clone(),
            _ => "a device with no name, key or class id".to_string(),
        }
    }
}

/// The whole file. Every field has a default, so `{}` is a valid configuration and so is no file.
#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    /// The sub-devices, in the order their channels appear to the DAW. Empty means: every
    /// Antelope driver this PC has, in registry order.
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
    /// Which device drives the DAW's callback, by its friendly name, its registry key or its class
    /// id. Left out means the first device in the list.
    #[serde(default)]
    pub callback_master: Option<String>,
    #[serde(default)]
    pub alignment: Alignment,
    /// The rate to put every device at when the driver is opened. Left out means whatever the DAW
    /// asks for.
    #[serde(default)]
    pub rate: Option<f64>,
    /// The buffer size to offer the DAW as preferred. Left out means the size the devices agree on.
    #[serde(default)]
    pub buffer_size: Option<i32>,
    /// How many of the master's buffers a device may miss before it is called stalled: its inputs
    /// read as silence and its outputs are muted until it comes back.
    #[serde(default = "default_stall_after")]
    pub stall_after_buffers: u32,
    /// How many of the master's buffers a stalled device must call back for before it is trusted
    /// again.
    #[serde(default = "default_recover_after")]
    pub recover_after_buffers: u32,
    /// How many buffers of slack each device's ring holds. Three is the smallest that lets a
    /// device run a whole buffer early or late without dropping anything.
    #[serde(default = "default_ring_buffers")]
    pub ring_buffers: usize,
}

fn default_stall_after() -> u32 {
    4
}

fn default_recover_after() -> u32 {
    2
}

fn default_ring_buffers() -> usize {
    4
}

impl Default for Config {
    fn default() -> Self {
        Config {
            devices: Vec::new(),
            callback_master: None,
            alignment: Alignment::default(),
            rate: None,
            buffer_size: None,
            stall_after_buffers: default_stall_after(),
            recover_after_buffers: default_recover_after(),
            ring_buffers: default_ring_buffers(),
        }
    }
}

impl Config {
    /// Read the file, or take the defaults when it is not there.
    pub fn read(path: &Path) -> Result<Config, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
        };
        Config::parse(&text).map_err(|why| format!("{} is not valid: {why}", path.display()))
    }

    /// Parse the text of the file. Unknown fields are ignored.
    pub fn parse(text: &str) -> Result<Config, String> {
        if text.trim().is_empty() {
            return Ok(Config::default());
        }
        serde_json::from_str(text).map_err(|error| error.to_string()).and_then(|config: Config| config.checked())
    }

    /// The things that are wrong however they were written.
    fn checked(self) -> Result<Config, String> {
        if self.ring_buffers < 2 {
            return Err("ring_buffers must be at least 2".to_string());
        }
        if let Some(size) = self.buffer_size {
            if size <= 0 {
                return Err("buffer_size must be more than zero".to_string());
            }
        }
        if let Some(rate) = self.rate {
            if !(rate.is_finite() && rate > 0.0) {
                return Err("rate must be a positive number of samples a second".to_string());
            }
        }
        for device in &self.devices {
            if device.key.is_none() && device.clsid.is_none() {
                return Err("every device needs a key or a clsid to find it by".to_string());
            }
            for channels in [&device.inputs, &device.outputs].into_iter().flatten() {
                if channels.iter().any(|&c| c < 0) {
                    return Err(format!("{} lists a channel below zero", device.described()));
                }
            }
            if let Some(why) = device.phase.as_ref().and_then(PhaseConfig::problem) {
                return Err(format!("{}: {why}", device.described()));
            }
            for (field, labels) in [("input_names", &device.input_names), ("output_names", &device.output_names)] {
                if let Some(why) = labels.problem() {
                    return Err(format!("{}'s {field}: {why}", device.described()));
                }
            }
        }
        Ok(self)
    }
}

/// Where the file is looked for: `%APPDATA%\gazelle\aggregate.json`.
pub fn config_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(Path::new(&appdata).join(FOLDER).join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_file_at_all_is_the_default_configuration() {
        let missing = std::env::temp_dir().join("gazelle-aggregate-no-such-file-2c9f.json");
        let config = Config::read(&missing).expect("a missing file is not an error");
        assert!(config.devices.is_empty(), "no devices named means every Antelope driver found");
        assert_eq!(config.alignment, Alignment::Aligned);
        assert!(config.callback_master.is_none());
        assert_eq!(config.stall_after_buffers, 4);
        assert_eq!(config.ring_buffers, 4);
    }

    #[test]
    fn an_empty_object_is_also_the_default() {
        let config = Config::parse("{}").expect("an empty object is valid");
        assert!(config.devices.is_empty());
        assert_eq!(config.alignment, Alignment::Aligned);
    }

    #[test]
    fn a_field_this_version_does_not_know_is_ignored() {
        let config = Config::parse(
            r#"{"devices": [{"key": "Zen Quadro", "name": "Quadro", "colour": "blue"}],
                "written_by": "a later version", "alignment": "lowest_latency"}"#,
        )
        .expect("an unknown field is not a refusal");
        assert_eq!(config.devices.len(), 1);
        assert_eq!(config.devices[0].name.as_deref(), Some("Quadro"));
        assert_eq!(config.alignment, Alignment::LowestLatency);
    }

    #[test]
    fn an_alignment_word_this_version_does_not_know_falls_back_to_lining_up() {
        let config = Config::parse(r#"{"alignment": "something_new"}"#).expect("still valid");
        assert_eq!(config.alignment, Alignment::Aligned);
        // Spelling and case are not what the answer turns on.
        assert_eq!(Config::parse(r#"{"alignment": "Lowest-Latency"}"#).unwrap().alignment, Alignment::LowestLatency);
    }

    #[test]
    fn everything_the_file_can_say_is_read() {
        let config = Config::parse(
            r#"{
                "devices": [
                    {"key": "Zen Quadro Synergy Core", "name": "Quadro", "inputs": [0, 1], "outputs": [0, 1, 2, 3]},
                    {"clsid": "{AE4A4452-A316-11E5-A113-080027F6C1F4}", "name": "Studio+"}
                ],
                "callback_master": "Quadro",
                "alignment": "aligned",
                "rate": 96000,
                "buffer_size": 512,
                "stall_after_buffers": 8,
                "recover_after_buffers": 3,
                "ring_buffers": 6
            }"#,
        )
        .expect("the worked example in the README must parse");
        assert_eq!(config.devices.len(), 2);
        assert_eq!(config.devices[0].inputs.as_deref(), Some(&[0, 1][..]));
        assert_eq!(config.devices[1].outputs, None, "a device that says nothing exposes all of its channels");
        assert_eq!(config.callback_master.as_deref(), Some("Quadro"));
        assert_eq!(config.rate, Some(96_000.0));
        assert_eq!(config.buffer_size, Some(512));
        assert_eq!(config.stall_after_buffers, 8);
        assert_eq!(config.recover_after_buffers, 3);
        assert_eq!(config.ring_buffers, 6);
    }

    #[test]
    fn the_names_a_person_gives_their_channels_survive_the_file() {
        let path = std::env::temp_dir().join("gazelle-aggregate-names-8b3d.json");
        std::fs::write(
            &path,
            r#"{
                "devices": [
                    {"key": "Zen Quadro Synergy Core", "name": "Quadro",
                     "input_names": {"0": "Vocal mic", "2": "DI"},
                     "output_names": {"0": "Main L", "1": "Main R"}}
                ]
            }"#,
        )
        .expect("the scratch file is writable");
        let config = Config::read(&path).expect("channel names are ordinary configuration");
        let device = &config.devices[0];
        assert_eq!(device.input_names.get(0), Some("Vocal mic"));
        assert_eq!(device.input_names.get(2), Some("DI"));
        assert_eq!(device.input_names.get(1), None, "a channel nobody named has no name");
        assert_eq!(device.output_names.get(1), Some("Main R"));
        assert!(device.output_names.get(2).is_none());
        let _ = std::fs::remove_file(&path);
    }

    /// Gazelle writes where the rate in the file came from beside it; the driver takes the rate and
    /// passes over the note, as it does any field it does not know.
    #[test]
    fn a_rate_with_gazelles_note_of_where_it_came_from_is_read_as_the_rate() {
        let config = Config::parse(r#"{"devices": [{"key": "Zen Quadro"}], "rate": 96000, "rate_from": "interfaces"}"#).expect("the note is not a refusal");
        assert_eq!(config.rate, Some(96_000.0));
    }

    #[test]
    fn a_device_that_names_no_channels_has_none_rather_than_a_refusal() {
        let config = Config::parse(r#"{"devices": [{"key": "Zen Quadro"}]}"#).expect("channel names are optional");
        assert!(config.devices[0].input_names.is_empty());
        assert!(config.devices[0].output_names.is_empty());
        let empty = Config::parse(r#"{"devices": [{"key": "Zen Quadro", "input_names": {}}]}"#).expect("still valid");
        assert!(empty.devices[0].input_names.get(0).is_none());
    }

    #[test]
    fn channel_names_written_in_a_way_nothing_can_read_are_refused_and_the_device_is_named() {
        let not_an_object = Config::parse(r#"{"devices": [{"key": "Zen Quadro", "name": "Quadro", "input_names": ["a"]}]}"#)
            .expect_err("a list is not a channel number to name");
        assert!(not_an_object.contains("Quadro") && not_an_object.contains("input_names"), "{not_an_object}");
        let not_a_number =
            Config::parse(r#"{"devices": [{"key": "Zen Quadro", "name": "Quadro", "output_names": {"first": "Main L"}}]}"#)
                .expect_err("a key that is not a channel number is a refusal");
        assert!(not_a_number.contains("Quadro") && not_a_number.contains("first"), "{not_a_number}");
        let not_text = Config::parse(r#"{"devices": [{"key": "Zen Quadro", "name": "Quadro", "input_names": {"0": 5}}]}"#)
            .expect_err("a name is text");
        assert!(not_text.contains("Quadro"), "{not_text}");
    }

    #[test]
    fn a_device_can_say_which_cable_its_phase_is_measured_over() {
        let config = Config::parse(
            r#"{
                "devices": [
                    {"key": "Zen Quadro Synergy Core", "name": "Quadro"},
                    {"key": "ZenStudioTB", "name": "Studio+", "input_trim": 28,
                     "phase": {"master_output": 8, "input": 16}}
                ],
                "callback_master": "Quadro"
            }"#,
        )
        .expect("the worked example in the README must parse");
        assert_eq!(config.devices[0].phase, None, "the device that drives the callback is not measured");
        let phase = config.devices[1].phase.expect("the follower is");
        assert_eq!(phase.channels(), Some((8, 16)));
        assert_eq!(phase.reference, None, "and nothing has been measured to line it up to yet");
        // A trim and a phase are different things, and a device can have both.
        assert_eq!(config.devices[1].input_trim, Some(28));
    }

    /// The reference is written by a calibration run beside the trim it measured, and read back as
    /// exactly what was written: a phase carries a large constant of its own, so a big negative
    /// number is an ordinary one.
    #[test]
    fn a_phase_setting_carries_the_reference_its_trim_was_measured_at() {
        let config = Config::parse(
            r#"{"devices": [{"key": "Zen Quadro"},
                            {"key": "ZenStudioTB", "name": "Studio+", "input_trim": 144,
                             "phase": {"master_output": 8, "input": 16, "reference": -84}}]}"#,
        )
        .expect("a reference is part of a phase setting");
        let phase = config.devices[1].phase.expect("the follower is measured");
        assert_eq!(phase.reference, Some(-84));
        assert_eq!(phase.channels(), Some((8, 16)), "and the cable is where it always was");
        assert_eq!(config.devices[1].input_trim, Some(144), "beside the trim it belongs to");
    }

    #[test]
    fn half_a_phase_setting_is_refused_and_says_which_half_is_missing() {
        let no_input = Config::parse(
            r#"{"devices": [{"key": "a", "name": "Studio+", "phase": {"master_output": 8}}]}"#,
        )
        .expect_err("a signal with nowhere to arrive is not a measurement");
        assert!(no_input.contains("Studio+") && no_input.contains("input"), "{no_input}");
        let no_output =
            Config::parse(r#"{"devices": [{"key": "a", "name": "Studio+", "phase": {"input": 16}}]}"#)
                .expect_err("and one with nowhere to leave from is not either");
        assert!(no_output.contains("master_output"), "{no_output}");
        let below_zero = Config::parse(
            r#"{"devices": [{"key": "a", "name": "Studio+", "phase": {"master_output": -1, "input": 16}}]}"#,
        )
        .expect_err("there is no channel below zero");
        assert!(below_zero.contains("below zero"), "{below_zero}");
    }

    #[test]
    fn a_device_that_says_nothing_about_a_phase_is_not_measured_at_all() {
        let config = Config::parse(r#"{"devices": [{"key": "Zen Quadro"}]}"#).expect("a phase is optional");
        assert_eq!(config.devices[0].phase, None);
    }

    #[test]
    fn a_file_that_is_not_json_is_refused_by_name() {
        let error = Config::parse("{ this is not json").expect_err("broken JSON is a refusal");
        assert!(!error.is_empty());
        let path = std::env::temp_dir().join("gazelle-aggregate-broken-4a71.json");
        std::fs::write(&path, "not json at all").expect("the scratch file is writable");
        let error = Config::read(&path).expect_err("a broken file is a refusal");
        assert!(error.contains("gazelle-aggregate-broken-4a71.json"), "{error}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn nonsense_values_are_refused_rather_than_used() {
        assert!(Config::parse(r#"{"ring_buffers": 1}"#).is_err());
        assert!(Config::parse(r#"{"buffer_size": 0}"#).is_err());
        assert!(Config::parse(r#"{"rate": -48000}"#).is_err());
        assert!(Config::parse(r#"{"devices": [{"name": "nameless"}]}"#).is_err(), "a device must be findable");
        assert!(Config::parse(r#"{"devices": [{"key": "a", "inputs": [-1]}]}"#).is_err());
    }
}
