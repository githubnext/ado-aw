//! Common types for the agentic pipeline compiler.
//!
//! This module defines the front matter grammar that is shared across all compile targets.

use ado_aw_derive::SanitizeConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::sanitize::SanitizeConfig as SanitizeConfigTrait;

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

impl SanitizeConfigTrait for PoolConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            PoolConfig::Name(name) => *name = crate::sanitize::sanitize_config(name),
            PoolConfig::Full(full) => full.sanitize_config_fields(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
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

impl SanitizeConfigTrait for ScheduleConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            ScheduleConfig::Simple(s) => *s = crate::sanitize::sanitize_config(s),
            ScheduleConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
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

impl SanitizeConfigTrait for EngineConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            EngineConfig::Simple(s) => *s = crate::sanitize::sanitize_config(s),
            EngineConfig::Full(opts) => opts.sanitize_config_fields(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
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
///
/// Examples:
/// ```yaml
/// tools:
///   bash: ["cat", "ls", "grep"]
///   edit: true
///   cache-memory:
///     allowed-extensions: [.md, .json]
///   azure-devops:
///     toolsets: [repos, wit]
///     allowed: [wit_get_work_item]
/// ```
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ToolsConfig {
    /// Bash command allow-list. If empty/not set, defaults to safe commands.
    /// Use [":*"] for unrestricted access.
    #[serde(default)]
    pub bash: Option<Vec<String>>,
    /// Enable the file editing tool (default: true)
    #[serde(default)]
    pub edit: Option<bool>,
    /// Persistent cache memory across agent runs.
    /// Enables the agent to read/write files to a memory directory
    /// that persists between pipeline executions.
    #[serde(default, rename = "cache-memory")]
    pub cache_memory: Option<CacheMemoryToolConfig>,
    /// First-class Azure DevOps MCP integration.
    /// Auto-configures the ADO MCP container, token mapping, MCPG entry,
    /// and network allowlist domains.
    #[serde(default, rename = "azure-devops")]
    pub azure_devops: Option<AzureDevOpsToolConfig>,
}

impl SanitizeConfigTrait for ToolsConfig {
    fn sanitize_config_fields(&mut self) {
        self.bash = self.bash.as_ref().map(|v| {
            v.iter().map(|s| crate::sanitize::sanitize_config(s)).collect()
        });
        if let Some(ref mut cm) = self.cache_memory {
            cm.sanitize_config_fields();
        }
        if let Some(ref mut ado) = self.azure_devops {
            ado.sanitize_config_fields();
        }
    }
}

/// Cache memory tool configuration — accepts both `true` and object formats
///
/// Examples:
/// ```yaml
/// # Simple enablement
/// cache-memory: true
///
/// # With options
/// cache-memory:
///   allowed-extensions: [.md, .json, .txt]
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum CacheMemoryToolConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(CacheMemoryOptions),
}

impl CacheMemoryToolConfig {
    /// Whether cache memory is enabled
    pub fn is_enabled(&self) -> bool {
        match self {
            CacheMemoryToolConfig::Enabled(enabled) => *enabled,
            CacheMemoryToolConfig::WithOptions(_) => true,
        }
    }

    /// Get the allowed file extensions (empty = all allowed)
    pub fn allowed_extensions(&self) -> &[String] {
        match self {
            CacheMemoryToolConfig::Enabled(_) => &[],
            CacheMemoryToolConfig::WithOptions(opts) => &opts.allowed_extensions,
        }
    }
}

impl SanitizeConfigTrait for CacheMemoryToolConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            CacheMemoryToolConfig::Enabled(_) => {}
            CacheMemoryToolConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// Cache memory options
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct CacheMemoryOptions {
    /// Allowed file extensions (e.g., [".md", ".json", ".txt"]).
    /// Defaults to all extensions if empty or not specified.
    #[serde(default, rename = "allowed-extensions")]
    pub allowed_extensions: Vec<String>,
}

/// Azure DevOps MCP tool configuration — accepts both `true` and object formats
///
/// Examples:
/// ```yaml
/// # Simple enablement (auto-infers org from git remote)
/// azure-devops: true
///
/// # With scoping options
/// azure-devops:
///   toolsets: [repos, wit, core]
///   allowed: [wit_get_work_item, wit_my_work_items]
///   org: myorg
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum AzureDevOpsToolConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(AzureDevOpsOptions),
}

impl AzureDevOpsToolConfig {
    /// Whether the ADO MCP is enabled
    pub fn is_enabled(&self) -> bool {
        match self {
            AzureDevOpsToolConfig::Enabled(enabled) => *enabled,
            AzureDevOpsToolConfig::WithOptions(_) => true,
        }
    }

