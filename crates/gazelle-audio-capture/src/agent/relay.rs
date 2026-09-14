//! `mcp-stdio`: an MCP server on stdio that relays to a running helper's `/mcp` (spec §9), for
//! clients such as Claude Code that launch MCP servers as child processes. Every list, call and
//! prompt request is forwarded unchanged with the helper's bearer token, and the helper's
//! handshake info (its instructions included) is served as the relay's own, so the helper stays
//! the single source of tools, prompts and errors.

use std::collections::HashMap;

use axum::http::{header, HeaderName, HeaderValue};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ErrorData, GetPromptRequestParams, GetPromptResponse, ListPromptsResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleClient, RoleServer, RunningService, ServiceError};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ServerHandler, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};

pub type RelayError = Box<dyn std::error::Error + Send + Sync>;

/// The stdio-side handler, holding the MCP client session with the helper.
pub struct Relay {
    upstream: RunningService<RoleClient, ()>,
}

impl Relay {
    /// Connects to the helper's MCP endpoint, e.g. `http://127.0.0.1:8430/mcp`.
    pub async fn connect(url: &str, token: &str) -> Result<Self, RelayError> {
        let bearer = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| "the token is not a valid header value")?;
        let headers: HashMap<HeaderName, HeaderValue> = HashMap::from([(header::AUTHORIZATION, bearer)]);
        let config = StreamableHttpClientTransportConfig::with_uri(url).custom_headers(headers);
        super::ensure_crypto_provider();
        let upstream = ().serve(StreamableHttpClientTransport::from_config(config)).await.map_err(|e| format!("connecting to {url}: {e}"))?;
        Ok(Self { upstream })
    }

    /// Serves MCP on `io` (a reader and writer pair such as `rmcp::transport::stdio()`) until the
    /// client disconnects.
    pub async fn serve_on<R, W>(self, io: (R, W)) -> Result<(), RelayError>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        self.serve(io).await?.waiting().await?;
        Ok(())
    }
}

/// Protocol errors from the helper pass through as they are; transport failures become internal
/// errors naming the helper.
fn relayed(e: ServiceError) -> ErrorData {
    match e {
        ServiceError::McpError(error) => error,
        other => ErrorData::internal_error(format!("gazelle-capture helper: {other}"), None),
    }
}

impl ServerHandler for Relay {
    fn get_info(&self) -> ServerInfo {
        let Some(upstream) = self.upstream.peer_info() else {
            return ServerInfo::new(ServerCapabilities::builder().enable_tools().enable_prompts().build());
        };
        let mut info = ServerInfo::new(upstream.capabilities.clone());
        if let Some(implementation) = &upstream.server_info {
            info = info.with_server_info(implementation.clone());
        }
        if let Some(instructions) = &upstream.instructions {
            info = info.with_instructions(instructions.clone());
        }
        info
    }

    async fn list_tools(&self, request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData> {
        self.upstream.list_tools(request).await.map_err(relayed)
    }

    async fn call_tool(&self, request: CallToolRequestParams, _context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        self.upstream.call_tool_once(request).await.map_err(relayed)
    }

    async fn list_prompts(&self, request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListPromptsResult, ErrorData> {
        self.upstream.list_prompts(request).await.map_err(relayed)
    }

    async fn get_prompt(&self, request: GetPromptRequestParams, _context: RequestContext<RoleServer>) -> Result<GetPromptResponse, ErrorData> {
        self.upstream.get_prompt_once(request).await.map_err(relayed)
    }
}
