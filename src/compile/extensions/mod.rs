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
use std::fmt;
use std::str::FromStr;

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

use crate::ado::AdoContext;
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
    /// Directory containing the agent markdown being compiled (i.e. the
    /// repo-relative dir against which paths like `global.json` /
    /// `nuget.config` should be resolved). `None` for unit-test contexts
    /// where no on-disk repo exists.
    pub compile_dir: Option<&'a Path>,
    /// Path of the input markdown file being compiled (e.g.
    /// `agents/release-readiness.md`). `None` for unit-test contexts.
    /// Consumed by the always-on `ado-aw-marker` compiler extension to
    /// embed source-path metadata in the compiled YAML.
    pub input_path: Option<&'a Path>,
}

impl<'a> CompileContext<'a> {
    /// Build a fully-resolved compile context.
    ///
    /// Resolves the engine implementation from front matter and infers ADO
    /// context from the git remote in the directory containing `input_path`.
    /// Returns an error if the engine identifier is unsupported.
    pub async fn new(front_matter: &'a FrontMatter, input_path: &'a Path) -> Result<Self> {
        // `Path::parent()` is subtle: for a bare filename like `foo.md`
        // it returns `Some(Path::new(""))` rather than `None`, so the
        // `unwrap_or(Path::new("."))` fallback wouldn't catch it. An
        // empty path passed to `git -C ""` behaves differently from
        // `git -C "."` (some platforms reject it, others quietly use
        // the parent process's cwd), so we normalise both the
        // `None` and empty-`Some` cases to `.`.
        let compile_dir = match input_path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let engine = engine::get_engine(front_matter.engine.engine_id())?;
        let ado_context = Self::infer_ado_context(compile_dir).await;
        Ok(Self {
            agent_name: &front_matter.name,
            front_matter,
            ado_context,
            engine,
            compile_dir: Some(compile_dir),
            input_path: Some(input_path),
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
        match crate::ado::get_git_remote_url(dir).await {
            Ok(url) => match crate::ado::parse_ado_remote(&url) {
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
            compile_dir: None,
            input_path: None,
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
            compile_dir: None,
            input_path: None,
        }
    }

    /// Create a context for tests with a specific compile directory.
    #[cfg(test)]
    pub fn for_test_with_compile_dir(front_matter: &'a FrontMatter, compile_dir: &'a Path) -> Self {
        Self {
            agent_name: &front_matter.name,
            front_matter,
            ado_context: None,
            engine: crate::engine::Engine::Copilot,
            compile_dir: Some(compile_dir),
            input_path: None,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// CompilerExtension trait
// ──────────────────────────────────────────────────────────────────────

/// Execution phase for extension ordering.
///
/// Extensions are collected and processed in phase order. The compiler
/// emits steps in this order: **System → Runtime → Tool**.
///
/// - **System** is reserved for compiler-internal infrastructure that
///   downstream phases assume is already in place (e.g.
///   `AdoScriptExtension`'s prompt-file resolver). System steps emit
///   their own self-contained tool installs and **must finish before
///   any other phase runs**, so that later phases can override shared
///   tool versions (notably the `node` on PATH).
/// - **Runtime** installs language toolchains (Lean, Python, Node, etc.)
///   for the user agent. A `NodeTool@0` here will land on top of any
///   System-phase Node install, so the user's pinned version wins on
///   PATH for everything after Runtime.
/// - **Tool** is first-party tooling (azure-devops, cache-memory, …)
///   that may depend on runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExtensionPhase {
    /// Compiler-internal infrastructure that everything else depends on.
    /// Reserved for ado-aw's own extensions (e.g. ado-script). Not for
    /// user-facing extension authors.
    System = 0,
    /// Language runtimes (Lean, Python, Node, etc.).
    Runtime = 1,
    /// First-party tools (azure-devops, cache-memory, etc.) — may
    /// depend on runtimes being available.
    Tool = 2,
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
    /// These are injected into the Agent job's `{{ prepare_steps }}`
    /// block — no new job/stage is created, so always-on extensions
    /// (like `ado-aw-marker`) can emit metadata steps with zero impact
    /// on pipeline structure.
    fn prepare_steps(&self, _ctx: &CompileContext) -> Vec<String> {
        vec![]
    }

    /// Pipeline steps (YAML strings) to inject into the Setup job.
    ///
    /// Unlike `prepare_steps()` which injects into the Execution job,
    /// these steps run in the Setup job (before the Execution job starts).
    /// Used by extensions that need to run gate logic or pre-activation
    /// checks before the agent is launched.
    fn setup_steps(&self, _ctx: &CompileContext) -> Result<Vec<String>> {
        Ok(vec![])
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

    /// AWF volume mounts this extension requires inside the chroot.
    ///
    /// AWF replaces `$HOME` with an empty directory overlay for security,
    /// only mounting specific known subdirectories. Extensions that install
    /// toolchains under `$HOME` (e.g., elan for Lean 4) must declare mounts
    /// here so the toolchain is accessible inside the chroot.
    ///
    /// Shell variables like `$HOME` are expanded at runtime by bash, not at
    /// compile time. AWF auto-adjusts container paths for chroot by prefixing
    /// `/host`.
    fn required_awf_mounts(&self) -> Vec<AwfMount> {
        vec![]
    }

    /// Directories to prepend to `PATH` inside the AWF chroot.
    ///
    /// Extensions that install toolchains outside standard system paths
    /// (e.g., elan installs Lean to `$HOME/.elan/bin`) should declare their
    /// bin directories here. The compiler collects these and generates a
    /// `GITHUB_PATH` file that AWF reads at startup to merge into the chroot
    /// PATH — bypassing the `sudo` PATH reset.
    ///
    /// Shell variables like `$HOME` are expanded at runtime by bash, not at
    /// compile time.
    fn awf_path_prepends(&self) -> Vec<String> {
        vec![]
    }

    /// Environment variables to inject into the agent execution environment.
    ///
    /// Returns `(key, value)` pairs that are emitted as `KEY: "value"` in
    /// the `{{ engine_env }}` YAML block. Used by runtimes to configure
    /// package managers via env vars (e.g., `PIP_INDEX_URL`, `NPM_CONFIG_REGISTRY`).
    ///
    /// Keys are validated against `BLOCKED_ENV_KEYS` at collection time.
    fn agent_env_vars(&self) -> Vec<(String, String)> {
        vec![]
    }
}

/// Mount access mode for an AWF bind mount.
///
/// Maps to the Docker bind-mount mode string: `ro` (read-only) or `rw`
/// (read-write, the Docker default when no mode is specified).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwfMountMode {
    /// Read-only mount (`ro`). The process inside the container cannot write
    /// to this path.
    ReadOnly,
    /// Read-write mount (`rw`). The container can write to this path.
    ReadWrite,
}

impl fmt::Display for AwfMountMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => f.write_str("ro"),
            Self::ReadWrite => f.write_str("rw"),
        }
    }
}

impl FromStr for AwfMountMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ro" => Ok(Self::ReadOnly),
            "rw" => Ok(Self::ReadWrite),
            other => anyhow::bail!("Unknown AWF mount mode '{}': expected 'ro' or 'rw'", other),
        }
    }
}

