//! The one operation set agents use. MCP tools and OpenAI function definitions are
//! both generated from [`types::operations`]; no operation marks a step done, redoes or skips
//! it. [`service::Ops`] executes them; [`guidance`] is the procedure agents are taught.

pub mod guidance;
pub mod service;
pub mod types;
