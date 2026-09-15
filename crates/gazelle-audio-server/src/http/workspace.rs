//! Workspace read and replace.

use axum::extract::State;
use axum::Json;

use std::collections::HashSet;

use crate::error::ServerError;
use crate::workspace::model::{DeviceMixer, Group, Workspace, MIXER_COUNT, MIXER_SLOTS};
use crate::AppState;

pub async fn get_workspace(State(state): State<AppState>) -> Result<Json<Workspace>, ServerError> {
    Ok(Json(state.store.load()?))
}

pub async fn put_workspace(
    State(state): State<AppState>,
    Json(workspace): Json<Workspace>,
) -> Result<Json<Workspace>, ServerError> {
    check_colours(&workspace.groups)?;
    for (device, mixer) in &workspace.mixers {
        check_mixer(mixer).map_err(|m| ServerError::BadValue(format!("mixer for {device}: {m}")))?;
    }
    state.store.save(&workspace)?;
    Ok(Json(workspace))
}

fn valid_colour(colour: &str) -> bool {
    colour.len() == 7 && colour.starts_with('#') && colour[1..].bytes().all(|b| b.is_ascii_hexdigit())
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