impl serde::Serialize for AwfMountMode {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for AwfMountMode {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// An AWF `--mount` specification in Docker bind-mount format.
///
/// The format is `host_path:container_path[:mode]`
/// (e.g. `"$HOME/.elan:$HOME/.elan:ro"`).
///
/// Serializes and deserializes as the Docker format string so it round-trips
/// cleanly through YAML/JSON configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwfMount {
    /// Host path to bind-mount into the container.
    pub host_path: String,
    /// Corresponding path inside the container.
    pub container_path: String,
    /// Mount access mode. Defaults to [`AwfMountMode::ReadOnly`] when not
    /// specified in the input — the secure default for AWF chroot mounts.
    pub mode: AwfMountMode,
}

impl AwfMount {
    /// Creates an `AwfMount` with the given host path, container path, and
    /// access mode.
    pub fn new(
        host_path: impl Into<String>,
        container_path: impl Into<String>,
        mode: AwfMountMode,
    ) -> Self {
        Self {
            host_path: host_path.into(),
            container_path: container_path.into(),
            mode,
        }
    }
}

impl fmt::Display for AwfMount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            self.host_path, self.container_path, self.mode
        )
    }
}

impl FromStr for AwfMount {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.splitn(3, ':').collect();
        match parts.as_slice() {
            [host, container] => Ok(Self {
                host_path: (*host).to_string(),
                container_path: (*container).to_string(),
                mode: AwfMountMode::ReadOnly,
            }),
            [host, container, mode_str] => Ok(Self {
                host_path: (*host).to_string(),
                container_path: (*container).to_string(),
                mode: mode_str.parse()?,
            }),
            _ => anyhow::bail!(
                "Invalid AWF mount spec '{}': expected 'host:container[:mode]'",
                s
            ),
        }
    }
}