    /// Get the ADO API toolset groups to enable (e.g., repos, wit, core)
    pub fn toolsets(&self) -> &[String] {
        match self {
            AzureDevOpsToolConfig::Enabled(_) => &[],
            AzureDevOpsToolConfig::WithOptions(opts) => &opts.toolsets,
        }
    }

    /// Get the explicit tool allow-list
    pub fn allowed(&self) -> &[String] {
        match self {
            AzureDevOpsToolConfig::Enabled(_) => &[],
            AzureDevOpsToolConfig::WithOptions(opts) => &opts.allowed,
        }
    }

    /// Get the org override (None = auto-infer from git remote)
    pub fn org(&self) -> Option<&str> {
        match self {
            AzureDevOpsToolConfig::Enabled(_) => None,
            AzureDevOpsToolConfig::WithOptions(opts) => opts.org.as_deref(),
        }
    }
}

impl SanitizeConfigTrait for AzureDevOpsToolConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            AzureDevOpsToolConfig::Enabled(_) => {}
            AzureDevOpsToolConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// Azure DevOps MCP options
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct AzureDevOpsOptions {
    /// ADO API toolset groups to enable (e.g., repos, wit, core, work-items)
    /// Passed as `-d` flags to the ADO MCP entrypoint.
    #[serde(default)]
    pub toolsets: Vec<String>,
    /// Explicit tool allow-list (e.g., wit_get_work_item, core_list_projects)
    /// Passed to MCPG for tool-level filtering.
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Azure DevOps organization name override.
    /// Auto-inferred from the git remote URL at compile time if not specified.
    #[serde(default)]
    pub org: Option<String>,
}

/// Runtime configuration for language environments.
///
/// Runtimes are language toolchains installed before the agent runs.
/// Unlike tools (which are agent capabilities like edit, bash, memory),
/// runtimes are execution environments (Lean, Python, Node, etc.).
///
/// Aligned with gh-aw's `runtimes:` front matter field.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct RuntimesConfig {
    /// Lean 4 theorem prover runtime.
    /// Auto-installs elan/lean/lake, adds Lean domains to the network allowlist,
    /// extends the bash command allow-list, and appends a prompt supplement.
    #[serde(default)]
    pub lean: Option<crate::runtimes::lean::LeanRuntimeConfig>,
}

impl SanitizeConfigTrait for RuntimesConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut lean) = self.lean {
            lean.sanitize_config_fields();
        }
    }
}

/// Azure DevOps runtime parameter definition.
///
/// These are emitted as top-level `parameters:` in the generated pipeline YAML,
/// surfaced in the ADO UI when manually queuing a run.
///
/// Example front matter:
/// ```yaml
/// parameters:
///   - name: debugLevel
///     displayName: "Debug verbosity"
///     type: string
///     default: "info"
///     values:
///       - info
///       - debug
///       - trace
/// ```
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, SanitizeConfig)]
pub struct PipelineParameter {
    /// Parameter name (must be a valid ADO identifier)
    pub name: String,
    /// Human-readable label shown in the ADO UI
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// ADO parameter type: boolean, string, number, object, etc.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub param_type: Option<String>,
    /// Default value for the parameter
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_yaml::Value>,
    /// Allowed values (for string/number parameters)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<serde_yaml::Value>>,
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
    /// Runtime configuration for language environments (e.g., Lean 4)
    #[serde(default)]
    pub runtimes: Option<RuntimesConfig>,
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
    /// Runtime parameters for the pipeline (surfaced in ADO UI when queuing a run)
    #[serde(default)]
    pub parameters: Vec<PipelineParameter>,
}

