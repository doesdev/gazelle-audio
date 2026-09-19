//! Transports over the operation set: OpenAI-compatible tools, MCP over HTTP, its stdio
//! relay, and the `drive` loop.

pub mod drive;
pub mod mcp;
pub mod openai;
pub mod relay;

/// Installs rustls' ring provider for this process. reqwest is built without a provider, so
/// every HTTP client (drive's, and rmcp's in the relay) needs this first; repeat calls are no-ops.
pub fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