impl serde::Serialize for AwfMount {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for AwfMount {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
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
            fn prepare_steps(&self, ctx: &CompileContext) -> Vec<String> {
                match self { $( $Enum::$Variant(e) => e.prepare_steps(ctx), )+ }
            }
            fn setup_steps(&self, ctx: &CompileContext) -> Result<Vec<String>> {
                match self { $( $Enum::$Variant(e) => e.setup_steps(ctx), )+ }
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
            fn required_awf_mounts(&self) -> Vec<AwfMount> {
                match self { $( $Enum::$Variant(e) => e.required_awf_mounts(), )+ }
            }
            fn awf_path_prepends(&self) -> Vec<String> {
                match self { $( $Enum::$Variant(e) => e.awf_path_prepends(), )+ }
            }
            fn agent_env_vars(&self) -> Vec<(String, String)> {
                match self { $( $Enum::$Variant(e) => e.agent_env_vars(), )+ }
            }
        }
    };
}

mod ado_aw_marker;
pub mod ado_script;
mod azure_cli;
mod exec_context;
mod github;
mod safe_outputs;

// Re-export tool/runtime extensions from their colocated homes
pub use crate::runtimes::dotnet::DotnetExtension;
pub use crate::runtimes::lean::LeanExtension;
pub use crate::runtimes::node::NodeExtension;
pub use crate::runtimes::python::PythonExtension;
pub use crate::tools::azure_devops::AzureDevOpsExtension;
pub use crate::tools::cache_memory::CacheMemoryExtension;
pub use ado_aw_marker::AdoAwMarkerExtension;
pub use ado_script::AdoScriptExtension;
pub use azure_cli::AzureCliExtension;
pub use exec_context::{ExecContextExtension, pr_contributor_will_activate};
pub use github::GitHubExtension;
pub use safe_outputs::SafeOutputsExtension;

