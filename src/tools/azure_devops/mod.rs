//! Azure DevOps first-class tool.
//!
//! Compile-time: injects network hosts (ADO domains), MCPG server entry
//! (containerized ADO MCP), and compile-time validation (org inference,
//! duplicate MCP).

pub mod extension;

pub use extension::AzureDevOpsExtension;

/// Azure DevOps resource ID for token acquisition.
pub const ADO_RESOURCE_ID: &str = "499b84ac-1321-427f-aa17-267ca6975798";

/// Pipeline variable name for the ADO MCP token (secret).
pub const ADO_MCP_PIPELINE_VAR: &str = "SC_ADO_MCP_TOKEN";

/// Container env var name for the ADO MCP token.
pub const ADO_MCP_TOKEN_VAR: &str = "ADO_MCP_AUTH_TOKEN";
