//! What is plugged into what, and how hard to measure it.
//!
//! A rig is the cabling written down: one output channel per device being measured, and one input
//! channel per device. Which side has to be on one device depends on which side is being measured,
//! and that is the whole difference between the two passes.
//!
//! On top of that a rig may carry **witnesses**: extra input channels that are recorded and
//! reported and take no part in any trim. One input per interface is what the trim arithmetic
//! means, so a second input on an interface cannot be a measurement; but it can be an observation,
//! and an observation is how a question about one interface's own inputs gets answered in a single
//! run.

use gazelle_aggregate::plan::{ChannelRef, Plan};
use gazelle_aggregate::sub::Description;
use serde::{Deserialize, Serialize};

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

/// One channel of one interface, named the way the interface itself numbers it: which interface,
/// by its place in `aggregate.json` counted from zero, and which of that interface's own channels,
/// counted from zero.
///
/// **Why not the aggregate's own channel number.** The aggregate's numbering is worked out from how
/// many channels each interface's driver really has, which only the drivers know, and from which of
/// them the setup keeps out or keeps for the phase measurement. Anything that counts them without
/// opening the drivers can be wrong, and a count that is two short puts every cable on the next
/// interface along. The run opens the drivers anyway, so it is the run that translates, and nothing
/// upstream of it has to know the layout at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, expecting = "an interface and one of its channels, as {\"device\": n, \"channel\": c}")]
pub struct Pick {
    /// The interface, by its place in `aggregate.json`, from zero.
    pub device: i32,
    /// That interface's own channel number, from zero.
    pub channel: i32,
}

impl Pick {
    pub fn new(device: i32, channel: i32) -> Pick {
        Pick { device, channel }
    }

    /// The interface this names, once it is known to be one of them.
    fn index(self) -> usize {
        usize::try_from(self.device).unwrap_or(usize::MAX)
    }

    /// What to call this channel in a sentence before anything is open: there are no names yet,
    /// so the interface is called by its place in the list. Channels are counted from one here,
    /// as a person counts them.
    fn unnamed(self, what: &str) -> String {
        format!("{what} {} of {}", self.channel.saturating_add(1), ordinal(self.device))
    }

    /// What to call this channel once the interfaces are known: "Studio+ input 3".
    fn named(self, what: &str, names: &[String]) -> String {
        let name = names.get(self.index()).map(String::as_str).unwrap_or("that interface");
        format!("{name} {what} {}", self.channel.saturating_add(1))
    }
}

/// An interface by its place in the list, in words.
fn ordinal(device: i32) -> String {
    const WORDS: [&str; 8] = ["first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth"];
    match usize::try_from(device).ok().and_then(|index| WORDS.get(index)) {
        Some(word) => format!("the {word} interface"),
        None => format!("interface {device}"),
    }
}

/// The cabling, in device order: entry `n` of `inputs` and `outputs` is the cable for device `n`
/// of the aggregate.
///
/// For [`Direction::Inputs`] every entry of `outputs` is a channel of the **same** device, fanned
/// out to one input of each device. For [`Direction::Outputs`] it is the other way round: one
/// output on each device, all of them into inputs of one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rig {
    pub direction: Direction,
    /// The output channel the click leaves on, per device.
    pub outputs: Vec<Pick>,
    /// The input channel the click comes back on, per device.
    pub inputs: Vec<Pick>,
    /// Extra input channels to record and report, which take no part in any trim.
    ///
    /// A witness may be any input channel the aggregate has, **including a second one on an
    /// interface that is already being measured**, which is the only way to see what an
    /// interface's own inputs do against each other in one run. It may not be a channel this rig
    /// is already measuring, because that channel is already a reading.
    pub witnesses: Vec<Pick>,
    /// Which device everything is measured against. Its lag is zero by definition and its trim is
    /// left alone, so this is the interface the others are moved to meet.
    pub reference: usize,
}

