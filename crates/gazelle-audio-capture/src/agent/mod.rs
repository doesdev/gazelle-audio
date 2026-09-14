//! Transports over the operation set (spec §9): OpenAI-compatible tools, MCP over HTTP, its stdio
//! relay, and the `drive` loop. Plan `plans/2026-09-15-capture-3-agent-interfaces.md`.

pub mod drive;
pub mod mcp;
pub mod openai;
pub mod relay;

/// Installs rustls' ring provider for this process. reqwest is built without a provider, so
/// every HTTP client (drive's, and rmcp's in the relay) needs this first; repeat calls are no-ops.
pub fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