extension_enum! {
    /// All known compiler extensions, collected via [`collect_extensions`].
    ///
    /// Uses static dispatch (no `Box<dyn>`) — each variant delegates to
    /// the inner type's [`CompilerExtension`] implementation.
    pub enum Extension {
        AdoAwMarker(AdoAwMarkerExtension),
        GitHub(GitHubExtension),
        SafeOutputs(SafeOutputsExtension),
        AdoScript(Box<AdoScriptExtension>),
        ExecContext(ExecContextExtension),
        Lean(LeanExtension),
        Python(PythonExtension),
        Node(NodeExtension),
        Dotnet(DotnetExtension),
        AzureDevOps(AzureDevOpsExtension),
        CacheMemory(CacheMemoryExtension),
        AzureCli(AzureCliExtension),
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
/// **System → Runtime → Tool**. System owns compiler-internal
/// infrastructure (ado-script bundle download + prompt resolver) that
/// must complete before user-facing toolchains land — notably so that a
/// later `NodeTool@0` from `NodeExtension` wins on PATH instead of
/// being silently overridden by the System-phase Node install.
///
/// Within the same phase, extensions preserve definition order
/// (runtimes in `RuntimesConfig` field order, tools in `ToolsConfig`
/// field order).
pub fn collect_extensions(front_matter: &FrontMatter) -> Vec<Extension> {
    // ── Always-on internal extensions ──
    // Always-on ado-script extension. Owns both the gate evaluator
    // (Setup job) and the runtime-import resolver (Agent job). Internal
    // gating on `filters:` and `inlined-imports` means the extension
    // emits no steps when neither feature is needed.
    //
    // Phase: `System` — so its `NodeTool@0` install + bundle download +
    // resolver step run BEFORE any user-facing Runtime extension (e.g.
    // `NodeExtension`). The user's pinned Node version then "wins last"
    // on PATH for the rest of the Agent job.
    let mut extensions = vec![
        Extension::AdoAwMarker(AdoAwMarkerExtension),
        Extension::GitHub(GitHubExtension),
        Extension::SafeOutputs(SafeOutputsExtension),
        Extension::AdoScript(Box::new({
            // PR trigger config drives both the PR-context contributor
            // (exec-context-pr.js) and the synthetic-from-ci path
            // (exec-context-pr-synth.js).
            //
            // `pr_trigger_for_synth` is the SINGLE source of truth for
            // synth-path activation: when `Some(_)` the extension emits
            // the synthPr Setup-job step and downstream wiring; when
            // `None` it doesn't. The previous separate `bool` flag is
            // now derived via `AdoScriptExtension::synthetic_pr_active()`.
            // The activation predicate (`mode == Synthetic`) lives in
            // `FrontMatter::is_synthetic_pr()` so it stays in lock-step
            // with the other two call sites (`compile_shared` and
            // `ExecContextExtension::new`).
            let pr_trigger_for_synth = if front_matter.is_synthetic_pr() {
                front_matter.pr_trigger().cloned()
            } else {
                None
            };
            AdoScriptExtension {
                pr_filters: front_matter.pr_filters().cloned(),
                pipeline_filters: front_matter.pipeline_filters().cloned(),
                inlined_imports: front_matter.inlined_imports,
                // Tell the ado-script extension whether the PR-context
                // contributor will activate so it can fire the Agent-job
                // install/download even when `inlined-imports: true` (no
                // import.js needed). The two extensions stay loosely
                // coupled: ExecContextExtension owns invoking the bundle;
                // AdoScriptExtension owns installing it. Shared helper
                // keeps the activation predicate in lock-step.
                exec_context_pr_active: pr_contributor_will_activate(front_matter),
                pr_trigger_for_synth,
            }
        })),
        // Always-on execution-context extension. Owns the `aw-context/`
        // precompute pipeline. Defaults to `ExecutionContextConfig::default()`
        // when the front matter omits the block — internal contributors
        // (currently: PR) self-gate via `should_activate`, so omitting
        // the block + having no `on.pr` produces zero output. See
        // `extensions/exec_context/`.
        Extension::ExecContext(ExecContextExtension::new(
            front_matter.execution_context.clone().unwrap_or_default(),
            front_matter,
        )),
        // Always-on Azure CLI. Tool phase — mounts host /opt/az and
        // /usr/bin/az into AWF and adds Azure auth hosts to the
        // allowlist so the agent can call `az`. No install step is
        // emitted: host pre-install is assumed (gh-aw parity).
        Extension::AzureCli(AzureCliExtension),
    ];

    // ── Runtimes (ExtensionPhase::Runtime) ──
    if let Some(lean) = front_matter.runtimes.as_ref().and_then(|r| r.lean.as_ref())
        && lean.is_enabled()
    {
        extensions.push(Extension::Lean(LeanExtension::new(lean.clone())));
    }
    if let Some(python) = front_matter
        .runtimes
        .as_ref()
        .and_then(|r| r.python.as_ref())
        && python.is_enabled()
    {
        extensions.push(Extension::Python(PythonExtension::new(python.clone())));
    }
    if let Some(node) = front_matter.runtimes.as_ref().and_then(|r| r.node.as_ref())
        && node.is_enabled()
    {
        extensions.push(Extension::Node(NodeExtension::new(node.clone())));
    }
    if let Some(dotnet) = front_matter
        .runtimes
        .as_ref()
        .and_then(|r| r.dotnet.as_ref())
        && dotnet.is_enabled()
    {
        extensions.push(Extension::Dotnet(DotnetExtension::new(dotnet.clone())));
    }

    // ── First-party tools (ExtensionPhase::Tool) ──
    if let Some(tools) = front_matter.tools.as_ref() {
        if let Some(ado) = tools.azure_devops.as_ref()
            && ado.is_enabled()
        {
            extensions.push(Extension::AzureDevOps(AzureDevOpsExtension::new(
                ado.clone(),
            )));
        }
        if let Some(memory) = tools.cache_memory.as_ref()
            && memory.is_enabled()
        {
            extensions.push(Extension::CacheMemory(CacheMemoryExtension::new(
                memory.clone(),
            )));
        }
    }

    // ── Trigger filters + runtime imports are owned by AdoScriptExtension
    // pushed above; no separate trigger-filters extension push is needed.

    // Enforce phase ordering: System → Runtime → Tool.
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
