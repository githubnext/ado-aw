//! First-class tool implementations for the ado-aw compiler.
//!
//! Each tool is colocated in its own subdirectory containing both
//! compile-time (`extension.rs` — [`CompilerExtension`] impl) and
//! runtime (`execute.rs` — Stage 3 logic) code where applicable.
//!
//! Tools are configured via the `tools:` front-matter section and provide
//! built-in functionality that the compiler knows how to auto-configure
//! (pipeline steps, MCPG entries, network allowlists, etc.).
//!
//! This is distinct from `safeoutputs/` which contains safe-output MCP tools
//! that serialize to NDJSON in Stage 1 and execute in Stage 3.

pub mod azure_devops;
pub mod cache_memory;
pub mod s360_breeze;
