//! Runtime implementations for the ado-aw compiler.
//!
//! Runtimes are language toolchains installed before the agent runs
//! (e.g., Lean 4, Python, Node.js, .NET, and in future: Go, etc.).
//!
//! Unlike `tools/` (agent capabilities like edit, bash, memory) or
//! `safe_outputs/` (MCP tools that serialize to NDJSON), runtimes are
//! execution environments the compiler auto-installs via pipeline steps.
//!
//! Aligned with gh-aw's `runtimes:` front matter field.

pub mod dotnet;
pub mod lean;
pub mod node;
pub mod python;
