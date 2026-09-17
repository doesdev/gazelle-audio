//! Workspace read and replace.

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::Json;

use std::collections::{HashMap, HashSet};

use crate::device::descriptor::DeviceId;
use crate::error::ServerError;
use crate::workspace::model::{Cable, ChannelLink, ControlRoom, DeviceMixer, Group, Surface, SurfaceStrip, Workspace, CABLE_RECEIVES, CABLE_SENDS, INPUT_KINDS, LINK_KINDS, LINK_MODES, MIXER_COUNT, MIXER_SLOTS, STRIP_KINDS, WORKSPACE_VERSION};
use crate::workspace::topology;
use crate::AppState;

pub async fn get_workspace(State(state): State<AppState>) -> Result<Json<Workspace>, ServerError> {
    Ok(Json(state.store.load()?))
}

pub async fn put_workspace(
    State(state): State<AppState>,
    document: Result<Json<Workspace>, JsonRejection>,
) -> Result<Json<Workspace>, ServerError> {
    // A document that is not a workspace is refused like any other bad value, in the JSON error body
    // and naming the part that is wrong: axum's own rejection is plain text, which a page cannot show.
    let Json(workspace) = document.map_err(|rejection| {
        let text = rejection.body_text();
        let reason = text.split_once(": ").map_or(text.as_str(), |(_, reason)| reason);
        ServerError::BadValue(format!("not a workspace: {reason}"))
    })?;
    if !(1..=WORKSPACE_VERSION).contains(&workspace.version) {
        return Err(ServerError::BadValue(format!("workspace version {} is not one this server reads ({WORKSPACE_VERSION})", workspace.version)));
    }
    check_colours(&workspace.groups)?;
    check_links(&workspace.links)?;
    for (device, mixer) in &workspace.mixers {
        check_mixer(mixer).map_err(|m| ServerError::BadValue(format!("mixer for {device}: {m}")))?;
    }
    check_layouts(&workspace.layouts)?;
    for (device, colour) in &workspace.device_colors {
        if !valid_colour(colour) {
            return Err(ServerError::BadValue(format!("device {device}: color must be #rrggbb, got {colour:?}")));
        }
    }
    // Indexes are checked against the models of the devices attached now; a device the server does
    // not know keeps what it names, to be drawn "not connected".
    let families: HashMap<DeviceId, String> = state.devices.descriptors().into_iter().filter_map(|d| Some((d.id, d.family?))).collect();
    check_surfaces(&workspace.surfaces, &families)?;
    check_cables(&workspace.cables, &families)?;
    for (device, control_room) in &workspace.control_room {
        check_control_room(control_room, families.get(device).map(String::as_str)).map_err(|m| ServerError::BadValue(format!("control room for {device}: {m}")))?;
    }
    state.store.save(&workspace)?;
    Ok(Json(workspace))
}

/// A Control Room lists each output once, and only outputs the device's model has (when attached).
fn check_control_room(control_room: &ControlRoom, family: Option<&str>) -> Result<(), String> {
    let mut seen = HashSet::new();
    for &output in &control_room.outputs {
        if !seen.insert(output) {
            return Err(format!("output {output} is listed twice"));
        }
        if let Some((family, count)) = family.and_then(|f| Some((f, topology::output_ids(f)?))) {
            if output >= count {
                return Err(format!("the {family} has outputs 0..{}, not {output}", count - 1));
            }
        }
    }
    Ok(())
}

/// Surfaces need unique ids and a name, mixes the devices have, and strips whose parts fit their
/// kind and, for an attached device, its model.
fn check_surfaces(surfaces: &[Surface], families: &HashMap<DeviceId, String>) -> Result<(), ServerError> {
    let mut ids = HashSet::new();
    for surface in surfaces {
        let bad = |message: String| ServerError::BadValue(format!("surface '{}': {message}", surface.id));
        if !ids.insert(surface.id.as_str()) {
            return Err(bad("the id is used twice".into()));
        }
        if surface.name.trim().is_empty() {
            return Err(bad("it needs a name".into()));
        }
        if let Some((device, _)) = surface.mixes.iter().find(|(_, &mix)| mix >= MIXER_COUNT) {
            return Err(bad(format!("the mix for {device} is outside 0..{}", MIXER_COUNT - 1)));
        }
        let mut strips = HashSet::new();
        for strip in &surface.strips {
            let bad_strip = |message: String| bad(format!("strip '{}': {message}", strip.id));
            if !strips.insert(strip.id.as_str()) {
                return Err(bad_strip("the id is used twice".into()));
            }
            check_strip(strip, families).map_err(bad_strip)?;
        }
    }
    Ok(())
}

