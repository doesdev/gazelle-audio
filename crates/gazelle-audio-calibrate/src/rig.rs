//! What is plugged into what, and how hard to measure it.
//!
//! A rig is the cabling written down: one output channel per device being measured, and one input
//! channel per device. Which side has to be on one device depends on which side is being measured,
//! and that is the whole difference between the two passes.

use serde::Serialize;

/// Which side of the interfaces this pass measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// One device plays, every device records. What comes out is the residual error between the
    /// devices' **inputs**, and it is cancelled with `input_trim`.
    Inputs,
    /// Every device plays, one device records. What comes out is the residual error between the
    /// devices' **outputs**, and it is cancelled with `output_trim`.
    Outputs,
}

impl Direction {
    /// The word for this side in a message, and the field in `aggregate.json` it belongs to.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Inputs => "inputs",
            Direction::Outputs => "outputs",
        }
    }

    /// The name of the trim a measurement in this direction changes.
    pub fn trim_field(self) -> &'static str {
        match self {
            Direction::Inputs => "input_trim",
            Direction::Outputs => "output_trim",
        }
    }

    /// The side of the cabling that must all be on one device: the side the click has in common,
    /// so that the difference between the channels is the other side and nothing else.
    fn common_side(self) -> &'static str {
        match self {
            Direction::Inputs => "output",
            Direction::Outputs => "input",
        }
    }
}

/// The cabling, in device order: entry `n` of each list belongs to device `n` of the aggregate.
///
/// For [`Direction::Inputs`] every entry of `outputs` is a channel of the **same** device, fanned
/// out to one input of each device. For [`Direction::Outputs`] it is the other way round: one
/// output on each device, all of them into inputs of one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rig {
    pub direction: Direction,
    /// The aggregate output channel the click leaves on, per device.
    pub outputs: Vec<i32>,
    /// The aggregate input channel the click comes back on, per device.
    pub inputs: Vec<i32>,
    /// Which device everything is measured against. Its lag is zero by definition and its trim is
    /// left alone, so this is the interface the others are moved to meet.
    pub reference: usize,
}

impl Rig {
    /// The two cable rig of the README: device order, reference first.
    pub fn new(direction: Direction, outputs: Vec<i32>, inputs: Vec<i32>) -> Rig {
        Rig { direction, outputs, inputs, reference: 0 }
    }

    /// How many devices this rig describes.
    pub fn devices(&self) -> usize {
        self.inputs.len()
    }

    /// Everything that is wrong with the cabling itself, decided without opening anything. What
    /// needs a device to answer is checked later, and still before a sample is played.
    pub fn refusal(&self) -> Option<String> {
        if self.inputs.len() != self.outputs.len() {
            return Some(format!(
                "this rig names {} input channels and {} output channels, and it needs one of each per interface: \
                 write the channel each interface's cable is plugged into, in the order the interfaces appear in \
                 aggregate.json",
                self.inputs.len(),
                self.outputs.len()
            ));
        }
        if self.devices() < 2 {
            return Some(
                "there is nothing to measure with fewer than two interfaces: a lag is the difference between two \
                 of them, so name a channel for each interface in aggregate.json"
                    .to_string(),
            );
        }
        if self.reference >= self.devices() {
            return Some(format!(
                "the reference is interface {}, and this rig only describes {}: the reference is the interface the \
                 others are lined up to, counted from zero",
                self.reference,
                self.devices()
            ));
        }
        for (what, list) in [("input", &self.inputs), ("output", &self.outputs)] {
            if let Some(channel) = list.iter().find(|&&channel| channel < 0) {
                return Some(format!(
                    "{channel} is not a channel: the aggregate's {what} channels are counted from zero, in the order \
                     a DAW lists them"
                ));
            }
        }
        let common = match self.direction {
            Direction::Inputs => &self.outputs,
            Direction::Outputs => &self.inputs,
        };
        if let Some(repeated) = first_repeat(common) {
            return Some(format!(
                "{} channel {repeated} is cabled twice, and every cable needs a channel of its own so that the two \
                 copies of the click can be told apart",
                self.direction.common_side()
            ));
        }
        let separate = match self.direction {
            Direction::Inputs => &self.inputs,
            Direction::Outputs => &self.outputs,
        };
        if let Some(repeated) = first_repeat(separate) {
            return Some(format!(
                "channel {repeated} is named for two interfaces at once, and each interface needs its own cable: \
                 check the channel numbers against the order the interfaces appear in aggregate.json"
            ));
        }
        None
    }

