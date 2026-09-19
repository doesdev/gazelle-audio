//! Snapshots over HTTP: take one, list them, rename, delete, fetch, and compare with now.
//!
//! **No route here writes to a device.** `POST /snapshots` and `GET /snapshots/{id}/compare` both
//! only read. Recall (phase 6) will be a route of its own, and deliberately is not one yet: it
//! needs a hardware session first.
//!
//! A snapshot is a record of reads, so a client cannot PUT one: only its name and note can be
//! changed, through `PATCH`. A snapshot whose values a client could write would be indistinguishable
//! from one a device gave, and recall is built on trusting exactly that difference.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value as Json2};

use crate::error::ServerError;
use crate::snapshot::capture::capture;
use crate::snapshot::diff::diff;
use crate::snapshot::model::{Snapshot, SNAPSHOT_VERSION};
use crate::snapshot::store::{check_id, check_version};
use crate::AppState;
use std::collections::BTreeSet;

/// The longest a name or note may be, so a client cannot fill the config directory through them.
const MAX_NAME: usize = 200;
const MAX_NOTE: usize = 2_000;

#[derive(Debug, Deserialize, Default)]
pub struct NewSnapshot {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct Rename {
    pub name: Option<String>,
    pub note: Option<String>,
}

/// Every snapshot, newest first, as the list shows them: name, when, and from which devices.
pub async fn list_snapshots(State(state): State<AppState>) -> Result<Json<Json2>, ServerError> {
    let snapshots: Vec<Json2> = state.snapshots.list()?.iter().map(Snapshot::summary).collect();
    Ok(Json(json!({ "version": SNAPSHOT_VERSION, "count": snapshots.len(), "snapshots": snapshots })))
}

/// Take a snapshot of the workspace and every attached device, and store it.
pub async fn create_snapshot(
    State(state): State<AppState>,
    body: Option<Json<NewSnapshot>>,
) -> Result<Json<Json2>, ServerError> {
    let asked = body.map(|Json(b)| b).unwrap_or_default();
    let name = check_name(&asked.name)?;
    let note = check_note(&asked.note)?;
    let workspace = state.store.load()?;
    let snapshot = capture(&state.devices, workspace, name, note, state.force_dry_run).await?;
    state.snapshots.save(&snapshot)?;
    Ok(Json(snapshot.summary()))
}

/// One snapshot, whole: every value it holds.
pub async fn get_snapshot(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Snapshot>, ServerError> {
    Ok(Json(state.snapshots.load(&id)?))
}

/// Change a snapshot's name or note. Nothing it recorded can be changed.
pub async fn rename_snapshot(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<Rename>, JsonRejection>,
) -> Result<Json<Json2>, ServerError> {
    let Json(change) = body.map_err(|rejection| ServerError::BadValue(format!("not a rename: {}", rejection.body_text())))?;
    let mut snapshot = state.snapshots.load(&id)?;
    if let Some(name) = change.name {
        snapshot.name = check_name(&name)?;
    }
    if let Some(note) = change.note {
        snapshot.note = check_note(&note)?;
    }
    state.snapshots.save(&snapshot)?;
    Ok(Json(snapshot.summary()))
}

pub async fn delete_snapshot(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Json2>, ServerError> {
    state.snapshots.delete(&id)?;
    Ok(Json(json!({ "deleted": id })))
}

/// What differs between a snapshot and the devices' state now, grouped by section.
///
/// The present is read exactly as a capture reads it, through the same plan, so a difference here
/// is a difference in the device and never in how the two were gathered.
pub async fn compare_snapshot(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Json2>, ServerError> {
    let snapshot = state.snapshots.load(&id)?;
    let workspace = state.store.load()?;
    let now = capture(&state.devices, workspace, "now".into(), String::new(), state.force_dry_run).await?;
    Ok(Json(serde_json::to_value(diff(&snapshot, &now)).map_err(|e| ServerError::Storage(e.to_string()))?))
}

/// Add snapshots from a backup file, keeping the ones already stored.
///
/// Add-only: an id that is already here is skipped and named, never silently replaced, because a
/// snapshot is a record of a moment and two of them with one id would be a lie about which moment.
pub async fn import_snapshots(
    State(state): State<AppState>,
    body: Result<Json<Vec<Snapshot>>, JsonRejection>,
) -> Result<Json<Json2>, ServerError> {
    let Json(incoming) = body.map_err(|rejection| {
        let text = rejection.body_text();
        let reason = text.split_once(": ").map_or(text.as_str(), |(_, reason)| reason);
        ServerError::BadValue(format!("not a list of snapshots: {reason}"))
    })?;
    let existing: BTreeSet<String> = state.snapshots.list()?.into_iter().map(|s| s.id).collect();
    let (mut added, mut skipped) = (Vec::new(), Vec::new());
    for mut snapshot in incoming {
        check_version(snapshot.version)?;
        check_id(&snapshot.id)?;
        snapshot.name = check_name(&snapshot.name)?;
        snapshot.note = check_note(&snapshot.note)?;
        if existing.contains(&snapshot.id) || added.contains(&snapshot.id) {
            skipped.push(snapshot.id);
            continue;
        }
        state.snapshots.save(&snapshot)?;
        added.push(snapshot.id);
    }
    Ok(Json(json!({ "added": added, "skipped": skipped })))
}

fn check_name(name: &str) -> Result<String, ServerError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ServerError::BadValue("a snapshot needs a name".into()));
    }
    if name.chars().count() > MAX_NAME {
        return Err(ServerError::BadValue(format!("a snapshot's name is at most {MAX_NAME} characters")));
    }
    Ok(name.to_string())
}

fn check_note(note: &str) -> Result<String, ServerError> {
    if note.chars().count() > MAX_NOTE {
        return Err(ServerError::BadValue(format!("a snapshot's note is at most {MAX_NOTE} characters")));
    }
    Ok(note.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_trimmed_required_and_bounded() {
        assert_eq!(check_name("  Drum tracking  ").expect("trimmed"), "Drum tracking");
        assert!(check_name("   ").is_err());
        assert!(check_name(&"x".repeat(MAX_NAME)).is_ok());
        assert!(check_name(&"x".repeat(MAX_NAME + 1)).is_err());
        assert!(check_note(&"x".repeat(MAX_NOTE + 1)).is_err());
    }
}