fn check_strip(strip: &SurfaceStrip, families: &HashMap<DeviceId, String>) -> Result<(), String> {
    let kind = strip.kind.as_str();
    if !STRIP_KINDS.contains(&kind) {
        return Err(format!("kind must be one of {}, not {kind:?}", STRIP_KINDS.join(", ")));
    }
    if kind == "label" {
        return if strip.text.is_some() { Ok(()) } else { Err("a label strip needs text".into()) };
    }
    let device = strip.device_id.as_ref().ok_or_else(|| format!("a {kind} strip needs a device_id"))?;
    let family = families.get(device).map(String::as_str);
    if let Some(mix) = strip.mix.filter(|&mix| mix >= MIXER_COUNT) {
        return Err(format!("mix {mix} is outside 0..{}", MIXER_COUNT - 1));
    }
    match kind {
        "channel" if strip.channel.as_deref().is_none_or(str::is_empty) => Err("a channel strip needs a channel id".into()),
        "input" => {
            let input = strip.input.as_ref().ok_or("an input strip needs an input")?;
            let Some(kind) = topology::input_type(&input.kind) else {
                return Err(format!("input kind must be one of {}, not {:?}", INPUT_KINDS.join(", "), input.kind));
            };
            match family.and_then(|f| Some((f, topology::input_channels(f, kind)?))) {
                Some((family, 0)) => Err(format!("the {family} has no {} inputs", input.kind)),
                Some((family, count)) if input.channel >= count => Err(format!("the {family} has {} inputs 0..{}, not {}", input.kind, count - 1, input.channel)),
                _ => Ok(()),
            }
        }
        "output" => {
            let output = strip.output.ok_or("an output strip needs an output")?;
            match family.and_then(|f| Some((f, topology::output_ids(f)?))) {
                Some((family, count)) if output >= count => Err(format!("the {family} has outputs 0..{}, not {output}", count - 1)),
                _ => Ok(()),
            }
        }
        "port" => {
            let port = strip.port.as_deref().ok_or("a port strip needs a port")?;
            if !CABLE_SENDS.contains(&port) {
                return Err(format!("port must be {}, not {port:?}", CABLE_SENDS.join(" or ")));
            }
            let first = strip.first.unwrap_or(0);
            let width = topology::port_width(port);
            if first % width != 0 {
                return Err(format!("an {} port starts at a multiple of {width}, not {first}", if port == "ADAT_OUT" { "ADAT" } else { "S/PDIF" }));
            }
            check_port_range(family, port, first, width)
        }
        _ => Ok(()),
    }
}

/// That a model (when known) has `port` and its channels `first..first + count`.
fn check_port_range(family: Option<&str>, port: &str, first: u32, count: u32) -> Result<(), String> {
    let Some((family, channels)) = family.and_then(|f| Some((f, topology::port_channels(f, port)?))) else {
        return Ok(());
    };
    if channels == 0 {
        return Err(format!("the {family} has no {port}"));
    }
    if first + count > channels {
        return Err(format!("the {family}'s {port} has channels 0..{}, not {first}..{}", channels - 1, first + count - 1));
    }
    Ok(())
}

/// Cables go from a digital output to an input of the same kind on another device, carrying as
/// many channels as one cable can and no more than either end has.
fn check_cables(cables: &[Cable], families: &HashMap<DeviceId, String>) -> Result<(), ServerError> {
    let mut ids = HashSet::new();
    for cable in cables {
        let bad = |message: String| ServerError::BadValue(format!("cable '{}': {message}", cable.id));
        if !ids.insert(cable.id.as_str()) {
            return Err(bad("the id is used twice".into()));
        }
        let (from, to) = (cable.from.port.as_str(), cable.to.port.as_str());
        let Some(kind) = CABLE_SENDS.iter().position(|&p| p == from) else {
            return Err(bad(format!("from must be {}, not {from:?}", CABLE_SENDS.join(" or "))));
        };
        if !CABLE_RECEIVES.contains(&to) {
            return Err(bad(format!("to must be {}, not {to:?}", CABLE_RECEIVES.join(" or "))));
        }
        if CABLE_RECEIVES[kind] != to {
            return Err(bad(format!("{from} cannot feed {to}")));
        }
        if cable.from.device_id == cable.to.device_id {
            return Err(bad("a cable joins two devices".into()));
        }
        let most = topology::port_width(from);
        if !(1..=most).contains(&cable.channels) {
            return Err(bad(format!("it needs 1..{most} channels, not {}", cable.channels)));
        }
        for end in [&cable.from, &cable.to] {
            check_port_range(families.get(&end.device_id).map(String::as_str), &end.port, end.first, cable.channels).map_err(bad)?;
        }
    }
    Ok(())
}

/// Saved layouts need a unique id, a name, a known model and a mixer the hardware can hold.
fn check_layouts(layouts: &[crate::workspace::model::SavedLayout]) -> Result<(), ServerError> {
    let mut ids = HashSet::new();
    for layout in layouts {
        let bad = |message: String| ServerError::BadValue(format!("saved layout '{}': {message}", layout.id));
        if !ids.insert(layout.id.as_str()) {
            return Err(bad("the id is used twice".into()));
        }
        if layout.name.trim().is_empty() {
            return Err(bad("it needs a name".into()));
        }
        if !crate::workspace::model::LAYOUT_FAMILIES.contains(&layout.family.as_str()) {
            return Err(bad(format!("family must be one of {}, not {:?}", crate::workspace::model::LAYOUT_FAMILIES.join(", "), layout.family)));
        }
        check_mixer(&layout.mixer).map_err(bad)?;
    }
    Ok(())
}

