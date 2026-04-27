//! S360 Breeze first-class tool.
//!
//! Compile-time: injects network hosts (S360 + auth domains), MCPG HTTP
//! server entry (S360 MCP endpoint with Bearer token), service principal
//! token acquisition step, and compile-time validation.

pub mod extension;

pub use extension::S360BreezeExtension;

/// S360 Breeze MCP PROD endpoint.
pub const S360_ENDPOINT: &str = "https://mcp.vnext.s360.msftcloudes.com/";

/// Azure AD resource ID for S360 MCP PROD (service-to-service scope).
pub const S360_RESOURCE_ID: &str = "api://08654c87-a8c1-4098-a44b-079efd603fdc";

/// MCPG server name for the S360 Breeze MCP entry.
pub const S360_SERVER_NAME: &str = "s360-breeze";

/// Pipeline variable name for the S360 token (secret).
pub const S360_PIPELINE_VAR: &str = "SC_S360_TOKEN";

/// MCPG config placeholder for the S360 token.
pub const S360_CONFIG_PLACEHOLDER: &str = "S360_TOKEN";