/// One interface as the opened aggregate has it: what its driver really has, and which of its
/// channels the setup keeps for the driver's phase measurement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    /// What `aggregate.json` calls it.
    pub name: String,
    /// How many inputs its driver has, whether or not the setup exposes them.
    pub inputs: i32,
    pub outputs: i32,
    /// Its own input its phase is measured on, when the setup measures one.
    pub phase_input: Option<i32>,
    /// Its own outputs another interface's phase is measured from, each with the interface whose
    /// phase that is.
    pub phase_outputs: Vec<(i32, String)>,
}

/// The aggregate's layout, as the run finds it once the drivers are open: every interface, and the
/// aggregate's own inputs and outputs as `(interface, that interface's own channel)`, in the order a
/// DAW is given them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub interfaces: Vec<Interface>,
    pub inputs: Vec<(usize, i32)>,
    pub outputs: Vec<(usize, i32)>,
}

impl Layout {
    /// The layout of an opened aggregate, from its plan and from what each driver said it has.
    pub fn of(plan: &Plan, drivers: &[Description]) -> Layout {
        let mut interfaces: Vec<Interface> = plan
            .devices
            .iter()
            .enumerate()
            .map(|(index, device)| {
                // The plan only has the channels the setup exposes. Should a driver's own answer be
                // missing, the most that can honestly be said is the highest channel it opened.
                let opened = |list: &[i32]| list.iter().copied().max().map_or(0, |top| top + 1);
                let driver = drivers.get(index);
                Interface {
                    name: device.name.clone(),
                    inputs: driver.map_or_else(|| opened(&device.inputs), |driver| driver.inputs),
                    outputs: driver.map_or_else(|| opened(&device.outputs), |driver| driver.outputs),
                    phase_input: device.phase.map(|phase| phase.input),
                    phase_outputs: Vec::new(),
                }
            })
            .collect();
        for device in &plan.devices {
            if let (Some(phase), Some(master)) = (device.phase, interfaces.get_mut(plan.master)) {
                master.phase_outputs.push((phase.master_output, device.name.clone()));
            }
        }
        let own = |list: &[ChannelRef], input: bool| -> Vec<(usize, i32)> {
            list.iter()
                .filter_map(|at| {
                    let device = plan.devices.get(at.device)?;
                    let channels = if input { &device.inputs } else { &device.outputs };
                    Some((at.device, *channels.get(at.slot)?))
                })
                .collect()
        };
        Layout { inputs: own(&plan.inputs, true), outputs: own(&plan.outputs, false), interfaces }
    }

    fn names(&self) -> Vec<String> {
        self.interfaces.iter().map(|interface| interface.name.clone()).collect()
    }
}

/// The rig in the aggregate's own channel numbers, which is what its buffers are asked for by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wired {
    pub outputs: Vec<i32>,
    pub inputs: Vec<i32>,
    pub witnesses: Vec<i32>,
}

impl Rig {
    /// The two cable rig of the README: device order, reference first, nothing carried along.
    pub fn new(direction: Direction, outputs: Vec<Pick>, inputs: Vec<Pick>) -> Rig {
        Rig { direction, outputs, inputs, witnesses: Vec::new(), reference: 0 }
    }

    /// The same rig, listening in on these input channels as well.
    pub fn watching(mut self, witnesses: Vec<Pick>) -> Rig {
        self.witnesses = witnesses;
        self
    }

    /// How many devices this rig describes.
    pub fn devices(&self) -> usize {
        self.inputs.len()
    }