    /// Whether every channel that has to be on one device is, given which device each aggregate
    /// channel belongs to. `on_input` and `on_output` answer with the device index of a channel,
    /// or `None` when the aggregate has no such channel.
    pub fn refusal_against(
        &self,
        on_input: impl Fn(i32) -> Option<usize>,
        on_output: impl Fn(i32) -> Option<usize>,
        names: &[String],
    ) -> Option<String> {
        if names.len() != self.devices() {
            return Some(format!(
                "this rig is written for {} interfaces and the aggregate has {}: every interface in aggregate.json \
                 needs a cable, and nothing else may be in the list",
                self.devices(),
                names.len()
            ));
        }
        let mut input_devices = Vec::new();
        let mut output_devices = Vec::new();
        for (index, (&input, &output)) in self.inputs.iter().zip(self.outputs.iter()).enumerate() {
            let name = names.get(index).map(String::as_str).unwrap_or("that interface");
            let Some(on) = on_input(input) else {
                return Some(format!(
                    "there is no input channel {input} on the aggregate, so {name}'s cable has nowhere to arrive"
                ));
            };
            input_devices.push(on);
            let Some(on) = on_output(output) else {
                return Some(format!(
                    "there is no output channel {output} on the aggregate, so {name}'s cable has nowhere to leave from"
                ));
            };
            output_devices.push(on);
        }

        // The side the click has in common must be one device, or the measurement is the
        // difference between two devices' clocks and two devices' converters at once.
        let (common, side) = match self.direction {
            Direction::Inputs => (&output_devices, "outputs"),
            Direction::Outputs => (&input_devices, "inputs"),
        };
        let first = common[0];
        if let Some(stray) = common.iter().position(|&device| device != first) {
            let here = names.get(first).map(String::as_str).unwrap_or("one interface");
            let there = names.get(common[stray]).map(String::as_str).unwrap_or("another");
            return Some(format!(
                "the {side} this rig names are spread across {here} and {there}, and they all have to be on one \
                 interface: that is what makes both copies of the click leave on the same sample"
            ));
        }

        // And the side being measured must be one channel per device, on its own device.
        let (separate, side, which) = match self.direction {
            Direction::Inputs => (&input_devices, "input", &self.inputs),
            Direction::Outputs => (&output_devices, "output", &self.outputs),
        };
        for (index, (&device, &channel)) in separate.iter().zip(which.iter()).enumerate() {
            if device != index {
                let name = names.get(index).map(String::as_str).unwrap_or("that interface");
                let actually = names.get(device).map(String::as_str).unwrap_or("another interface");
                return Some(format!(
                    "{side} channel {channel} was given as {name}'s, but it belongs to {actually}: the cables are \
                     listed in the order the interfaces appear in aggregate.json"
                ));
            }
        }
        None
    }
}

fn first_repeat(list: &[i32]) -> Option<i32> {
    for (index, &value) in list.iter().enumerate() {
        if list[index + 1..].contains(&value) {
            return Some(value);
        }
    }
    None
}

/// The loudest click this crate will ever play, whatever it is asked for. Well below full scale,
/// because the far end of the cable is somebody's monitoring.
pub const LOUDEST_DBFS: f64 = -6.0;

