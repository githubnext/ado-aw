//! Azure DevOps first-class tool.
//!
//! Compile-time: injects network hosts (ADO domains), MCPG server entry
//! (containerized ADO MCP), and compile-time validation (org inference,
//! duplicate MCP).

pub mod extension;

pub use extension::{AdoAuthMode, AzureDevOpsExtension};
