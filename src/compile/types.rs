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
    #[allow(dead_code)]
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

/// Engine configuration — aligned with gh-aw's engine front matter.
///
/// The string form is an engine identifier (e.g., `copilot`). The object form
/// uses `id` for the engine identifier plus additional options.
///
/// Currently only `copilot` (GitHub Copilot CLI) is supported. Other engine
/// identifiers produce a compile error.
///
/// Examples:
/// ```yaml
/// # Simple string format (engine identifier, defaults to copilot)
/// engine: copilot
///
/// # Object format (with additional options)
/// engine:
///   id: copilot
///   model: claude-opus-4.7
///   timeout-minutes: 30
///   version: latest
///   agent: my-custom-agent
///   api-target: api.acme.ghe.com
///   args: ["--verbose"]
///   env:
///     DEBUG_MODE: "true"
///   command: /usr/local/bin/copilot
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum EngineConfig {
    /// Engine identifier string (e.g., "copilot")
    Simple(String),
    /// Full engine configuration object
    Full(EngineOptions),
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig::Simple("copilot".to_string())
    }
}

impl EngineConfig {
    /// Get the engine identifier (e.g., "copilot").
    pub fn engine_id(&self) -> &str {
        match self {
            EngineConfig::Simple(s) => s,
            EngineConfig::Full(opts) => opts.id.as_deref().unwrap_or("copilot"),
        }
    }

    /// Get the model name override, if specified.
    /// Returns `None` when the engine should use its default model.
    pub fn model(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.model.as_deref(),
        }
    }

    /// Get the timeout in minutes
    pub fn timeout_minutes(&self) -> Option<u32> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.timeout_minutes,
        }
    }

    /// Get the engine version override (e.g., "0.0.422", "latest")
    pub fn version(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.version.as_deref(),
        }
    }

    /// Get the custom agent file identifier (Copilot only, e.g., "my-agent")
    pub fn agent(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.agent.as_deref(),
        }
    }

    /// Get the custom API endpoint hostname (GHEC/GHES)
    pub fn api_target(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.api_target.as_deref(),
        }
    }

    /// Get custom CLI arguments
    pub fn args(&self) -> &[String] {
        match self {
            EngineConfig::Simple(_) => &[],
            EngineConfig::Full(opts) => &opts.args,
        }
    }

    /// Get custom environment variables
    pub fn env(&self) -> Option<&HashMap<String, String>> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.env.as_ref(),
        }
    }

    /// Get custom engine command path
    pub fn command(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.command.as_deref(),
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
    /// Engine identifier (e.g., "copilot"). Defaults to "copilot" when omitted.
    #[serde(default)]
    pub id: Option<String>,
    /// AI model to use (engine-specific default when omitted)
    #[serde(default)]
    pub model: Option<String>,
    /// Engine CLI version to install (e.g., "0.0.422", "latest")
    #[serde(default)]
    pub version: Option<String>,
    /// Custom agent file identifier (Copilot only — references .github/agents/)
    #[serde(default)]
    pub agent: Option<String>,
    /// Custom API endpoint hostname (GHEC/GHES, e.g., "api.acme.ghe.com")
    #[serde(default, rename = "api-target")]
    pub api_target: Option<String>,
    /// Custom CLI arguments injected before the prompt
    #[serde(default)]
    pub args: Vec<String>,
    /// Engine-specific environment variables
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// Custom engine executable path (skips default installation)
    #[serde(default)]
    pub command: Option<String>,
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
    /// Workspace setting: "root" or "repo" (auto-computed if not set)
    #[serde(default)]
    pub workspace: Option<String>,
    /// Agent pool configuration
    #[serde(default)]
    pub pool: Option<PoolConfig>,
    /// AI engine configuration (defaults to copilot)
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
    /// Unified trigger configuration: schedule, pipeline, PR triggers and filters
    #[serde(default, rename = "on")]
    pub on_config: Option<OnConfig>,
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
    /// - `write`: MI for Stage 3 (executor) — write access for safe-outputs, never given to agent
    #[serde(default)]
    pub permissions: Option<PermissionsConfig>,
    /// Workflow-level environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Runtime parameters for the pipeline (surfaced in ADO UI when queuing a run)
    #[serde(default)]
    pub parameters: Vec<PipelineParameter>,
}

impl FrontMatter {
    /// Get the schedule configuration (if any).
    pub fn schedule(&self) -> Option<&ScheduleConfig> {
        self.on_config.as_ref().and_then(|o| o.schedule.as_ref())
    }

