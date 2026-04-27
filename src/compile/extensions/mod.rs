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
use std::collections::BTreeMap;

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
    pub headers: Option<BTreeMap<String, String>>,
    /// Environment variables for the server process
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
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
    pub mcp_servers: BTreeMap<String, McpgServerConfig>,
    pub gateway: McpgGatewayConfig,
}

// ──────────────────────────────────────────────────────────────────────
// Compile context
// ──────────────────────────────────────────────────────────────────────

use crate::configure::AdoContext;
use crate::engine::{self, Engine};
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
    /// Resolved engine based on the front matter `engine:` field.
    pub engine: Engine,
}

impl<'a> CompileContext<'a> {
    /// Build a fully-resolved compile context.
    ///
    /// Resolves the engine implementation from front matter and infers ADO
    /// context from the git remote in `compile_dir`. Returns an error if
    /// the engine identifier is unsupported.
    pub async fn new(front_matter: &'a FrontMatter, compile_dir: &Path) -> Result<Self> {
        let engine = engine::get_engine(front_matter.engine.engine_id())?;
        let ado_context = Self::infer_ado_context(compile_dir).await;
        Ok(Self {
            agent_name: &front_matter.name,
            front_matter,
            ado_context,
            engine,
        })
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
    // TODO: resolve engine from front_matter.engine when multiple engines are supported,
    // instead of hardcoding Engine::Copilot. Currently safe because "copilot" is the only
    // engine variant, but this will need to call get_engine() once more are added.
    #[cfg(test)]
    pub fn for_test(front_matter: &'a FrontMatter) -> Self {
        Self {
            agent_name: &front_matter.name,
            front_matter,
            ado_context: None,
            engine: crate::engine::Engine::Copilot,
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
            engine: crate::engine::Engine::Copilot,
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

    /// Pipeline variable mappings needed by this extension's MCP containers.
    ///
    /// Each mapping declares that a container env var (e.g., `AZURE_DEVOPS_EXT_PAT`)
    /// should be populated from a pipeline variable (e.g., `SC_READ_TOKEN`).
    /// The compiler uses these to generate:
    /// 1. `env:` block on the MCPG step (maps ADO secret → bash var)
    /// 2. `-e` flags on the MCPG docker run (passes bash var → MCPG process)
    /// 3. MCPG config keeps `""` (MCPG passthrough from its env → child container)
    fn required_pipeline_vars(&self) -> Vec<PipelineEnvMapping> {
        vec![]
    }

    /// Runtime config substitutions for MCPG config JSON.
    ///
    /// Extensions that embed `${PLACEHOLDER}` references in their MCPG
    /// server config (e.g., HTTP headers, URLs) declare the corresponding
    /// substitutions here. The compiler generates `sed` commands that
    /// replace each `${placeholder}` with `$(pipeline_var)` at runtime,
    /// before the config is piped to MCPG via stdin.
    fn mcpg_config_replacements(&self) -> Vec<McpgConfigReplacement> {
        vec![]
    }
}

/// Maps a container environment variable to a pipeline variable.
///
/// Used by extensions to declare that an MCP container needs a specific
/// pipeline variable (typically a secret) injected into its environment.
#[derive(Debug, Clone)]
pub struct PipelineEnvMapping {
    /// The env var name inside the MCP container (e.g., `AZURE_DEVOPS_EXT_PAT`).
    pub container_var: String,
    /// The ADO pipeline variable name (e.g., `SC_READ_TOKEN`).
    pub pipeline_var: String,
}

/// Declares a runtime `sed` substitution for the MCPG config JSON.
///
/// Extensions that use `${PLACEHOLDER}` references in their MCPG server
/// config (e.g., HTTP headers with Bearer tokens) declare substitutions
/// here. The compiler generates `sed` lines that replace each placeholder
/// with the actual pipeline variable value before passing the config to MCPG.
#[derive(Debug, Clone)]
pub struct McpgConfigReplacement {
    /// Placeholder name in the MCPG config JSON (e.g., `"S360_TOKEN"`).
    /// Referenced as `${placeholder}` in the config.
    pub placeholder: String,
    /// ADO pipeline variable name whose value replaces the placeholder
    /// at runtime (e.g., `"SC_S360_TOKEN"`).
    pub pipeline_var: String,
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
            fn required_pipeline_vars(&self) -> Vec<PipelineEnvMapping> {
                match self { $( $Enum::$Variant(e) => e.required_pipeline_vars(), )+ }
            }
            fn mcpg_config_replacements(&self) -> Vec<McpgConfigReplacement> {
                match self { $( $Enum::$Variant(e) => e.mcpg_config_replacements(), )+ }
            }
        }
    };
}

mod github;
mod safe_outputs;

// Re-export tool/runtime extensions from their colocated homes
pub use crate::tools::azure_devops::AzureDevOpsExtension;
pub use crate::tools::cache_memory::CacheMemoryExtension;
pub use crate::tools::s360_breeze::S360BreezeExtension;
pub use github::GitHubExtension;
pub use crate::runtimes::lean::LeanExtension;
pub use safe_outputs::SafeOutputsExtension;

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
        S360Breeze(S360BreezeExtension),
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
    if let Some(lean) = front_matter.runtimes.as_ref().and_then(|r| r.lean.as_ref()) {
        if lean.is_enabled() {
            extensions.push(Extension::Lean(LeanExtension::new(lean.clone())));
        }
    }

    // ── First-party tools (ExtensionPhase::Tool) ──
    if let Some(tools) = front_matter.tools.as_ref() {
        if let Some(ado) = tools.azure_devops.as_ref() {
            if ado.is_enabled() {
                extensions.push(Extension::AzureDevOps(
                    AzureDevOpsExtension::new(ado.clone()),
                ));
            }
        }
        if let Some(s360) = tools.s360_breeze.as_ref() {
            if s360.is_enabled() {
                extensions.push(Extension::S360Breeze(
                    S360BreezeExtension::new(s360.clone()),
                ));
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

#[cfg(test)]
mod tests;
