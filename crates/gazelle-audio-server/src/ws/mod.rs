//! WebSocket surface: events out, JSON-RPC in.
//!
//! The inbound framing deliberately mirrors the recovered panel↔manager vocabulary
//! (`{"command", "args", ...}`) so the original command shape maps across directly.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::device::descriptor::DeviceId;
use crate::device::manager::ServerEvent;
use crate::device::worker::DeviceEvent;
use crate::error::ServerError;
use crate::value::{fields_to_json, json_to_payload_values, to_hex};
use crate::AppState;

/// One inbound RPC call.
#[derive(Debug, Deserialize)]
struct RpcRequest {
    /// Correlates the response; echoed back verbatim.
    id: Json,
    device_id: String,
    command: String,
    #[serde(default)]
    args: Json,
    #[serde(default)]
    dry_run: bool,
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let mut events = state.devices.subscribe();
    // RPCs run as their own tasks and report back here, so a request waiting on a device
    // (up to the 3 s device timeout) never holds back events. Per-device ordering is
    // unaffected: the device worker still serves one request at a time. Unbounded is safe
    // here because every entry corresponds to a frame this client sent.
    let (rpc_tx, mut rpc_rx) = mpsc::unbounded_channel::<Json>();

    // Send the current device list first, so a client never has to poll HTTP to bootstrap.
    let hello = json!({
        "type": "hello",
        "version": crate::VERSION,
        "backend": state.backend,
        "dry_run": state.force_dry_run,
        "devices": state.devices.descriptors(),
    });
    if sink.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }

    loop {
        let frame = tokio::select! {
            // Outbound: device and lifecycle events.
            ev = events.recv() => match ev {
                Ok(e) => event_frame(&e),
                // A slow client falls behind rather than stalling a device worker.
                Err(RecvError::Lagged(n)) => json!({"type":"lagged","missed":n}),
                Err(RecvError::Closed) => break,
            },
            // Outbound: finished RPCs, in completion order. `rpc_tx` lives in this
            // function, so the channel never closes while the loop runs.
            Some(frame) = rpc_rx.recv() => frame,
            // Inbound: RPC calls.
            msg = stream.next() => {
                let Some(Ok(msg)) = msg else { break };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => break,
                    // Ping/Pong are handled by the transport; binary frames are not used.
                    _ => continue,
                };
                let (state, tx) = (state.clone(), rpc_tx.clone());
                tokio::spawn(async move {
                    // The client may have gone; nothing to do with an undeliverable reply.
                    let _ = tx.send(handle_rpc(&state, &text).await);
                });
                continue;
            }
        };
        if sink.send(Message::Text(frame.to_string().into())).await.is_err() {
            break;
        }
    }
}

fn event_frame(e: &ServerEvent) -> Json {
    match e {
        ServerEvent::DeviceAdded(d) => json!({"type":"device_added","device":d}),
        ServerEvent::DeviceRemoved(id) => json!({"type":"device_removed","device_id":id}),
        ServerEvent::Device(DeviceEvent::Cyclic { device_id, report_id, fields }) => json!({
            "type": "cyclic",
            "device_id": device_id,
            "report_id": format!("0x{report_id:X}"),
            "fields": fields_to_json(fields),
        }),
        ServerEvent::Device(DeviceEvent::Undecoded { device_id, report_id, len }) => json!({
            "type": "undecoded",
            "device_id": device_id,
            "report_id": format!("0x{report_id:X}"),
            "len": len,
        }),
    }
}

async fn handle_rpc(state: &AppState, text: &str) -> Json {
    let req: RpcRequest = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            return json!({
                "type": "rpc_error",
                "id": Json::Null,
                "error": {"code": "bad_request", "message": format!("malformed rpc frame: {e}")}
            })
        }
    };

    match invoke(state, &req).await {
        Ok(result) => json!({"type":"rpc_response","id":req.id,"result":result}),
        Err(e) => {
            let mut body = e.to_json();
            let obj = body.as_object_mut().expect("error body is an object");
            obj.insert("type".into(), json!("rpc_error"));
            obj.insert("id".into(), req.id.clone());
            body
        }
    }
}

async fn invoke(state: &AppState, req: &RpcRequest) -> Result<Json, ServerError> {
    let id = DeviceId(req.device_id.clone());
    let handle = state.devices.handle(&id)?;
    let values = json_to_payload_values(&req.args)?;
    let dry_run = req.dry_run || state.force_dry_run;
    let outcome = handle.request(&req.command, values, dry_run).await?;
    Ok(json!({
        "device_id": id,
        "command": req.command,
        "sent_hex": to_hex(&outcome.sent),
        "sent_len": outcome.sent.len(),
        "dry_run": outcome.dry_run,
        "response": outcome.response.as_ref().map(fields_to_json),
        "response_error": outcome.response_error,
    }))
}
