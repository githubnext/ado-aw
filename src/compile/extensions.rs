//! Compiler extension trait and MCPG types.
//!
//! The [`CompilerExtension`] trait provides a unified interface for runtimes
//! and first-party tools to declare their compilation requirements (network
//! hosts, bash commands, prompt supplements, prepare steps, MCPG entries).
//!
//! Instead of scattering special-case `if` blocks across the compiler,
//! each runtime/tool implements this trait and the compiler collects
//! requirements via [`collect_extensions`].
//!
//! ## Adding a new runtime or tool
//!
//! 1. Create a struct wrapping your config type
//! 2. Implement [`CompilerExtension`] for it
//! 3. Add a variant to the [`Extension`] enum and update [`collect_extensions`]

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

use super::types::FrontMatter;

// ──────────────────────────────────────────────────────────────────────
// MCPG types (used by both the trait and standalone compiler)
// ──────────────────────────────────────────────────────────────────────

/// MCPG server configuration for a single MCP upstream.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpgServerConfig {
    /// Server type: "stdio" for container-based, "http" for HTTP backends
    #[serde(rename = "type")]
    pub server_type: String,
    /// Docker container image (for stdio type, per MCPG spec §4.1.2)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Container entrypoint override (for stdio type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Arguments passed to the container entrypoint (for stdio type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_args: Option<Vec<String>>,
    /// Volume mounts for containerized servers (format: "source:dest:mode")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mounts: Option<Vec<String>>,
    /// Additional Docker runtime arguments (inserted before image in `docker run`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// URL for HTTP backends
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP headers (e.g., Authorization)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// Environment variables for the server process
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    /// Tool allow-list (if empty or absent, all tools are allowed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
}

/// MCPG gateway configuration.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpgGatewayConfig {
    pub port: u16,
    pub domain: String,
    pub api_key: String,
    pub payload_dir: String,
}

/// Top-level MCPG configuration.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpgConfig {
    pub mcp_servers: HashMap<String, McpgServerConfig>,
    pub gateway: McpgGatewayConfig,
}

// ──────────────────────────────────────────────────────────────────────
// Compile context
// ──────────────────────────────────────────────────────────────────────

use crate::configure::AdoContext;
use std::path::Path;

/// Metadata resolved at compile time from the local environment.
///
/// Built once via [`CompileContext::new`] and passed to all extension
/// methods. Follows the same pattern as
/// [`ExecutionContext`](crate::safeoutputs::result::ExecutionContext)
/// for Stage 3 — a single context struct with all resolved metadata.
pub struct CompileContext<'a> {
    /// The agent name from front matter.
    pub agent_name: &'a str,
    /// The full front matter (for cross-cutting checks like bash access level).
    pub front_matter: &'a FrontMatter,
    /// ADO context inferred from the git remote (org URL, project, repo name).
    /// `None` if the compile directory has no ADO remote.
    pub ado_context: Option<AdoContext>,
}

impl<'a> CompileContext<'a> {
    /// Build a fully-resolved compile context.
    ///
    /// Infers ADO context from the git remote in `compile_dir`. This is
    /// async because it shells out to `git remote get-url origin`.
    pub async fn new(front_matter: &'a FrontMatter, compile_dir: &Path) -> Self {
        let ado_context = Self::infer_ado_context(compile_dir).await;
        Self {
            agent_name: &front_matter.name,
            front_matter,
            ado_context,
        }
    }

    /// Convenience accessor: extract the ADO org name from the inferred context.
    pub fn ado_org(&self) -> Option<&str> {
        self.ado_context.as_ref().and_then(|ctx| {
            let org = ctx.org_url.trim_end_matches('/').rsplit('/').next()?;
            if org.is_empty() { None } else { Some(org) }
        })
    }

    async fn infer_ado_context(dir: &Path) -> Option<AdoContext> {
        match crate::configure::get_git_remote_url(dir).await {
            Ok(url) => match crate::configure::parse_ado_remote(&url) {
                Ok(ctx) => {
                    log::info!(
                        "Inferred ADO org from git remote: {}",
                        ctx.org_url
                            .trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .unwrap_or("?")
                    );
                    Some(ctx)
                }
                Err(_) => {
                    log::debug!("Git remote is not an ADO URL — cannot infer org");
                    None
                }
            },
            Err(_) => {
                log::debug!("No git remote found — cannot infer ADO context");
                None
            }
        }
    }

    /// Create a context for tests (no async, no git remote inference).
    #[cfg(test)]
    pub fn for_test(front_matter: &'a FrontMatter) -> Self {
        Self {
            agent_name: &front_matter.name,
            front_matter,
            ado_context: None,
        }
    }

