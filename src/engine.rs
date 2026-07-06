use std::collections::HashMap;

use anyhow::Result;

use crate::compile::extensions::Declarations;
use crate::compile::types::{CompileTarget, EngineConfig, FrontMatter, McpConfig};
use crate::validate::{
    contains_ado_expression, contains_ado_template_expression, contains_newline,
    contains_pipeline_command, is_valid_arg, is_valid_command_path, is_valid_env_var_name,
    is_valid_hostname, is_valid_identifier, is_valid_version,
};

/// Flags that the compiler controls — user args must not attempt to override these.
const BLOCKED_ARG_PREFIXES: &[&str] = &[
    "--prompt",
    "--additional-mcp-config",
    "--allow-tool",
    "--allow-all-tools",
    "--allow-all-paths",
    "--disable-builtin-mcps",
    "--no-ask-user",
    "--ask-user",
];

/// Environment variable keys that the compiler controls — users must not override these.
pub const BLOCKED_ENV_KEYS: &[&str] = &[
    "GITHUB_TOKEN",
    "GITHUB_READ_ONLY",
    "COPILOT_OTEL_ENABLED",
    "COPILOT_OTEL_EXPORTER_TYPE",
    "COPILOT_OTEL_FILE_EXPORTER_PATH",
    // Shell/system vars that could affect AWF or pipeline behavior
    "PATH",
    "HOME",
    "BASH_ENV",
    "ENV",
    "IFS",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
];

/// Copilot Bring Your Own Model / Key (BYOM/BYOK) provider env-var keys that are
/// permitted to carry an ADO **macro** (`$(...)`) expression in `engine.env`.
/// Every other key remains literal-only.
///
/// This mirrors gh-aw's `CopilotEngine.GetSupportedEnvVarKeys()` allowlist
/// (`COPILOT_PROVIDER_*`), which gates dynamic provider credentials to a known
/// set of keys rather than relaxing expression handling globally. Setting
/// `COPILOT_PROVIDER_BASE_URL` activates BYOM mode so requests route to an
/// external Azure Copilot Foundry / OpenAI-compatible provider.
///
/// Only ADO **macros** `$(...)` are allowed — they are the sole expression form
/// ADO evaluates inside a step `env:` block. Template expressions (`${{ }}`,
/// compile-time, no secret-scoped analog) and runtime expressions (`$[ ... ]`,
/// not evaluated in step env — passed verbatim, see #1076) are rejected, as are
/// pipeline-command injection (`##vso[`) and newlines.
///
/// Matching against these keys is **case-sensitive** (exact), unlike the
/// case-insensitive [`BLOCKED_ENV_KEYS`]: the Copilot CLI only reads the
/// canonical uppercase `COPILOT_PROVIDER_*` names, so a lowercase lookalike is
/// never a usable provider var. Requiring exact case fails closed — a lowercase
/// `copilot_provider_api_key` carrying an expression is rejected with a clear
/// error rather than half-activating BYOM and then silently breaking at runtime.
pub const COPILOT_PROVIDER_EXPR_ENV_KEYS: &[&str] = &[
    "COPILOT_PROVIDER_BASE_URL",
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_BEARER_TOKEN",
    "COPILOT_PROVIDER_WIRE_API",
];

/// Case-sensitive prefix shared by every BYOM provider env-var key. Used to
/// select the provider subset for the Detection step.
const COPILOT_PROVIDER_PREFIX: &str = "COPILOT_PROVIDER_";

/// Returns true when `key` is an allowlisted BYOM/BYOK provider env-var key that
/// may carry ADO macro/runtime expressions in `engine.env`. Case-sensitive: see
/// [`COPILOT_PROVIDER_EXPR_ENV_KEYS`] for why exact case is required.
fn is_provider_expr_env_key(key: &str) -> bool {
    COPILOT_PROVIDER_EXPR_ENV_KEYS.contains(&key)
}

/// BYOM/BYOK provider **credential** env keys. Their presence in `engine.env`
/// activates BYOM mode and, in turn, the AWF api-proxy sidecar that holds the
/// real credential and injects it only at the proxy layer — keeping it out of
/// the agent container. These are also passed as AWF `--exclude-env` flags so
/// the raw value never reaches the agent via `--env-all` passthrough
/// (defense-in-depth; AWF also overrides them with placeholders).
///
/// This is the credential subset of [`COPILOT_PROVIDER_EXPR_ENV_KEYS`]:
/// `COPILOT_PROVIDER_WIRE_API` is **deliberately excluded** because it is
/// non-sensitive wire-format config, not a credential. Consequently a workflow
/// that only sets `COPILOT_PROVIDER_WIRE_API` dynamically may carry an expression
/// on it (it is in the expression allowlist) without activating the api-proxy
/// sidecar — which is correct, since there is no credential to isolate.
pub const COPILOT_BYOM_CREDENTIAL_ENV_KEYS: &[&str] = &[
    "COPILOT_PROVIDER_BASE_URL",
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_BEARER_TOKEN",
];

/// Derive the `COPILOT_PROVIDER_*` env pairs implied by an `engine.provider`
/// block. Pure mapping (infallible) — structural validity is enforced earlier by
/// [`validate_engine_feature_support`]. Empty when no `provider` block is set.
///
/// The provider credential is always plumbed as **`COPILOT_PROVIDER_API_KEY`**,
/// because the AWF api-proxy sidecar (which ado-aw always enables for BYOK) reads
/// its BYOK credential exclusively from `COPILOT_PROVIDER_API_KEY` and sends it
/// outbound as `Authorization: Bearer <value>` (verified against AWF v0.27.9
/// `containers/api-proxy/providers/copilot.js` + `provider-env-constants.js` —
/// there is no `COPILOT_PROVIDER_BEARER_TOKEN` in the sidecar). Both credential
/// sources map to this one key:
/// - `provider.token`  → `$(AW_PROVIDER_BEARER_TOKEN)` (the same-job secret the
///   in-job `AzureCLI@2` mint step publishes — resolves at runtime with no
///   cross-job output plumbing). The minted value is an AAD access token; the
///   sidecar sends it as a bearer token, which Azure AI Foundry accepts.
/// - `provider.api-key` → the user-supplied static `$(VAR)` secret.
///
/// `token` and `api-key` are mutually exclusive (enforced in
/// [`crate::compile::types::ProviderConfig::validate`]), so this never emits a
/// duplicate `COPILOT_PROVIDER_API_KEY`.
fn provider_derived_env(engine_config: &EngineConfig) -> Vec<(String, String)> {
    let Some(p) = engine_config.provider() else {
        return Vec::new();
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    pairs.push((
        "COPILOT_PROVIDER_BASE_URL".to_string(),
        p.base_url.as_str().to_string(),
    ));
    if let Some(t) = p.provider_type {
        pairs.push(("COPILOT_PROVIDER_TYPE".to_string(), t.as_ado_str().to_string()));
    }
    if let Some(w) = p.wire_api {
        pairs.push((
            "COPILOT_PROVIDER_WIRE_API".to_string(),
            w.as_ado_str().to_string(),
        ));
    }
    if p.token.is_some() {
        pairs.push((
            "COPILOT_PROVIDER_API_KEY".to_string(),
            format!("$({})", crate::compile::types::PROVIDER_BEARER_TOKEN_VAR),
        ));
    } else if let Some(api_key) = &p.api_key {
        pairs.push(("COPILOT_PROVIDER_API_KEY".to_string(), api_key.clone()));
    }
    pairs
}

/// The effective engine env = raw `engine.env` merged with the
/// [`provider_derived_env`] `COPILOT_PROVIDER_*` pairs. The two sources are
/// mutually exclusive (a conflict is a hard error in
/// [`validate_engine_feature_support`]), so the merge never silently clobbers.
/// Returned owned so a `provider`-only config still yields the provider vars.
///
/// **Pre-condition:** callers rely on [`validate_engine_feature_support`] having
/// already rejected a `provider` + raw `COPILOT_PROVIDER_*` conflict. This merge
/// itself does not re-check that; if invoked before validation (e.g. directly in
/// a unit test) provider-derived pairs win over any colliding raw keys.
fn effective_engine_env(engine_config: &EngineConfig) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = engine_config.env().cloned().unwrap_or_default();
    let derived = provider_derived_env(engine_config);
    // Self-enforce the pre-condition in debug/test builds: a `provider` block and
    // a colliding raw `engine.env COPILOT_PROVIDER_*` key must have been rejected
    // by `validate_engine_feature_support` upstream. No prod overhead.
    debug_assert!(
        derived.iter().all(|(k, _)| !map.contains_key(k)),
        "effective_engine_env: provider-derived key collides with a raw engine.env \
         COPILOT_PROVIDER_* key — validate_engine_feature_support should have rejected this"
    );
    for (k, v) in derived {
        map.insert(k, v);
    }
    map
}

/// Returns true when `engine.env` (or an `engine.provider` block) activates
/// Copilot BYOM/BYOK mode — i.e. any provider credential / base-URL key is
/// present. Used by the pipeline compiler to enable the AWF api-proxy sidecar
/// (`--enable-api-proxy`) for credential isolation and to pre-pull the api-proxy
/// container image. Case-sensitive.
pub fn copilot_byom_active(engine_config: &EngineConfig) -> bool {
    effective_engine_env(engine_config)
        .keys()
        .any(|key| COPILOT_BYOM_CREDENTIAL_ENV_KEYS.contains(&key.as_str()))
}

