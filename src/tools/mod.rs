//! First-class tool implementations for the ado-aw compiler.
//!
//! These tools are configured via the `tools:` front-matter section and provide
//! built-in functionality that the compiler knows how to auto-configure
//! (pipeline steps, MCPG entries, network allowlists, etc.).
//!
//! This is distinct from `safeoutputs/` which contains safe-output MCP tools
//! that serialize to NDJSON in Stage 1 and execute in Stage 3.

pub mod cache_memory;
