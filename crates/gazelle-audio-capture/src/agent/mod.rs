//! Transports over the operation set (spec §9): OpenAI-compatible tools, MCP over HTTP, its stdio
//! relay, and the `drive` loop. Plan `plans/2026-09-15-capture-3-agent-interfaces.md`.

pub mod drive;
pub mod mcp;
pub mod openai;
pub mod relay;