/// Return the BYOM credential env keys present in the effective engine env,
/// sorted. These are passed to AWF's `--exclude-env` so the raw credential is
/// kept out of the `--env-all` passthrough. Matching is case-sensitive (exact),
/// so the emitted `--exclude-env <key>` always names the canonical uppercase key
/// AWF and the Copilot CLI actually use.
pub fn copilot_byom_credential_keys(engine_config: &EngineConfig) -> Vec<String> {
    let env_map = effective_engine_env(engine_config);
    let mut keys: Vec<String> = env_map
        .keys()
        .filter(|key| COPILOT_BYOM_CREDENTIAL_ENV_KEYS.contains(&key.as_str()))
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// Extract the hostname from a literal `COPILOT_PROVIDER_BASE_URL` value so it can
/// be added to the AWF network allowlist. Returns `None` when the value is not a
/// parseable absolute URL with a host.
fn provider_base_url_host(base_url: &str) -> Option<String> {
    let parsed = url::Url::parse(base_url.trim()).ok()?;
    parsed.host_str().map(str::to_string)
}

/// Outcome of resolving `COPILOT_PROVIDER_BASE_URL` into an AWF-allowlist host.
///
/// Single source of truth shared by [`Engine::required_hosts`] (which consumes
/// the `Host` case) and [`Engine::network_host_warnings`] (which surfaces the
/// `Unresolved` case as a compile warning), so the two paths can never diverge.
enum ProviderBaseUrlHost {
    /// No literal base URL to resolve: the key is absent, or its value is an ADO
    /// expression (documented as requiring a manual `network.allowed` entry — not
    /// a surprising silent drop).
    None,
    /// A literal URL that parsed to a DNS-safe host — added to the allowlist.
    Host(String),
    /// A literal value that is **not** a parseable absolute URL, or whose host is
    /// not DNS-safe (e.g. an IPv6 literal). Not added — the author must add the
    /// provider hostname manually. Surfaced as a warning so the misconfiguration
    /// is visible at compile time rather than failing at runtime with a firewall
    /// block.
    Unresolved(String),
}

/// Classify the literal `COPILOT_PROVIDER_BASE_URL` in `engine_config.env` into a
/// [`ProviderBaseUrlHost`]. Case-sensitive key match (canonical uppercase).
fn resolve_provider_base_url_host(engine_config: &EngineConfig) -> ProviderBaseUrlHost {
    let env_map = effective_engine_env(engine_config);
    let Some(base_url) = env_map.get("COPILOT_PROVIDER_BASE_URL") else {
        return ProviderBaseUrlHost::None;
    };
    // Expression-valued base URLs resolve to an unknown host at compile time;
    // the author adds the host to `network.allowed` (documented) — no warning.
    if contains_ado_expression(base_url) {
        return ProviderBaseUrlHost::None;
    }
    match provider_base_url_host(base_url) {
        // Validate the parsed host with the same DNS-safe check the api-target
        // path uses, so non-DNS-safe hosts (e.g. an IPv6 literal `::1`, or an
        // IDN) never land in `--allow-domains`.
        Some(host) if is_valid_hostname(&host) => ProviderBaseUrlHost::Host(host),
        _ => ProviderBaseUrlHost::Unresolved(base_url.clone()),
    }
}

/// Default model used by the Copilot engine when no model is specified in front matter.
pub const DEFAULT_COPILOT_MODEL: &str = "claude-opus-4.7";

/// Default pinned version of the Copilot CLI.
/// Override per-agent via `engine: { id: copilot, version: "1.0.35" }` in front matter.
pub const COPILOT_CLI_VERSION: &str = "1.0.64";
const COPILOT_CLI_RELEASES_BASE: &str = "https://github.com/github/copilot-cli/releases";

/// Resolved engine — enum dispatch over supported engine identifiers.
///
/// Currently only `Copilot` (GitHub Copilot CLI) is supported. New engines
/// are added as variants here rather than via trait objects.
#[derive(Debug, Clone, Copy)]
pub enum Engine {
    Copilot,
}

/// Resolve the engine for a given engine identifier from front matter.
///
/// Currently only `copilot` is supported. Other identifiers produce a
/// compile error to prevent misconfiguration.
pub fn get_engine(engine_id: &str) -> Result<Engine> {
    match engine_id {
        "copilot" => Ok(Engine::Copilot),
        other => anyhow::bail!(
            "Unsupported engine '{}'. Only 'copilot' is supported by ado-aw. \
             See gh-aw documentation for engine identifiers.",
            other
        ),
    }
}

impl Engine {
    /// The default engine binary name (e.g., "copilot").
    ///
    /// Currently scaffolding — the pipeline templates hard-code the binary path
    /// (`/tmp/awf-tools/copilot`). This will be wired into template substitution
    /// when additional engines are added. Can be overridden per-agent via
    /// `engine.command` in front matter.
    #[allow(dead_code)]
    pub fn command(&self) -> &str {
        match self {
            Engine::Copilot => "copilot",
        }
    }

    /// Generate CLI arguments for the engine invocation.
    pub fn args(
        &self,
        front_matter: &FrontMatter,
        extension_declarations: &[Declarations],
    ) -> Result<String> {
        match self {
            Engine::Copilot => copilot_args(front_matter, extension_declarations),
        }
    }

    /// Generate the env block entries for the engine's sandbox step.
    pub fn env(&self, engine_config: &EngineConfig) -> Result<String> {
        match self {
            Engine::Copilot => copilot_env(engine_config),
        }
    }

    /// Return the engine's log directory path.
    ///
    /// Used by log collection steps to copy engine logs to pipeline artifacts.
    pub fn log_dir(&self) -> &str {
        match self {
            // `$HOME` (not `~`) so that the bash `[ -d "..." ]` test below
            // actually expands. Tilde does not expand inside double quotes,
            // so the previous value caused the directory check to always
            // fail and Copilot logs were silently never collected.
            Engine::Copilot => "$HOME/.copilot/logs",
        }
    }

    /// Return additional hosts the engine needs based on its configuration.
    ///
    /// Used by the domain allowlist generator to ensure engine-specific endpoints
    /// (e.g., GHES/GHEC API targets) are reachable through AWF.
    pub fn required_hosts(&self, engine_config: &EngineConfig) -> Vec<String> {
        match self {
            Engine::Copilot => {
                let mut hosts = Vec::new();
                if let Some(api_target) = engine_config.api_target() {
                    hosts.push(api_target.to_string());
                }
                // BYOM/BYOK: when COPILOT_PROVIDER_BASE_URL is a literal URL,
                // add its hostname to the AWF allowlist so the agent can reach
                // the external provider. Expression-valued or malformed URLs
                // resolve to `None`/`Unresolved` and are handled elsewhere
                // (`network_host_warnings` surfaces the malformed case).
                if let ProviderBaseUrlHost::Host(host) =
                    resolve_provider_base_url_host(engine_config)
                {
                    hosts.push(host);
                }
                hosts
            }
        }
    }

    /// Non-fatal compile-time warnings about network-host resolution.
    ///
    /// Currently surfaces the case where a **literal** `COPILOT_PROVIDER_BASE_URL`
    /// could not be resolved to a DNS-safe host (malformed URL, missing scheme, or
    /// an IPv6/IDN host). Without this the host would be silently dropped from the
    /// AWF allowlist and the agent would fail at runtime with a firewall block,
    /// contradicting the documented promise that a literal base URL is added
    /// automatically. The compile stays non-fatal; the operator is told to add the
    /// host via `network.allowed`.
    pub fn network_host_warnings(&self, engine_config: &EngineConfig) -> Vec<String> {
        match self {
            Engine::Copilot => match resolve_provider_base_url_host(engine_config) {
                ProviderBaseUrlHost::Unresolved(value) => vec![format!(
                    "COPILOT_PROVIDER_BASE_URL: '{value}' is not a parseable absolute URL \
                     (or its host is not DNS-safe); the host was not added to the AWF \
                     allowlist — add the provider hostname manually via network.allowed."
                )],
                ProviderBaseUrlHost::None | ProviderBaseUrlHost::Host(_) => Vec::new(),
            },
        }
    }

    /// Generate pipeline YAML steps to install the engine binary.
    ///
    /// Uses `engine_config.version()` if set in front matter, otherwise falls back
    /// to the pinned `COPILOT_CLI_VERSION` constant. Returns an empty string when
    /// `engine.command` is set (the user provides their own binary).
    ///
    /// `ado_org` is the ADO organization name inferred from the git remote at
    /// compile time. For 1ES targets it is embedded directly into the NuGet
    /// feed URL; when `None` a runtime extraction step is emitted instead.
    pub fn install_steps(
        &self,
        engine_config: &EngineConfig,
        target: &CompileTarget,
        ado_org: Option<&str>,
    ) -> Result<String> {
        match self {
            Engine::Copilot => copilot_install_steps(engine_config, target, ado_org),
        }
    }

    /// Generate the full AWF `--` command string for running the engine.
    ///
    /// Returns the content for the AWF `-- '<command>'` argument, including the
    /// binary path, prompt delivery flag, MCP config flag, and all CLI arguments.
    /// The engine controls how the prompt is provided (e.g., `--prompt "$(cat ...)"`
    /// for Copilot) and how MCP config is referenced.
    ///
    /// `prompt_path` is the path to the prompt file inside the AWF container.
    /// `mcp_config_path` is optionally the path to the MCP config file
    /// (Some for Agent job, None for Detection job which has no MCP).
    pub fn invocation(
        &self,
        front_matter: &FrontMatter,
        extension_declarations: &[Declarations],
        prompt_path: &str,
        mcp_config_path: Option<&str>,
    ) -> Result<String> {
        let args = self.args(front_matter, extension_declarations)?;
        match self {
            Engine::Copilot => {
                let command_path = match front_matter.engine.command() {
                    Some(cmd) => {
                        if !is_valid_command_path(cmd) {
                            anyhow::bail!(
                                "engine.command '{}' contains invalid characters. \
                                 Only ASCII alphanumerics, '.', '_', '/', and '-' are allowed.",
                                cmd
                            );
                        }
                        cmd.to_string()
                    }
                    None => "/tmp/awf-tools/copilot".to_string(),
                };
                Ok(copilot_invocation(
                    &command_path,
                    prompt_path,
                    mcp_config_path,
                    &args,
                ))
            }
        }
    }
}

/// Collects the list of allowed tool identifiers when bash is not in wildcard mode.
///
/// Returns a flat `Vec<String>` of fully-qualified tool identifiers ready to be
/// passed as `--allow-tool` arguments. Only called when `use_allow_all_tools` is
/// `false`; the caller upholds that invariant.
fn collect_allowed_tools(
    front_matter: &FrontMatter,
    extension_declarations: &[Declarations],
    edit_enabled: bool,
) -> Result<Vec<String>> {
    let mut allowed_tools: Vec<String> = Vec::new();

    // Tools from compiler extensions (github, safeoutputs, azure-devops, etc.)
    for decl in extension_declarations {
        for tool in &decl.copilot_allow_tools {
            if !allowed_tools.contains(tool) {
                allowed_tools.push(tool.clone());
            }
        }
    }

    // Tools from user-defined MCP servers (sorted for deterministic output).
    // Only add --allow-tool for MCPs that will actually produce an MCPG entry (i.e.,
    // WithOptions that have a container or url). McpConfig::Enabled(true) has no backing
    // server in MCPG, so granting the permission would cause confusing runtime errors.
    let mut sorted_mcps: Vec<_> = front_matter.mcp_servers.iter().collect();
    sorted_mcps.sort_by_key(|(a, _)| *a);
    for (name, config) in sorted_mcps {
        // Skip servers already provided by extensions (case-insensitive to match
        // generate_mcpg_config's eq_ignore_ascii_case guard for reserved names)
        if allowed_tools.iter().any(|t| t.eq_ignore_ascii_case(name)) {
            continue;
        }
        // Only add MCPs that have a backing server (container or url)
        let has_backing_server = match config {
            McpConfig::Enabled(_) => false,
            McpConfig::WithOptions(opts) => {
                opts.enabled.unwrap_or(true) && (opts.container.is_some() || opts.url.is_some())
            }
        };
        if has_backing_server {
            allowed_tools.push(name.clone());
        }
    }

    // Intentional: with restricted bash, both --allow-tool write (tool identity)
    // and --allow-all-paths (path scope) are emitted. --allow-all-tools subsumes
    // --allow-tool write, so only --allow-all-paths is needed on that path.
    if edit_enabled {
        allowed_tools.push("write".to_string());
    }

    // Bash tool: use the explicitly configured list.
    // When bash is None (not specified), use_allow_all_tools is true and this
    // function is not called — that invariant is upheld by the caller.
    let mut bash_commands: Vec<String> =
        match front_matter.tools.as_ref().and_then(|t| t.bash.as_ref()) {
            Some(cmds) if cmds.is_empty() => {
                // Explicitly disabled: no bash commands
                vec![]
            }
            Some(cmds) => {
                // Explicit list of commands
                cmds.clone()
            }
            None => {
                // Invariant: bash=None → use_allow_all_tools=true → this function is
                // not called. Panic if the invariant is ever broken.
                unreachable!("bash=None should imply use_allow_all_tools=true")
            }
        };

    // Auto-add extension-declared bash commands (runtimes + first-party tools)
    for decl in extension_declarations {
        for cmd in &decl.bash_commands {
            if !bash_commands.contains(cmd) {
                bash_commands.push(cmd.clone());
            }
        }
    }

    for cmd in &bash_commands {
        // Reject single quotes in bash commands — copilot_params are embedded inside
        // a single-quoted bash string in the AWF command.
        if cmd.contains('\'') {
            anyhow::bail!(
                "Bash command '{}' contains a single quote, which is not allowed \
                 (would break AWF shell quoting).",
                cmd
            );
        }
        allowed_tools.push(format!("shell({})", cmd));
    }

    Ok(allowed_tools)
}

/// Validates a single `engine.args` entry.
///
/// Returns an error if the argument contains unsafe characters or attempts to
/// override a compiler-controlled flag.
fn validate_user_arg(arg: &str) -> Result<()> {
    if !is_valid_arg(arg) {
        anyhow::bail!(
            "engine.args entry '{}' contains invalid characters. \
             Only ASCII alphanumerics and '.', '_', ':', '-', '=', '/', '@' are allowed.",
            arg
        );
    }
    // Reject args that attempt to override compiler-controlled flags
    for blocked in BLOCKED_ARG_PREFIXES {
        if arg.starts_with(blocked) {
            anyhow::bail!(
                "engine.args entry '{}' conflicts with compiler-controlled flag '{}'. \
                 These flags are managed by the compiler and cannot be overridden.",
                arg,
                blocked
            );
        }
    }
    Ok(())
}

fn copilot_args(
    front_matter: &FrontMatter,
    extension_declarations: &[Declarations],
) -> Result<String> {
    // Check if bash triggers --allow-all-tools. This happens when:
    // 1. Bash has an explicit wildcard entry (":*" or "*"), OR
    // 2. Bash is not specified at all (None) — ado-aw agents always run in AWF sandbox,
    //    and gh-aw defaults to bash: ["*"] when sandbox is enabled (applyDefaultTools).
    //
    // Note: wildcard detection requires exactly one entry (cmds.len() == 1). Mixing a
    // wildcard with other commands (e.g. bash: [":*", "cat"]) is not supported and will
    // fall through to the restricted path, emitting "shell(:*)" literally.
    let bash_config = front_matter.tools.as_ref().and_then(|t| t.bash.as_ref());
    let use_allow_all_tools = match bash_config {
        Some(cmds) if cmds.len() == 1 && (cmds[0] == ":*" || cmds[0] == "*") => true,
        None => true, // default: all tools (matches gh-aw sandbox default)
        _ => false,
    };

    // Edit tool: enabled by default, can be disabled with `edit: false`
    let edit_enabled = front_matter
        .tools
        .as_ref()
        .and_then(|t| t.edit)
        .unwrap_or(true);

    // When --allow-all-tools is active, skip individual tool collection entirely.
    // --allow-all-tools is a superset that permits all tool calls regardless.
    let allowed_tools: Vec<String> = if use_allow_all_tools {
        Vec::new()
    } else {
        collect_allowed_tools(front_matter, extension_declarations, edit_enabled)?
    };

    let mut params = Vec::new();

    // Validate model name to prevent shell injection — copilot_params are embedded
    // inside a single-quoted bash string in the AWF command.
    let model = front_matter.engine.model().unwrap_or(DEFAULT_COPILOT_MODEL);
    if model.is_empty()
        || !model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
    {
        anyhow::bail!(
            "Model name '{}' contains invalid characters. \
             Only ASCII alphanumerics, '.', '_', ':', and '-' are allowed.",
            model
        );
    }
    params.push(format!("--model {}", model));
    if let Some(0) = front_matter.engine.timeout_minutes() {
        eprintln!(
            "Warning: Agent '{}' has timeout-minutes: 0, which means no time is allowed. \
            The agent job will time out immediately. \
            Consider setting timeout-minutes to at least 1.",
            front_matter.name
        );
    }

    // Wire engine.agent — selects a custom agent from .github/agents/
    if let Some(agent) = front_matter.engine.agent() {
        if !is_valid_identifier(agent) {
            anyhow::bail!(
                "engine.agent '{}' contains invalid characters. \
                 Only ASCII alphanumerics, '.', '_', ':', and '-' are allowed.",
                agent
            );
        }
        params.push(format!("--agent {}", agent));
    }

    // Wire engine.api-target — sets the GHES/GHEC API endpoint hostname
    if let Some(api_target) = front_matter.engine.api_target() {
        if !is_valid_hostname(api_target) {
            anyhow::bail!(
                "engine.api-target '{}' contains invalid characters. \
                 Only ASCII alphanumerics, '.', and '-' are allowed.",
                api_target
            );
        }
        params.push(format!("--api-target {}", api_target));
    }

    params.push("--disable-builtin-mcps".to_string());
    params.push("--no-ask-user".to_string());

    if use_allow_all_tools {
        params.push("--allow-all-tools".to_string());
    } else {
        for tool in allowed_tools {
            if tool.contains('(') || tool.contains(')') || tool.contains(' ') {
                // Use double quotes - the copilot_params are embedded inside a single-quoted
                // bash string in the AWF command, so single quotes would break quoting.
                params.push(format!("--allow-tool \"{}\"", tool));
            } else {
                params.push(format!("--allow-tool {}", tool));
            }
        }
    }

    // --allow-all-paths when edit is enabled — lets the agent write to any file path.
    // Emitted independently of --allow-all-tools (matches gh-aw behavior).
    if edit_enabled {
        params.push("--allow-all-paths".to_string());
    }

    // Wire engine.args — append user-provided CLI arguments after compiler-generated args.
    // User args are additive; they cannot remove compiler security flags but may override
    // non-security defaults via last-wins semantics (e.g., --model).
    for arg in front_matter.engine.args() {
        validate_user_arg(arg)?;
        params.push(arg.to_string());
    }

    Ok(params.join(" "))
}

/// The masked, same-job pipeline variable the `github-app-token` ado-script
/// bundle sets. When `engine.github-app-token` is configured, the Copilot
/// engine's `GITHUB_TOKEN` is sourced from this variable (set by the mint step
/// earlier in the same job) instead of the operator-provided `GITHUB_TOKEN`
/// pipeline variable.
pub const GITHUB_APP_TOKEN_VAR: &str = "GITHUB_APP_TOKEN";

/// Return the ADO pipeline-variable name that `GITHUB_TOKEN` should be sourced
/// from for the Copilot engine, given the engine config. When
/// `engine.github-app-token` is set this is [`GITHUB_APP_TOKEN_VAR`] (minted
/// same-job by the token step); otherwise it is the operator-provided
/// `GITHUB_TOKEN` pipeline variable.
pub fn github_token_source_var(engine_config: &EngineConfig) -> &'static str {
    if engine_config.github_app_token().is_some() {
        GITHUB_APP_TOKEN_VAR
    } else {
        "GITHUB_TOKEN"
    }
}