    /// Create a context for tests with a specific ADO org.
    #[cfg(test)]
    pub fn for_test_with_org(front_matter: &'a FrontMatter, org: &str) -> Self {
        Self {
            agent_name: &front_matter.name,
            front_matter,
            ado_context: Some(AdoContext {
                org_url: format!("https://dev.azure.com/{}", org),
                project: "test-project".to_string(),
                repo_name: "test-repo".to_string(),
            }),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// CompilerExtension trait
// ──────────────────────────────────────────────────────────────────────

/// Execution phase for extension ordering.
///
/// Extensions are collected and processed in phase order. Runtimes run
/// before tools because tools may depend on runtimes (e.g., `uv` requires
/// a Python runtime to already be installed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExtensionPhase {
    /// Language runtimes (Lean, Python, Node, etc.) — installed first.
    Runtime = 0,
    /// First-party tools (azure-devops, cache-memory, etc.) — may depend
    /// on runtimes being available.
    Tool = 1,
}

/// Unified interface for runtimes and first-party tools to declare
/// compilation requirements.
///
/// The compiler calls [`collect_extensions`] to gather all enabled
/// extensions, then iterates over them **in phase order** to merge
/// requirements into the generated pipeline.
///
/// ## Ordering policy
///
/// Extensions declare their [`phase`](CompilerExtension::phase) which
/// controls the order in which `prepare_steps` and `prompt_supplement`
/// are emitted. Runtimes ([`ExtensionPhase::Runtime`]) always run
/// before tools ([`ExtensionPhase::Tool`]) because tools may depend on
/// runtimes being installed (e.g., a Python-based tool needs the Python
/// runtime first).
pub trait CompilerExtension {
    /// Human-readable name for logging and diagnostics (e.g., "Lean 4").
    fn name(&self) -> &str;

    /// The execution phase of this extension, controlling ordering.
    fn phase(&self) -> ExtensionPhase;

    /// Network hosts this extension requires (added to AWF allowlist).
    fn required_hosts(&self) -> Vec<String> {
        vec![]
    }

    /// Bash commands this extension needs in the agent's allow-list.
    fn required_bash_commands(&self) -> Vec<String> {
        vec![]
    }

    /// Markdown prompt content to append to the agent prompt.
    ///
    /// The compiler wraps the returned content in a `cat >>` pipeline
    /// step so it is appended to the agent prompt file.
    fn prompt_supplement(&self) -> Option<String> {
        None
    }

    /// Pipeline steps (YAML strings) to run before the agent.
    ///
    /// Each element is a complete YAML step (e.g., `- bash: |...`).
    fn prepare_steps(&self) -> Vec<String> {
        vec![]
    }

    /// MCPG server entries this extension contributes.
    ///
    /// Returns `(server_name, config)` pairs inserted into the MCPG
    /// JSON configuration. Only consumed by the standalone compiler.
    fn mcpg_servers(&self, _ctx: &CompileContext) -> Result<Vec<(String, McpgServerConfig)>> {
        Ok(vec![])
    }

    /// Copilot CLI `--allow-tool` values this extension requires.
    ///
    /// Returns tool names (e.g., `"github"`, `"safeoutputs"`, `"azure-devops"`)
    /// that are emitted as `--allow-tool <name>` in the Copilot CLI invocation.
    fn allowed_copilot_tools(&self) -> Vec<String> {
        vec![]
    }

    /// Compile-time warnings to emit. Errors in the `Result` abort
    /// compilation; the inner `Vec<String>` contains non-fatal warnings
    /// printed to stderr.
    fn validate(&self, _ctx: &CompileContext) -> Result<Vec<String>> {
        Ok(vec![])
    }
}

// ──────────────────────────────────────────────────────────────────────
// Extension enum (static dispatch)
// ──────────────────────────────────────────────────────────────────────

/// Delegates every [`CompilerExtension`] method on an enum to the
/// inner variant, eliminating boilerplate when adding new extensions.
///
/// Usage:
/// ```ignore
/// extension_enum! {
///     pub enum Extension {
///         Lean(LeanExtension),
///         AzureDevOps(AzureDevOpsExtension),
///         CacheMemory(CacheMemoryExtension),
///     }
/// }
/// ```
macro_rules! extension_enum {
    (
        $(#[$meta:meta])*
        pub enum $Enum:ident {
            $( $Variant:ident($Inner:ty) ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        pub enum $Enum {
            $( $Variant($Inner), )+
        }

        impl CompilerExtension for $Enum {
            fn name(&self) -> &str {
                match self { $( $Enum::$Variant(e) => e.name(), )+ }
            }
            fn phase(&self) -> ExtensionPhase {
                match self { $( $Enum::$Variant(e) => e.phase(), )+ }
            }
            fn required_hosts(&self) -> Vec<String> {
                match self { $( $Enum::$Variant(e) => e.required_hosts(), )+ }
            }
            fn required_bash_commands(&self) -> Vec<String> {
                match self { $( $Enum::$Variant(e) => e.required_bash_commands(), )+ }
            }
            fn prompt_supplement(&self) -> Option<String> {
                match self { $( $Enum::$Variant(e) => e.prompt_supplement(), )+ }
            }
            fn prepare_steps(&self) -> Vec<String> {
                match self { $( $Enum::$Variant(e) => e.prepare_steps(), )+ }
            }
            fn mcpg_servers(&self, ctx: &CompileContext) -> Result<Vec<(String, McpgServerConfig)>> {
                match self { $( $Enum::$Variant(e) => e.mcpg_servers(ctx), )+ }
            }
            fn allowed_copilot_tools(&self) -> Vec<String> {
                match self { $( $Enum::$Variant(e) => e.allowed_copilot_tools(), )+ }
            }
            fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>> {
                match self { $( $Enum::$Variant(e) => e.validate(ctx), )+ }
            }
        }
    };
}

extension_enum! {
    /// All known compiler extensions, collected via [`collect_extensions`].
    ///
    /// Uses static dispatch (no `Box<dyn>`) — each variant delegates to
    /// the inner type's [`CompilerExtension`] implementation.
    pub enum Extension {
        GitHub(GitHubExtension),
        SafeOutputs(SafeOutputsExtension),
        Lean(LeanExtension),
        AzureDevOps(AzureDevOpsExtension),
        CacheMemory(CacheMemoryExtension),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Extension implementations
// ──────────────────────────────────────────────────────────────────────

// ─── Lean 4 ──────────────────────────────────────────────────────────

use crate::runtimes::lean::{
    self, LeanRuntimeConfig, LEAN_BASH_COMMANDS,
};

/// Lean 4 runtime extension.
///
/// Injects: network hosts (elan, lean-lang), bash commands (lean, lake,
/// elan), install steps (elan + toolchain), and a prompt supplement.
pub struct LeanExtension {
    config: LeanRuntimeConfig,
}

impl LeanExtension {
    pub fn new(config: LeanRuntimeConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for LeanExtension {
    fn name(&self) -> &str {
        "Lean 4"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Runtime
    }

    fn required_hosts(&self) -> Vec<String> {
        vec!["lean".to_string()]
    }

    fn required_bash_commands(&self) -> Vec<String> {
        LEAN_BASH_COMMANDS.iter().map(|c| (*c).to_string()).collect()
    }

    fn prompt_supplement(&self) -> Option<String> {
        Some(
            "\n\
---\n\
\n\
## Lean 4 Formal Verification\n\
\n\
Lean 4 is installed and available. Use `lean` to typecheck `.lean` files, \
`lake build` to build Lake projects, and `lake env printPaths` to inspect \
the toolchain. Lean files use the `.lean` extension.\n"
                .to_string(),
        )
    }

    fn prepare_steps(&self) -> Vec<String> {
        vec![lean::generate_lean_install(&self.config)]
    }

    fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        let is_bash_disabled = ctx
            .front_matter
            .tools
            .as_ref()
            .and_then(|t| t.bash.as_ref())
            .is_some_and(|cmds| cmds.is_empty());

        if is_bash_disabled {
            warnings.push(format!(
                "Agent '{}' has runtimes.lean enabled but tools.bash is empty. \
                 Lean requires bash access (lean, lake, elan commands).",
                ctx.agent_name
            ));
        }

        Ok(warnings)
    }
}

// ─── Azure DevOps MCP ────────────────────────────────────────────────

use crate::allowed_hosts::mcp_required_hosts;
use super::common::{ADO_MCP_IMAGE, ADO_MCP_CACHED_IMAGE, ADO_MCP_ENTRYPOINT, ADO_MCP_PACKAGE, ADO_MCP_SERVER_NAME};
use super::types::AzureDevOpsToolConfig;

/// Azure DevOps first-party tool extension.
///
/// Injects: network hosts (ADO domains), MCPG server entry (containerized
/// ADO MCP), and compile-time validation (org inference, duplicate MCP).
pub struct AzureDevOpsExtension {
    config: AzureDevOpsToolConfig,
}

impl AzureDevOpsExtension {
    pub fn new(config: AzureDevOpsToolConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for AzureDevOpsExtension {
    fn name(&self) -> &str {
        "Azure DevOps MCP"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn required_hosts(&self) -> Vec<String> {
        mcp_required_hosts("ado")
            .iter()
            .map(|h| (*h).to_string())
            .collect()
    }

    fn allowed_copilot_tools(&self) -> Vec<String> {
        vec![ADO_MCP_SERVER_NAME.to_string()]
    }

    fn prepare_steps(&self) -> Vec<String> {
        // Pre-build a local Docker image with @azure-devops/mcp pre-installed.
        // Without this, MCPG's 30s startup timeout is exceeded because `npx -y`
        // needs to download and install the package at container launch time.
        vec![format!(
            r#"- bash: |
    echo "Pre-building Azure DevOps MCP container image..."
    docker pull {base_image}
    CONTAINER_ID=$(docker create {base_image} sh -c "npm install -g {package}")
    docker start -a "$CONTAINER_ID"
    docker commit "$CONTAINER_ID" {cached_image}
    docker rm "$CONTAINER_ID"
    echo "Azure DevOps MCP container image ready: {cached_image}"
    docker images {cached_image}
  displayName: "Pre-install Azure DevOps MCP""#,
            base_image = ADO_MCP_IMAGE,
            package = ADO_MCP_PACKAGE,
            cached_image = ADO_MCP_CACHED_IMAGE,
        )]
    }

    fn mcpg_servers(&self, ctx: &CompileContext) -> Result<Vec<(String, McpgServerConfig)>> {
        // Build entrypoint args: @azure-devops/mcp <org> [-d toolset1 toolset2 ...]
        // Uses the pre-installed package from the cached image (no npx -y download needed).
        let mut entrypoint_args = vec![ADO_MCP_PACKAGE.to_string()];

        // Org: use explicit override, then inferred from git remote, then fail
        let org = self
            .config
            .org()
            .map(|s| s.to_string())
            .or_else(|| ctx.ado_org().map(|s| s.to_string()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Agent '{}' has tools.azure-devops enabled but no ADO organization could be \
                     determined. Either set tools.azure-devops.org explicitly, or compile from \
                     within a git repository with an Azure DevOps remote URL.",
                    ctx.agent_name
                )
            })?;
        if !org.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            anyhow::bail!(
                "Invalid ADO org name '{}': must contain only alphanumerics and hyphens",
                org
            );
        }
        entrypoint_args.push(org);

        // Toolsets: passed as -d flag followed by space-separated toolset names
        if !self.config.toolsets().is_empty() {
            entrypoint_args.push("-d".to_string());
            for toolset in self.config.toolsets() {
                if !toolset
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    anyhow::bail!(
                        "Invalid ADO toolset name '{}': must contain only alphanumerics and hyphens",
                        toolset
                    );
                }
                entrypoint_args.push(toolset.clone());
            }
        }

        // Tool allow-list for MCPG filtering
        let tools = if self.config.allowed().is_empty() {
            None
        } else {
            Some(self.config.allowed().to_vec())
        };

        // ADO MCP needs the PAT token passed via environment
        let env = Some(HashMap::from([(
            "AZURE_DEVOPS_EXT_PAT".to_string(),
            String::new(), // Passthrough from pipeline
        )]));

        Ok(vec![(
            ADO_MCP_SERVER_NAME.to_string(),
            McpgServerConfig {
                server_type: "stdio".to_string(),
                container: Some(ADO_MCP_CACHED_IMAGE.to_string()),
                entrypoint: Some(ADO_MCP_ENTRYPOINT.to_string()),
                entrypoint_args: Some(entrypoint_args),
                mounts: None,
                args: None,
                url: None,
                headers: None,
                env,
                tools,
            },
        )])
    }

    fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        // Warn if user also has a manual mcp-servers entry for azure-devops
        if ctx
            .front_matter
            .mcp_servers
            .contains_key(ADO_MCP_SERVER_NAME)
        {
            warnings.push(format!(
                "Agent '{}' has both tools.azure-devops and mcp-servers.azure-devops configured. \
                 The tools.azure-devops auto-configuration takes precedence. \
                 Remove the mcp-servers entry to silence this warning.",
                ctx.agent_name
            ));
        }

        Ok(warnings)
    }
}

// ─── Cache Memory ────────────────────────────────────────────────────

use super::types::CacheMemoryToolConfig;

/// Cache memory tool extension.
///
/// Injects: prepare steps (download/restore previous memory), and a
/// prompt supplement informing the agent about its memory directory.
pub struct CacheMemoryExtension {
    /// Config options (e.g., `allowed-extensions`) are consumed at Stage 3
    /// execution time, not at compile time. Retained here for potential
    /// future compile-time validation.
    #[allow(dead_code)]
    config: CacheMemoryToolConfig,
}

impl CacheMemoryExtension {
    pub fn new(config: CacheMemoryToolConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for CacheMemoryExtension {
    fn name(&self) -> &str {
        "Cache Memory"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn prepare_steps(&self) -> Vec<String> {
        vec![generate_memory_download()]
    }

    fn prompt_supplement(&self) -> Option<String> {
        Some(
            "\n\
---\n\
\n\
## Agent Memory\n\
\n\
You have persistent memory across runs. Your memory directory is located at `/tmp/awf-tools/staging/agent_memory/`.\n\
\n\
- **Read** previous memory files from this directory to recall context from prior runs.\n\
- **Write** new files or update existing ones in this directory to persist knowledge for future runs.\n\
- Use this memory to track patterns, accumulate findings, remember decisions, and improve over time.\n\
- The memory directory is yours to organize as you see fit (files, subdirectories, any structure).\n\
- Memory files are sanitized between runs for security; avoid including pipeline commands or secrets.\n"
                .to_string(),
        )
    }
}

/// Generate the steps to download agent memory from the previous successful run
/// and restore it to the staging directory.
fn generate_memory_download() -> String {
    r#"- task: DownloadPipelineArtifact@2
  displayName: "Download previous agent memory"
  condition: eq(${{ parameters.clearMemory }}, false)
  continueOnError: true
  inputs:
    source: "specific"
    project: "$(System.TeamProject)"
    pipeline: "$(System.DefinitionId)"
    runVersion: "latestFromBranch"
    branchName: "$(Build.SourceBranch)"
    artifact: "safe_outputs"
    targetPath: "$(Agent.TempDirectory)/previous_memory"
    allowPartiallySucceededBuilds: true

- bash: |
    mkdir -p /tmp/awf-tools/staging/agent_memory
    if [ -d "$(Agent.TempDirectory)/previous_memory/agent_memory" ]; then
      cp -a "$(Agent.TempDirectory)/previous_memory/agent_memory/." /tmp/awf-tools/staging/agent_memory/ 2>/dev/null || true
      echo "Previous agent memory restored to /tmp/awf-tools/staging/agent_memory"
      ls -laR /tmp/awf-tools/staging/agent_memory
    else
      echo "No previous agent memory found - empty memory directory created"
    fi
  displayName: "Restore previous agent memory"
  condition: eq(${{ parameters.clearMemory }}, false)
  continueOnError: true

- bash: |
    mkdir -p /tmp/awf-tools/staging/agent_memory
    echo "Memory cleared by pipeline parameter - starting fresh"
  displayName: "Initialize empty agent memory (clearMemory=true)"
  condition: eq(${{ parameters.clearMemory }}, true)"#
        .to_string()
}

// ─── GitHub (always-on, internal) ────────────────────────────────────

/// GitHub MCP extension.
///
/// Always-on internal extension that grants the agent access to the
/// Copilot CLI built-in GitHub MCP server via `--allow-tool github`.
/// The GitHub MCP uses `GITHUB_TOKEN` from the pipeline environment.
pub struct GitHubExtension;

impl CompilerExtension for GitHubExtension {
    fn name(&self) -> &str {
        "GitHub"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn allowed_copilot_tools(&self) -> Vec<String> {
        vec!["github".to_string()]
    }
}

// ─── SafeOutputs (always-on, internal) ───────────────────────────────

/// SafeOutputs MCP extension.
///
/// Always-on internal extension that configures the SafeOutputs HTTP
/// backend in MCPG and appends prompt guidance for the agent.
pub struct SafeOutputsExtension;

impl CompilerExtension for SafeOutputsExtension {
    fn name(&self) -> &str {
        "SafeOutputs"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn allowed_copilot_tools(&self) -> Vec<String> {
        vec!["safeoutputs".to_string()]
    }

    fn mcpg_servers(&self, _ctx: &CompileContext) -> Result<Vec<(String, McpgServerConfig)>> {
        Ok(vec![(
            "safeoutputs".to_string(),
            McpgServerConfig {
                server_type: "http".to_string(),
                container: None,
                entrypoint: None,
                entrypoint_args: None,
                mounts: None,
                args: None,
                url: Some("http://localhost:${SAFE_OUTPUTS_PORT}/mcp".to_string()),
                headers: Some(HashMap::from([(
                    "Authorization".to_string(),
                    "Bearer ${SAFE_OUTPUTS_API_KEY}".to_string(),
                )])),
                env: None,
                tools: None,
            },
        )])
    }

    fn prompt_supplement(&self) -> Option<String> {
        Some(
            r#"
---

## Important: Safe Outputs

You have access to the `safeoutputs` MCP server which provides tools for creating work items and reporting issues. **Always prefer using safeoutputs tools over other methods**.

These tools generate safe outputs that will be reviewed and executed in a separate pipeline stage, ensuring proper validation and security controls.
"#
            .to_string(),
        )
    }
}

// ──────────────────────────────────────────────────────────────────────
// Collection
// ──────────────────────────────────────────────────────────────────────

/// Collect all enabled compiler extensions from front matter.
///
/// ## Ordering policy
///
/// Extensions are sorted by [`ExtensionPhase`] before being returned:
/// runtimes run before tools. This guarantees that runtime install steps
/// execute before tool steps — critical when a tool depends on a runtime
/// (e.g., a Python-based tool like `uv` needs the Python runtime first).
///
/// Within the same phase, extensions preserve definition order
/// (runtimes in `RuntimesConfig` field order, tools in `ToolsConfig`
/// field order).
pub fn collect_extensions(front_matter: &FrontMatter) -> Vec<Extension> {
    let mut extensions = Vec::new();

    // ── Always-on internal extensions ──
    extensions.push(Extension::GitHub(GitHubExtension));
    extensions.push(Extension::SafeOutputs(SafeOutputsExtension));

    // ── Runtimes (ExtensionPhase::Runtime) ──
    if let Some(lean) = front_matter
        .runtimes
        .as_ref()
        .and_then(|r| r.lean.as_ref())
    {
        if lean.is_enabled() {
            extensions.push(Extension::Lean(LeanExtension::new(lean.clone())));
        }
    }

    // ── First-party tools (ExtensionPhase::Tool) ──
    if let Some(tools) = front_matter.tools.as_ref() {
        if let Some(ado) = tools.azure_devops.as_ref() {
            if ado.is_enabled() {
                extensions.push(Extension::AzureDevOps(AzureDevOpsExtension::new(
                    ado.clone(),
                )));
            }
        }
        if let Some(memory) = tools.cache_memory.as_ref() {
            if memory.is_enabled() {
                extensions.push(Extension::CacheMemory(CacheMemoryExtension::new(
                    memory.clone(),
                )));
            }
        }
    }

    // Enforce phase ordering: runtimes before tools.
    // sort_by_key is stable, preserving definition order within the same phase.
    extensions.sort_by_key(|ext| ext.phase());

    extensions
}

/// Wrap prompt supplement content in a `cat >>` pipeline step.
///
/// This is the generic wrapper used by the compiler to append extension
/// prompt supplements to the agent prompt file. Each line of content is
/// indented by 4 spaces to match the YAML block scalar indentation.
///
/// Returns an error if `display_name` contains characters unsafe for
/// embedding in bash `echo` or YAML `displayName` fields.
pub fn wrap_prompt_append(content: &str, display_name: &str) -> Result<String> {
    // Reject names that would break bash echo or YAML displayName.
    // This is a runtime guard (not debug_assert) because wrap_prompt_append
    // is pub and callable from future extension implementations.
    anyhow::ensure!(
        display_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')),
        "Extension display_name '{}' contains characters unsafe for bash/YAML embedding. \
         Only ASCII alphanumerics, spaces, hyphens, and underscores are allowed.",
        display_name
    );

    // Generate a unique heredoc delimiter from the display name
    let delimiter = display_name
        .to_uppercase()
        .replace(' ', "_")
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "");
    let delimiter = format!("{}_EOF", delimiter);

    // Indent every line of content by 4 spaces to match the heredoc indentation
    let indented_content: String = content
        .trim()
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("    {}", line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        r#"- bash: |
    cat >> "/tmp/awf-tools/agent-prompt.md" << '{delimiter}'
{indented_content}
    {delimiter}

    echo "{display_name} prompt appended"
  displayName: "Append {display_name} prompt""#,
        delimiter = delimiter,
        indented_content = indented_content,
        display_name = display_name,
    ))
}

// ──────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::parse_markdown;

    fn minimal_front_matter() -> FrontMatter {
        let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
        fm
    }

    fn ctx_from(fm: &FrontMatter) -> CompileContext<'_> {
        CompileContext::for_test(fm)
    }

    // ── collect_extensions ──────────────────────────────────────────

    #[test]
    fn test_collect_extensions_empty_front_matter() {
        let fm = minimal_front_matter();
        let exts = collect_extensions(&fm);
        // Always-on: GitHub + SafeOutputs
        assert_eq!(exts.len(), 2);
        assert!(exts.iter().any(|e| e.name() == "GitHub"));
        assert!(exts.iter().any(|e| e.name() == "SafeOutputs"));
    }

    #[test]
    fn test_collect_extensions_lean_enabled() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  lean: true\n---\n",
        )
        .unwrap();
        let exts = collect_extensions(&fm);
        assert_eq!(exts.len(), 3); // GitHub + SafeOutputs + Lean
        assert_eq!(exts[0].name(), "Lean 4"); // Runtime phase sorts first
    }

    #[test]
    fn test_collect_extensions_lean_disabled() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  lean: false\n---\n",
        )
        .unwrap();
        let exts = collect_extensions(&fm);
        assert_eq!(exts.len(), 2); // Just always-on
    }

    #[test]
    fn test_collect_extensions_azure_devops_enabled() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n",
        )
        .unwrap();
        let exts = collect_extensions(&fm);
        assert_eq!(exts.len(), 3); // GitHub + SafeOutputs + AzureDevOps
        assert!(exts.iter().any(|e| e.name() == "Azure DevOps MCP"));
    }

    #[test]
    fn test_collect_extensions_cache_memory_enabled() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  cache-memory: true\n---\n",
        )
        .unwrap();
        let exts = collect_extensions(&fm);
        assert_eq!(exts.len(), 3); // GitHub + SafeOutputs + CacheMemory
        assert!(exts.iter().any(|e| e.name() == "Cache Memory"));
    }

