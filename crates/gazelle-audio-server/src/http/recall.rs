//! Recall over HTTP: the plan, and the apply route that is switched off.
//!
//! **Neither route here writes to a device.**
//!
//! - `POST /api/v1/snapshots/{id}/recall/plan` reads every device fresh, compares, and answers the
//!   ordered list of commands recall *would* send, each with its bytes and its guards. It is the
//!   preview the first recall guard asks for, and it is all the Workspace page uses.
//! - `POST /api/v1/snapshots/{id}/recall` is the seam where applying will attach. It refuses unless
//!   the server was started with `--enable-recall` **and** the request body says so as well; even
//!   then it only reports the bytes, because sending them waits for a session at the hardware to
//!   confirm them. Two switches rather than one so that neither a stray request nor a server left
//!   running with the flag can drive a device on its own.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value as Json2;

use crate::error::ServerError;
use crate::snapshot::plan::{prepare, RecallRequest};
use crate::AppState;

/// The apply route's body: the plan's own choices, plus the switch that must be thrown by hand.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ApplyRequest {
    /// Must be `true`. The server flag alone is not enough.
    pub enable_recall: bool,
    #[serde(flatten)]
    pub plan: RecallRequest,
}

fn asked(body: Result<Json<RecallRequest>, JsonRejection>) -> Result<RecallRequest, ServerError> {
    match body {
        Ok(Json(request)) => Ok(request),
        // No body at all is the defaults, which is what the page asks for.
        Err(JsonRejection::MissingJsonContentType(_)) | Err(JsonRejection::BytesRejection(_)) => Ok(RecallRequest::default()),
        Err(rejection) => Err(ServerError::BadValue(format!("not a recall request: {}", rejection.body_text()))),
    }
}

/// What recall would send, in order, with every guard attached. Reads; sends nothing.
pub async fn plan_recall(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<RecallRequest>, JsonRejection>,
) -> Result<Json<Json2>, ServerError> {
    let request = asked(body)?;
    let snapshot = state.snapshots.load(&id)?;
    let workspace = state.store.load()?;
    let plan = prepare(&state.devices, &snapshot, workspace, state.force_dry_run, &request).await?;
    Ok(Json(serde_json::to_value(plan).map_err(|e| ServerError::Storage(e.to_string()))?))
}

/// Apply a snapshot. **Not built**: this is the seam, and it is closed.
pub async fn apply_recall(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<ApplyRequest>, JsonRejection>,
) -> Result<Json<Json2>, ServerError> {
    if !state.enable_recall {
        return Err(ServerError::Unsupported(
            "recall is not enabled: this server was not started with --enable-recall, so it will not apply a snapshot to a device".into(),
        ));
    }
    let request = match body {
        Ok(Json(request)) => request,
        Err(rejection) => return Err(ServerError::BadValue(format!("not a recall request: {}", rejection.body_text()))),
    };
    if !request.enable_recall {
        return Err(ServerError::Unsupported(
            "recall is not enabled: the request must also carry \"enable_recall\": true, so that applying a snapshot is never something a page does by accident".into(),
        ));
    }
    let snapshot = state.snapshots.load(&id)?;
    let workspace = state.store.load()?;
    let plan = prepare(&state.devices, &snapshot, workspace, state.force_dry_run, &request.plan).await?;
    if !state.force_dry_run {
        // The seam. Everything above this line is built and tested; what is missing is the loop
        // that sends `plan.steps`, stops on the first failure and leaves the outputs silenced, and
        // it is missing on purpose: a session at the hardware has to confirm the order,
        // the hard mute's behaviour and what a preset recall changes first.
        return Err(ServerError::Unsupported(
            "applying a recall to a device is not built: the plan above is complete, and sending it waits for a hardware session to confirm the order of the steps, how the hard mute behaves and what a preset recall changes. Run the server with --dry-run to see the bytes each step would send".into(),
        ));
    }
    let mut body = serde_json::to_value(plan).map_err(|e| ServerError::Storage(e.to_string()))?;
    body["dry_run"] = Json2::Bool(true);
    body["note"] = Json2::String(
        "The server is in dry run: these are the bytes each step would send, and not one of them was sent.".into(),
    );
    Ok(Json(body))
}
