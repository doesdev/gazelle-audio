//! MCP end to end with a real rmcp client over streamable HTTP: the tools are the operation set,
//! calls reach the operations, the prompt carries the guidance, and the token is required.

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::{HeaderName, HeaderValue};
use gazelle_audio_capture::agent::mcp::router;
use gazelle_audio_capture::agent::openai::openai_tools;
use gazelle_audio_capture::ops::guidance::PROMPT_NAME;
use gazelle_audio_capture::ops::service::Ops;
use gazelle_audio_capture::ops::types::operations;
use gazelle_audio_capture::session::controller::Environment;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, GetPromptRequestParams};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::{json, Value};

const TOKEN: &str = "mcp-test-token";

async fn serve() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(Ops::new(Environment::default()), Arc::from(TOKEN), port);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

fn config(port: u16, token: Option<&str>) -> StreamableHttpClientTransportConfig {
    let mut headers = HashMap::new();
    if let Some(t) = token {
        headers.insert(HeaderName::from_static("authorization"), HeaderValue::from_str(&format!("Bearer {t}")).unwrap());
    }
    StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp")).custom_headers(headers)
}

fn text(result: &CallToolResult) -> String {
    match result.content.first() {
        Some(ContentBlock::Text(t)) => t.text.clone(),
        other => panic!("expected text content, got {other:?}"),
    }
}

fn args(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().unwrap().clone()
}

#[tokio::test]
async fn mcp_serves_the_operation_set_and_the_guidance() {
    let port = serve().await;
    let client = ().serve(StreamableHttpClientTransport::from_config(config(port, Some(TOKEN)))).await.expect("client connects with the token");

    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    let expected: Vec<String> = operations().iter().map(|o| o.name.to_string()).collect();
    assert_eq!(names, expected);
    let openai: Vec<String> = openai_tools().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, openai, "MCP and OpenAI tool names are identical");
    for (tool, op) in tools.iter().zip(operations()) {
        assert_eq!(Value::Object(tool.input_schema.as_ref().clone()), op.input_schema, "{}", op.name);
        assert_eq!(tool.description.as_deref(), Some(op.description));
    }

    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("s").display().to_string();
    let opened = client.call_tool(CallToolRequestParams::new("session_open").with_arguments(args(json!({ "path": session, "vid": 4660, "pid": 43981 })))).await.unwrap();
    assert_ne!(opened.is_error, Some(true), "{}", text(&opened));
    let opened: Value = serde_json::from_str(&text(&opened)).unwrap();
    assert_eq!(opened["open"], true);

    let listed = client.call_tool(CallToolRequestParams::new("list_parameters")).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&text(&listed)).unwrap(), json!([]));

    let failed = client.call_tool(CallToolRequestParams::new("get_field_map").with_arguments(args(json!({ "parameter": "gain" })))).await.unwrap();
    assert_eq!(failed.is_error, Some(true));
    assert!(text(&failed).contains("no field map for gain"), "{}", text(&failed));

    assert!(client.call_tool(CallToolRequestParams::new("mark_step_done")).await.is_err(), "unknown tools are protocol errors");

    let prompts = client.list_prompts(None).await.unwrap();
    assert_eq!(prompts.prompts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), [PROMPT_NAME]);
    let prompt = client.get_prompt(GetPromptRequestParams::new(PROMPT_NAME)).await.unwrap();
    let guidance = match &prompt.messages[0].content {
        ContentBlock::Text(t) => t.text.clone(),
        other => panic!("expected text, got {other:?}"),
    };
    assert!(guidance.contains("import_capture") && guidance.contains("cannot mark steps done"), "{guidance}");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_requires_the_token() {
    let port = serve().await;
    assert!(().serve(StreamableHttpClientTransport::from_config(config(port, None))).await.is_err());
    assert!(().serve(StreamableHttpClientTransport::from_config(config(port, Some("wrong")))).await.is_err());
}