    /// Everything that is wrong with the cabling itself, decided without opening anything. What
    /// needs the drivers to answer is checked by [`Rig::wired`], and still before a sample is
    /// played.
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
        for (what, list) in [("input", &self.inputs), ("output", &self.outputs), ("input", &self.witnesses)] {
            for pick in list.iter() {
                if pick.device < 0 {
                    return Some(format!(
                        "interface {} is not one of them: a request counts the interfaces from zero, in the order \
                         aggregate.json lists them",
                        pick.device
                    ));
                }
                if pick.channel < 0 {
                    return Some(format!(
                        "{} is not a channel: a request counts each interface's own {what}s from zero, so its first \
                         {what} is 0",
                        pick.channel
                    ));
                }
            }
        }
        let common = match self.direction {
            Direction::Inputs => &self.outputs,
            Direction::Outputs => &self.inputs,
        };
        if let Some(repeated) = first_repeat(common) {
            return Some(format!(
                "{} is cabled twice, and every cable needs a channel of its own so that the two copies of the click \
                 can be told apart",
                repeated.unnamed(self.direction.common_side())
            ));
        }
        let (separate, side) = match self.direction {
            Direction::Inputs => (&self.inputs, "input"),
            Direction::Outputs => (&self.outputs, "output"),
        };
        if let Some(repeated) = first_repeat(separate) {
            return Some(format!(
                "{} is named for two interfaces at once, and each interface needs its own cable",
                repeated.unnamed(side)
            ));
        }
        if let Some(repeated) = first_repeat(&self.witnesses) {
            return Some(format!(
                "{} is listened in on twice, and one channel is one recording: name it once and it is carried along \
                 once",
                repeated.unnamed("input")
            ));
        }
        if let Some(clash) = self.witnesses.iter().find(|pick| self.inputs.contains(pick)) {
            return Some(format!(
                "{} is already one of the channels this run measures, so it cannot also be carried along as a \
                 witness: a witness is an extra channel to listen in on, and this one is already a reading",
                clash.unnamed("input")
            ));
        }
        None
    }

    /// **The rig in the aggregate's own channel numbers**, now that the drivers are open and the
    /// layout is known, or the sentence that says what is wrong with it and what would be right.
    ///
    /// This is the one place the interface's own numbering meets the aggregate's, so every way a
    /// name can miss is refused here, before a buffer is made: an interface that is not one of
    /// them, a channel the interface has not got, one the setup keeps out of the aggregate, and
    /// one the setup keeps for the phase measurement.
    pub fn wired(&self, layout: &Layout) -> Result<Wired, String> {
        let names = layout.names();
        if names.len() != self.devices() {
            return Err(format!(
                "this rig is written for {} interfaces and the aggregate has {} ({}): every interface in \
                 aggregate.json needs a cable, and nothing else may be in the list",
                self.devices(),
                names.len(),
                names.join(", ")
            ));
        }
        for pick in self.outputs.iter().chain(&self.inputs).chain(&self.witnesses) {
            if pick.index() >= names.len() {
                let counted: Vec<String> =
                    names.iter().enumerate().map(|(index, name)| format!("{index} is {name}")).collect();
                return Err(format!(
                    "interface {} is not one of them: a request counts the interfaces from zero, in the order \
                     aggregate.json lists them, and there are {} ({})",
                    pick.device,
                    names.len(),
                    counted.join(", ")
                ));
            }
        }

        // The side the click has in common must be one device, or the measurement is the
        // difference between two devices' clocks and two devices' converters at once.
        let (common, side) = match self.direction {
            Direction::Inputs => (&self.outputs, "outputs"),
            Direction::Outputs => (&self.inputs, "inputs"),
        };
        let first = common[0].index();
        if let Some(stray) = common.iter().find(|pick| pick.index() != first) {
            return Err(format!(
                "the {side} this rig names are spread across {} and {}, and they all have to be on one interface: \
                 that is what makes both copies of the click leave on the same sample",
                names[first],
                names[stray.index()]
            ));
        }

        // And the side being measured must be one channel per device, on its own device.
        let (separate, side) = match self.direction {
            Direction::Inputs => (&self.inputs, "input"),
            Direction::Outputs => (&self.outputs, "output"),
        };
        for (index, pick) in separate.iter().enumerate() {
            if pick.index() != index {
                return Err(format!(
                    "{} was given as {}'s cable, and it has to be one of {}'s own {side}s: the cables are listed one \
                     per interface, in the order the interfaces appear in aggregate.json",
                    pick.named(side, &names),
                    names[index],
                    names[index]
                ));
            }
        }

        let outputs = self
            .outputs
            .iter()
            .map(|&pick| locate(layout, &names, pick, false, "play the click from"))
            .collect::<Result<Vec<_>, _>>()?;
        let inputs = self
            .inputs
            .iter()
            .map(|&pick| locate(layout, &names, pick, true, "record the click on"))
            .collect::<Result<Vec<_>, _>>()?;
        let witnesses = self
            .witnesses
            .iter()
            .map(|&pick| locate(layout, &names, pick, true, "listen in on"))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Wired { outputs, inputs, witnesses })
    }
}