impl SanitizeConfigTrait for FrontMatter {
    fn sanitize_config_fields(&mut self) {
        self.name = crate::sanitize::sanitize_config(&self.name);
        self.description = crate::sanitize::sanitize_config(&self.description);
        if let Some(ref mut s) = self.schedule {
            s.sanitize_config_fields();
        }
        self.workspace = self.workspace.as_deref().map(crate::sanitize::sanitize_config);
        if let Some(ref mut p) = self.pool {
            p.sanitize_config_fields();
        }
        self.engine.sanitize_config_fields();
        if let Some(ref mut t) = self.tools {
            t.sanitize_config_fields();
        }
        if let Some(ref mut r) = self.runtimes {
            r.sanitize_config_fields();
        }
        for repo in &mut self.repositories {
            repo.sanitize_config_fields();
        }
        self.checkout = self.checkout.iter().map(|s| crate::sanitize::sanitize_config(s)).collect();
        for mcp in self.mcp_servers.values_mut() {
            mcp.sanitize_config_fields();
        }
        // safe_outputs: HashMap<String, serde_json::Value> — opaque JSON, sanitized at
        // Stage 2 execution via get_tool_config() when deserialized into typed configs.
        if let Some(ref mut t) = self.triggers {
            t.sanitize_config_fields();
        }
        if let Some(ref mut n) = self.network {
            n.sanitize_config_fields();
        }
        // steps, post_steps, setup, teardown: Vec<serde_yaml::Value> — opaque YAML
        // passed through to the pipeline, validated by ADO at parse time.
        if let Some(ref mut p) = self.permissions {
            p.sanitize_config_fields();
        }
        for v in self.env.values_mut() {
            *v = crate::sanitize::sanitize_config(v);
        }
        for p in &mut self.parameters {
            p.sanitize_config_fields();
        }
    }
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
/// - User-specified additional hosts from `allowed` field
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// Additional allowed host patterns (supports wildcards like *.example.com)
    /// Core Azure DevOps and GitHub hosts are always allowed.
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Blocked host patterns (takes precedence over allowed)
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
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
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
#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
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

impl SanitizeConfigTrait for McpConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            McpConfig::Enabled(_) => {}
            McpConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// Detailed MCP options
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct McpOptions {
    /// Whether this MCP is enabled (default: true)
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Docker container image for containerized stdio MCPs (MCPG-native)
    #[serde(default)]
    pub container: Option<String>,
    /// Container entrypoint override (equivalent to `docker run --entrypoint`)
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// Arguments passed to the container entrypoint
    #[serde(default, rename = "entrypoint-args")]
    pub entrypoint_args: Vec<String>,
    /// Additional Docker runtime arguments (inserted before the image in `docker run`)
    #[serde(default)]
    pub args: Vec<String>,
    /// HTTP endpoint URL for remote MCPs
    #[serde(default)]
    pub url: Option<String>,
    /// HTTP headers for remote MCPs (e.g., Authorization, X-MCP-Toolsets)
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Volume mounts for containerized MCPs (format: "source:dest:mode")
    #[serde(default)]
    pub mounts: Vec<String>,
    /// Allowed tool names (for MCPG tool filtering)
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Environment variables for the MCP server process
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Trigger configuration for the pipeline
#[derive(Debug, Deserialize, Clone, Default)]
pub struct TriggerConfig {
    /// Pipeline completion trigger
    #[serde(default)]
    pub pipeline: Option<PipelineTrigger>,
}

impl SanitizeConfigTrait for TriggerConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut p) = self.pipeline {
            p.sanitize_config_fields();
        }
    }
}

