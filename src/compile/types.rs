//! Common types for the agentic pipeline compiler.
//!
//! This module defines the front matter grammar that is shared across all compile targets.

use serde::Deserialize;
use std::collections::HashMap;

/// Target platform for compiled pipeline
#[derive(Debug, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CompileTarget {
    /// Standalone pipeline with full feature set (default)
    #[default]
    Standalone,
    /// 1ES Pipeline Template integration using agencyJob
    #[serde(rename = "1es")]
    OneES,
}

/// Pool configuration - accepts both string and object formats
///
/// Examples:
/// ```yaml
/// # Simple string format (works for both targets)
/// pool: AZS-1ES-L-MMS-ubuntu-22.04
///
/// # Object format (required for 1ES if specifying os)
/// pool:
///   name: AZS-1ES-L-MMS-ubuntu-22.04
///   os: linux
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum PoolConfig {
    /// Simple pool name string
    Name(String),
    /// Full pool configuration object
    Full(PoolConfigFull),
}

impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig::Name("AZS-1ES-L-MMS-ubuntu-22.04".to_string())
    }
}

impl PoolConfig {
    /// Get the pool name
    pub fn name(&self) -> &str {
        match self {
            PoolConfig::Name(name) => name,
            PoolConfig::Full(full) => &full.name,
        }
    }

