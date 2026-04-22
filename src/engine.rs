use anyhow::Result;

use crate::compile::COPILOT_CLI_VERSION;
use crate::compile::extensions::{CompilerExtension, Extension};
use crate::compile::types::{FrontMatter, McpConfig};

/// Default model used by the Copilot engine when no model is specified in front matter.
pub const DEFAULT_COPILOT_MODEL: &str = "claude-opus-4.5";

/// Default path where the engine binary is placed for AWF container access.
const DEFAULT_COPILOT_COMMAND_PATH: &str = "/tmp/awf-tools/copilot";

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
    pub fn env(&self) -> String {
        match self {
            Engine::Copilot => copilot_env(),
        }
    }

    /// Resolve the command path for the engine binary inside the AWF container.
    ///
    /// When `engine.command` is set in front matter, the custom path is used
    /// (skipping the default install). Otherwise returns the default path
    /// (`/tmp/awf-tools/copilot`).
    pub fn command_path(&self, front_matter: &FrontMatter) -> Result<String> {
        match self {
            Engine::Copilot => copilot_command_path(front_matter),
        }
    }

    /// Generate the YAML install steps for the engine (NuGet install + binary copy).
    ///
    /// When `engine.command` is set, install steps are skipped (empty string)
    /// because the user is providing their own engine binary.
    pub fn install_steps(&self, front_matter: &FrontMatter) -> Result<String> {
        match self {
            Engine::Copilot => copilot_install_steps(front_matter),
        }
    }
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
    let mut allowed_tools: Vec<String> = Vec::new();

    if !use_allow_all_tools {
        // Collect tool permissions from extensions (github, safeoutputs, azure-devops, etc.)
        for ext in extensions {
            for tool in ext.allowed_copilot_tools() {
                if !allowed_tools.contains(&tool) {
                    allowed_tools.push(tool);
                }
            }
        }

        // Collect tool permissions from user-defined MCP servers (sorted for deterministic output).
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
                    opts.enabled.unwrap_or(true)
                        && (opts.container.is_some() || opts.url.is_some())
                }
            };
            if !has_backing_server {
                continue;
            }
            allowed_tools.push(name.clone());
        }

        // Intentional: with restricted bash, both --allow-tool write (tool identity)
        // and --allow-all-paths (path scope) are emitted. --allow-all-tools subsumes
        // --allow-tool write, so only --allow-all-paths is needed on that path.
        if edit_enabled {
            allowed_tools.push("write".to_string());
        }

        // Bash tool: use the explicitly configured list.
        // When bash is None (not specified), use_allow_all_tools is true and this
        // block is skipped entirely (gh-aw sandbox default = wildcard).
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
                    // Invariant: bash=None → use_allow_all_tools=true → this block is
                    // skipped. Panic if the invariant is ever broken.
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
    }

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
    if front_matter.engine.max_turns().is_some() {
        eprintln!(
            "Warning: Agent '{}' has max-turns set, but max-turns is not supported by Copilot CLI \
            and will be ignored. Consider removing it from the engine configuration.",
            front_matter.name
        );
    }
    if let Some(timeout_minutes) = front_matter.engine.timeout_minutes() {
        if timeout_minutes == 0 {
            eprintln!(
                "Warning: Agent '{}' has timeout-minutes: 0, which means no time is allowed. \
                The agent job will time out immediately. \
                Consider setting timeout-minutes to at least 1.",
                front_matter.name
            );
        }
    }

    // Warn about engine options that are parsed but not yet wired into the pipeline.
    // These fields are scaffolding for future engines/features — users should know
    // they have no effect today so they aren't confused by silent no-ops.
    if !front_matter.engine.args().is_empty() {
        eprintln!(
            "Warning: Agent '{}' has engine.args set, but custom CLI arguments are not yet \
            wired into the pipeline and will be ignored.",
            front_matter.name
        );
    }
    if front_matter.engine.api_target().is_some() {
        eprintln!(
            "Warning: Agent '{}' has engine.api-target set, but custom API target (GHES/GHEC) is \
            not yet wired into the pipeline and will be ignored.",
            front_matter.name
        );
    }
    if front_matter.engine.env().is_some() {
        eprintln!(
            "Warning: Agent '{}' has engine.env set, but custom engine environment variables are \
            not yet wired into the pipeline and will be ignored.",
            front_matter.name
        );
    }

    // Validate and wire engine.agent — add --agent <value> to Copilot CLI args
    if let Some(agent) = front_matter.engine.agent() {
        if agent.is_empty()
            || !agent
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            anyhow::bail!(
                "engine.agent '{}' contains invalid characters. \
                 Only ASCII alphanumerics and hyphens are allowed.",
                agent
            );
        }
        params.push(format!("--agent {}", agent));
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

    Ok(params.join(" "))
}

