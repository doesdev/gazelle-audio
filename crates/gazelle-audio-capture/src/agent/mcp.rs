//! MCP over streamable HTTP (spec §9) at `/mcp`, behind the helper's bearer token. Tools come
//! from the operation table and the `probe-parameter` prompt from the shared guidance, so the
//! MCP and OpenAI interfaces cannot drift apart.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::Router;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData, GetPromptRequestParams, GetPromptResponse, GetPromptResult, Implementation,
    ListPromptsResult, ListToolsResult, PaginatedRequestParams, Prompt, PromptMessage, Role, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{MaybeSendFuture, RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::ServerHandler;
use serde_json::Value;

use crate::ops::guidance::{GUIDANCE, PROMPT_DESCRIPTION, PROMPT_NAME};
use crate::ops::service::{Ops, OpsError};
use crate::ops::types::operations;
use crate::panel::security;

/// One MCP session's handler; every session shares the helper's [`Ops`].
#[derive(Clone)]
pub struct McpServer {
    ops: Ops,
}

impl McpServer {
    pub fn new(ops: Ops) -> Self {
        Self { ops }
    }
}

/// MCP tool definitions, one per operation, with the operation's input schema unchanged.
pub fn mcp_tools() -> Vec<Tool> {
    operations()
        .into_iter()
        .map(|op| {
            let schema = match op.input_schema {
                Value::Object(map) => map,
                _ => serde_json::Map::new(),
            };
            Tool::new(op.name, op.description, Arc::new(schema))
        })
        .collect()
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
            .with_server_info(Implementation::new("gazelle-capture", env!("CARGO_PKG_VERSION")))
            .with_instructions(GUIDANCE)
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(mcp_tools())))
    }

    /// Operation failures are tool results with `is_error` so the model can read and react to
    /// them; unknown tools and malformed arguments are protocol errors.
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResponse, ErrorData>> + MaybeSendFuture + '_ {
        let ops = self.ops.clone();
        async move {
            let name = request.name.to_string();
            let arguments = request.arguments.map_or(Value::Null, Value::Object);
            let outcome = tokio::task::spawn_blocking(move || ops.call(&name, arguments))
                .await
                .map_err(|e| ErrorData::internal_error(format!("operation task failed: {e}"), None))?;
            match outcome {
                Ok(result) => {
                    let text = serde_json::to_string_pretty(&result).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                    Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
                }
                Err(e @ (OpsError::UnknownOperation(_) | OpsError::BadArguments { .. })) => Err(ErrorData::invalid_params(e.to_string(), None)),
                Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e.to_string())]).into()),
            }
        }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, ErrorData>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListPromptsResult::with_all_items(vec![Prompt::new(PROMPT_NAME, Some(PROMPT_DESCRIPTION), None)])))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResponse, ErrorData>> + MaybeSendFuture + '_ {
        std::future::ready(if request.name == PROMPT_NAME {
            Ok(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, GUIDANCE)]).with_description(PROMPT_DESCRIPTION).into())
        } else {
            Err(ErrorData::invalid_params(format!("unknown prompt {}", request.name), None))
        })
    }
}

/// `/mcp` behind the bearer token, without the Host check (merge before `with_host_check`).
/// rmcp's own allowed-hosts check is set to this helper's port as a second layer.
pub fn router(ops: Ops, token: Arc<str>, port: u16) -> Router {
    let config = StreamableHttpServerConfig::default().with_allowed_hosts([format!("127.0.0.1:{port}"), format!("localhost:{port}")]);
    let service: StreamableHttpService<McpServer, LocalSessionManager> = StreamableHttpService::new(move || Ok(McpServer::new(ops.clone())), Default::default(), config);
    Router::new().nest_service("/mcp", service).layer(middleware::from_fn(move |request: Request, next: Next| {
        let token = Arc::clone(&token);
        async move {
            if !security::bearer_ok(request.headers(), &token) {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            next.run(request).await
        }
    }))
}
