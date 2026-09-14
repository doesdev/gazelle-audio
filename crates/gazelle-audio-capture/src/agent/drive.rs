//! `drive`: a chat-completions agent loop (spec §9) for models behind an OpenAI-compatible API.
//! The system message is the shared guidance, the tools are [`openai_tools`], and each tool call
//! runs through [`execute`] against the helper's own [`Ops`], so the loop has exactly the powers
//! an MCP or `/openai/call` client has. Operation failures go back to the model as tool results;
//! only transport failures and the turn limit end the loop.

use serde_json::{json, Value};

use super::openai::{execute, openai_tools, ToolCall};
use crate::ops::guidance::GUIDANCE;
use crate::ops::service::Ops;

/// Model turns before giving up; a full live probe with `await_progress` polling needs dozens.
pub const DEFAULT_MAX_TURNS: usize = 200;

#[derive(Debug, Clone)]
pub struct DriveConfig {
    /// API root that `/chat/completions` is appended to, e.g. `http://127.0.0.1:11434/v1`.
    pub base_url: String,
    pub model: String,
    /// Sent as `Authorization: Bearer` when set.
    pub api_key: Option<String>,
    pub max_turns: usize,
}

/// Progress reported while the loop runs.
#[derive(Debug, Clone, PartialEq)]
pub enum DriveEvent {
    Assistant(String),
    ToolCall { name: String, arguments: String },
    ToolResult { name: String, ok: bool },
}

#[derive(Debug)]
pub struct Outcome {
    /// The model's final message, which carried no tool calls.
    pub answer: String,
    /// The whole conversation, system message first.
    pub messages: Vec<Value>,
    pub tool_calls: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum DriveError {
    #[error("--base-url must start with http:// or https://: {0}")]
    Scheme(String),
    #[error("chat completions request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("chat completions returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("chat completions response has no choices[0].message: {0}")]
    Malformed(String),
    #[error("no final answer after {0} model turns")]
    TurnLimit(usize),
}

/// Runs `task` to a final answer.
pub async fn drive(ops: &Ops, config: &DriveConfig, task: &str, on_event: &mut dyn FnMut(&DriveEvent)) -> Result<Outcome, DriveError> {
    if !(config.base_url.starts_with("http://") || config.base_url.starts_with("https://")) {
        return Err(DriveError::Scheme(config.base_url.clone()));
    }
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    super::ensure_crypto_provider();
    let client = reqwest::Client::new();
    let tools = openai_tools();
    let mut messages = vec![json!({ "role": "system", "content": GUIDANCE }), json!({ "role": "user", "content": task })];
    let mut tool_calls = 0;
    for _ in 0..config.max_turns {
        let mut request = client.post(&url).json(&json!({ "model": config.model, "messages": messages, "tools": tools }));
        if let Some(key) = &config.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(DriveError::Status { status: status.as_u16(), body });
        }
        let parsed: Value = serde_json::from_str(&body).map_err(|e| DriveError::Malformed(format!("{e}: {body}")))?;
        let message = parsed["choices"][0]["message"].clone();
        if !message.is_object() {
            return Err(DriveError::Malformed(body));
        }
        let content = message["content"].as_str().unwrap_or_default().to_string();
        let calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
        messages.push(message);
        if !content.is_empty() {
            on_event(&DriveEvent::Assistant(content.clone()));
        }
        if calls.is_empty() {
            return Ok(Outcome { answer: content, messages, tool_calls });
        }
        for call in calls {
            let tool = ToolCall { name: call["function"]["name"].as_str().unwrap_or_default().to_string(), arguments: call["function"]["arguments"].clone() };
            let arguments = tool.arguments.as_str().map_or_else(|| tool.arguments.to_string(), str::to_string);
            on_event(&DriveEvent::ToolCall { name: tool.name.clone(), arguments });
            let (content, ok) = match execute(ops, &tool).await {
                Ok(result) => (result.to_string(), true),
                Err((_, error)) => (json!({ "error": error }).to_string(), false),
            };
            on_event(&DriveEvent::ToolResult { name: tool.name.clone(), ok });
            messages.push(json!({ "role": "tool", "tool_call_id": call["id"], "content": content }));
            tool_calls += 1;
        }
    }
    Err(DriveError::TurnLimit(config.max_turns))
}