/// Cross-field validation: reject engine-specific config that only the Copilot
/// engine wires up when it is set alongside a different `engine.id`. Without
/// this, `engine.github-app-token` on a non-Copilot engine would silently be a
/// no-op — the mint/revoke steps would run but `copilot_env` (the only place
/// `GITHUB_TOKEN` is sourced from `$(GITHUB_APP_TOKEN)`) is Copilot-only.
///
/// Modelled on gh-aw's engine-gated config validation (e.g.
/// `validateMaxToolDenialsSupport`, which hard-errors "…supported only with
/// engine 'copilot'…"). ado-aw currently only supports the Copilot engine
/// (`get_engine` rejects other ids), so this is primarily a future-proofing
/// guard + a precise error message; when a second engine is added it becomes
/// the load-bearing check that prevents a silent no-op.
pub fn validate_engine_feature_support(engine_config: &EngineConfig) -> Result<()> {
    let id = engine_config.engine_id();
    if engine_config.github_app_token().is_some() && id != "copilot" {
        anyhow::bail!(
            "engine.github-app-token is only supported for engine.id = copilot (got '{id}'). \
             The minted installation token is wired into GITHUB_TOKEN only on the Copilot \
             engine path; on another engine it would be a no-op. Remove github-app-token or \
             set engine.id to copilot."
        );
    }
    if let Some(provider) = engine_config.provider() {
        if id != "copilot" {
            anyhow::bail!(
                "engine.provider is only supported for engine.id = copilot (got '{id}'). \
                 The COPILOT_PROVIDER_* routing + api-proxy isolation are wired only on the \
                 Copilot engine path. Remove engine.provider or set engine.id to copilot."
            );
        }
        provider.validate()?;
        // Single source of truth: reject provider settings declared both in the
        // typed block and as raw engine.env COPILOT_PROVIDER_* keys.
        if let Some(env) = engine_config.env()
            && let Some(conflict) = env.keys().find(|k| k.starts_with(COPILOT_PROVIDER_PREFIX))
        {
            anyhow::bail!(
                "engine.provider is set, so provider settings must not also be declared in \
                 engine.env (found '{conflict}'). Move all COPILOT_PROVIDER_* config into the \
                 engine.provider block, or remove engine.provider."
            );
        }
    }
    Ok(())
}

