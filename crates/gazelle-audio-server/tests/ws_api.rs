//! WebSocket tests: the event stream and the JSON-RPC invocation path.
//!
//! These run against a real listener on an ephemeral port, because the upgrade handshake
//! cannot be exercised through `oneshot`.

use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::snapshot::store::MemorySnapshotStore;
use gazelle_audio_server::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_tungstenite::tungstenite::Message;

use gazelle_audio_protocol::field::Field;
use gazelle_audio_protocol::wire::{Header, WireError};
use gazelle_audio_server::device::descriptor::DeviceId;
use gazelle_audio_transport::{Device, RawPacket, Report};
use std::time::{Duration, Instant};

/// Start the server on an ephemeral port for these devices and return its ws:// URL.
async fn serve_with(devices: Arc<DeviceManager>) -> String {
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    let app = http::router(AppState {
        devices,
        snapshots: Arc::new(MemorySnapshotStore::default()),
        store,
        force_dry_run: false,
        backend: "loopback".into(),
        themes_dir: None,
        show_window: None,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("ws://{addr}/api/v1/ws")
}

/// Start the server on an ephemeral port and return its ws:// URL.
async fn serve() -> String {
    let devices = DeviceManager::new(RegistrySet::builtin().expect("registries"));
    devices.attach_loopbacks(&[PID_QUADRO, PID_STUDIO], 64);
    serve_with(devices).await
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
    // The same notices the health endpoint carries, so a connected client hears them without
    // polling. The loopback never has any.
    assert_eq!(hello["notices"], json!([]));
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

/// The RPC frame carries the same `ext3` selector as the HTTP query (see `http_api.rs`).
#[tokio::test]
async fn rpc_ext3_selects_the_routing_group() {
    let url = serve().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");
    let _hello = next_json(&mut ws).await;

    ws.send(Message::Text(
        json!({"id": 1, "device_id": "loopback-1", "command": "get_routing", "ext3": 13, "dry_run": true}).to_string(),
    ))
    .await
    .expect("send rpc");
    let reply = next_json(&mut ws).await;
    assert_eq!(reply["type"], "rpc_response", "reply: {reply}");
    assert_eq!(&reply["result"]["sent_hex"].as_str().unwrap()[24..32], "0d000000");

    ws.send(Message::Text(
        json!({"id": 2, "device_id": "loopback-1", "command": "set_mixer_cfg", "ext3": 1, "dry_run": true}).to_string(),
    ))
    .await
    .expect("send rpc");
    let reply = next_json(&mut ws).await;
    assert_eq!(reply["type"], "rpc_error", "reply: {reply}");
    assert_eq!(reply["error"]["code"], "bad_value");
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

/// Never answers a request, but pushes a 0x73 state report every 20 ms.
struct SilentChattyDevice {
    contents: Vec<u8>,
    last: Instant,
}

impl Device for SilentChattyDevice {
    fn send(&mut self, _report: &Report) -> Result<bool, WireError> {
        Ok(true)
    }
    fn on_received_data(&mut self, _packet: RawPacket) {}
    fn max_packet_size(&self) -> usize {
        304
    }
    fn vid(&self) -> u16 {
        9189
    }
    fn pid(&self) -> u16 {
        PID_QUADRO
    }
    fn poll_reports(&mut self) -> Vec<Report> {
        if self.last.elapsed() < Duration::from_millis(20) {
            return Vec::new();
        }
        self.last = Instant::now();
        vec![Report { header: Header::new(0x73, 0, 0, 0), contents: self.contents.clone() }]
    }
}

/// A request stuck waiting on its device must not hold back events on the same socket.
///
/// The device emits ~50 reports/s and never answers, so the RPC runs to the 3 s timeout.
/// Before the fix the socket loop awaited the RPC inline and delivered nothing meanwhile;
/// only the handful of frames already in flight when the RPC arrived could get through.
#[tokio::test]
async fn a_pending_rpc_does_not_stall_events() {
    let registries = RegistrySet::builtin().expect("registries");
    let size: usize = registries
        .for_pid(PID_QUADRO)
        .unwrap()
        .registry
        .cyclic(0x73)
        .expect("0x73 layout")
        .fields
        .iter()
        .map(Field::size)
        .sum();
    let devices = DeviceManager::new(registries);
    devices.attach(
        DeviceId::loopback(0),
        Box::new(SilentChattyDevice { contents: vec![0xA5; size.max(256)], last: Instant::now() }),
        "loopback",
        true,
    );

    let url = serve_with(devices).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");
    let _hello = next_json(&mut ws).await;
    // Prove events flow before the request.
    while next_json(&mut ws).await["type"] != "cyclic" {}

    ws.send(Message::Text(
        json!({"id": 1, "device_id": "loopback-0", "command": "set_mixer",
               "args": {"level": 64}})
        .to_string(),
    ))
    .await
    .unwrap();

    let sent = Instant::now();
    let mut cyclic = 0;
    while sent.elapsed() < Duration::from_millis(1500) {
        let remaining = Duration::from_millis(1500).saturating_sub(sent.elapsed());
        match tokio::time::timeout(remaining, next_json(&mut ws)).await {
            Ok(frame) if frame["type"] == "cyclic" => cyclic += 1,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(
        cyclic >= 20,
        "only {cyclic} cyclic events in the 1.5 s after sending an RPC; the socket loop is \
         blocked on the request"
    );
}

/// Answers nothing, and goes away once `unplugged` is set, as an unplugged USB device does.
struct UnpluggableDevice {
    unplugged: Arc<std::sync::atomic::AtomicBool>,
    connected: bool,
}

impl Device for UnpluggableDevice {
    fn send(&mut self, _report: &Report) -> Result<bool, WireError> {
        Ok(self.connected)
    }
    fn on_received_data(&mut self, _packet: RawPacket) {}
    fn max_packet_size(&self) -> usize {
        320
    }
    fn vid(&self) -> u16 {
        9189
    }
    fn pid(&self) -> u16 {
        PID_STUDIO
    }
    fn poll_reports(&mut self) -> Vec<Report> {
        if self.unplugged.load(std::sync::atomic::Ordering::SeqCst) {
            self.connected = false;
        }
        Vec::new()
    }
    fn is_connected(&self) -> bool {
        self.connected
    }
}

/// A device that goes away mid-run is announced to connected clients, and one plugged in is too.
#[tokio::test]
async fn devices_coming_and_going_reach_a_connected_client() {
    let devices = DeviceManager::new(RegistrySet::builtin().expect("registries"));
    let unplugged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let studio = DeviceId::from_serial("S1");
    devices.attach(studio.clone(), Box::new(UnpluggableDevice { unplugged: unplugged.clone(), connected: true }), "usb", true);

    let url = serve_with(devices.clone()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["devices"][0]["id"], "serial:S1");

    unplugged.store(true, std::sync::atomic::Ordering::SeqCst);
    let frame = tokio::time::timeout(Duration::from_secs(2), next_json(&mut ws)).await.expect("told within 2 s");
    assert_eq!(frame, json!({"type": "device_removed", "device_id": "serial:S1"}));

    unplugged.store(false, std::sync::atomic::Ordering::SeqCst);
    devices.attach(studio, Box::new(UnpluggableDevice { unplugged, connected: true }), "usb", true);
    let frame = tokio::time::timeout(Duration::from_secs(2), next_json(&mut ws)).await.expect("told within 2 s");
    assert_eq!(frame["type"], "device_added");
    assert_eq!(frame["device"]["id"], "serial:S1");
    assert_eq!(frame["device"]["model"], "Zen Studio+");
    devices.shutdown_all();
}