fn valid_colour(colour: &str) -> bool {
    colour.len() == 7 && colour.starts_with('#') && colour[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Links must name a known kind and mode, join at least two distinct channels, and not put a
/// channel in two links of the same kind (a change could then not tell which link it follows).
fn check_links(links: &[ChannelLink]) -> Result<(), ServerError> {
    let mut ids = HashSet::new();
    let mut taken = HashSet::new();
    for link in links {
        let id = &link.id;
        let bad = |message: String| ServerError::BadValue(format!("link '{id}': {message}"));
        if !ids.insert(id.as_str()) {
            return Err(bad("the id is used twice".into()));
        }
        if !LINK_KINDS.contains(&link.kind.as_str()) {
            return Err(bad(format!("kind must be one of {}, not {:?}", LINK_KINDS.join(", "), link.kind)));
        }
        if !LINK_MODES.contains(&link.mode.as_str()) {
            return Err(bad(format!("mode must be one of {}, not {:?}", LINK_MODES.join(", "), link.mode)));
        }
        if link.members.len() < 2 {
            return Err(bad("a link needs at least two channels".into()));
        }
        let mut own = HashSet::new();
        for member in &link.members {
            let key = format!("{}:{}", member.device_id, member.channel);
            if !own.insert(key.clone()) {
                return Err(bad(format!("{key} is listed twice")));
            }
            if !taken.insert(format!("{}|{key}", link.kind)) {
                return Err(bad(format!("{key} is already in another {} link", link.kind)));
            }
        }
    }
    Ok(())
}

/// Every group colour, nested groups included, must be `#rrggbb`.
fn check_colours(groups: &[Group]) -> Result<(), ServerError> {
    for group in groups {
        if let Some(colour) = &group.color {
            if !valid_colour(colour) {
                return Err(ServerError::BadValue(format!("group '{}': color must be #rrggbb, got {colour:?}", group.id)));
            }
        }
        check_colours(&group.children)?;
    }
    Ok(())
}

/// A layout must fit the hardware: slots are distinct and within the mix, mixes exist, a channel
/// does not send to its own main mix or twice to one mix, and names refer to real groups.
/// Family-specific rules (the Quadro's effect slots) are the client's, which knows the model.
fn check_mixer(mixer: &DeviceMixer) -> Result<(), String> {
    if mixer.mixes.len() > MIXER_COUNT as usize {
        return Err(format!("{} mixes named, the device has {MIXER_COUNT}", mixer.mixes.len()));
    }
    use crate::workspace::model::{PAN_MAX, PAN_MIN};
    for (index, mix) in mixer.mixes.iter().enumerate() {
        for (&slot, &pan) in mix.mono.iter().flat_map(|m| m.pans.iter()) {
            if slot >= MIXER_SLOTS {
                return Err(format!("mix {}: mono pan for slot {slot}, outside 0..{}", index + 1, MIXER_SLOTS - 1));
            }
            if !(PAN_MIN..=PAN_MAX).contains(&pan) {
                return Err(format!("mix {}: mono pan {pan} for slot {slot} is outside {PAN_MIN}..{PAN_MAX}", index + 1));
            }
        }
    }
    let mut groups = HashSet::new();
    for group in &mixer.groups {
        if !groups.insert(group.id.as_str()) {
            return Err(format!("group id '{}' is used twice", group.id));
        }
        if group.color.as_deref().is_some_and(|c| !valid_colour(c)) {
            return Err(format!("group '{}': color must be #rrggbb", group.id));
        }
    }
    let (mut ids, mut slots) = (HashSet::new(), HashSet::new());
    for channel in &mixer.channels {
        let id = &channel.id;
        if !ids.insert(id.as_str()) {
            return Err(format!("channel id '{id}' is used twice"));
        }
        if channel.slot >= MIXER_SLOTS {
            return Err(format!("channel '{id}': slot {} is outside 0..{}", channel.slot, MIXER_SLOTS - 1));
        }
        if !slots.insert(channel.slot) {
            return Err(format!("channel '{id}': slot {} is already used", channel.slot));
        }
        if channel.main_mix.is_some_and(|m| m >= MIXER_COUNT) {
            return Err(format!("channel '{id}': main mix is outside 0..{}", MIXER_COUNT - 1));
        }
        let mut sends = HashSet::new();
        for &send in &channel.sends {
            if send >= MIXER_COUNT || Some(send) == channel.main_mix || !sends.insert(send) {
                return Err(format!("channel '{id}': send to mix {send} is outside 0..{}, repeated, or its main mix", MIXER_COUNT - 1));
            }
        }
        if channel.group.as_deref().is_some_and(|g| !groups.contains(g)) {
            return Err(format!("channel '{id}': no group '{}'", channel.group.as_deref().unwrap_or_default()));
        }
        if channel.color.as_deref().is_some_and(|c| !valid_colour(c)) {
            return Err(format!("channel '{id}': color must be #rrggbb"));
        }
    }
    Ok(())
}