/// Validate an engine command path for safety.
///
/// Rejects shell metacharacters that could break the AWF invocation (the command
/// is embedded inside a single-quoted bash string). Accepts absolute paths and
/// bare binary names.
fn validate_engine_command(cmd: &str) -> Result<()> {
    if cmd.is_empty() {
        anyhow::bail!("engine.command must not be empty");
    }
    // Allow only safe characters: alphanumeric, path separators, dots, hyphens, underscores
    if !cmd
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'))
    {
        anyhow::bail!(
            "engine.command '{}' contains invalid characters. \
             Only ASCII alphanumerics, '/', '.', '-', and '_' are allowed.",
            cmd
        );
    }
    Ok(())
}

/// Validate an engine version string for safety.
///
/// The version is embedded in a NuGet command line, so it must not contain shell
/// metacharacters or whitespace.
fn validate_engine_version(version: &str) -> Result<()> {
    if version.is_empty() {
        anyhow::bail!("engine.version must not be empty");
    }
    // "latest" is a special value that omits the -Version flag
    if version == "latest" {
        return Ok(());
    }
    // Allow only safe version characters: alphanumeric, dots, hyphens, underscores
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        anyhow::bail!(
            "engine.version '{}' contains invalid characters. \
             Only ASCII alphanumerics, '.', '-', and '_' are allowed (or \"latest\").",
            version
        );
    }
    Ok(())
}

/// Resolve the Copilot CLI command path inside the AWF container.
///
/// When `engine.command` is set, returns the custom path.
/// Otherwise returns the default path (`/tmp/awf-tools/copilot`).
fn copilot_command_path(front_matter: &FrontMatter) -> Result<String> {
    match front_matter.engine.command() {
        Some(cmd) => {
            validate_engine_command(cmd)?;
            Ok(cmd.to_string())
        }
        None => Ok(DEFAULT_COPILOT_COMMAND_PATH.to_string()),
    }
}

/// Generate the Copilot CLI install steps for the pipeline.
///
/// When `engine.command` is set, returns an empty string (no install needed).
/// When `engine.version` is `"latest"`, omits the `-Version` flag from the NuGet command.
/// Otherwise uses the specified version (or the default `COPILOT_CLI_VERSION` constant).
///
/// The returned string uses no leading indentation — `replace_with_indent` will
/// add the correct indent based on where `{{ engine_install_steps }}` appears
/// in the template.
fn copilot_install_steps(front_matter: &FrontMatter) -> Result<String> {
    // When a custom command is provided, skip all install steps.
    // Validation of the command path is handled by copilot_command_path(),
    // which runs earlier in the compilation flow.
    if front_matter.engine.command().is_some() {
        return Ok(String::new());
    }

    // Determine the NuGet version argument
    let version_arg = match front_matter.engine.version() {
        Some("latest") => String::new(), // Omit -Version flag entirely
        Some(v) => {
            validate_engine_version(v)?;
            format!(" -Version {v}")
        }
        None => format!(" -Version {}", COPILOT_CLI_VERSION),
    };

    let nuget_source = "https://pkgs.dev.azure.com/msazuresphere/_packaging/Guardian1ESPTUpstreamOrgFeed/nuget/v3/index.json";
    let nuget_args = format!(
        "install Microsoft.Copilot.CLI.linux-x64 -Source \"{nuget_source}\"{version_arg} -OutputDirectory $(Agent.TempDirectory)/tools -ExcludeVersion -NonInteractive"
    );

    let mut lines = Vec::new();
    lines.push("- task: NuGetAuthenticate@1".to_string());
    lines.push("  displayName: \"Authenticate NuGet Feed\"".to_string());
    lines.push(String::new());
    lines.push("- task: NuGetCommand@2".to_string());
    lines.push("  displayName: \"Install Copilot CLI\"".to_string());
    lines.push("  inputs:".to_string());
    lines.push("    command: 'custom'".to_string());
    lines.push(format!("    arguments: '{nuget_args}'"));
    lines.push(String::new());
    lines.push("- bash: |".to_string());
    lines.push("    ls -la \"$(Agent.TempDirectory)/tools\"".to_string());
    lines.push("    echo \"##vso[task.prependpath]$(Agent.TempDirectory)/tools/Microsoft.Copilot.CLI.linux-x64\"".to_string());
    lines.push(String::new());
    lines.push("    # Copy copilot binary to /tmp so it's accessible inside AWF container".to_string());
    lines.push("    # (AWF auto-mounts /tmp:/tmp:rw but not Agent.TempDirectory)".to_string());
    lines.push("    mkdir -p /tmp/awf-tools".to_string());
    lines.push("    cp \"$(Agent.TempDirectory)/tools/Microsoft.Copilot.CLI.linux-x64/copilot\" /tmp/awf-tools/copilot".to_string());
    lines.push("    chmod +x /tmp/awf-tools/copilot".to_string());
    lines.push("  displayName: \"Add copilot to PATH\"".to_string());
    lines.push(String::new());
    lines.push("- bash: |".to_string());
    lines.push("    copilot --version".to_string());
    lines.push("    copilot -h".to_string());
    lines.push("  displayName: \"Output copilot version\"".to_string());

    Ok(lines.join("\n"))
}