    /// Check if a schedule is configured.
    pub fn has_schedule(&self) -> bool {
        self.schedule().is_some()
    }

    /// Get the pipeline trigger configuration (if any).
    pub fn pipeline_trigger(&self) -> Option<&PipelineTrigger> {
        self.on_config.as_ref().and_then(|o| o.pipeline.as_ref())
    }

    /// Get the PR trigger configuration (if any).
    pub fn pr_trigger(&self) -> Option<&PrTriggerConfig> {
        self.on_config.as_ref().and_then(|o| o.pr.as_ref())
    }

    /// Get the PR runtime filters (if any).
    pub fn pr_filters(&self) -> Option<&PrFilters> {
        self.pr_trigger().and_then(|pr| pr.filters.as_ref())
    }

    /// Get the pipeline runtime filters (if any).
    pub fn pipeline_filters(&self) -> Option<&PipelineFilters> {
        self.pipeline_trigger()
            .and_then(|pt| pt.filters.as_ref())
    }
}

impl SanitizeConfigTrait for FrontMatter {
    fn sanitize_config_fields(&mut self) {
        self.name = crate::sanitize::sanitize_config(&self.name);
        self.description = crate::sanitize::sanitize_config(&self.description);
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
        // Stage 3 execution via get_tool_config() when deserialized into typed configs.
        if let Some(ref mut o) = self.on_config {
            o.sanitize_config_fields();
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
    /// Token is minted and used only by the executor in Stage 3 (Execution).
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

/// Unified trigger configuration — `on:` front matter key.
///
/// Consolidates all trigger types: schedule, pipeline completion, and PR triggers.
/// Aligns with gh-aw's `on:` key.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct OnConfig {
    /// Fuzzy schedule configuration
    #[serde(default)]
    pub schedule: Option<ScheduleConfig>,
    /// Pipeline completion trigger
    #[serde(default)]
    pub pipeline: Option<PipelineTrigger>,
    /// PR trigger configuration (native ADO branch/path filters + runtime filters)
    #[serde(default)]
    pub pr: Option<PrTriggerConfig>,
}

impl SanitizeConfigTrait for OnConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut s) = self.schedule {
            s.sanitize_config_fields();
        }
        if let Some(ref mut p) = self.pipeline {
            p.sanitize_config_fields();
        }
        if let Some(ref mut pr) = self.pr {
            pr.sanitize_config_fields();
        }
    }
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
    /// Pipeline-specific runtime filters
    #[serde(default)]
    pub filters: Option<PipelineFilters>,
}

impl SanitizeConfigTrait for PipelineTrigger {
    fn sanitize_config_fields(&mut self) {
        self.name = crate::sanitize::sanitize_config(&self.name);
        if let Some(ref mut p) = self.project {
            *p = crate::sanitize::sanitize_config(p);
        }
        self.branches = self.branches.iter().map(|s| crate::sanitize::sanitize_config(s)).collect();
        if let Some(ref mut f) = self.filters {
            f.sanitize_config_fields();
        }
    }
}

/// Pipeline completion trigger filters.
/// Only exposes filters applicable to pipeline triggers.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PipelineFilters {
    /// Only run during a specific time window (UTC)
    #[serde(default, rename = "time-window")]
    pub time_window: Option<TimeWindowFilter>,
    /// Regex match on upstream pipeline name (Build.TriggeredBy.DefinitionName)
    #[serde(default, rename = "source-pipeline")]
    pub source_pipeline: Option<PatternFilter>,
    /// Regex match on triggering branch (Build.SourceBranch)
    #[serde(default)]
    pub branch: Option<PatternFilter>,
    /// Include/exclude by build reason
    #[serde(default, rename = "build-reason")]
    pub build_reason: Option<IncludeExcludeFilter>,
    /// Raw ADO condition expression escape hatch
    #[serde(default)]
    pub expression: Option<String>,
}

impl SanitizeConfigTrait for PipelineFilters {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut tw) = self.time_window {
            tw.sanitize_config_fields();
        }
        if let Some(ref mut sp) = self.source_pipeline {
            sp.sanitize_config_fields();
        }
        if let Some(ref mut b) = self.branch {
            b.sanitize_config_fields();
        }
        if let Some(ref mut br) = self.build_reason {
            br.sanitize_config_fields();
        }
        if let Some(ref mut e) = self.expression {
            *e = crate::sanitize::sanitize_config(e);
        }
    }
}