/// How the run is made: how many clicks, how far apart, how loud, and what to ask the devices for.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Settings {
    /// How many clicks the run plays. Each one is a measurement of its own, and several of them
    /// are what turn one number into a number with a spread and a slope.
    pub clicks: u32,
    /// Seconds between one click and the next.
    pub spacing_seconds: f64,
    /// How long a click is. A handful of samples, not one: a converter's filters smear a single
    /// sample into something with no defined start.
    pub click_samples: usize,
    /// How loud the click is, in dBFS. Held at [`LOUDEST_DBFS`] however large a number is asked
    /// for, because this plays into whatever is plugged in.
    pub level_dbfs: f64,
    /// The rate to put every device at. Left out means whatever they are already running at.
    pub rate: Option<f64>,
    /// The buffer size to ask for. Left out means what the devices prefer.
    pub buffer_size: Option<i32>,
    /// Seconds of run to let go by before the first click, so that nothing is measured while the
    /// devices are still settling into their first buffers.
    pub settle_seconds: f64,
    /// How far either side of the reference's click the other channels are searched, in samples.
    /// Tens of samples is the error being looked for; this is room around it, not a target.
    pub search_samples: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            clicks: 8,
            spacing_seconds: 0.5,
            click_samples: 64,
            level_dbfs: -20.0,
            rate: None,
            buffer_size: None,
            settle_seconds: 0.5,
            search_samples: 512,
        }
    }
}

impl Settings {
    /// The level actually played, which is never louder than [`LOUDEST_DBFS`] and is silence when
    /// a caller asks for something that is not a number.
    pub fn level(&self) -> f64 {
        if !self.level_dbfs.is_finite() {
            return 0.0;
        }
        let held = self.level_dbfs.min(LOUDEST_DBFS);
        10f64.powf(held / 20.0)
    }

