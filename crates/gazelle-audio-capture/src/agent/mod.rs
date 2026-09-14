//! Transports over the operation set (spec §9): OpenAI-compatible tools, MCP over HTTP and its
//! stdio relay; the `drive` loop follows. Plan `plans/2026-09-15-capture-3-agent-interfaces.md`.

pub mod mcp;
pub mod openai;
pub mod relay;
