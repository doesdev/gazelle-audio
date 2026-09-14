//! The `mcp-stdio` relay in process: an rmcp client on one end of an in-memory pipe sees exactly
//! what the helper's `/mcp` serves — tools, results, errors, prompt and instructions.

use std::sync::Arc;

use gazelle_audio_capture::agent::mcp::router;
use gazelle_audio_capture::agent::relay::Relay;
use gazelle_audio_capture::ops::guidance::{GUIDANCE, PROMPT_NAME};
use gazelle_audio_capture::ops::service::Ops;
use gazelle_audio_capture::ops::types::operations;
use gazelle_audio_capture::session::controller::Environment;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, GetPromptRequestParams};
use rmcp::ServiceExt;
use serde_json::{json, Value};

const TOKEN: &str = "relay-test-token";

async fn helper() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(Ops::new(Environment::default()), Arc::from(TOKEN), port);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://127.0.0.1:{port}/mcp")
}

fn text(result: &CallToolResult) -> String {
    match result.content.first() {
        Some(ContentBlock::Text(t)) => t.text.clone(),
        other => panic!("expected text content, got {other:?}"),
    }
}

#[tokio::test]
async fn the_relay_serves_what_the_helper_serves() {
    let relay = Relay::connect(&helper().await, TOKEN).await.expect("relay connects with the token");
    let (relay_io, client_io) = tokio::io::duplex(1 << 16);
    let served = tokio::spawn(relay.serve_on(tokio::io::split(relay_io)));
    let client = ().serve(client_io).await.expect("client connects through the relay");

    let info = client.peer_info().expect("handshake info");
    assert_eq!(info.server_info.as_ref().map(|i| i.name.as_str()), Some("gazelle-capture"));
    assert_eq!(info.instructions.as_deref(), Some(GUIDANCE));

    let names: Vec<String> = client.list_all_tools().await.unwrap().iter().map(|t| t.name.to_string()).collect();
    assert_eq!(names, operations().iter().map(|o| o.name.to_string()).collect::<Vec<_>>());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s").display().to_string();
    let arguments = json!({ "path": path, "vid": 4660, "pid": 43981 }).as_object().unwrap().clone();
    let opened = client.call_tool(CallToolRequestParams::new("session_open").with_arguments(arguments)).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&text(&opened)).unwrap()["open"], true);

    let arguments = json!({ "parameter": "gain" }).as_object().unwrap().clone();
    let failed = client.call_tool(CallToolRequestParams::new("get_field_map").with_arguments(arguments)).await.unwrap();
    assert_eq!(failed.is_error, Some(true), "operation failures stay tool results");
    assert!(text(&failed).contains("no field map for gain"), "{}", text(&failed));
    assert!(client.call_tool(CallToolRequestParams::new("mark_step_done")).await.is_err(), "unknown tools stay protocol errors");

    let prompt = client.get_prompt(GetPromptRequestParams::new(PROMPT_NAME)).await.unwrap();
    assert!(matches!(&prompt.messages[0].content, ContentBlock::Text(t) if t.text == GUIDANCE));
    assert!(client.get_prompt(GetPromptRequestParams::new("nope")).await.is_err());

    client.cancel().await.unwrap();
    served.await.unwrap().expect("the relay ends cleanly when its client leaves");
}

#[tokio::test]
async fn the_relay_refuses_to_start_without_the_right_token() {
    let url = helper().await;
    let error = Relay::connect(&url, "wrong").await.err().expect("a wrong token fails the connection");
    assert!(error.to_string().contains(&url), "{error}");
}
