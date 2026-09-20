//! The configuration file: `%APPDATA%\gazelle\aggregate.json`.
//!
//! Read once, at `init`, and never on a callback thread. Everything in it is optional, and a field
//! this version does not know is ignored rather than refused, so an older driver can read a file
//! written by a newer one. A file that is not valid JSON is a refusal with the file named, because
//! silently ignoring what someone asked for is worse than saying no.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Where the file lives, under the same folder Gazelle keeps its other settings in.
pub const FILE_NAME: &str = "aggregate.json";
pub const FOLDER: &str = "gazelle";

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
    /// Samples to add to this device's reported input latency, when the figure its driver gives is
    /// not the whole truth. A device that records late takes a negative trim, which brings it
    /// forward; one that records early takes a positive one. Measured by recording one source into
    /// both devices and comparing, which the README describes.
    #[serde(default)]
    pub input_trim: Option<i32>,
    /// The same for the device's outputs.
    #[serde(default)]
    pub output_trim: Option<i32>,
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