/// Non-blocking advisory emitted at compile time when `engine.github-app-token`
/// is configured. The compiler cannot introspect ADO variable metadata, so it
/// cannot verify that the referenced `private-key` variable is actually marked
/// **secret** — it only validates the variable *name*. This advisory reminds
/// the author to store the private key as a secret (via `ado-aw secrets set`),
/// which is the one thing they must get right and the compiler can't enforce.
///
/// Returns `None` when the feature is not used, so the caller emits nothing for
/// the common case. Naming the specific variable keeps the message actionable.
pub fn github_app_token_secrecy_advisory(engine_config: &EngineConfig) -> Option<String> {
    engine_config.github_app_token().map(|cfg| {
        format!(
            "engine.github-app-token uses pipeline variable '{0}' for the GitHub App \
             private key. Ensure '{0}' is stored as a SECRET (e.g. `ado-aw secrets set {0} \
             \"$(cat key.pem)\"`); the compiler cannot verify a variable is marked secret.",
            cfg.private_key_var()
        )
    })
}

fn copilot_env(engine_config: &EngineConfig) -> Result<String> {
    let token_var = github_token_source_var(engine_config);
    let mut lines: Vec<String> = vec![
        format!("GITHUB_TOKEN: $({token_var})"),
        "GITHUB_READ_ONLY: 1".to_string(),
        "COPILOT_OTEL_ENABLED: \"true\"".to_string(),
        "COPILOT_OTEL_EXPORTER_TYPE: \"file\"".to_string(),
        "COPILOT_OTEL_FILE_EXPORTER_PATH: \"/tmp/awf-tools/staging/otel.jsonl\"".to_string(),
    ];

    // Wire engine.env — merge user-provided environment variables plus any
    // `COPILOT_PROVIDER_*` vars derived from an `engine.provider` block.
    let env_map = effective_engine_env(engine_config);
    if !env_map.is_empty() {
        let mut sorted_keys: Vec<&String> = env_map.keys().collect();
        sorted_keys.sort();

        for key in sorted_keys {
            lines.push(render_engine_env_entry(key, &env_map[key])?);
        }
    }

    Ok(lines.join("\n"))
}

/// Return the `COPILOT_PROVIDER_*` entries of `engine.env` as validated,
/// **raw** `(key, value)` pairs (value un-rendered; empty vec when none present),
/// sorted by key.
///
/// Used by the Detection (threat-analysis) step so the detection Copilot run
/// inherits the same BYOM/BYOK provider routing (and credential isolation) as
/// the main agent. Mirrors gh-aw, whose detection engine config inherits the
/// main engine's `Env` (`threat_detection_inline_engine.go`). The main model is
/// already threaded via the `--model` flag on the detection invocation, so only
/// the provider routing/credential keys are needed here.
///
/// Returning raw pairs (rather than a rendered YAML string) lets the call site
/// build typed `EnvValue`s directly — no render-to-YAML-then-reparse round-trip,
/// and no implicit coupling between a quoting format and a re-parser. Each entry
/// is validated with [`validate_engine_env_entry`], identical to the agent path.
///
/// **Scope note — two distinct provider-key sets, do not conflate:**
/// - This function forwards **every** `COPILOT_PROVIDER_*` key (prefix match) to
///   the detection step's env, so detection routes to the same provider as the
///   agent (including non-credential config like `COPILOT_PROVIDER_TYPE` /
///   `COPILOT_PROVIDER_WIRE_API`). Unknown extras are harmless — the CLI ignores
///   keys it doesn't recognise.
/// - The narrower [`COPILOT_BYOM_CREDENTIAL_ENV_KEYS`] set (via
///   [`copilot_byom_credential_keys`]) drives BYOM *activation* and the AWF
///   `--exclude-env` isolation flags — only the actual credential-bearing keys.
pub fn copilot_provider_env(engine_config: &EngineConfig) -> Result<Vec<(String, String)>> {
    let mut pairs: Vec<(String, String)> = Vec::new();

    // Raw `engine.env` provider keys are **user input** → validate each with the
    // user-input validator, exactly as the agent path does.
    if let Some(env_map) = engine_config.env() {
        let mut keys: Vec<&String> = env_map
            .keys()
            .filter(|k| k.starts_with(COPILOT_PROVIDER_PREFIX))
            .collect();
        keys.sort();
        for key in keys {
            let value = &env_map[key];
            validate_engine_env_entry(key, value)?;
            pairs.push((key.clone(), value.clone()));
        }
    }

    // Compiler-derived provider entries (from `engine.provider`) are NOT user
    // input: their values are either compiler-owned macros (e.g.
    // `$(AW_PROVIDER_BEARER_TOKEN)`) or already structurally validated at
    // deserialization (`ProviderBaseUrl`, enum-typed `type`/`wire-api`). They are
    // deliberately **not** passed through `validate_engine_env_entry` — that
    // validator is scoped to untrusted `engine.env` values, and coupling a
    // compiler-generated macro to it would be fragile (a future tightening of the
    // user-input rules must not break the `provider` path). `engine.provider` and
    // raw `COPILOT_PROVIDER_*` keys are mutually exclusive (hard error in
    // `validate_engine_feature_support`), so the two sources never both contribute.
    for (k, v) in provider_derived_env(engine_config) {
        pairs.push((k, v));
    }

    pairs.sort();
    Ok(pairs)
}

/// Validate a single `engine.env` entry: enforces env-var-name rules, blocks
/// compiler-controlled keys, rejects pipeline-command injection / newlines, and
/// applies the BYOM provider expression allowlist (allowlisted keys may carry an
/// ADO macro `$(...)`; all others are literal-only). Shared by
/// [`render_engine_env_entry`] (agent path) and [`copilot_provider_env`]
/// (detection path) so both validate identically.
fn validate_engine_env_entry(key: &str, value: &str) -> Result<()> {
    // Validate key: must be a valid env var name
    if key.is_empty() {
        anyhow::bail!(
            "engine.env contains an empty key. \
             Keys must match [A-Za-z_][A-Za-z0-9_]*."
        );
    }
    if !is_valid_env_var_name(key) {
        anyhow::bail!(
            "engine.env key '{}' is not a valid environment variable name. \
             Must match [A-Za-z_][A-Za-z0-9_]*.",
            key
        );
    }

    // Block compiler-controlled env vars.
    // Intentionally case-insensitive: while Linux env vars are case-sensitive,
    // blocking both "GITHUB_TOKEN" and "github_token" prevents accidental
    // shadowing and confusion. The trade-off is that a legitimate custom var
    // whose name collides case-insensitively with a blocked key is rejected.
    if BLOCKED_ENV_KEYS
        .iter()
        .any(|blocked| key.eq_ignore_ascii_case(blocked))
    {
        anyhow::bail!(
            "engine.env key '{}' conflicts with a compiler-controlled environment variable. \
             These variables are managed by the compiler and cannot be overridden.",
            key
        );
    }

    // Validate value: reject ADO command injection and YAML-breaking content
    if contains_pipeline_command(value) {
        anyhow::bail!(
            "engine.env value for '{}' contains ADO pipeline command injection ('##vso[' or '##['). \
             This is not allowed.",
            key
        );
    }
    // Allowlisted BYOM/BYOK provider keys may carry an ADO **macro** expression
    // (e.g. `$(Setup.FOUNDRY_TOKEN)`) so credentials can be sourced from
    // Setup-job outputs or pipeline variables. Macros are the only expression
    // form ADO evaluates inside a step `env:` block. They may NOT carry:
    //   - template expressions `${{ }}` — expanded at compile time (secret
    //     exfiltration risk), and
    //   - runtime expressions `$[ ... ]` — ADO does *not* evaluate these inside
    //     step env; the literal string is passed verbatim (see ir::lower guard
    //     and #1076), so permitting them would silently ship a broken value.
    // Every other key remains literal-only.
    if is_provider_expr_env_key(key) {
        if contains_ado_template_expression(value) || value.contains("$[") {
            anyhow::bail!(
                "engine.env value for '{}' contains an ADO template ('${{{{ }}}}') or \
                 runtime ('$[...]') expression. Provider keys may use macro '$(...)' \
                 expressions only (ADO does not evaluate '${{{{ }}}}' or '$[...]' inside step env).",
                key
            );
        }
    } else if contains_ado_expression(value) {
        anyhow::bail!(
            "engine.env value for '{}' contains an ADO expression \
             (one of '$(...)', '${{{{ }}}}', or '$[...]'). \
             Use literal values only — ADO macro/expression expansion is not allowed \
             (allowed for BYOM provider keys: {}).",
            key,
            COPILOT_PROVIDER_EXPR_ENV_KEYS.join(", ")
        );
    }
    if contains_newline(value) {
        anyhow::bail!(
            "engine.env value for '{}' contains newline characters, \
             which would break YAML formatting.",
            key
        );
    }
    Ok(())
}