    #[test]
    fn test_collect_extensions_all_enabled() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  lean: true\ntools:\n  azure-devops: true\n  cache-memory: true\n---\n",
        )
        .unwrap();
        let exts = collect_extensions(&fm);
        assert_eq!(exts.len(), 5); // GitHub + SafeOutputs + Lean + AzureDevOps + CacheMemory
        assert_eq!(exts[0].name(), "Lean 4"); // Runtime phase first
        // All tool-phase extensions follow
        assert!(exts[1..].iter().all(|e| e.phase() == ExtensionPhase::Tool));
    }

    #[test]
    fn test_collect_extensions_runtimes_always_before_tools() {
        // Verify the phase ordering policy: all Runtime-phase extensions
        // must appear before any Tool-phase extensions, regardless of
        // front matter field order.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n  cache-memory: true\nruntimes:\n  lean: true\n---\n",
        )
        .unwrap();
        let exts = collect_extensions(&fm);
        assert_eq!(exts.len(), 5); // GitHub + SafeOutputs + Lean + AzureDevOps + CacheMemory

        // Find the boundary: last Runtime and first Tool
        let last_runtime_idx = exts
            .iter()
            .rposition(|e| e.phase() == ExtensionPhase::Runtime);
        let first_tool_idx = exts
            .iter()
            .position(|e| e.phase() == ExtensionPhase::Tool);

        if let (Some(last_rt), Some(first_tool)) = (last_runtime_idx, first_tool_idx) {
            assert!(
                last_rt < first_tool,
                "Runtime extensions must come before Tool extensions. \
                 Last runtime at index {last_rt}, first tool at index {first_tool}"
            );
        }
    }

    // ── LeanExtension ──────────────────────────────────────────────

    #[test]
    fn test_lean_required_hosts() {
        let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
        let hosts = ext.required_hosts();
        // Lean extension returns the ecosystem identifier; domain expansion
        // happens in generate_allowed_domains().
        assert_eq!(hosts, vec!["lean".to_string()]);
    }

    #[test]
    fn test_lean_required_bash_commands() {
        let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
        let cmds = ext.required_bash_commands();
        assert!(cmds.contains(&"lean".to_string()));
        assert!(cmds.contains(&"lake".to_string()));
        assert!(cmds.contains(&"elan".to_string()));
    }

    #[test]
    fn test_lean_prompt_supplement() {
        let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
        let prompt = ext.prompt_supplement().unwrap();
        assert!(prompt.contains("Lean 4"));
        assert!(prompt.contains("lake build"));
    }

    #[test]
    fn test_lean_prepare_steps() {
        let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
        let steps = ext.prepare_steps();
        assert_eq!(steps.len(), 1);
        assert!(steps[0].contains("elan-init.sh"));
    }

    #[test]
    fn test_lean_validate_bash_disabled_warning() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  bash: []\n---\n",
        )
        .unwrap();
        let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
        let ctx = ctx_from(&fm);
        let warnings = ext.validate(&ctx).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("tools.bash is empty"));
    }

    #[test]
    fn test_lean_validate_bash_not_disabled_no_warning() {
        let fm = minimal_front_matter();
        let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
        let ctx = ctx_from(&fm);
        let warnings = ext.validate(&ctx).unwrap();
        assert!(warnings.is_empty());
    }

    // ── AzureDevOpsExtension ───────────────────────────────────────

    #[test]
    fn test_ado_required_hosts() {
        let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
        let hosts = ext.required_hosts();
        assert!(hosts.contains(&"dev.azure.com".to_string()));
    }

    #[test]
    fn test_ado_mcpg_servers_with_inferred_org() {
        let fm = minimal_front_matter();
        let ctx = CompileContext::for_test_with_org(&fm, "myorg");
        let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
        let servers = ext.mcpg_servers(&ctx).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, ADO_MCP_SERVER_NAME);
        assert_eq!(servers[0].1.server_type, "stdio");
        assert!(servers[0]
            .1
            .entrypoint_args
            .as_ref()
            .unwrap()
            .contains(&"myorg".to_string()));
    }

    #[test]
    fn test_ado_mcpg_servers_no_org_fails() {
        let fm = minimal_front_matter();
        let ctx = CompileContext::for_test(&fm);
        let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
        assert!(ext.mcpg_servers(&ctx).is_err());
    }

    #[test]
    fn test_ado_validate_duplicate_mcp_warning() {
        let (mut fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\n---\n",
        )
        .unwrap();
        fm.mcp_servers.insert(
            ADO_MCP_SERVER_NAME.to_string(),
            crate::compile::types::McpConfig::Enabled(true),
        );
        let ctx = ctx_from(&fm);
        let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
        let warnings = ext.validate(&ctx).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("both tools.azure-devops and mcp-servers"));
    }

    #[test]
    fn test_ado_prepare_steps_builds_cached_image() {
        let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
        let steps = ext.prepare_steps();
        assert_eq!(steps.len(), 1);
        assert!(steps[0].contains(ADO_MCP_IMAGE), "should pull base image");
        assert!(steps[0].contains(ADO_MCP_PACKAGE), "should install package");
        assert!(steps[0].contains(ADO_MCP_CACHED_IMAGE), "should commit as cached image");
    }

    #[test]
    fn test_ado_mcpg_servers_uses_cached_image() {
        let fm = minimal_front_matter();
        let ctx = CompileContext::for_test_with_org(&fm, "myorg");
        let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
        let servers = ext.mcpg_servers(&ctx).unwrap();
        assert_eq!(servers[0].1.container.as_deref(), Some(ADO_MCP_CACHED_IMAGE));
        let args = servers[0].1.entrypoint_args.as_ref().unwrap();
        assert!(!args.contains(&"-y".to_string()), "cached image should not use -y flag");
    }

    // ── CacheMemoryExtension ───────────────────────────────────────

    #[test]
    fn test_cache_memory_prepare_steps() {
        let ext = CacheMemoryExtension::new(CacheMemoryToolConfig::Enabled(true));
        let steps = ext.prepare_steps();
        assert_eq!(steps.len(), 1);
        assert!(steps[0].contains("DownloadPipelineArtifact"));
    }

    #[test]
    fn test_cache_memory_prompt_supplement() {
        let ext = CacheMemoryExtension::new(CacheMemoryToolConfig::Enabled(true));
        let prompt = ext.prompt_supplement().unwrap();
        assert!(prompt.contains("Agent Memory"));
        assert!(prompt.contains("/tmp/awf-tools/staging/agent_memory/"));
    }

    // ── wrap_prompt_append ─────────────────────────────────────────

    #[test]
    fn test_wrap_prompt_append_generates_valid_yaml_step() {
        let content = "## Test\n\nSome instructions.";
        let step = wrap_prompt_append(content, "Test Feature").unwrap();
        assert!(step.contains("cat >>"));
        assert!(step.contains("agent-prompt.md"));
        assert!(step.contains("TEST_FEATURE_EOF"));
        assert!(step.contains("Test Feature"));
    }

    #[test]
    fn test_wrap_prompt_append_rejects_unsafe_display_name() {
        let result = wrap_prompt_append("content", "My \"Ext\"");
        assert!(result.is_err());

        let result = wrap_prompt_append("content", "ext$(rm -rf)");
        assert!(result.is_err());
    }
}
