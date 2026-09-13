//! The async side of the sync/async boundary.

use gazelle_audio_protocol::payload::PayloadValues;
use std::sync::mpsc::Sender;
use tokio::sync::oneshot;

use crate::device::worker::{CommandOutcome, WorkerCommand};
use crate::error::ServerError;

/// An async handle to one device worker.
///
/// Cloneable and cheap: it is a channel sender plus the device id. Timeout and cancellation
/// live here, on the async side, while the worker keeps the protocol's one-outstanding-
/// request discipline on its own thread.
#[derive(Clone)]
pub struct DeviceHandle {
    device_id: String,
    tx: Sender<WorkerCommand>,
}

impl DeviceHandle {
    pub fn new(device_id: String, tx: Sender<WorkerCommand>) -> Self {
        DeviceHandle { device_id, tx }
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Run a command on the device and await its outcome.
    pub async fn request(
        &self,
        name: &str,
        values: PayloadValues,
        dry_run: bool,
    ) -> Result<CommandOutcome, ServerError> {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkerCommand::Request {
            name: name.to_string(),
            values,
            dry_run,
            respond: Box::new(move |result| {
                // The receiver is dropped if the caller went away (client disconnect);
                // that is normal, not an error worth logging.
                let _ = tx.send(result);
            }),
        };
        self.tx
            .send(cmd)
            .map_err(|_| ServerError::DeviceGone(self.device_id.clone()))?;
        rx.await
            .map_err(|_| ServerError::DeviceGone(self.device_id.clone()))?
    }

    /// Ask the worker to stop. Best-effort: a worker already gone is not an error.
    pub fn shutdown(&self) {
        let _ = self.tx.send(WorkerCommand::Shutdown);
    }
}