    /// Everything that is wrong with the settings themselves.
    pub fn refusal(&self) -> Option<String> {
        if self.clicks < 2 {
            return Some(
                "a run needs at least two clicks: one number on its own has no spread and no slope, so there is no \
                 way to tell a measurement from a coincidence"
                    .to_string(),
            );
        }
        if !(self.spacing_seconds.is_finite() && self.spacing_seconds > 0.0) {
            return Some("the clicks have to be some time apart, and this run puts them all at once".to_string());
        }
        if self.click_samples < 8 {
            return Some(format!(
                "a click of {} samples is too short to find again: a converter's filters need a shaped burst of a few \
                 dozen samples to keep a start worth measuring",
                self.click_samples
            ));
        }
        if self.search_samples == 0 {
            return Some("nothing can be found if the search is zero samples wide".to_string());
        }
        if !self.settle_seconds.is_finite() || self.settle_seconds < 0.0 {
            return Some("the settling time is not a number of seconds".to_string());
        }
        if let Some(rate) = self.rate {
            if !(rate.is_finite() && rate > 0.0) {
                return Some(format!("{rate} is not a sample rate"));
            }
        }
        if let Some(size) = self.buffer_size {
            if size <= 0 {
                return Some(format!("{size} samples is not a buffer size"));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rig of the README: the Quadro plays out of two outputs, one into each interface.
    fn two_cables() -> Rig {
        Rig::new(Direction::Inputs, vec![0, 1], vec![0, 16])
    }

    /// Which device each channel is on, for the two interface aggregate: sixteen Quadro channels
    /// and then the Studio+'s.
    fn on_device(channel: i32) -> Option<usize> {
        match channel {
            0..=15 => Some(0),
            16..=39 => Some(1),
            _ => None,
        }
    }

    fn names() -> Vec<String> {
        vec!["Quadro".to_string(), "Studio+".to_string()]
    }

    #[test]
    fn the_two_cable_rig_from_the_readme_is_accepted_as_written() {
        let rig = two_cables();
        assert_eq!(rig.refusal(), None);
        assert_eq!(rig.refusal_against(on_device, on_device, &names()), None);
        assert_eq!(rig.devices(), 2);
    }

    #[test]
    fn a_third_interface_is_a_third_cable_and_nothing_else() {
        let rig = Rig::new(Direction::Inputs, vec![0, 1, 2], vec![0, 16, 40]);
        assert_eq!(rig.refusal(), None);
        let on = |channel: i32| match channel {
            0..=15 => Some(0),
            16..=39 => Some(1),
            40..=71 => Some(2),
            _ => None,
        };
        let three = vec!["Quadro".to_string(), "Studio+".to_string(), "Orion".to_string()];
        assert_eq!(rig.refusal_against(on, on, &three), None);
    }

    #[test]
    fn measuring_the_outputs_is_the_same_rig_with_the_cabling_turned_round() {
        // One output on each interface, all of them into inputs of the first.
        let rig = Rig::new(Direction::Outputs, vec![0, 16], vec![0, 1]);
        assert_eq!(rig.refusal(), None);
        assert_eq!(rig.refusal_against(on_device, on_device, &names()), None);
    }

    #[test]
    fn outputs_spread_over_two_interfaces_are_refused_when_the_inputs_are_being_measured() {
        // Both copies of the click must leave the same device on the same sample, or the number
        // that comes out is not the inputs.
        let rig = Rig::new(Direction::Inputs, vec![0, 16], vec![0, 16]);
        let refusal = rig.refusal_against(on_device, on_device, &names()).expect("the outputs are on two devices");
        assert!(refusal.contains("Quadro") && refusal.contains("Studio+"), "{refusal}");
        assert!(refusal.contains("one interface"), "{refusal}");
    }

    #[test]
    fn an_input_that_is_not_on_the_interface_it_was_listed_for_is_refused_by_name() {
        // Both cables arrive on the Quadro, so the second interface is not being measured at all.
        let rig = Rig::new(Direction::Inputs, vec![0, 1], vec![0, 2]);
        let refusal = rig.refusal_against(on_device, on_device, &names()).expect("the second cable is on the Quadro");
        assert!(refusal.contains("Studio+") && refusal.contains("Quadro"), "{refusal}");
    }

    #[test]
    fn a_channel_the_aggregate_has_not_got_is_refused_and_the_interface_is_named() {
        let rig = Rig::new(Direction::Inputs, vec![0, 1], vec![0, 99]);
        let refusal = rig.refusal_against(on_device, on_device, &names()).expect("there is no channel 99");
        assert!(refusal.contains("99") && refusal.contains("Studio+"), "{refusal}");
    }

    #[test]
    fn a_rig_that_does_not_cover_every_interface_is_refused_before_anything_is_opened() {
        let one = Rig::new(Direction::Inputs, vec![0], vec![0]);
        let refusal = one.refusal().expect("one interface is nothing to compare");
        assert!(refusal.contains("two"), "{refusal}");
        let lopsided = Rig::new(Direction::Inputs, vec![0, 1, 2], vec![0, 16]);
        assert!(lopsided.refusal().expect("the two lists differ").contains("one of each"));
        let missing = two_cables().refusal_against(on_device, on_device, &["Quadro".to_string()]);
        assert!(missing.expect("the aggregate has one device").contains("2"));
    }

    #[test]
    fn one_cable_named_twice_is_refused_rather_than_measured() {
        let same_output = Rig::new(Direction::Inputs, vec![0, 0], vec![0, 16]);
        assert!(same_output.refusal().expect("one output, two cables").contains("twice"));
        let same_input = Rig::new(Direction::Inputs, vec![0, 1], vec![0, 0]);
        assert!(same_input.refusal().expect("one input for two interfaces").contains("two interfaces"));
        let negative = Rig::new(Direction::Inputs, vec![0, 1], vec![0, -3]);
        assert!(negative.refusal().is_some());
    }

    #[test]
    fn the_click_is_never_louder_than_the_cap_however_loudly_it_is_asked_for() {
        let modest = Settings::default();
        assert!((modest.level() - 0.1).abs() < 1e-9, "minus twenty dBFS is a tenth of full scale");
        let shouted = Settings { level_dbfs: 0.0, ..Settings::default() };
        assert!(shouted.level() <= 10f64.powf(LOUDEST_DBFS / 20.0) + 1e-12);
        let nonsense = Settings { level_dbfs: f64::NAN, ..Settings::default() };
        assert_eq!(nonsense.level(), 0.0, "a level that is not a number plays nothing at all");
    }

    #[test]
    fn settings_that_could_not_measure_anything_are_refused_in_plain_words() {
        assert!(Settings { clicks: 1, ..Settings::default() }.refusal().is_some());
        assert!(Settings { spacing_seconds: 0.0, ..Settings::default() }.refusal().is_some());
        assert!(Settings { click_samples: 1, ..Settings::default() }.refusal().is_some());
        assert!(Settings { rate: Some(0.0), ..Settings::default() }.refusal().is_some());
        assert!(Settings { buffer_size: Some(-1), ..Settings::default() }.refusal().is_some());
        assert_eq!(Settings::default().refusal(), None);
    }
}