// ─── PR Trigger Types ───────────────────────────────────────────────────────

/// PR trigger configuration with native ADO filters and runtime gate filters.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PrTriggerConfig {
    /// Native ADO branch filter for PR triggers
    #[serde(default)]
    pub branches: Option<BranchFilter>,
    /// Native ADO path filter for PR triggers
    #[serde(default)]
    pub paths: Option<PathFilter>,
    /// Runtime filters evaluated via gate steps in the Setup job
    #[serde(default)]
    pub filters: Option<PrFilters>,
}

impl SanitizeConfigTrait for PrTriggerConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut b) = self.branches {
            b.sanitize_config_fields();
        }
        if let Some(ref mut p) = self.paths {
            p.sanitize_config_fields();
        }
        if let Some(ref mut f) = self.filters {
            f.sanitize_config_fields();
        }
    }
}

/// Branch include/exclude filter for PR triggers.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct BranchFilter {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Path include/exclude filter for PR triggers.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct PathFilter {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Runtime PR filters evaluated via gate steps in the Setup job.
/// Multiple filters use AND semantics — all must pass for the agent to run.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PrFilters {
    /// Regex match on PR title (System.PullRequest.Title)
    #[serde(default)]
    pub title: Option<PatternFilter>,
    /// Include/exclude by author email (Build.RequestedForEmail)
    #[serde(default)]
    pub author: Option<IncludeExcludeFilter>,
    /// Regex match on source branch (System.PullRequest.SourceBranch)
    #[serde(default, rename = "source-branch")]
    pub source_branch: Option<PatternFilter>,
    /// Regex match on target branch (System.PullRequest.TargetBranch)
    #[serde(default, rename = "target-branch")]
    pub target_branch: Option<PatternFilter>,
    /// Regex match on last commit message (Build.SourceVersionMessage)
    #[serde(default, rename = "commit-message")]
    pub commit_message: Option<PatternFilter>,
    /// PR label matching (any-of, all-of, none-of)
    #[serde(default)]
    pub labels: Option<LabelFilter>,
    /// Filter by PR draft status
    #[serde(default)]
    pub draft: Option<bool>,
    /// Glob patterns for changed file paths
    #[serde(default, rename = "changed-files")]
    pub changed_files: Option<IncludeExcludeFilter>,
    /// Only run during a specific time window (UTC)
    #[serde(default, rename = "time-window")]
    pub time_window: Option<TimeWindowFilter>,
    /// Minimum number of changed files required
    #[serde(default, rename = "min-changes")]
    pub min_changes: Option<u32>,
    /// Maximum number of changed files allowed
    #[serde(default, rename = "max-changes")]
    pub max_changes: Option<u32>,
    /// Include/exclude by build reason (e.g., PullRequest, Manual, IndividualCI)
    #[serde(default, rename = "build-reason")]
    pub build_reason: Option<IncludeExcludeFilter>,
    /// Raw ADO condition expression appended to the Agent job condition (escape hatch)
    #[serde(default)]
    pub expression: Option<String>,
}

impl SanitizeConfigTrait for PrFilters {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut t) = self.title {
            t.sanitize_config_fields();
        }
        if let Some(ref mut a) = self.author {
            a.sanitize_config_fields();
        }
        if let Some(ref mut s) = self.source_branch {
            s.sanitize_config_fields();
        }
        if let Some(ref mut t) = self.target_branch {
            t.sanitize_config_fields();
        }
        if let Some(ref mut cm) = self.commit_message {
            cm.sanitize_config_fields();
        }
        if let Some(ref mut l) = self.labels {
            l.sanitize_config_fields();
        }
        if let Some(ref mut c) = self.changed_files {
            c.sanitize_config_fields();
        }
        if let Some(ref mut tw) = self.time_window {
            tw.sanitize_config_fields();
        }
        if let Some(ref mut br) = self.build_reason {
            br.sanitize_config_fields();
        }
        if let Some(ref mut e) = self.expression {
            *e = crate::sanitize::sanitize_config(e);
        }
    }
}

/// Time window filter — only run during a specific UTC time range.
///
/// Example: `{ start: "09:00", end: "17:00" }` means business hours UTC.
/// Handles overnight windows (e.g., `{ start: "22:00", end: "06:00" }`).
#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
pub struct TimeWindowFilter {
    /// Start time in HH:MM format (UTC)
    pub start: String,
    /// End time in HH:MM format (UTC)
    pub end: String,
}

