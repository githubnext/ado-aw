use anyhow::Result;

use crate::compile::extensions::{CompilerExtension, Extension};
use crate::compile::types::{EngineConfig, FrontMatter, McpConfig};
use crate::validate::{
    contains_ado_expression, contains_newline, contains_pipeline_command, is_valid_arg,
    is_valid_command_path, is_valid_env_var_name, is_valid_hostname, is_valid_identifier,
    is_valid_version,
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
const BLOCKED_ENV_KEYS: &[&str] = &[
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

/// Default model used by the Copilot engine when no model is specified in front matter.
pub const DEFAULT_COPILOT_MODEL: &str = "claude-opus-4.7";

/// Default pinned version of the Copilot CLI NuGet package.
/// Override per-agent via `engine: { id: copilot, version: "1.0.35" }` in front matter.
pub const COPILOT_CLI_VERSION: &str = "1.0.36";

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
        extensions: &[Extension],
    ) -> Result<String> {
        match self {
            Engine::Copilot => copilot_args(front_matter, extensions),
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
            Engine::Copilot => "~/.copilot/logs",
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
                hosts
            }
        }
    }

    /// Generate pipeline YAML steps to install the engine binary.
    ///
    /// Uses `engine_config.version()` if set in front matter, otherwise falls back
    /// to the pinned `COPILOT_CLI_VERSION` constant. Returns an empty string when
    /// `engine.command` is set (the user provides their own binary).
    pub fn install_steps(&self, engine_config: &EngineConfig) -> Result<String> {
        match self {
            Engine::Copilot => copilot_install_steps(engine_config),
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
        extensions: &[Extension],
        prompt_path: &str,
        mcp_config_path: Option<&str>,
    ) -> Result<String> {
        let args = self.args(front_matter, extensions)?;
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
                Ok(copilot_invocation(&command_path, prompt_path, mcp_config_path, &args))
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
    extensions: &[Extension],
    edit_enabled: bool,
) -> Result<Vec<String>> {
    let mut allowed_tools: Vec<String> = Vec::new();

    // Tools from compiler extensions (github, safeoutputs, azure-devops, etc.)
    for ext in extensions {
        for tool in ext.allowed_copilot_tools() {
            if !allowed_tools.contains(&tool) {
                allowed_tools.push(tool);
            }
        }
    }

    // Tools from user-defined MCP servers (sorted for deterministic output).
    // Only add --allow-tool for MCPs that will actually produce an MCPG entry (i.e.,
    // WithOptions that have a container or url). McpConfig::Enabled(true) has no backing
    // server in MCPG, so granting the permission would cause confusing runtime errors.
    let mut sorted_mcps: Vec<_> = front_matter.mcp_servers.iter().collect();
    sorted_mcps.sort_by(|(a, _), (b, _)| a.cmp(b));
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
    for ext in extensions {
        for cmd in ext.required_bash_commands() {
            if !bash_commands.contains(&cmd) {
                bash_commands.push(cmd);
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
    extensions: &[Extension],
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
        collect_allowed_tools(front_matter, extensions, edit_enabled)?
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

fn copilot_env(engine_config: &EngineConfig) -> Result<String> {
    let mut lines: Vec<String> = vec![
        "GITHUB_TOKEN: $(GITHUB_TOKEN)".to_string(),
        "GITHUB_READ_ONLY: 1".to_string(),
        "COPILOT_OTEL_ENABLED: \"true\"".to_string(),
        "COPILOT_OTEL_EXPORTER_TYPE: \"file\"".to_string(),
        "COPILOT_OTEL_FILE_EXPORTER_PATH: \"/tmp/awf-tools/staging/otel.jsonl\"".to_string(),
    ];

    // Wire engine.env — merge user-provided environment variables
    if let Some(env_map) = engine_config.env() {
        let mut sorted_keys: Vec<&String> = env_map.keys().collect();
        sorted_keys.sort();

        for key in sorted_keys {
            let value = &env_map[key];

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
            if BLOCKED_ENV_KEYS.iter().any(|blocked| key.eq_ignore_ascii_case(blocked)) {
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
            if contains_ado_expression(value) {
                anyhow::bail!(
                    "engine.env value for '{}' contains ADO expression syntax ('$(' or '${{{{}}}}')). \
                     Use literal values only — ADO macro/expression expansion is not allowed.",
                    key
                );
            }
            if contains_newline(value) {
                anyhow::bail!(
                    "engine.env value for '{}' contains newline characters, \
                     which would break YAML formatting.",
                    key
                );
            }

            // YAML-quote the value to prevent injection
            lines.push(format!("{}: \"{}\"", key, value.replace('\\', "\\\\").replace('"', "\\\"")));
        }
    }

    Ok(lines.join("\n"))
}

/// Generate Copilot CLI install steps for Azure DevOps pipelines.
///
/// Produces the YAML block that authenticates with NuGet, installs the
/// `Microsoft.Copilot.CLI.linux-x64` package, copies the binary to
/// `/tmp/awf-tools/copilot`, and verifies the install.
fn copilot_install_steps(engine_config: &EngineConfig) -> Result<String> {
    // Custom binary path → skip NuGet install entirely
    if engine_config.command().is_some() {
        return Ok(String::new());
    }

    let version = engine_config
        .version()
        .unwrap_or(COPILOT_CLI_VERSION);

    // Validate version to prevent NuGet argument injection — the version string
    // is embedded directly into NuGet command arguments.
    if !is_valid_version(version) {
        anyhow::bail!(
            "engine.version '{}' contains invalid characters. \
             Only ASCII alphanumerics, '.', '_', and '-' are allowed.",
            version
        );
    }

    // "latest" means "install the newest available version" — NuGet doesn't
    // recognise "latest" as a version string; omitting -Version installs the newest.
    let version_arg = if version == "latest" {
        String::new()
    } else {
        format!("-Version {version} ")
    };

    Ok(format!(
        "\
- task: NuGetAuthenticate@1
  displayName: \"Authenticate NuGet Feed\"

- task: NuGetCommand@2
  displayName: \"Install Copilot CLI\"
  inputs:
    command: 'custom'
    arguments: 'install Microsoft.Copilot.CLI.linux-x64 -Source \"https://pkgs.dev.azure.com/msazuresphere/_packaging/Guardian1ESPTUpstreamOrgFeed/nuget/v3/index.json\" {version_arg}-OutputDirectory $(Agent.TempDirectory)/tools -ExcludeVersion -NonInteractive'

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
    use super::{get_engine, Engine};
    use crate::compile::{extensions::collect_extensions, parse_markdown};

    #[test]
    fn copilot_engine_command() {
        assert_eq!(Engine::Copilot.command(), "copilot");
    }

    #[test]
    fn copilot_engine_args() {
        let (front_matter, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let params = Engine::Copilot
            .args(&front_matter, &collect_extensions(&front_matter))
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
            .args(&front_matter, &collect_extensions(&front_matter))
            .unwrap();
        assert!(params.contains("--model gpt-5"));
    }

    #[test]
    fn copilot_engine_env() {
        let (front_matter, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let env = Engine::Copilot.env(&front_matter.engine).unwrap();
        assert!(env.contains("GITHUB_TOKEN: $(GITHUB_TOKEN)"));
        assert!(env.contains("GITHUB_READ_ONLY: 1"));
        assert!(env.contains("COPILOT_OTEL_ENABLED"));
        assert!(!env.contains("SYSTEM_ACCESSTOKEN"));
        assert!(!env.contains("AZURE_DEVOPS_EXT_PAT"));
    }

    #[test]
    fn get_engine_resolves_copilot() {
        let engine = get_engine("copilot").unwrap();
        assert_eq!(engine.command(), "copilot");
        let (front_matter, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let params = engine
            .args(&front_matter, &collect_extensions(&front_matter))
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

    #[test]
    fn get_engine_rejects_codex() {
        assert!(get_engine("codex").is_err());
    }

    // ─── engine.command tests ─────────────────────────────────────────────

    #[test]
    fn engine_command_overrides_binary_path() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  command: /usr/local/bin/my-copilot\n---\n",
        ).unwrap();
        let result = Engine::Copilot
            .invocation(&fm, &collect_extensions(&fm), "/tmp/prompt.md", Some("/tmp/mcp.json"))
            .unwrap();
        assert!(result.starts_with("/usr/local/bin/my-copilot "));
        assert!(!result.contains("/tmp/awf-tools/copilot"));
    }

    #[test]
    fn engine_command_default_uses_awf_path() {
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let result = Engine::Copilot
            .invocation(&fm, &collect_extensions(&fm), "/tmp/prompt.md", Some("/tmp/mcp.json"))
            .unwrap();
        assert!(result.starts_with("/tmp/awf-tools/copilot "));
    }

    #[test]
    fn engine_command_rejects_shell_metacharacters() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  command: \"/tmp/copilot; rm -rf /\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.invocation(&fm, &collect_extensions(&fm), "/tmp/prompt.md", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }

    #[test]
    fn engine_command_rejects_single_quotes() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  command: \"/tmp/co'pilot\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.invocation(&fm, &collect_extensions(&fm), "/tmp/prompt.md", None);
        assert!(result.is_err());
    }

    // ─── engine.agent tests ───────────────────────────────────────────────

    #[test]
    fn engine_agent_adds_flag() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  agent: my-custom-agent\n---\n",
        ).unwrap();
        let params = Engine::Copilot.args(&fm, &collect_extensions(&fm)).unwrap();
        assert!(params.contains("--agent my-custom-agent"));
    }

    #[test]
    fn engine_agent_validates_identifier() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  agent: \"bad agent!\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &collect_extensions(&fm));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }

    // ─── engine.api-target tests ──────────────────────────────────────────

    #[test]
    fn engine_api_target_adds_flag() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  api-target: api.acme.ghe.com\n---\n",
        ).unwrap();
        let params = Engine::Copilot.args(&fm, &collect_extensions(&fm)).unwrap();
        assert!(params.contains("--api-target api.acme.ghe.com"));
    }

    #[test]
    fn engine_api_target_validates_hostname() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  api-target: \"bad host/path\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &collect_extensions(&fm));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
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
        let params = Engine::Copilot.args(&fm, &collect_extensions(&fm)).unwrap();
        // Compiler args come first
        assert!(params.contains("--disable-builtin-mcps"));
        assert!(params.contains("--no-ask-user"));
        // User args come after
        let disable_pos = params.find("--disable-builtin-mcps").unwrap();
        let verbose_pos = params.find("--verbose").unwrap();
        assert!(verbose_pos > disable_pos, "User args must come after compiler args");
        assert!(params.contains("--debug"));
    }

    #[test]
    fn engine_args_rejects_shell_injection() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - \"--flag; rm -rf /\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &collect_extensions(&fm));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }

    #[test]
    fn engine_args_blocks_prompt_override() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - --prompt=evil\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &collect_extensions(&fm));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("compiler-controlled"));
    }

    #[test]
    fn engine_args_blocks_allow_tool() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - --allow-tool=evil\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &collect_extensions(&fm));
        assert!(result.is_err());
    }

    #[test]
    fn engine_args_blocks_ask_user() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - --ask-user\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &collect_extensions(&fm));
        assert!(result.is_err());
    }

    #[test]
    fn engine_args_blocks_additional_mcp_config() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  args:\n    - --additional-mcp-config=@evil.json\n---\n",
        ).unwrap();
        let result = Engine::Copilot.args(&fm, &collect_extensions(&fm));
        assert!(result.is_err());
    }

    // ─── engine.env tests ─────────────────────────────────────────────────

    #[test]
    fn engine_env_merges_user_vars() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: hello\n---\n",
        ).unwrap();
        let env = Engine::Copilot.env(&fm.engine).unwrap();
        assert!(env.contains("GITHUB_TOKEN: $(GITHUB_TOKEN)"), "compiler vars preserved");
        assert!(env.contains("MY_VAR: \"hello\""), "user var included");
    }

    #[test]
    fn engine_env_blocks_github_token() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    GITHUB_TOKEN: evil\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("compiler-controlled"));
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
        assert!(result.unwrap_err().to_string().contains("ADO pipeline command injection"));
    }

    #[test]
    fn engine_env_rejects_ado_expressions() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: \"$(SYSTEM_ACCESSTOKEN)\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression syntax"));
    }

    #[test]
    fn engine_env_rejects_newlines() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    MY_VAR: \"line1\\nline2\"\n---\n",
        ).unwrap();
        // YAML double-quoted strings interpret \n as an actual newline
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("newline characters"));
    }

    #[test]
    fn engine_env_rejects_invalid_key() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  env:\n    \"123bad\": value\n---\n",
        ).unwrap();
        let result = Engine::Copilot.env(&fm.engine);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a valid environment variable name"));
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
        let result = Engine::Copilot.install_steps(&fm.engine);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }

    #[test]
    fn engine_version_rejects_single_quotes() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: \"1.0.0'\"\n---\n",
        ).unwrap();
        let result = Engine::Copilot.install_steps(&fm.engine);
        assert!(result.is_err());
    }

    #[test]
    fn engine_version_accepts_valid() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: '1.0.34'\n---\n",
        ).unwrap();
        let result = Engine::Copilot.install_steps(&fm.engine).unwrap();
        assert!(result.contains("-Version 1.0.34"));
    }

    #[test]
    fn engine_version_accepts_latest() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: latest\n---\n",
        ).unwrap();
        let result = Engine::Copilot.install_steps(&fm.engine).unwrap();
        // "latest" omits -Version entirely so NuGet installs the newest available
        assert!(!result.contains("-Version"), "should not contain -Version flag for 'latest'");
        assert!(result.contains("-OutputDirectory"), "should still contain other NuGet args");
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
}