fn copilot_env() -> String {
    let lines = [
        "GITHUB_TOKEN: $(GITHUB_TOKEN)",
        "GITHUB_READ_ONLY: 1",
        "COPILOT_OTEL_ENABLED: \"true\"",
        "COPILOT_OTEL_EXPORTER_TYPE: \"file\"",
        "COPILOT_OTEL_FILE_EXPORTER_PATH: \"/tmp/awf-tools/staging/otel.jsonl\"",
    ];
    lines.join("\n")
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
        // Default engine (copilot) uses default model (claude-opus-4.5)
        assert!(params.contains("--model claude-opus-4.5"));
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
        let env = Engine::Copilot.env();
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
        assert!(params.contains("--model claude-opus-4.5"));
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

    // ─── engine.agent tests ─────────────────────────────────────────

    #[test]
    fn copilot_args_with_agent() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  agent: my-agent\n---\n",
        )
        .unwrap();
        let params = Engine::Copilot
            .args(&fm, &collect_extensions(&fm))
            .unwrap();
        assert!(params.contains("--agent my-agent"));
    }

    #[test]
    fn copilot_args_without_agent() {
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let params = Engine::Copilot
            .args(&fm, &collect_extensions(&fm))
            .unwrap();
        assert!(!params.contains("--agent"));
    }

    #[test]
    fn copilot_args_agent_validation_rejects_path_separators() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  agent: ../etc/passwd\n---\n",
        )
        .unwrap();
        let result = Engine::Copilot.args(&fm, &collect_extensions(&fm));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }

    #[test]
    fn copilot_args_agent_validation_rejects_spaces() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  agent: \"my agent\"\n---\n",
        )
        .unwrap();
        let result = Engine::Copilot.args(&fm, &collect_extensions(&fm));
        assert!(result.is_err());
    }

    // ─── engine.command tests ───────────────────────────────────────

    #[test]
    fn command_path_default() {
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let path = Engine::Copilot.command_path(&fm).unwrap();
        assert_eq!(path, "/tmp/awf-tools/copilot");
    }

    #[test]
    fn command_path_custom() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  command: /usr/local/bin/my-copilot\n---\n",
        )
        .unwrap();
        let path = Engine::Copilot.command_path(&fm).unwrap();
        assert_eq!(path, "/usr/local/bin/my-copilot");
    }

    #[test]
    fn command_path_rejects_shell_metacharacters() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  command: \"/bin/sh; rm -rf /\"\n---\n",
        )
        .unwrap();
        let result = Engine::Copilot.command_path(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }

    #[test]
    fn install_steps_default_includes_version() {
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let steps = Engine::Copilot.install_steps(&fm).unwrap();
        assert!(steps.contains("NuGetAuthenticate@1"));
        assert!(steps.contains(&format!("-Version {}", super::COPILOT_CLI_VERSION)));
        assert!(steps.contains("Add copilot to PATH"));
    }

    #[test]
    fn install_steps_custom_version() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: \"0.0.422\"\n---\n",
        )
        .unwrap();
        let steps = Engine::Copilot.install_steps(&fm).unwrap();
        assert!(steps.contains("-Version 0.0.422"));
        assert!(!steps.contains(&format!("-Version {}", super::COPILOT_CLI_VERSION)));
    }

    #[test]
    fn install_steps_latest_version_omits_flag() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: \"latest\"\n---\n",
        )
        .unwrap();
        let steps = Engine::Copilot.install_steps(&fm).unwrap();
        assert!(!steps.contains("-Version "));
        assert!(steps.contains("NuGetAuthenticate@1"));
    }

    #[test]
    fn install_steps_skipped_when_custom_command() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  command: /usr/local/bin/copilot\n---\n",
        )
        .unwrap();
        let steps = Engine::Copilot.install_steps(&fm).unwrap();
        assert!(steps.is_empty(), "install steps should be empty when engine.command is set");
    }

    // ─── engine.version validation tests ────────────────────────────

    #[test]
    fn version_validation_rejects_shell_injection() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  id: copilot\n  version: \"1.0; rm -rf /\"\n---\n",
        )
        .unwrap();
        let result = Engine::Copilot.install_steps(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }
}