    /// Get the OS (defaults to "linux" if not specified)
    pub fn os(&self) -> &str {
        match self {
            PoolConfig::Name(_) => "linux",
            PoolConfig::Full(full) => full.os.as_deref().unwrap_or("linux"),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct PoolConfigFull {
    pub name: String,
    #[serde(default)]
    pub os: Option<String>,
}

/// Schedule configuration - accepts both string and object formats
///
/// Examples:
/// ```yaml
/// # Simple string format (defaults to main branch only)
/// schedule: daily around 14:00
///
/// # Object format (with custom branch filtering)
/// schedule:
///   run: daily around 14:00
///   branches:
///     - main
///     - release/*
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum ScheduleConfig {
    /// Simple schedule expression string
    Simple(String),
    /// Schedule with options (branch filtering)
    WithOptions(ScheduleOptions),
}

impl ScheduleConfig {
    /// Get the schedule expression string
    pub fn expression(&self) -> &str {
        match self {
            ScheduleConfig::Simple(s) => s,
            ScheduleConfig::WithOptions(opts) => &opts.run,
        }
    }

    /// Get the branches filter (empty means default to "main" branch)
    pub fn branches(&self) -> &[String] {
        match self {
            ScheduleConfig::Simple(_) => &[],
            ScheduleConfig::WithOptions(opts) => &opts.branches,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ScheduleOptions {
    /// Fuzzy schedule expression (e.g., "daily around 14:00")
    pub run: String,
    /// Branches to restrict the schedule to (empty = defaults to "main")
    #[serde(default)]
    pub branches: Vec<String>,
}

/// Engine configuration - accepts both string and object formats
///
/// Examples:
/// ```yaml
/// # Simple string format (just a model name)
/// engine: claude-opus-4.5
///
/// # Object format (with additional options)
/// engine:
///   model: claude-opus-4.5
///   max-turns: 50
///   timeout-minutes: 30
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum EngineConfig {
    /// Simple model name string
    Simple(String),
    /// Full engine configuration object
    Full(EngineOptions),
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig::Simple(default_model())
    }
}

impl EngineConfig {
    /// Get the model name
    pub fn model(&self) -> &str {
        match self {
            EngineConfig::Simple(s) => s,
            EngineConfig::Full(opts) => opts.model.as_deref().unwrap_or("claude-opus-4.5"),
        }
    }

    /// Get the max turns setting
    pub fn max_turns(&self) -> Option<u32> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.max_turns,
        }
    }

    /// Get the timeout in minutes
    pub fn timeout_minutes(&self) -> Option<u32> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.timeout_minutes,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct EngineOptions {
    /// AI model to use (defaults to claude-opus-4.5)
    #[serde(default)]
    pub model: Option<String>,
    /// Maximum number of chat iterations per run
    #[serde(default, rename = "max-turns")]
    pub max_turns: Option<u32>,
    /// Workflow timeout in minutes
    #[serde(default, rename = "timeout-minutes")]
    pub timeout_minutes: Option<u32>,
}

/// Tools configuration for the agent
///
/// Controls which tools are available and their settings.
/// If not specified, defaults are used.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ToolsConfig {
    /// Bash command allow-list. If empty/not set, defaults to safe commands.
    /// Use [":*"] for unrestricted access.
    #[serde(default)]
    pub bash: Option<Vec<String>>,
    /// Enable the file editing tool (default: true)
    #[serde(default)]
    pub edit: Option<bool>,
}

/// Front matter configuration from the input markdown file
#[derive(Debug, Deserialize)]
pub struct FrontMatter {
    /// Agent name (required)
    pub name: String,
    /// One-line description (required)
    pub description: String,
    /// Target platform: "standalone" (default) or "1es"
    #[serde(default)]
    pub target: CompileTarget,
    /// Fuzzy schedule configuration
    #[serde(default)]
    pub schedule: Option<ScheduleConfig>,
    /// Workspace setting: "root" or "repo" (auto-computed if not set)
    #[serde(default)]
    pub workspace: Option<String>,
    /// Agent pool configuration
    #[serde(default)]
    pub pool: Option<PoolConfig>,
    /// AI engine configuration (defaults to claude-opus-4.5)
    #[serde(default)]
    pub engine: EngineConfig,
    /// Tools configuration
    #[serde(default)]
    pub tools: Option<ToolsConfig>,
    /// Additional repository resources
    #[serde(default)]
    pub repositories: Vec<Repository>,
    /// Repositories to checkout (subset of repositories)
    #[serde(default)]
    pub checkout: Vec<String>,
    /// MCP server configurations
    #[serde(default, rename = "mcp-servers")]
    pub mcp_servers: HashMap<String, McpConfig>,
    /// Per-tool configuration for safe outputs
    #[serde(default, rename = "safe-outputs")]
    pub safe_outputs: HashMap<String, serde_json::Value>,
    /// Pipeline trigger configuration
    #[serde(default)]
    pub triggers: Option<TriggerConfig>,
    /// Network policy for standalone target (ignored in 1ES)
    #[serde(default)]
    pub network: Option<NetworkConfig>,
    /// Custom steps before agent runs (same job)
    #[serde(default)]
    pub steps: Vec<serde_yaml::Value>,
    /// Custom steps after agent runs (same job)
    #[serde(default, rename = "post-steps")]
    pub post_steps: Vec<serde_yaml::Value>,
    /// Separate setup job before agentic task
    #[serde(default)]
    pub setup: Vec<serde_yaml::Value>,
    /// Separate teardown job after safe outputs
    #[serde(default)]
    pub teardown: Vec<serde_yaml::Value>,
    /// Azure Resource Manager service connection for read-only ADO token
    /// When set, uses AzureCLI@2 to mint an ADO-scoped token from this connection.
    /// When unset, ADO access tokens are omitted from the copilot invocation.
    #[serde(default, rename = "read-only-service-connection")]
    pub read_only_service_connection: Option<String>,
    /// Workflow-level environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
}

fn default_model() -> String {
    "claude-opus-4.5".to_string()
}

/// Network policy configuration (standalone target only)
///
/// Network isolation uses AWF (Agentic Workflow Firewall) for L7 domain whitelisting.
/// The domain allowlist is dynamically generated based on:
/// - Core Azure DevOps/GitHub endpoints (always included)
/// - MCP-specific endpoints for each enabled MCP
/// - User-specified additional hosts from `allow` field
#[derive(Debug, Deserialize, Clone, Default)]
pub struct NetworkConfig {
    /// Additional allowed host patterns (supports wildcards like *.example.com)
    /// Core Azure DevOps and GitHub hosts are always allowed.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Blocked host patterns (takes precedence over allow)
    #[serde(default)]
    pub blocked: Vec<String>,
}

/// Repository resource definition
#[derive(Debug, Deserialize, Clone)]
pub struct Repository {
    pub repository: String,
    #[serde(rename = "type")]
    pub repo_type: String,
    pub name: String,
    #[serde(default = "default_ref")]
    #[serde(rename = "ref")]
    pub repo_ref: String,
}

fn default_ref() -> String {
    "refs/heads/main".to_string()
}

/// MCP configuration - can be `true` for simple enablement or an object with options
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum McpConfig {
    Enabled(bool),
    WithOptions(McpOptions),
}

/// Detailed MCP options
#[derive(Debug, Deserialize, Clone, Default)]
pub struct McpOptions {
    /// Custom command (if present, it's a custom MCP - standalone only)
    #[serde(default)]
    pub command: Option<String>,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Allowed tool names (for firewall filtering)
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Service connection name (1ES only, auto-generated if not specified)
    #[serde(default, rename = "service-connection")]
    pub service_connection: Option<String>,
}

/// Trigger configuration for the pipeline
#[derive(Debug, Deserialize, Clone, Default)]
pub struct TriggerConfig {
    /// Pipeline completion trigger
    #[serde(default)]
    pub pipeline: Option<PipelineTrigger>,
}

/// Pipeline completion trigger configuration
#[derive(Debug, Deserialize, Clone)]
pub struct PipelineTrigger {
    /// The name of the source pipeline that triggers this one
    pub name: String,
    /// Optional project name if the pipeline is in a different project
    #[serde(default)]
    pub project: Option<String>,
    /// Branches to trigger on (empty = any branch)
    #[serde(default)]
    pub branches: Vec<String>,
}