/// Pipeline completion trigger configuration
#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
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

    // ─── CacheMemoryToolConfig deserialization ──────────────────────────────

    #[test]
    fn test_cache_memory_bool_true() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  cache-memory: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let cm = fm.tools.as_ref().unwrap().cache_memory.as_ref().unwrap();
        assert!(cm.is_enabled());
        assert!(cm.allowed_extensions().is_empty());
    }

    #[test]
    fn test_cache_memory_bool_false() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  cache-memory: false
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let cm = fm.tools.as_ref().unwrap().cache_memory.as_ref().unwrap();
        assert!(!cm.is_enabled());
    }

    #[test]
    fn test_cache_memory_with_options() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  cache-memory:
    allowed-extensions:
      - .md
      - .json
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let cm = fm.tools.as_ref().unwrap().cache_memory.as_ref().unwrap();
        assert!(cm.is_enabled());
        assert_eq!(cm.allowed_extensions(), &[".md", ".json"]);
    }

    #[test]
    fn test_cache_memory_not_set() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  edit: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(fm.tools.as_ref().unwrap().cache_memory.is_none());
    }

    // ─── AzureDevOpsToolConfig deserialization ──────────────────────────────

    #[test]
    fn test_azure_devops_bool_true() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  azure-devops: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let ado = fm.tools.as_ref().unwrap().azure_devops.as_ref().unwrap();
        assert!(ado.is_enabled());
        assert!(ado.toolsets().is_empty());
        assert!(ado.allowed().is_empty());
        assert!(ado.org().is_none());
    }

    #[test]
    fn test_azure_devops_with_options() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  azure-devops:
    toolsets: [repos, wit, core]
    allowed: [wit_get_work_item, core_list_projects]
    org: myorg
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let ado = fm.tools.as_ref().unwrap().azure_devops.as_ref().unwrap();
        assert!(ado.is_enabled());
        assert_eq!(ado.toolsets(), &["repos", "wit", "core"]);
        assert_eq!(ado.allowed(), &["wit_get_work_item", "core_list_projects"]);
        assert_eq!(ado.org(), Some("myorg"));
    }

    #[test]
    fn test_azure_devops_partial_config() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  azure-devops:
    toolsets: [wit]
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let ado = fm.tools.as_ref().unwrap().azure_devops.as_ref().unwrap();
        assert!(ado.is_enabled());
        assert_eq!(ado.toolsets(), &["wit"]);
        assert!(ado.allowed().is_empty());
        assert!(ado.org().is_none());
    }

    #[test]
    fn test_azure_devops_not_set() {
        let content = r#"---
name: "Test"
description: "Test"
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(fm.tools.is_none());
    }

    #[test]
    fn test_both_tools_together() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  bash: ["cat", "ls"]
  edit: true
  cache-memory: true
  azure-devops:
    toolsets: [wit]
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let tools = fm.tools.as_ref().unwrap();
        assert!(tools.cache_memory.as_ref().unwrap().is_enabled());
        assert!(tools.azure_devops.as_ref().unwrap().is_enabled());
        assert_eq!(tools.bash.as_ref().unwrap(), &["cat", "ls"]);
        assert_eq!(tools.edit, Some(true));
    }

    // ─── LeanRuntimeConfig deserialization ──────────────────────────────

    #[test]
    fn test_lean_bool_true() {
        let content = r#"---
name: "Test"
description: "Test"
runtimes:
  lean: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let lean = fm.runtimes.as_ref().unwrap().lean.as_ref().unwrap();
        assert!(lean.is_enabled());
        assert!(lean.toolchain().is_none());
    }

    #[test]
    fn test_lean_bool_false() {
        let content = r#"---
name: "Test"
description: "Test"
runtimes:
  lean: false
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let lean = fm.runtimes.as_ref().unwrap().lean.as_ref().unwrap();
        assert!(!lean.is_enabled());
    }

    #[test]
    fn test_lean_with_toolchain() {
        let content = r#"---
name: "Test"
description: "Test"
runtimes:
  lean:
    toolchain: "leanprover/lean4:v4.29.1"
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let lean = fm.runtimes.as_ref().unwrap().lean.as_ref().unwrap();
        assert!(lean.is_enabled());
        assert_eq!(lean.toolchain(), Some("leanprover/lean4:v4.29.1"));
    }

    #[test]
    fn test_lean_with_empty_options() {
        let content = r#"---
name: "Test"
description: "Test"
runtimes:
  lean: {}
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let lean = fm.runtimes.as_ref().unwrap().lean.as_ref().unwrap();
        assert!(lean.is_enabled());
        assert!(lean.toolchain().is_none());
    }

    #[test]
    fn test_lean_not_set() {
        let content = r#"---
name: "Test"
description: "Test"
runtimes: {}
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(fm.runtimes.as_ref().unwrap().lean.is_none());
    }

    #[test]
    fn test_all_tools_and_runtimes_together() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  bash: ["cat", "ls"]
  edit: true
  cache-memory: true
  azure-devops:
    toolsets: [wit]
runtimes:
  lean: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let tools = fm.tools.as_ref().unwrap();
        assert!(tools.cache_memory.as_ref().unwrap().is_enabled());
        assert!(tools.azure_devops.as_ref().unwrap().is_enabled());
        assert_eq!(tools.bash.as_ref().unwrap(), &["cat", "ls"]);
        assert_eq!(tools.edit, Some(true));
        let runtimes = fm.runtimes.as_ref().unwrap();
        assert!(runtimes.lean.as_ref().unwrap().is_enabled());
    }

    // ─── NetworkConfig deny_unknown_fields ──────────────────────────────────

    #[test]
    fn test_network_config_rejects_old_allow_field() {
        let content = r#"---
name: "Test"
description: "Test"
network:
  allow:
    - "*.mycompany.com"
---

Body
"#;
        let result = super::super::common::parse_markdown(content);
        assert!(result.is_err(), "network.allow (old field name) should be rejected");
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("unknown field `allow`"),
            "error should mention unknown field `allow`, got: {}",
            err
        );
    }

    #[test]
    fn test_network_config_accepts_allowed_field() {
        let content = r#"---
name: "Test"
description: "Test"
network:
  allowed:
    - "*.mycompany.com"
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let net = fm.network.unwrap();
        assert_eq!(net.allowed, vec!["*.mycompany.com"]);
        assert!(net.blocked.is_empty());
    }

    #[test]
    fn test_network_config_rejects_arbitrary_unknown_field() {
        let content = r#"---
name: "Test"
description: "Test"
network:
  typo-field: true
---

Body
"#;
        let result = super::super::common::parse_markdown(content);
        assert!(result.is_err(), "unknown fields in network should be rejected");
    }
}
