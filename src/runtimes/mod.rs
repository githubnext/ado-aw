//! Runtime implementations for the ado-aw compiler.
//!
//! Runtimes are language toolchains installed before the agent runs
//! (e.g., Lean 4, and in future: Python, Node, Go, etc.).
//!
//! Unlike `tools/` (agent capabilities like edit, bash, memory) or
//! `safeoutputs/` (MCP tools that serialize to NDJSON), runtimes are
//! execution environments the compiler auto-installs via pipeline steps.
//!
//! Aligned with gh-aw's `runtimes:` front matter field.

pub mod lean;