/// Where one interface's own channel is in the aggregate's list, or why it is not there.
fn locate(layout: &Layout, names: &[String], pick: Pick, input: bool, purpose: &str) -> Result<i32, String> {
    let what = if input { "input" } else { "output" };
    let interface = &layout.interfaces[pick.index()];
    let name = &interface.name;
    let has = if input { interface.inputs } else { interface.outputs };
    if pick.channel >= has {
        return Err(if has <= 0 {
            format!("{name} has no {what}s at all, so there is no {what} {} on it to {purpose}", pick.channel + 1)
        } else {
            format!(
                "{name} has {has} {what}s, numbered 1 to {has}, so there is no {what} {} on it to {purpose} (a \
                 request counts them from zero, 0 to {})",
                pick.channel + 1,
                has - 1
            )
        });
    }

    // Kept for the driver's phase measurement: opened at the interface and never given to anyone,
    // so that nothing played can land on the measurement and the measurement is never heard.
    let this = pick.named(what, names);
    if input && interface.phase_input == Some(pick.channel) {
        return Err(format!(
            "{this} is where {name}'s phase measurement arrives, so the setup keeps it for the driver and out of \
             the aggregate, and there is nothing there to {purpose}: choose another input, or move the phase cable \
             under Phase setup on {name}'s card"
        ));
    }
    if !input {
        if let Some((_, whose)) = interface.phase_outputs.iter().find(|(channel, _)| *channel == pick.channel) {
            return Err(format!(
                "{this} carries the signal {whose}'s phase is measured with, so the setup keeps it for the driver \
                 and out of the aggregate, and there is nothing there to {purpose}: choose another output, or move \
                 the phase cable under Phase setup on {whose}'s card"
            ));
        }
    }

    let list = if input { &layout.inputs } else { &layout.outputs };
    if let Some(at) = list.iter().position(|&(device, channel)| device == pick.index() && channel == pick.channel) {
        return Ok(at as i32);
    }
    let exposed: Vec<i32> =
        list.iter().filter(|&&(device, _)| device == pick.index()).map(|&(_, channel)| channel).collect();
    Err(if exposed.is_empty() {
        format!(
            "{this} is kept out of the aggregate by the setup, and so is every other {what} {name} has, so there is \
             nothing there to {purpose}: expose it on {name}'s card"
        )
    } else {
        format!(
            "{this} is kept out of the aggregate by the setup, so there is nothing there to {purpose}: choose one of \
             the {what}s {name} does expose ({}), or expose this one on {name}'s card",
            numbered(&exposed)
        )
    })
}

/// A set of channel numbers from zero, said as a person counts them: "1 to 4, 7 and 9".
fn numbered(channels: &[i32]) -> String {
    let mut sorted = channels.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut runs: Vec<String> = Vec::new();
    let mut at = 0;
    while at < sorted.len() {
        let start = sorted[at];
        let mut end = start;
        while at + 1 < sorted.len() && sorted[at + 1] == end + 1 {
            at += 1;
            end = sorted[at];
        }
        runs.push(if end > start { format!("{} to {}", start + 1, end + 1) } else { format!("{}", start + 1) });
        at += 1;
    }
    match runs.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => last.clone(),
        None => String::new(),
    }
}