/// A regex pattern filter.
#[derive(Debug, Deserialize, Clone)]
pub struct PatternFilter {
    /// Regex pattern to match against
    #[serde(rename = "match")]
    pub pattern: String,
}

impl SanitizeConfigTrait for PatternFilter {
    fn sanitize_config_fields(&mut self) {
        self.pattern = crate::sanitize::sanitize_config(&self.pattern);
    }
}

/// Include/exclude list filter.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct IncludeExcludeFilter {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Label matching filter for PR labels.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct LabelFilter {
    /// PR must have at least one of these labels
    #[serde(default, rename = "any-of")]
    pub any_of: Vec<String>,
    /// PR must have all of these labels
    #[serde(default, rename = "all-of")]
    pub all_of: Vec<String>,
    /// PR must not have any of these labels
    #[serde(default, rename = "none-of")]
    pub none_of: Vec<String>,
}

impl SanitizeConfigTrait for LabelFilter {
    fn sanitize_config_fields(&mut self) {
        self.any_of = self.any_of.iter().map(|s| crate::sanitize::sanitize_config(s)).collect();
        self.all_of = self.all_of.iter().map(|s| crate::sanitize::sanitize_config(s)).collect();
        self.none_of = self.none_of.iter().map(|s| crate::sanitize::sanitize_config(s)).collect();
    }
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
        let ec = EngineConfig::Simple("copilot".to_string());
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), None);
        assert_eq!(ec.timeout_minutes(), None);
    }

    #[test]
    fn test_engine_config_full_object() {
        let yaml = "id: copilot\nmodel: claude-sonnet-4.5\ntimeout-minutes: 30";
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let ec = EngineConfig::Full(opts);
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), Some("claude-sonnet-4.5"));
        assert_eq!(ec.timeout_minutes(), Some(30));
    }

    #[test]
    fn test_engine_config_full_object_partial_fields() {
        let yaml = "timeout-minutes: 10";
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let ec = EngineConfig::Full(opts);
        // id defaults to "copilot" when not specified
        assert_eq!(ec.engine_id(), "copilot");
        // model is None when not specified (engine impl decides default)
        assert_eq!(ec.model(), None);
        assert_eq!(ec.timeout_minutes(), Some(10));
    }

    #[test]
    fn test_engine_config_default() {
        let ec = EngineConfig::default();
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), None);
        assert_eq!(ec.timeout_minutes(), None);
    }

    #[test]
    fn test_engine_config_deserialized_as_string() {
        let yaml = "engine: copilot";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let ec: EngineConfig = serde_yaml::from_value(fm["engine"].clone()).unwrap();
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), None);
        assert_eq!(ec.timeout_minutes(), None);
    }

    #[test]
    fn test_engine_config_deserialized_as_object() {
        let yaml =
            "engine:\n  id: copilot\n  model: claude-opus-4.5\n  timeout-minutes: 30";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let ec: EngineConfig = serde_yaml::from_value(fm["engine"].clone()).unwrap();
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), Some("claude-opus-4.5"));
        assert_eq!(ec.timeout_minutes(), Some(30));
    }

    #[test]
    fn test_engine_config_full_with_all_gh_aw_fields() {
        let yaml = r#"
id: copilot
model: gpt-5
version: "0.0.422"
agent: my-custom-agent
api-target: api.acme.ghe.com
args: ["--verbose", "--add-dir", "/workspace"]
env:
  DEBUG_MODE: "true"
  AWS_REGION: us-west-2
command: /usr/local/bin/copilot
timeout-minutes: 60
"#;
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let ec = EngineConfig::Full(opts);
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), Some("gpt-5"));
        assert_eq!(ec.version(), Some("0.0.422"));
        assert_eq!(ec.agent(), Some("my-custom-agent"));
        assert_eq!(ec.api_target(), Some("api.acme.ghe.com"));
        assert_eq!(ec.args(), &["--verbose", "--add-dir", "/workspace"]);
        assert_eq!(ec.command(), Some("/usr/local/bin/copilot"));
        assert_eq!(ec.timeout_minutes(), Some(60));
        let env = ec.env().unwrap();
        assert_eq!(env.get("DEBUG_MODE").unwrap(), "true");
        assert_eq!(env.get("AWS_REGION").unwrap(), "us-west-2");
    }

    #[test]
    fn test_engine_config_id_defaults_to_copilot() {
        let yaml = "model: gpt-5\ntimeout-minutes: 30";
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let ec = EngineConfig::Full(opts);
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), Some("gpt-5"));
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

    // ─── PrTriggerConfig deserialization ─────────────────────────────────────

    #[test]
    fn test_pr_trigger_config_title_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      title:
        match: "\\[agent\\]"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let pr = tc.pr.unwrap();
        let filters = pr.filters.unwrap();
        assert_eq!(filters.title.unwrap().pattern, "\\[agent\\]");
    }

    #[test]
    fn test_pr_trigger_config_author_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      author:
        include: ["alice@corp.com", "bob@corp.com"]
        exclude: ["bot@noreply.com"]
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let pr = tc.pr.unwrap();
        let author = pr.filters.unwrap().author.unwrap();
        assert_eq!(author.include, vec!["alice@corp.com", "bob@corp.com"]);
        assert_eq!(author.exclude, vec!["bot@noreply.com"]);
    }

    #[test]
    fn test_pr_trigger_config_branch_filters() {
        let yaml = r#"
triggers:
  pr:
    branches:
      include: [main, "release/*"]
      exclude: ["test/*"]
    filters:
      source-branch:
        match: "^feature/.*"
      target-branch:
        match: "^main$"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let pr = tc.pr.unwrap();
        let branches = pr.branches.unwrap();
        assert_eq!(branches.include, vec!["main", "release/*"]);
        assert_eq!(branches.exclude, vec!["test/*"]);
        let filters = pr.filters.unwrap();
        assert_eq!(filters.source_branch.unwrap().pattern, "^feature/.*");
        assert_eq!(filters.target_branch.unwrap().pattern, "^main$");
    }

    #[test]
    fn test_pr_trigger_config_label_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      labels:
        any-of: ["run-agent", "automated"]
        none-of: ["do-not-run"]
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let labels = tc.pr.unwrap().filters.unwrap().labels.unwrap();
        assert_eq!(labels.any_of, vec!["run-agent", "automated"]);
        assert!(labels.all_of.is_empty());
        assert_eq!(labels.none_of, vec!["do-not-run"]);
    }

    #[test]
    fn test_pr_trigger_config_draft_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      draft: false
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        assert_eq!(tc.pr.unwrap().filters.unwrap().draft, Some(false));
    }

    #[test]
    fn test_pr_trigger_config_changed_files_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      changed-files:
        include: ["src/**/*.rs"]
        exclude: ["docs/**"]
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let changed = tc.pr.unwrap().filters.unwrap().changed_files.unwrap();
        assert_eq!(changed.include, vec!["src/**/*.rs"]);
        assert_eq!(changed.exclude, vec!["docs/**"]);
    }

    #[test]
    fn test_pr_trigger_config_paths_only() {
        let yaml = r#"
triggers:
  pr:
    paths:
      include: ["src/*"]
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let pr = tc.pr.unwrap();
        assert!(pr.filters.is_none());
        assert_eq!(pr.paths.unwrap().include, vec!["src/*"]);
    }

    #[test]
    fn test_pr_trigger_config_combined_with_pipeline_trigger() {
        let yaml = r#"
triggers:
  pipeline:
    name: "Build Pipeline"
  pr:
    filters:
      title:
        match: "\\[review\\]"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        assert!(tc.pipeline.is_some());
        assert!(tc.pr.is_some());
        assert_eq!(tc.pr.unwrap().filters.unwrap().title.unwrap().pattern, "\\[review\\]");
    }

    #[test]
    fn test_pr_trigger_config_empty_filters() {
        let yaml = r#"
triggers:
  pr:
    filters: {}
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let filters = tc.pr.unwrap().filters.unwrap();
        assert!(filters.title.is_none());
        assert!(filters.author.is_none());
        assert!(filters.draft.is_none());
    }

    #[test]
    fn test_pr_trigger_in_full_front_matter() {
        let content = r#"---
name: "Test Agent"
description: "Test"
on:
  pr:
    branches:
      include: [main]
    filters:
      title:
        match: "\\[agent\\]"
      draft: false
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let pr = fm.on_config.unwrap().pr.unwrap();
        assert_eq!(pr.branches.unwrap().include, vec!["main"]);
        let filters = pr.filters.unwrap();
        assert_eq!(filters.title.unwrap().pattern, "\\[agent\\]");
        assert_eq!(filters.draft, Some(false));
    }
}
