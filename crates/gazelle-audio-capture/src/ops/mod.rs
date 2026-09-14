//! The one operation set agents use (spec §9). MCP tools and OpenAI function definitions are
//! both generated from [`types::operations`]; no operation marks a step done, redoes or skips
//! it. Plan `plans/2026-09-15-capture-3-agent-interfaces.md`.

pub mod types;
