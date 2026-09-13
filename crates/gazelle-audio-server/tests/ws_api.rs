//! WebSocket tests: the event stream and the JSON-RPC invocation path.
//!
//! These run against a real listener on an ephemeral port, because the upgrade handshake
//! cannot be exercised through `oneshot`.

use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_tungstenite::tungstenite::Message;

/// Start the server on an ephemeral port and return its ws:// URL.
async fn serve() -> String {
    let registries = RegistrySet::builtin().expect("registries");
    let devices = DeviceManager::new(registries);
    devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64);
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    let app = http::router(AppState {
        devices,
        store,
        force_dry_run: false,
        backend: "loopback".into(),
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("ws://{addr}/api/v1/ws")
}

async fn next_json<S>(stream: &mut S) -> Value
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match stream.next().await.expect("stream open").expect("no ws error") {
            Message::Text(t) => return serde_json::from_str(&t).expect("json frame"),
            // Ignore transport-level frames.
            _ => continue,
        }
    }
}

#[tokio::test]
async fn connecting_yields_a_hello_with_the_device_list() {
    let url = serve().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");

    let hello = next_json(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["backend"], "loopback");
    let devices = hello["devices"].as_array().expect("devices array");
    assert_eq!(devices.len(), 2, "client should not need HTTP to bootstrap");
}

#[tokio::test]
async fn rpc_invokes_a_command_and_echoes_the_id() {
    let url = serve().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");
    let _hello = next_json(&mut ws).await;

    ws.send(Message::Text(
        json!({
            "id": 7,
            "device_id": "loopback-0",
            "command": "set_mixer",
            "args": {"level": 64},
            "dry_run": true
        })
        .to_string(),
    ))
    .await
    .expect("send rpc");

    let reply = next_json(&mut ws).await;
    assert_eq!(reply["type"], "rpc_response");
    assert_eq!(reply["id"], 7, "the id must be echoed so clients can correlate");
    assert_eq!(reply["result"]["command"], "set_mixer");
    assert_eq!(reply["result"]["dry_run"], true);
    assert!(!reply["result"]["sent_hex"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn rpc_errors_carry_the_id_and_a_code() {
    let url = serve().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");
    let _hello = next_json(&mut ws).await;

    ws.send(Message::Text(
        json!({"id": "abc", "device_id": "loopback-1", "command": "set_mixer", "args": {}})
            .to_string(),
    ))
    .await
    .unwrap();

    let reply = next_json(&mut ws).await;
    assert_eq!(reply["type"], "rpc_error");
    assert_eq!(reply["id"], "abc");
    assert_eq!(reply["error"]["code"], "unknown_command");
}

#[tokio::test]
async fn malformed_frame_does_not_drop_the_connection() {
    let url = serve().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");
    let _hello = next_json(&mut ws).await;

    ws.send(Message::Text("not json at all".into())).await.unwrap();
    let reply = next_json(&mut ws).await;
    assert_eq!(reply["type"], "rpc_error");
    assert_eq!(reply["error"]["code"], "bad_request");

    // The socket must still be usable afterwards.
    ws.send(Message::Text(
        json!({"id": 1, "device_id": "loopback-0", "command": "set_routing",
               "args": {}, "dry_run": true})
        .to_string(),
    ))
    .await
    .unwrap();
    let reply = next_json(&mut ws).await;
    assert_eq!(reply["type"], "rpc_response");
}