/// Validate a single `engine.env` entry and render it as a YAML `KEY: "VALUE"`
/// line for the aggregated agent env block (see [`copilot_env`]).
fn render_engine_env_entry(key: &str, value: &str) -> Result<String> {
    validate_engine_env_entry(key, value)?;
    // YAML-quote the value to prevent injection
    Ok(format!(
        "{}: \"{}\"",
        key,
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Generate Copilot CLI install steps for Azure DevOps pipelines.
///
/// Produces target-specific YAML:
/// - 1ES: authenticate with NuGet and install `Microsoft.Copilot.CLI.linux-x64`.
/// - Non-1ES: download Copilot CLI from GitHub Releases and verify SHA256.
///
/// Both paths stage the binary at `/tmp/awf-tools/copilot`.
///
/// `ado_org` is the ADO organization name inferred from the git remote at
/// compile time. For 1ES it is used to construct the NuGet feed URL; when
/// `None` a runtime extraction step is emitted that derives the org from
/// `$(System.CollectionUri)`.
fn copilot_install_steps(
    engine_config: &EngineConfig,
    target: &CompileTarget,
    ado_org: Option<&str>,
) -> Result<String> {
    // Custom binary path → skip NuGet install entirely
    if engine_config.command().is_some() {
        return Ok(String::new());
    }

    let version = engine_config.version().unwrap_or(COPILOT_CLI_VERSION);

    // Validate version to prevent injection — this value is used in NuGet
    // command arguments for 1ES and in GitHub Releases URL construction for
    // non-1ES targets.
    if !is_valid_version(version) {
        anyhow::bail!(
            "engine.version '{}' contains invalid characters. \
             Only ASCII alphanumerics, '.', '_', and '-' are allowed.",
            version
        );
    }

    if *target == CompileTarget::OneES {
        // "latest" means "install the newest available version" — NuGet doesn't
        // recognise "latest" as a version string; omitting -Version installs the newest.
        let version_arg = if version == "latest" {
            String::new()
        } else {
            format!("-Version {version} ")
        };

        // Build the NuGet feed URL using the org name.  When the org is known
        // at compile time (inferred from the git remote) it is embedded
        // directly.  When it is not available a preceding bash step extracts
        // the org at runtime from the $(System.CollectionUri) ADO variable and
        // exposes it as $(AW_ADO_ORG) for use in the NuGetCommand arguments.
        let (org_resolve_step, nuget_org) = match ado_org {
            Some(org) => {
                // Validate the org name against ADO organization naming rules to
                // prevent injection.  ADO org names are composed of ASCII
                // alphanumerics and hyphens only (no dots, no underscores).
                let org_valid =
                    !org.is_empty() && org.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
                if !org_valid {
                    anyhow::bail!(
                        "ADO organization '{}' contains invalid characters. \
                         Only ASCII alphanumerics and '-' are allowed.",
                        org
                    );
                }
                (String::new(), org.to_string())
            }
            None => {
                // Emit a bash step that extracts the org name from the ADO
                // system variable $(System.CollectionUri) at runtime and
                // stores it as a pipeline variable.
                //
                // $(System.CollectionUri) is expanded by ADO before bash runs
                // (e.g. "https://dev.azure.com/myorg/"); the parameter
                // expansions strip the prefix and trailing slash to yield just
                // the org name ("myorg").
                let step = "\
- bash: |
    set -eo pipefail
    # $(System.CollectionUri) is expanded by ADO before bash runs,
    # e.g. \"https://dev.azure.com/myorg/\".
    _COLLECTION_URI=\"$(System.CollectionUri)\"
    _ORG=\"${_COLLECTION_URI#https://dev.azure.com/}\"
    _ORG=\"${_ORG%/}\"
    echo \"##vso[task.setvariable variable=AW_ADO_ORG]$_ORG\"
  displayName: \"Resolve ADO organization\"

"
                .to_string();
                (step, "$(AW_ADO_ORG)".to_string())
            }
        };

        return Ok(format!(
            "\
{org_resolve_step}- task: NuGetAuthenticate@1
  displayName: \"Authenticate NuGet Feed\"

- task: NuGetCommand@2
  displayName: \"Install Copilot CLI\"
  inputs:
    command: 'custom'
    arguments: 'install Microsoft.Copilot.CLI.linux-x64 -Source \"https://pkgs.dev.azure.com/{nuget_org}/_packaging/Guardian1ESPTUpstreamOrgFeed/nuget/v3/index.json\" {version_arg}-OutputDirectory $(Agent.TempDirectory)/tools -ExcludeVersion -NonInteractive'

- bash: |
    ls -la \"$(Agent.TempDirectory)/tools\"
    echo \"##vso[task.prependpath]$(Agent.TempDirectory)/tools/Microsoft.Copilot.CLI.linux-x64\"

    # Copy copilot binary to /tmp so it's accessible inside AWF container
    # (AWF auto-mounts /tmp:/tmp:rw but not Agent.TempDirectory)
    mkdir -p /tmp/awf-tools
    cp \"$(Agent.TempDirectory)/tools/Microsoft.Copilot.CLI.linux-x64/copilot\" /tmp/awf-tools/copilot
    chmod +x /tmp/awf-tools/copilot
  displayName: \"Add copilot to PATH\"

- bash: |
    copilot --version
    copilot -h
  displayName: \"Output copilot version\""
        ));
    }

    if version == "latest" {
        return copilot_install_from_github_release(
            &format!("{COPILOT_CLI_RELEASES_BASE}/latest/download"),
            "Install Copilot CLI (latest)",
        );
    }

    let version_tag = normalize_version_tag(version);
    let base_url = format!("{COPILOT_CLI_RELEASES_BASE}/download/{version_tag}");
    copilot_install_from_github_release(&base_url, &format!("Install Copilot CLI ({version_tag})"))
}

fn normalize_version_tag(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

fn copilot_install_from_github_release(base_url: &str, display_name: &str) -> Result<String> {
    Ok(format!(
        "\
- bash: |
    set -euo pipefail
    TARBALL_NAME=\"copilot-linux-x64.tar.gz\"
    BASE_URL=\"{base_url}\"
    TARBALL_URL=\"$BASE_URL/$TARBALL_NAME\"
    CHECKSUMS_URL=\"$BASE_URL/SHA256SUMS.txt\"
    TOOLS_DIR=\"$(Agent.TempDirectory)/tools\"
    TEMP_DIR=\"$(mktemp -d)\"
    trap 'rm -rf \"$TEMP_DIR\"' EXIT
    mkdir -p \"$TOOLS_DIR\" /tmp/awf-tools

    curl -fsSL --retry 3 --retry-delay 5 -o \"$TEMP_DIR/SHA256SUMS.txt\" \"$CHECKSUMS_URL\"
    curl -fsSL --retry 3 --retry-delay 5 -o \"$TEMP_DIR/$TARBALL_NAME\" \"$TARBALL_URL\"

    EXPECTED_CHECKSUM=$(awk -v fname=\"$TARBALL_NAME\" '$2 == fname {{print $1; exit}}' \"$TEMP_DIR/SHA256SUMS.txt\" | tr 'A-F' 'a-f')
    if [ -z \"$EXPECTED_CHECKSUM\" ]; then
      echo \"ERROR: failed to resolve expected checksum for $TARBALL_NAME\"
      exit 1
    fi

    if command -v sha256sum > /dev/null 2>&1; then
      ACTUAL_CHECKSUM=$(sha256sum \"$TEMP_DIR/$TARBALL_NAME\" | awk '{{print $1}}' | tr 'A-F' 'a-f')
    elif command -v shasum > /dev/null 2>&1; then
      ACTUAL_CHECKSUM=$(shasum -a 256 \"$TEMP_DIR/$TARBALL_NAME\" | awk '{{print $1}}' | tr 'A-F' 'a-f')
    else
      echo \"ERROR: neither sha256sum nor shasum is available\"
      exit 1
    fi

    if [ \"$EXPECTED_CHECKSUM\" != \"$ACTUAL_CHECKSUM\" ]; then
      echo \"ERROR: checksum verification failed\"
      echo \"Expected: $EXPECTED_CHECKSUM\"
      echo \"Actual:   $ACTUAL_CHECKSUM\"
      exit 1
    fi

    tar -xz -C \"$TOOLS_DIR\" -f \"$TEMP_DIR/$TARBALL_NAME\"
    ls -la \"$TOOLS_DIR\"
    echo \"##vso[task.prependpath]$TOOLS_DIR\"
    cp \"$TOOLS_DIR/copilot\" /tmp/awf-tools/copilot
    chmod +x /tmp/awf-tools/copilot
  displayName: \"{display_name}\"

- bash: |
    copilot --version
    copilot -h
  displayName: \"Output copilot version\""
    ))
}

/// Build the full AWF `--` command string for the Copilot CLI.
///
/// The returned string goes inside `-- '...'` in the pipeline YAML.
fn copilot_invocation(
    command_path: &str,
    prompt_path: &str,
    mcp_config_path: Option<&str>,
    args: &str,
) -> String {
    let mut parts = vec![
        command_path.to_string(),
        format!("--prompt \"$(cat {prompt_path})\""),
    ];

    if let Some(mcp_path) = mcp_config_path {
        parts.push(format!("--additional-mcp-config @{mcp_path}"));
    }

    if !args.is_empty() {
        parts.push(args.to_string());
    }

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        Engine, GITHUB_APP_TOKEN_VAR, copilot_byom_active, copilot_byom_credential_keys,
        copilot_provider_env, get_engine, github_app_token_secrecy_advisory,
        github_token_source_var, normalize_version_tag, validate_engine_feature_support,
    };
    use crate::compile::{
        extensions::{CompileContext, CompilerExtension, Declarations, collect_extensions},
        parse_markdown,
    };

    fn declarations_for(fm: &crate::compile::types::FrontMatter) -> Vec<Declarations> {
        let extensions = collect_extensions(fm);
        let ctx = CompileContext::for_test(fm);
        extensions
            .iter()
            .map(|ext| ext.declarations(&ctx).unwrap())
            .collect()
    }

    #[test]
    fn copilot_engine_command() {
        assert_eq!(Engine::Copilot.command(), "copilot");
    }

    #[test]
    fn copilot_engine_args() {
        let (front_matter, _) =
            parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let params = Engine::Copilot
            .args(&front_matter, &declarations_for(&front_matter))
            .unwrap();
        // Default engine (copilot) uses default model (claude-opus-4.7)
        assert!(params.contains("--model claude-opus-4.7"));
        assert!(params.contains("--disable-builtin-mcps"));
    }

    #[test]
    fn copilot_engine_with_explicit_model() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  model: gpt-5\n---\n",
        )
        .unwrap();
        let params = Engine::Copilot
            .args(&front_matter, &declarations_for(&front_matter))
            .unwrap();
        assert!(params.contains("--model gpt-5"));
    }

    #[test]
    fn copilot_engine_env() {
        let (front_matter, _) =
            parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let env = Engine::Copilot.env(&front_matter.engine).unwrap();
        assert!(env.contains("GITHUB_TOKEN: $(GITHUB_TOKEN)"));
        assert!(env.contains("GITHUB_READ_ONLY: 1"));
        assert!(env.contains("COPILOT_OTEL_ENABLED"));
        assert!(!env.contains("SYSTEM_ACCESSTOKEN"));
        assert!(!env.contains("AZURE_DEVOPS_EXT_PAT"));
    }

    #[test]
    fn copilot_engine_env_sources_github_token_from_app_token_var_when_configured() {
        let src = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  \
                   github-app-token:\n    app-id: GH_APP_ID\n    private-key: GH_APP_KEY\n    \
                   owner: octo-org\n---\n";
        let (front_matter, _) = parse_markdown(src).unwrap();
        let env = Engine::Copilot.env(&front_matter.engine).unwrap();
        assert!(
            env.contains("GITHUB_TOKEN: $(GITHUB_APP_TOKEN)"),
            "expected GITHUB_APP_TOKEN source, got:\n{env}"
        );
        assert!(!env.contains("GITHUB_TOKEN: $(GITHUB_TOKEN)"));
    }

    #[test]
    fn github_token_source_var_switches_on_config() {
        let (default_fm, _) =
            parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        assert_eq!(github_token_source_var(&default_fm.engine), "GITHUB_TOKEN");

        let src = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  \
                   github-app-token:\n    app-id: GH_APP_ID\n    private-key: GH_APP_KEY\n    \
                   owner: octo-org\n---\n";
        let (app_fm, _) = parse_markdown(src).unwrap();
        assert_eq!(github_token_source_var(&app_fm.engine), GITHUB_APP_TOKEN_VAR);
    }

    #[test]
    fn github_app_token_secrecy_advisory_fires_only_when_configured() {
        let (default_fm, _) =
            parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        assert!(github_app_token_secrecy_advisory(&default_fm.engine).is_none());

        let src = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  \
                   github-app-token:\n    app-id: 1234567\n    owner: octo-org\n---\n";
        let (app_fm, _) = parse_markdown(src).unwrap();
        let advisory = github_app_token_secrecy_advisory(&app_fm.engine)
            .expect("advisory present when github-app-token configured");
        // Names the effective (default) variable and points at secrecy.
        assert!(advisory.contains("GITHUB_APP_PRIVATE_KEY"), "advisory: {advisory}");
        assert!(advisory.contains("SECRET"), "advisory: {advisory}");
    }

    #[test]
    fn validate_engine_feature_support_rejects_github_app_token_on_non_copilot() {
        // github-app-token on a non-copilot engine is a hard error (gh-aw
        // pattern for engine-gated config), preventing a silent no-op.
        let src = "---\nname: test\ndescription: test\nengine:\n  id: claude\n  \
                   github-app-token:\n    app-id: 1234567\n    owner: octo-org\n---\n";
        let (fm, _) = parse_markdown(src).unwrap();
        let err = validate_engine_feature_support(&fm.engine).unwrap_err().to_string();
        assert!(err.contains("github-app-token"), "err: {err}");
        assert!(err.contains("copilot"), "err: {err}");
    }

    #[test]
    fn validate_engine_feature_support_allows_copilot_and_absent() {
        // Copilot + github-app-token: ok.
        let src = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  \
                   github-app-token:\n    app-id: 1234567\n    owner: octo-org\n---\n";
        let (fm, _) = parse_markdown(src).unwrap();
        validate_engine_feature_support(&fm.engine).expect("copilot + app-token is valid");

        // No github-app-token: ok regardless of engine.
        let (default_fm, _) =
            parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        validate_engine_feature_support(&default_fm.engine).expect("absent config is valid");
    }

    #[test]
    fn get_engine_resolves_copilot() {
        let engine = get_engine("copilot").unwrap();
        assert_eq!(engine.command(), "copilot");
        let (front_matter, _) =
            parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let params = engine
            .args(&front_matter, &declarations_for(&front_matter))
            .unwrap();
        assert!(params.contains("--model claude-opus-4.7"));
    }

    #[test]
    fn get_engine_rejects_unsupported() {
        let result = get_engine("claude");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported engine 'claude'"));
    }

    // ─── engine.command tests ─────────────────────────────────────────────

    #[test]
    fn engine_command_overrides_binary_path() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  command: /usr/local/bin/my-copilot\n---\n",
        ).unwrap();
        let result = Engine::Copilot
            .invocation(
                &fm,
                &declarations_for(&fm),
                "/tmp/prompt.md",
                Some("/tmp/mcp.json"),
            )
            .unwrap();
        assert!(result.starts_with("/usr/local/bin/my-copilot "));
        assert!(!result.contains("/tmp/awf-tools/copilot"));
    }

    #[test]
    fn engine_command_default_uses_awf_path() {
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let result = Engine::Copilot
            .invocation(
                &fm,
                &declarations_for(&fm),
                "/tmp/prompt.md",
                Some("/tmp/mcp.json"),
            )
            .unwrap();
        assert!(result.starts_with("/tmp/awf-tools/copilot "));
    }

    #[test]
    fn engine_command_rejects_shell_metacharacters() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  command: \"/tmp/copilot; rm -rf /\"\n---\n",
        ).unwrap();
        let result =
            Engine::Copilot.invocation(&fm, &declarations_for(&fm), "/tmp/prompt.md", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[test]
    fn engine_command_rejects_single_quotes() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  command: \"/tmp/co'pilot\"\n---\n",
        ).unwrap();
        let result =
            Engine::Copilot.invocation(&fm, &declarations_for(&fm), "/tmp/prompt.md", None);
        assert!(result.is_err());
    }

    // ─── engine.agent tests ───────────────────────────────────────────────

    #[test]
    fn engine_agent_adds_flag() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  agent: my-custom-agent\n---\n",
        ).unwrap();
        let params = Engine::Copilot.args(&fm, &declarations_for(&fm)).unwrap();
        assert!(params.contains("--agent my-custom-agent"));
    }

    #[test]
    fn engine_agent_validates_identifier() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  agent: \"bad agent!\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &declarations_for(&fm));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    // ─── engine.api-target tests ──────────────────────────────────────────

    #[test]
    fn engine_api_target_adds_flag() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  api-target: api.acme.ghe.com\n---\n",
        ).unwrap();
        let params = Engine::Copilot.args(&fm, &declarations_for(&fm)).unwrap();
        assert!(params.contains("--api-target api.acme.ghe.com"));
    }

    #[test]
    fn engine_api_target_validates_hostname() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  api-target: \"bad host/path\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &declarations_for(&fm));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[test]
    fn engine_api_target_adds_to_required_hosts() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  api-target: api.acme.ghe.com\n---\n",
        ).unwrap();
        let hosts = Engine::Copilot.required_hosts(&fm.engine);
        assert_eq!(hosts, vec!["api.acme.ghe.com"]);
    }

    #[test]
    fn engine_no_api_target_no_required_hosts() {
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let hosts = Engine::Copilot.required_hosts(&fm.engine);
        assert!(hosts.is_empty());
    }

    // ─── engine.args tests ────────────────────────────────────────────────

    #[test]
    fn engine_args_appended_after_compiler_args() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - --verbose\n    - --debug\n---\n",
        ).unwrap();
        let params = Engine::Copilot.args(&fm, &declarations_for(&fm)).unwrap();
        // Compiler args come first
        assert!(params.contains("--disable-builtin-mcps"));
        assert!(params.contains("--no-ask-user"));
        // User args come after
        let disable_pos = params.find("--disable-builtin-mcps").unwrap();
        let verbose_pos = params.find("--verbose").unwrap();
        assert!(
            verbose_pos > disable_pos,
            "User args must come after compiler args"
        );
        assert!(params.contains("--debug"));
    }

    #[test]
    fn engine_args_rejects_shell_injection() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - \"--flag; rm -rf /\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &declarations_for(&fm));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[test]
    fn engine_args_blocks_prompt_override() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - --prompt=evil\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &declarations_for(&fm));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("compiler-controlled")
        );
    }

    #[test]
    fn engine_args_blocks_allow_tool() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - --allow-tool=evil\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &declarations_for(&fm));
        assert!(result.is_err());
    }

    #[test]
    fn engine_args_blocks_ask_user() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - --ask-user\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &declarations_for(&fm));
        assert!(result.is_err());
    }

    #[test]
    fn engine_args_blocks_additional_mcp_config() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - --additional-mcp-config=@evil.json\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &declarations_for(&fm));
        assert!(result.is_err());
    }

    // ─── engine.env tests ─────────────────────────────────────────────────

    #[test]
    fn engine_env_merges_user_vars() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: hello\n---\n",
        ).unwrap();
        let env = Engine::Copilot.env(&fm.engine).unwrap();
        assert!(
            env.contains("GITHUB_TOKEN: $(GITHUB_TOKEN)"),
            "compiler vars preserved"
        );
        assert!(env.contains("MY_VAR: \"hello\""), "user var included");
    }

    #[test]
    fn engine_env_blocks_github_token() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    GITHUB_TOKEN: evil\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("compiler-controlled")
        );
    }

    #[test]
    fn engine_env_blocks_path() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    PATH: /evil\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
    }

    #[test]
    fn engine_env_blocks_bash_env() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    BASH_ENV: /evil\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
    }

    #[test]
    fn engine_env_blocks_ld_preload() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    LD_PRELOAD: /evil.so\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
    }

    #[test]
    fn engine_env_rejects_vso_injection() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: \"##vso[task.setvariable]evil\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("ADO pipeline command injection")
        );
    }

    #[test]
    fn engine_env_rejects_ado_expressions() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: \"$(SYSTEM_ACCESSTOKEN)\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("contains an ADO expression")
        );
    }

    #[test]
    fn engine_env_allows_macro_on_provider_key() {
        // BYOM/BYOK: provider keys may carry ADO macro expressions sourced from
        // Setup-job outputs.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BEARER_TOKEN: \"$(Setup.FOUNDRY_TOKEN)\"\n---\n",
        ).unwrap();
        let env = Engine::Copilot.env(&fm.engine).unwrap();
        assert!(
            env.contains(r#"COPILOT_PROVIDER_BEARER_TOKEN: "$(Setup.FOUNDRY_TOKEN)""#),
            "provider key macro expression should be emitted verbatim: {env}"
        );
    }

    #[test]
    fn engine_env_rejects_runtime_expr_on_provider_key() {
        // ADO runtime expressions `$[ ... ]` are NOT evaluated inside step env
        // (passed verbatim, see #1076), so they are rejected even on provider
        // keys — only macros `$(...)` are permitted.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_API_KEY: \"$[ variables.Key ]\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("runtime ('$[...]') expression")
        );
    }

    #[test]
    fn copilot_byom_active_detects_credential_keys() {
        for key in [
            "COPILOT_PROVIDER_BASE_URL",
            "COPILOT_PROVIDER_API_KEY",
            "COPILOT_PROVIDER_BEARER_TOKEN",
        ] {
            let md = format!(
                "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    {key}: value\n---\n"
            );
            let (fm, _) = parse_markdown(&md).unwrap();
            assert!(
                copilot_byom_active(&fm.engine),
                "BYOM should be active when {key} is present"
            );
        }
    }

    #[test]
    fn copilot_byom_inactive_without_credential_keys() {
        // No env at all.
        let (fm, _) =
            parse_markdown("---\nname: test\ndescription: test\nengine:\n  id: copilot\n---\n")
                .unwrap();
        assert!(!copilot_byom_active(&fm.engine));

        // Only the non-credential WIRE_API config key and an unrelated var.
        let (fm2, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_WIRE_API: responses\n    MY_VAR: hi\n---\n",
        ).unwrap();
        assert!(
            !copilot_byom_active(&fm2.engine),
            "WIRE_API alone must not activate BYOM (it is non-credential config)"
        );
    }

    #[test]
    fn copilot_byom_credential_keys_returns_present_keys() {
        // Only the credential keys present are returned (sorted); the
        // non-credential WIRE_API and unrelated vars are excluded.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: https://x/y\n    COPILOT_PROVIDER_WIRE_API: responses\n    MY_VAR: hi\n---\n",
        ).unwrap();
        let keys = copilot_byom_credential_keys(&fm.engine);
        assert_eq!(keys, vec!["COPILOT_PROVIDER_BASE_URL".to_string()]);
    }

    #[test]
    fn provider_key_matching_is_case_sensitive() {
        // A lowercase provider key is NOT a real Copilot provider var (the CLI
        // reads uppercase). It must not activate BYOM, must not appear in the
        // exclude list, and — carrying an expression — must be rejected as a
        // normal literal-only key (fail-closed, no silent runtime break).
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    copilot_provider_api_key: literal-value\n---\n",
        ).unwrap();
        assert!(
            !copilot_byom_active(&fm.engine),
            "lowercase provider key must not activate BYOM"
        );
        assert!(
            copilot_byom_credential_keys(&fm.engine).is_empty(),
            "lowercase provider key must not appear in the exclude list"
        );

        let (fm_expr, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    copilot_provider_api_key: \"$(Setup.Key)\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm_expr.engine);
        assert!(
            result.is_err(),
            "lowercase provider key carrying an expression must be rejected (not silently allowed)"
        );
    }

    #[test]
    fn copilot_byom_credential_keys_empty_without_provider() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: hi\n---\n",
        ).unwrap();
        assert!(copilot_byom_credential_keys(&fm.engine).is_empty());
    }

    #[test]
    fn engine_expression_base_url_host_rejects_non_dns_safe() {
        // An IPv6 literal parses to a host but is not DNS-safe, so it must not
        // be added to the AWF allow-domains list (consistent with api-target).
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: \"http://[::1]:8080/v1\"\n---\n",
        ).unwrap();
        let hosts = Engine::Copilot.required_hosts(&fm.engine);
        assert!(
            hosts.is_empty(),
            "non-DNS-safe host (IPv6 literal) must be rejected from the allowlist: {hosts:?}"
        );
    }

    #[test]
    fn copilot_provider_env_returns_only_provider_keys() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_TYPE: azure\n    COPILOT_PROVIDER_BEARER_TOKEN: \"$(Setup.FOUNDRY_TOKEN)\"\n    MY_VAR: keep-out\n---\n",
        ).unwrap();
        let pairs = copilot_provider_env(&fm.engine).unwrap();
        // Validated raw (key, value) pairs, sorted by key; non-provider and
        // compiler-managed base env excluded.
        assert_eq!(
            pairs,
            vec![
                (
                    "COPILOT_PROVIDER_BEARER_TOKEN".to_string(),
                    "$(Setup.FOUNDRY_TOKEN)".to_string()
                ),
                ("COPILOT_PROVIDER_TYPE".to_string(), "azure".to_string()),
            ]
        );
    }

    #[test]
    fn copilot_provider_env_empty_without_provider_keys() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: hi\n---\n",
        ).unwrap();
        assert!(copilot_provider_env(&fm.engine).unwrap().is_empty());
    }

    #[test]
    fn engine_env_provider_key_rejects_template_expression() {
        // Compile-time template expressions remain forbidden even on provider keys.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_API_KEY: \"${{ variables.Key }}\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("ADO template ('${{ }}') or")
        );
    }

    #[test]
    fn engine_env_provider_key_rejects_vso_injection() {
        // Pipeline-command injection is still blocked on provider keys.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: \"##vso[task.setvariable]evil\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("ADO pipeline command injection")
        );
    }

    #[test]
    fn engine_env_non_provider_key_still_rejects_macro() {
        // A COPILOT_PROVIDER_* lookalike that is not on the allowlist is rejected.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_TYPE: \"$(Setup.Type)\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("contains an ADO expression")
        );
    }

    #[test]
    fn engine_env_literal_base_url_adds_required_host() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: https://my-foundry.cognitiveservices.azure.com/openai/v1\n---\n",
        ).unwrap();
        let hosts = Engine::Copilot.required_hosts(&fm.engine);
        assert!(
            hosts.contains(&"my-foundry.cognitiveservices.azure.com".to_string()),
            "literal base URL host should be added to allowlist: {hosts:?}"
        );
    }

    #[test]
    fn engine_env_expression_base_url_skips_required_host() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: \"$(Setup.BASE_URL)\"\n---\n",
        ).unwrap();
        let hosts = Engine::Copilot.required_hosts(&fm.engine);
        assert!(
            hosts.is_empty(),
            "expression-valued base URL must not contribute a host: {hosts:?}"
        );
    }

    #[test]
    fn engine_env_malformed_base_url_warns() {
        // A literal but scheme-less / unparseable URL must not silently drop:
        // required_hosts adds nothing AND a warning is surfaced.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: my-foundry/openai/v1\n---\n",
        ).unwrap();
        assert!(
            Engine::Copilot.required_hosts(&fm.engine).is_empty(),
            "malformed base URL must not contribute a host"
        );
        let warnings = Engine::Copilot.network_host_warnings(&fm.engine);
        assert_eq!(warnings.len(), 1, "expected one warning: {warnings:?}");
        assert!(
            warnings[0].contains("COPILOT_PROVIDER_BASE_URL")
                && warnings[0].contains("my-foundry/openai/v1")
                && warnings[0].contains("network.allowed"),
            "warning must name the key, the bad value, and the network.allowed fix: {warnings:?}"
        );
    }

    #[test]
    fn engine_env_ipv6_base_url_warns() {
        // Non-DNS-safe host (IPv6 literal) is also surfaced, not silently dropped.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: \"http://[::1]:8080/v1\"\n---\n",
        ).unwrap();
        assert!(Engine::Copilot.required_hosts(&fm.engine).is_empty());
        assert_eq!(Engine::Copilot.network_host_warnings(&fm.engine).len(), 1);
    }

    #[test]
    fn engine_env_no_warning_for_valid_or_expression_or_absent_base_url() {
        // Valid literal → host added, no warning.
        let (fm_ok, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: https://x.example.com/v1\n---\n",
        ).unwrap();
        assert!(Engine::Copilot.network_host_warnings(&fm_ok.engine).is_empty());

        // Expression-valued → documented manual path, no warning.
        let (fm_expr, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: \"$(Setup.BASE_URL)\"\n---\n",
        ).unwrap();
        assert!(Engine::Copilot.network_host_warnings(&fm_expr.engine).is_empty());

        // Absent → no warning.
        let (fm_none, _) =
            parse_markdown("---\nname: test\ndescription: test\nengine:\n  id: copilot\n---\n")
                .unwrap();
        assert!(Engine::Copilot.network_host_warnings(&fm_none.engine).is_empty());
    }

    #[test]
    fn engine_env_rejects_newlines() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: \"line1\\nline2\"\n---\n",
        ).unwrap();
        // YAML double-quoted strings interpret \n as an actual newline
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("newline characters")
        );
    }

    #[test]
    fn engine_env_rejects_invalid_key() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    \"123bad\": value\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not a valid environment variable name")
        );
    }

    #[test]
    fn engine_env_escapes_quotes_in_values() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: 'has \"quotes\"'\n---\n",
        ).unwrap();
        let env = Engine::Copilot.env(&fm.engine).unwrap();
        assert!(env.contains(r#"MY_VAR: "has \"quotes\"""#));
    }

    // ─── engine.version validation tests ──────────────────────────────────

    #[test]
    fn engine_version_rejects_injection() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: '1.0.0 -Source https://evil.com'\n---\n",
        ).unwrap();
        let result = Engine::Copilot.install_steps(&fm.engine, &fm.target, None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[test]
    fn engine_version_rejects_single_quotes() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: \"1.0.0'\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.install_steps(&fm.engine, &fm.target, None);
        assert!(result.is_err());
    }

    #[test]
    fn engine_version_accepts_valid() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: '1.0.34'\n---\n",
        ).unwrap();
        let result = Engine::Copilot
            .install_steps(&fm.engine, &fm.target, None)
            .unwrap();
        assert!(result.contains("releases/download/v1.0.34"));
        assert!(result.contains("Install Copilot CLI (v1.0.34)"));
    }

    #[test]
    fn engine_version_accepts_valid_with_v_prefix() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: 'v1.0.34'\n---\n",
        ).unwrap();
        let result = Engine::Copilot
            .install_steps(&fm.engine, &fm.target, None)
            .unwrap();
        assert!(result.contains("releases/download/v1.0.34"));
        assert!(result.contains("Install Copilot CLI (v1.0.34)"));
    }

    #[test]
    fn engine_version_accepts_latest() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: latest\n---\n",
        )
        .unwrap();
        let result = Engine::Copilot
            .install_steps(&fm.engine, &fm.target, None)
            .unwrap();
        assert!(
            result.contains("releases/latest/download"),
            "latest should resolve via latest release URL"
        );
        assert!(result.contains("Install Copilot CLI (latest)"));
    }

    #[test]
    fn engine_install_onees_latest_omits_version_argument() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntarget: 1es\nengine:\n  id: copilot\n  version: latest\n---\n",
        ).unwrap();
        let result = Engine::Copilot
            .install_steps(&fm.engine, &fm.target, Some("myorg"))
            .unwrap();
        assert!(result.contains("NuGetCommand@2"));
        assert!(result.contains("Guardian1ESPTUpstreamOrgFeed"));
        assert!(result.contains("pkgs.dev.azure.com/myorg/"));
        assert!(!result.contains("-Version latest"));
    }

    #[test]
    fn engine_install_onees_uses_nuget_feed() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntarget: 1es\nengine:\n  id: copilot\n  version: '1.0.34'\n---\n",
        ).unwrap();
        let result = Engine::Copilot
            .install_steps(&fm.engine, &fm.target, Some("myorg"))
            .unwrap();
        assert!(result.contains("NuGetCommand@2"));
        assert!(result.contains("Guardian1ESPTUpstreamOrgFeed"));
        assert!(result.contains("pkgs.dev.azure.com/myorg/"));
        assert!(result.contains("-Version 1.0.34"));
    }

    #[test]
    fn engine_install_onees_uses_user_org_not_msazuresphere() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntarget: 1es\nengine:\n  id: copilot\n  version: '1.0.34'\n---\n",
        ).unwrap();
        let result = Engine::Copilot
            .install_steps(&fm.engine, &fm.target, Some("contoso"))
            .unwrap();
        assert!(result.contains("pkgs.dev.azure.com/contoso/"));
        assert!(!result.contains("msazuresphere"));
    }

    #[test]
    fn engine_install_onees_runtime_fallback_when_org_unknown() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntarget: 1es\nengine:\n  id: copilot\n  version: '1.0.34'\n---\n",
        ).unwrap();
        let result = Engine::Copilot
            .install_steps(&fm.engine, &fm.target, None)
            .unwrap();
        assert!(result.contains("NuGetCommand@2"));
        assert!(result.contains("Guardian1ESPTUpstreamOrgFeed"));
        // Runtime fallback: org extracted from $(System.CollectionUri)
        assert!(result.contains("$(AW_ADO_ORG)"));
        assert!(result.contains("$(System.CollectionUri)"));
        assert!(result.contains("Resolve ADO organization"));
        assert!(!result.contains("msazuresphere"));
    }

    #[test]
    fn engine_install_onees_rejects_invalid_org() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntarget: 1es\nengine:\n  id: copilot\n  version: '1.0.34'\n---\n",
        ).unwrap();
        let result = Engine::Copilot.install_steps(&fm.engine, &fm.target, Some("evil; rm -rf /"));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[test]
    fn normalize_version_tag_does_not_double_prefix_v() {
        assert_eq!(normalize_version_tag("v1.0.34"), "v1.0.34");
        assert_eq!(normalize_version_tag("1.0.34"), "v1.0.34");
    }

    // ─── engine.env empty key test ────────────────────────────────────────

    #[test]
    fn engine_env_rejects_empty_key() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    \"\": value\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty key"));
    }

    // ─── engine.provider (BYOK) block, #1372 ──────────────────────────────

    const PROVIDER_MD: &str = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: https://my-foundry.cognitiveservices.azure.com/openai/v1\n    type: azure\n    token:\n      service-connection: my-arm-connection\n---\n";

    #[test]
    fn provider_block_maps_to_copilot_provider_env() {
        let (fm, _) = parse_markdown(PROVIDER_MD).unwrap();
        let env = Engine::Copilot.env(&fm.engine).unwrap();
        assert!(
            env.contains(
                r#"COPILOT_PROVIDER_BASE_URL: "https://my-foundry.cognitiveservices.azure.com/openai/v1""#
            ),
            "provider.base-url must map to COPILOT_PROVIDER_BASE_URL: {env}"
        );
        assert!(
            env.contains(r#"COPILOT_PROVIDER_TYPE: "azure""#),
            "provider.type must map to COPILOT_PROVIDER_TYPE: {env}"
        );
        // token → same-job mint var, wired into COPILOT_PROVIDER_API_KEY (the
        // credential env var the AWF sidecar reads). Rendered here as a quoted
        // macro; the final lock re-serializes it unquoted via the typed EnvValue
        // path.
        assert!(
            env.contains(r#"COPILOT_PROVIDER_API_KEY: "$(AW_PROVIDER_BEARER_TOKEN)""#),
            "provider.token must wire the minted token to COPILOT_PROVIDER_API_KEY: {env}"
        );
        assert!(
            !env.contains("COPILOT_PROVIDER_BEARER_TOKEN"),
            "the token must NOT be plumbed as COPILOT_PROVIDER_BEARER_TOKEN (sidecar ignores it): {env}"
        );
    }

    #[test]
    fn provider_token_activates_byom() {
        let (fm, _) = parse_markdown(PROVIDER_MD).unwrap();
        assert!(
            copilot_byom_active(&fm.engine),
            "an engine.provider.token block must activate BYOM isolation"
        );
        // Only the credential keys (base-url + api-key) are excluded — not TYPE.
        assert_eq!(
            copilot_byom_credential_keys(&fm.engine),
            vec![
                "COPILOT_PROVIDER_API_KEY".to_string(),
                "COPILOT_PROVIDER_BASE_URL".to_string(),
            ]
        );
    }

    #[test]
    fn provider_detection_env_inherits_routing() {
        let (fm, _) = parse_markdown(PROVIDER_MD).unwrap();
        let pairs = copilot_provider_env(&fm.engine).unwrap();
        assert!(pairs.iter().any(|(k, v)| k == "COPILOT_PROVIDER_API_KEY"
            && v == "$(AW_PROVIDER_BEARER_TOKEN)"));
        assert!(pairs.iter().any(|(k, _)| k == "COPILOT_PROVIDER_BASE_URL"));
    }

    #[test]
    fn provider_conflicts_with_raw_env_provider_keys() {
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_TYPE: azure\n  provider:\n    base-url: https://x.example.com/v1\n    token:\n      service-connection: sc\n---\n";
        let (fm, _) = parse_markdown(md).unwrap();
        let err = validate_engine_feature_support(&fm.engine).unwrap_err().to_string();
        assert!(
            err.contains("must not also be declared in") && err.contains("COPILOT_PROVIDER_TYPE"),
            "provider + raw engine.env provider key must conflict: {err}"
        );
    }

    #[test]
    fn provider_rejects_token_and_api_key_together() {
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: https://x.example.com/v1\n    api-key: $(MY_KEY)\n    token:\n      service-connection: sc\n---\n";
        let (fm, _) = parse_markdown(md).unwrap();
        let err = validate_engine_feature_support(&fm.engine).unwrap_err().to_string();
        assert!(
            err.contains("mutually exclusive"),
            "token + api-key must be rejected: {err}"
        );
    }

    #[test]
    fn provider_requires_base_url() {
        // Empty base-url is rejected at deserialization by the ProviderBaseUrl
        // newtype.
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: \"\"\n    token:\n      service-connection: sc\n---\n";
        assert!(
            parse_markdown(md).is_err(),
            "an empty base-url must be rejected"
        );
    }

    #[test]
    fn provider_token_resource_rejects_shell_metacharacters() {
        // A resource with a shell-breakout payload must be rejected at
        // deserialization (the ProviderResourceUrl newtype), never reaching the
        // generated mint script.
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: https://x.example.com/v1\n    token:\n      service-connection: sc\n      resource: \"https://x));whoami\"\n---\n";
        assert!(
            parse_markdown(md).is_err(),
            "a resource containing shell metacharacters must be rejected"
        );
    }

    #[test]
    fn provider_base_url_rejects_non_https_scheme() {
        // The provider endpoint receives the bearer token, so plaintext HTTP
        // must be rejected — at deserialization by the ProviderBaseUrl newtype.
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: http://insecure.example.com/v1\n    token:\n      service-connection: sc\n---\n";
        assert!(
            parse_markdown(md).is_err(),
            "a plaintext http base-url must be rejected"
        );
    }

    #[test]
    fn provider_base_url_rejects_template_expression() {
        // A template expression on base-url would be expanded at ADO
        // template-compile time and bypass the AWF allowlist host check — the
        // ProviderBaseUrl newtype rejects it at deserialization.
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: \"${{ parameters.externalUrl }}\"\n    token:\n      service-connection: sc\n---\n";
        assert!(
            parse_markdown(md).is_err(),
            "a base-url with a template expression must be rejected"
        );
    }

    #[test]
    fn provider_base_url_accepts_macro() {
        // A macro-bearing base-url is accepted (concrete host unknown at compile
        // time; the author adds it to network.allowed).
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: \"$(FOUNDRY_BASE_URL)\"\n    token:\n      service-connection: sc\n---\n";
        let (fm, _) = parse_markdown(md).unwrap();
        assert!(validate_engine_feature_support(&fm.engine).is_ok());
    }

    #[test]
    fn provider_api_key_rejects_empty() {
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: https://x.example.com/v1\n    api-key: \"\"\n---\n";
        let (fm, _) = parse_markdown(md).unwrap();
        let err = validate_engine_feature_support(&fm.engine).unwrap_err().to_string();
        assert!(
            err.contains("api-key must not be empty"),
            "an empty api-key must be rejected (would send an empty credential): {err}"
        );
    }

    #[test]
    fn provider_api_key_rejects_injection() {
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: https://x.example.com/v1\n    api-key: \"##vso[task.setvariable variable=x]y\"\n---\n";
        let (fm, _) = parse_markdown(md).unwrap();
        let err = validate_engine_feature_support(&fm.engine).unwrap_err().to_string();
        assert!(
            err.contains("pipeline command injection"),
            "api-key with a ##vso injection must be rejected: {err}"
        );
    }

    #[test]
    fn provider_api_key_maps_without_mint() {
        let md = "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  provider:\n    base-url: https://x.example.com/v1\n    api-key: $(MY_KEY)\n---\n";
        let (fm, _) = parse_markdown(md).unwrap();
        let env = Engine::Copilot.env(&fm.engine).unwrap();
        assert!(env.contains(r#"COPILOT_PROVIDER_API_KEY: "$(MY_KEY)""#), "{env}");
        assert!(
            !env.contains("AW_PROVIDER_BEARER_TOKEN"),
            "api-key path must not reference the bearer mint var: {env}"
        );
    }
}