fn first_repeat(list: &[Pick]) -> Option<Pick> {
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
    /// A check rather than a measurement: the session is lined up exactly as a DAW's session is,
    /// phase applied from its reference and trims in force, and the click lag heard is then what
    /// a recording would get. Nothing is offered to write, because a lag heard on top of a
    /// correction is a verdict on the trim, not a new one.
    pub checking: bool,
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
            checking: false,
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

    fn at(device: i32, channel: i32) -> Pick {
        Pick::new(device, channel)
    }

    /// The rig of the README: the Quadro plays out of two outputs, one into each interface.
    fn two_cables() -> Rig {
        Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(1, 0)])
    }

    /// Interfaces with every channel exposed, in the order given.
    fn layout(interfaces: &[(&str, i32, i32)]) -> Layout {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for (index, &(_, ins, outs)) in interfaces.iter().enumerate() {
            inputs.extend((0..ins).map(|channel| (index, channel)));
            outputs.extend((0..outs).map(|channel| (index, channel)));
        }
        Layout {
            interfaces: interfaces
                .iter()
                .map(|&(name, ins, outs)| Interface {
                    name: name.to_string(),
                    inputs: ins,
                    outputs: outs,
                    phase_input: None,
                    phase_outputs: Vec::new(),
                })
                .collect(),
            inputs,
            outputs,
        }
    }

    /// The rig as it is at the hardware: the Quadro's driver has sixteen channels each way, and the
    /// Studio+'s twenty four.
    fn quadro_and_studio() -> Layout {
        layout(&[("Quadro", 16, 16), ("Studio+", 24, 24)])
    }

    #[test]
    fn the_two_cable_rig_from_the_readme_is_accepted_and_put_into_the_aggregates_numbers() {
        let rig = two_cables();
        assert_eq!(rig.refusal(), None);
        let wired = rig.wired(&quadro_and_studio()).expect("a good rig");
        // The Studio+'s first input comes after all sixteen of the Quadro's.
        assert_eq!(wired, Wired { outputs: vec![0, 1], inputs: vec![0, 16], witnesses: vec![] });
        assert_eq!(rig.devices(), 2);
    }

    #[test]
    fn a_third_interface_is_a_third_cable_and_nothing_else() {
        let rig = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1), at(0, 2)], vec![at(0, 0), at(1, 0), at(2, 0)]);
        assert_eq!(rig.refusal(), None);
        let three = layout(&[("Quadro", 16, 16), ("Studio+", 24, 24), ("Orion", 32, 32)]);
        assert_eq!(rig.wired(&three).expect("three cables").inputs, vec![0, 16, 40]);
    }

    #[test]
    fn measuring_the_outputs_is_the_same_rig_with_the_cabling_turned_round() {
        // One output on each interface, all of them into inputs of the first.
        let rig = Rig::new(Direction::Outputs, vec![at(0, 0), at(1, 0)], vec![at(0, 0), at(0, 1)]);
        assert_eq!(rig.refusal(), None);
        let wired = rig.wired(&quadro_and_studio()).expect("a good rig");
        assert_eq!((wired.outputs, wired.inputs), (vec![0, 16], vec![0, 1]));
    }

    #[test]
    fn outputs_spread_over_two_interfaces_are_refused_when_the_inputs_are_being_measured() {
        // Both copies of the click must leave the same device on the same sample, or the number
        // that comes out is not the inputs.
        let rig = Rig::new(Direction::Inputs, vec![at(0, 0), at(1, 0)], vec![at(0, 0), at(1, 0)]);
        let refusal = rig.wired(&quadro_and_studio()).expect_err("the outputs are on two devices");
        assert!(refusal.contains("Quadro") && refusal.contains("Studio+"), "{refusal}");
        assert!(refusal.contains("one interface"), "{refusal}");
    }

    #[test]
    fn an_input_that_is_not_on_the_interface_it_was_listed_for_is_refused_by_name() {
        // Both cables arrive on the Quadro, so the second interface is not being measured at all.
        let rig = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(0, 2)]);
        let refusal = rig.wired(&quadro_and_studio()).expect_err("the second cable is on the Quadro");
        assert!(refusal.starts_with("Quadro input 3 was given as Studio+'s cable"), "{refusal}");
    }

    #[test]
    fn a_channel_the_interface_has_not_got_is_refused_with_how_many_it_has() {
        // Input 17 of a sixteen input interface, which is exactly the channel a count of the
        // interface's channels that was two short would never have offered.
        let rig = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 16), at(1, 0)]);
        let refusal = rig.wired(&quadro_and_studio()).expect_err("the Quadro has sixteen inputs");
        assert!(refusal.contains("Quadro has 16 inputs, numbered 1 to 16"), "{refusal}");
        assert!(refusal.contains("no input 17"), "{refusal}");
        assert!(refusal.contains("0 to 15"), "and how a request counts them: {refusal}");

        let far = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 30)], vec![at(0, 0), at(1, 0)]);
        let refusal = far.wired(&quadro_and_studio()).expect_err("the Quadro has sixteen outputs");
        assert!(refusal.contains("Quadro has 16 outputs, numbered 1 to 16"), "{refusal}");
    }

    #[test]
    fn an_interface_that_is_not_one_of_them_is_refused_and_the_ones_there_are_are_named() {
        let rig = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(1, 0)]).watching(vec![at(2, 0)]);
        assert_eq!(rig.refusal(), None, "nothing about the request on its own says there is no third interface");
        let refusal = rig.wired(&quadro_and_studio()).expect_err("there are two interfaces");
        assert!(refusal.starts_with("interface 2 is not one of them"), "{refusal}");
        assert!(refusal.contains("0 is Quadro, 1 is Studio+"), "{refusal}");

        let negative = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(-1, 0)]);
        assert!(negative.refusal().expect("minus one is no interface").contains("interface -1 is not one of them"));
    }

    #[test]
    fn a_channel_the_setup_keeps_out_of_the_aggregate_is_refused_and_the_ones_it_exposes_are_listed() {
        let mut kept = quadro_and_studio();
        // The Studio+ exposes its first four inputs and its seventh, and nothing else.
        kept.inputs.retain(|&(device, channel)| device == 0 || channel < 4 || channel == 6);
        let rig = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(1, 4)]);
        let refusal = rig.wired(&kept).expect_err("Studio+ input 5 is not exposed");
        assert!(refusal.starts_with("Studio+ input 5 is kept out of the aggregate"), "{refusal}");
        assert!(refusal.contains("(1 to 4 and 7)"), "{refusal}");
        // And the ones it does expose are where the Quadro's sixteen leave them.
        let fine = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(1, 6)]);
        assert_eq!(fine.wired(&kept).expect("Studio+ input 7 is exposed").inputs, vec![0, 20]);
    }

    #[test]
    fn a_channel_kept_for_the_phase_measurement_is_refused_and_says_that_is_why() {
        let mut phased = quadro_and_studio();
        phased.interfaces[1].phase_input = Some(2);
        phased.interfaces[0].phase_outputs = vec![(2, "Studio+".to_string())];
        phased.inputs.retain(|&at| at != (1, 2));
        phased.outputs.retain(|&at| at != (0, 2));

        let on_input = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(1, 2)]);
        let refusal = on_input.wired(&phased).expect_err("Studio+ input 3 carries the phase");
        assert!(refusal.starts_with("Studio+ input 3 is where Studio+'s phase measurement arrives"), "{refusal}");
        assert!(refusal.contains("Phase setup"), "{refusal}");

        let on_output = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 2)], vec![at(0, 0), at(1, 0)]);
        let refusal = on_output.wired(&phased).expect_err("Quadro output 3 carries the phase");
        assert!(refusal.starts_with("Quadro output 3 carries the signal Studio+'s phase is measured with"), "{refusal}");

        let witness = two_cables().watching(vec![at(1, 2)]);
        let refusal = witness.wired(&phased).expect_err("and it is not there to listen in on either");
        assert!(refusal.contains("phase measurement") && refusal.contains("listen in on"), "{refusal}");

        // The Studio+'s fourth input is its third channel in the aggregate now.
        let beside = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(1, 3)]);
        assert_eq!(beside.wired(&phased).expect("input 4 is exposed").inputs, vec![0, 18]);
    }

    #[test]
    fn a_rig_that_does_not_cover_every_interface_is_refused_before_anything_is_opened() {
        let one = Rig::new(Direction::Inputs, vec![at(0, 0)], vec![at(0, 0)]);
        let refusal = one.refusal().expect("one interface is nothing to compare");
        assert!(refusal.contains("two"), "{refusal}");
        let lopsided = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1), at(0, 2)], vec![at(0, 0), at(1, 0)]);
        assert!(lopsided.refusal().expect("the two lists differ").contains("one of each"));
        let missing = two_cables().wired(&layout(&[("Quadro", 16, 16)]));
        assert!(missing.expect_err("the aggregate has one device").contains("aggregate has 1 (Quadro)"));
    }

    #[test]
    fn one_cable_named_twice_is_refused_rather_than_measured() {
        let same_output = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 0)], vec![at(0, 0), at(1, 0)]);
        let refusal = same_output.refusal().expect("one output, two cables");
        assert_eq!(
            refusal,
            "output 1 of the first interface is cabled twice, and every cable needs a channel of its own so that the \
             two copies of the click can be told apart"
        );
        let same_input = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(0, 0)]);
        assert!(same_input.refusal().expect("one input for two interfaces").contains("two interfaces"));
        let negative = Rig::new(Direction::Inputs, vec![at(0, 0), at(0, 1)], vec![at(0, 0), at(1, -3)]);
        assert!(negative.refusal().expect("minus three is no channel").contains("counts each interface's own inputs from zero"));
    }

    #[test]
    fn a_witness_on_an_interface_that_is_already_being_measured_is_the_whole_point_of_one() {
        // Studio+ input 1 is the measured channel, and Studio+ input 2 is carried along beside it.
        // One run, two of the same interface's inputs, which is what answers whether they move
        // together.
        let rig = two_cables().watching(vec![at(1, 1)]);
        assert_eq!(rig.refusal(), None);
        assert_eq!(rig.wired(&quadro_and_studio()).expect("a good rig").witnesses, vec![17]);
    }

    #[test]
    fn a_witness_on_the_reference_interface_is_allowed_just_the_same() {
        let rig = two_cables().watching(vec![at(0, 4)]);
        assert_eq!(rig.refusal(), None);
        assert_eq!(rig.wired(&quadro_and_studio()).expect("a good rig").witnesses, vec![4]);
    }

    #[test]
    fn a_witness_that_is_not_a_channel_is_refused_before_anything_is_opened() {
        let negative = two_cables().watching(vec![at(1, -1)]);
        assert!(negative.refusal().is_some(), "minus one is not a channel");
        // A channel the interface has not got needs its driver to say so, and it is said before
        // a buffer is made or a sample is played.
        let missing = two_cables().watching(vec![at(1, 99)]);
        assert_eq!(missing.refusal(), None, "nothing about the cabling itself is wrong");
        let refusal = missing.wired(&quadro_and_studio()).expect_err("the Studio+ has 24 inputs");
        assert!(refusal.contains("Studio+ has 24 inputs, numbered 1 to 24"), "{refusal}");
        assert!(refusal.contains("listen in on"), "{refusal}");
    }

    #[test]
    fn a_witness_that_is_already_being_measured_is_refused_rather_than_recorded_twice() {
        let clash = two_cables().watching(vec![at(1, 0)]);
        let refusal = clash.refusal().expect("the Studio+'s first input is its measured input");
        assert!(refusal.starts_with("input 1 of the second interface is already"), "{refusal}");
        assert!(refusal.contains("witness"), "and it says what a witness is for: {refusal}");
        // The reference's own input is a measured channel too.
        assert!(two_cables().watching(vec![at(0, 0)]).refusal().is_some());
        // The same channel number on another interface is another channel.
        assert_eq!(two_cables().watching(vec![at(0, 16)]).refusal(), None);
        // And naming one twice is the same mistake said the other way round.
        let twice = two_cables().watching(vec![at(1, 1), at(1, 1)]);
        assert!(twice.refusal().expect("named twice").contains("twice"));
    }

    #[test]
    fn a_pick_is_read_from_exactly_the_interface_and_its_channel_and_nothing_else() {
        let pick: Pick = serde_json::from_str(r#"{"device":1,"channel":0}"#).expect("a pick");
        assert_eq!(pick, at(1, 0));
        assert!(serde_json::from_str::<Pick>(r#"{"device":1,"channel":0,"number":16}"#).is_err());
        assert!(serde_json::from_str::<Pick>("16").is_err(), "an aggregate channel number on its own is not a pick");
    }

    #[test]
    fn channel_numbers_are_said_as_a_person_counts_them() {
        assert_eq!(numbered(&[0, 1, 2, 3, 6]), "1 to 4 and 7");
        assert_eq!(numbered(&[8, 0, 1, 4]), "1 to 2, 5 and 9");
        assert_eq!(numbered(&[5]), "6");
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
