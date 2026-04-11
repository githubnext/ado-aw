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

    /// Get the max turns setting (deprecated — ignored at compile time)
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
    /// Maximum number of chat iterations per run (deprecated — not supported by Copilot CLI)
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
    /// Permissions configuration for ADO access tokens.
    ///
    /// ADO supports two access levels: blanket read and blanket write.
    /// Tokens are minted from ARM service connections — System.AccessToken is never used.
    ///
    /// - `read`: MI for Stage 1 (agent) — read-only ADO access
    /// - `write`: MI for Stage 2 (executor) — write access for safe-outputs, never given to agent
    #[serde(default)]
    pub permissions: Option<PermissionsConfig>,
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

/// Permissions configuration for ADO access tokens.
///
/// ADO does not support fine-grained permissions. There are two access levels:
/// blanket read and blanket write, each backed by an ARM service connection
/// that mints an ADO-scoped token.
///
/// Examples:
/// ```yaml
/// # Both read and write
/// permissions:
///   read: my-read-arm-connection
///   write: my-write-arm-connection
///
/// # Read-only (agent can query ADO APIs, no write safe-outputs)
/// permissions:
///   read: my-read-arm-connection
///
/// # Write-only (safe-outputs can write, agent gets no ADO token)
/// permissions:
///   write: my-write-arm-connection
/// ```
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PermissionsConfig {
    /// ARM service connection for read-only ADO access.
    /// Token is minted and given to the agent in Stage 1 (inside AWF sandbox).
    #[serde(default)]
    pub read: Option<String>,
    /// ARM service connection for write ADO access.
    /// Token is minted and used only by the executor in Stage 2 (ProcessSafeOutputs).
    /// This token is never exposed to the agent.
    #[serde(default)]
    pub write: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─── PoolConfig deserialization ──────────────────────────────────────────

    #[test]
    fn test_pool_config_string_form() {
        let yaml = "pool: MyPool";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let pool: PoolConfig = serde_yaml::from_value(fm["pool"].clone()).unwrap();
        assert_eq!(pool.name(), "MyPool");
        assert_eq!(pool.os(), "linux"); // default
    }

    #[test]
    fn test_pool_config_object_form_with_os() {
        let yaml = "pool:\n  name: WinPool\n  os: windows";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let pool: PoolConfig = serde_yaml::from_value(fm["pool"].clone()).unwrap();
        assert_eq!(pool.name(), "WinPool");
        assert_eq!(pool.os(), "windows");
    }

    #[test]
    fn test_pool_config_object_form_default_os() {
        let yaml = "pool:\n  name: LinuxPool";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let pool: PoolConfig = serde_yaml::from_value(fm["pool"].clone()).unwrap();
        assert_eq!(pool.name(), "LinuxPool");
        assert_eq!(pool.os(), "linux");
    }

    #[test]
    fn test_pool_config_default() {
        let pool = PoolConfig::default();
        assert_eq!(pool.name(), "AZS-1ES-L-MMS-ubuntu-22.04");
        assert_eq!(pool.os(), "linux");
    }

    // ─── ScheduleConfig deserialization ─────────────────────────────────────

    #[test]
    fn test_schedule_config_simple_has_empty_branches() {
        let sc = ScheduleConfig::Simple("daily around 14:00".to_string());
        assert_eq!(sc.expression(), "daily around 14:00");
        assert!(sc.branches().is_empty());
    }

    #[test]
    fn test_schedule_config_with_options_returns_branches() {
        let yaml = "run: weekly on monday\nbranches:\n  - main\n  - release/*";
        let opts: ScheduleOptions = serde_yaml::from_str(yaml).unwrap();
        let sc = ScheduleConfig::WithOptions(opts);
        assert_eq!(sc.expression(), "weekly on monday");
        assert_eq!(sc.branches(), &["main", "release/*"]);
    }

    #[test]
    fn test_schedule_config_with_options_empty_branches() {
        let yaml = "run: hourly";
        let opts: ScheduleOptions = serde_yaml::from_str(yaml).unwrap();
        let sc = ScheduleConfig::WithOptions(opts);
        assert_eq!(sc.expression(), "hourly");
        assert!(sc.branches().is_empty());
    }

    #[test]
    fn test_schedule_config_deserialized_as_simple_string() {
        let yaml = "schedule: daily around 14:00";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let sc: ScheduleConfig = serde_yaml::from_value(fm["schedule"].clone()).unwrap();
        assert_eq!(sc.expression(), "daily around 14:00");
        assert!(sc.branches().is_empty());
    }

    #[test]
    fn test_schedule_config_deserialized_as_object() {
        let yaml = "schedule:\n  run: weekly on friday\n  branches:\n    - main\n    - develop";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let sc: ScheduleConfig = serde_yaml::from_value(fm["schedule"].clone()).unwrap();
        assert_eq!(sc.expression(), "weekly on friday");
        assert_eq!(sc.branches(), &["main", "develop"]);
    }

    // ─── EngineConfig deserialization ────────────────────────────────────────

    #[test]
    fn test_engine_config_simple_string() {
        let ec = EngineConfig::Simple("gpt-5.1".to_string());
        assert_eq!(ec.model(), "gpt-5.1");
        assert_eq!(ec.max_turns(), None);
        assert_eq!(ec.timeout_minutes(), None);
    }

    #[test]
    fn test_engine_config_full_object() {
        let yaml = "model: claude-sonnet-4.5\nmax-turns: 50\ntimeout-minutes: 30";
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let ec = EngineConfig::Full(opts);
        assert_eq!(ec.model(), "claude-sonnet-4.5");
        assert_eq!(ec.max_turns(), Some(50));
        assert_eq!(ec.timeout_minutes(), Some(30));
    }

    #[test]
    fn test_engine_config_full_object_partial_fields() {
        let yaml = "max-turns: 10";
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let ec = EngineConfig::Full(opts);
        // model defaults to claude-opus-4.5 when not specified
        assert_eq!(ec.model(), "claude-opus-4.5");
        assert_eq!(ec.max_turns(), Some(10));
        assert_eq!(ec.timeout_minutes(), None);
    }

    #[test]
    fn test_engine_config_default() {
        let ec = EngineConfig::default();
        assert_eq!(ec.model(), "claude-opus-4.5");
        assert_eq!(ec.max_turns(), None);
        assert_eq!(ec.timeout_minutes(), None);
    }

    #[test]
    fn test_engine_config_deserialized_as_string() {
        let yaml = "engine: my-custom-model";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let ec: EngineConfig = serde_yaml::from_value(fm["engine"].clone()).unwrap();
        assert_eq!(ec.model(), "my-custom-model");
        assert_eq!(ec.max_turns(), None);
        assert_eq!(ec.timeout_minutes(), None);
    }

    #[test]
    fn test_engine_config_deserialized_as_object() {
        let yaml =
            "engine:\n  model: claude-opus-4.5\n  max-turns: 50\n  timeout-minutes: 30";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let ec: EngineConfig = serde_yaml::from_value(fm["engine"].clone()).unwrap();
        assert_eq!(ec.model(), "claude-opus-4.5");
        assert_eq!(ec.max_turns(), Some(50));
        assert_eq!(ec.timeout_minutes(), Some(30));
    }

    // ─── PermissionsConfig deserialization ───────────────────────────────

    #[test]
    fn test_permissions_both_fields() {
        let yaml = "read: my-read-sc\nwrite: my-write-sc";
        let pc: PermissionsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(pc.read.as_deref(), Some("my-read-sc"));
        assert_eq!(pc.write.as_deref(), Some("my-write-sc"));
    }

    #[test]
    fn test_permissions_read_only() {
        let yaml = "read: my-read-sc";
        let pc: PermissionsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(pc.read.as_deref(), Some("my-read-sc"));
        assert!(pc.write.is_none());
    }

    #[test]
    fn test_permissions_write_only() {
        let yaml = "write: my-write-sc";
        let pc: PermissionsConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(pc.read.is_none());
        assert_eq!(pc.write.as_deref(), Some("my-write-sc"));
    }

    #[test]
    fn test_permissions_default() {
        let pc = PermissionsConfig::default();
        assert!(pc.read.is_none());
        assert!(pc.write.is_none());
    }

    #[test]
    fn test_permissions_in_front_matter() {
        let content = r#"---
name: "Test Agent"
description: "Test"
permissions:
  read: my-read-sc
  write: my-write-sc
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let perms = fm.permissions.unwrap();
        assert_eq!(perms.read.as_deref(), Some("my-read-sc"));
        assert_eq!(perms.write.as_deref(), Some("my-write-sc"));
    }

    #[test]
    fn test_permissions_omitted_in_front_matter() {
        let content = r#"---
name: "Test Agent"
description: "Test"
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(fm.permissions.is_none());
    }
}
