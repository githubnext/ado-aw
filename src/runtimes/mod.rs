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

/// Compile warning for a runtime package-source setting.
///
/// The Azure Pipelines package-authenticate tasks publish their credential as
/// a non-secret job variable or an agent-visible file, so the compiler no
/// longer runs them in the Agent job. The source setting still selects the
/// feed, but requests from the agent carry no credential.
pub(crate) fn unauthenticated_feed_warning(field: &str) -> String {
    format!(
        "{field} selects the package source only. The agent has no feed credential, so \
         feeds that require authentication (including Azure Artifacts) will reject \
         requests from inside the sandbox."
    )
}
