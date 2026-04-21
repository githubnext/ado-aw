use anyhow::Result;

use crate::compile::COPILOT_CLI_VERSION;
use crate::compile::extensions::{CompilerExtension, Extension};
use crate::compile::types::{FrontMatter, McpConfig};

pub trait Engine {
    fn generate_cli_params(
        &self,
        front_matter: &FrontMatter,
        extensions: &[Extension],
    ) -> Result<String>;

    fn generate_agent_ado_env(&self, read_service_connection: Option<&str>) -> String;

    /// Generate the pipeline install steps for the engine CLI.
    /// Returns empty string when `engine.command` is set (custom command, skip install).
    fn generate_install_steps(&self, front_matter: &FrontMatter) -> Result<String>;

    /// Return the engine command path used inside the AWF container.
    /// Returns `engine.command` when set, otherwise the default NuGet-installed path.
    fn generate_command_path(&self, front_matter: &FrontMatter) -> Result<String>;
}

pub struct GitHubCopilotCliEngine;

pub const GITHUB_COPILOT_CLI_ENGINE: GitHubCopilotCliEngine = GitHubCopilotCliEngine;

impl Engine for GitHubCopilotCliEngine {
    fn generate_cli_params(
        &self,
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
        let model = front_matter.engine.model();
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

        // --agent <identifier> when engine.agent is set — references a custom Copilot agent
        // file in .github/agents/ (e.g., "technical-doc-writer" →
        // .github/agents/technical-doc-writer.agent.md). Copilot-only feature.
        if let Some(agent) = front_matter.engine.agent() {
            // Validate: alphanumeric + hyphens only (no path separators, shell metacharacters)
            if agent.is_empty()
                || !agent
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                anyhow::bail!(
                    "Agent identifier '{}' contains invalid characters. \
                     Only ASCII alphanumerics and hyphens are allowed.",
                    agent
                );
            }
            params.push(format!("--agent {}", agent));
        }

        Ok(params.join(" "))
    }

    fn generate_agent_ado_env(&self, read_service_connection: Option<&str>) -> String {
        match read_service_connection {
            Some(_) => {
                "AZURE_DEVOPS_EXT_PAT: $(SC_READ_TOKEN)\nSYSTEM_ACCESSTOKEN: $(SC_READ_TOKEN)"
                    .to_string()
            }
            None => String::new(),
        }
    }

    fn generate_install_steps(&self, front_matter: &FrontMatter) -> Result<String> {
        // When engine.command is set, skip install entirely — the user provides
        // a pre-installed binary.
        if front_matter.engine.command().is_some() {
            return Ok(String::new());
        }

        // Determine the NuGet -Version flag
        let version_flag = match front_matter.engine.version() {
            Some(v) if v.eq_ignore_ascii_case("latest") => {
                // "latest" → omit -Version flag; NuGet resolves to latest available
                String::new()
            }
            Some(v) => {
                // Validate version string for shell safety
                if v.is_empty()
                    || !v
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
                {
                    anyhow::bail!(
                        "Engine version '{}' contains invalid characters. \
                         Only ASCII alphanumerics, '.', '-', and '+' are allowed.",
                        v
                    );
                }
                format!(" -Version {}", v)
            }
            None => {
                // Default: use the pinned COPILOT_CLI_VERSION constant
                format!(" -Version {}", COPILOT_CLI_VERSION)
            }
        };

        Ok(format!(
            r###"- task: NuGetAuthenticate@1
  displayName: "Authenticate NuGet Feed"

- task: NuGetCommand@2
  displayName: "Install Copilot CLI"
  inputs:
    command: 'custom'
    arguments: 'install Microsoft.Copilot.CLI.linux-x64 -Source "https://pkgs.dev.azure.com/msazuresphere/_packaging/Guardian1ESPTUpstreamOrgFeed/nuget/v3/index.json"{version_flag} -OutputDirectory $(Agent.TempDirectory)/tools -ExcludeVersion -NonInteractive'

- bash: |
    ls -la "$(Agent.TempDirectory)/tools"
    echo "##vso[task.prependpath]$(Agent.TempDirectory)/tools/Microsoft.Copilot.CLI.linux-x64"

    # Copy copilot binary to /tmp so it's accessible inside AWF container
    # (AWF auto-mounts /tmp:/tmp:rw but not Agent.TempDirectory)
    mkdir -p /tmp/awf-tools
    cp "$(Agent.TempDirectory)/tools/Microsoft.Copilot.CLI.linux-x64/copilot" /tmp/awf-tools/copilot
    chmod +x /tmp/awf-tools/copilot
  displayName: "Add copilot to PATH"

- bash: |
    copilot --version
    copilot -h
  displayName: "Output copilot version""###,
            version_flag = version_flag
        ))
    }

    fn generate_command_path(&self, front_matter: &FrontMatter) -> Result<String> {
        match front_matter.engine.command() {
            Some(cmd) => {
                // Validate: must be an absolute path or a bare binary name (no shell
                // metacharacters). The command is embedded in a single-quoted bash string
                // inside the AWF invocation, so no shell injection is possible, but we
                // still validate for correctness and to catch operator mistakes.
                if cmd.is_empty() {
                    anyhow::bail!("Engine command path must not be empty.");
                }
                // Reject shell metacharacters and path traversal
                if cmd.contains('\'')
                    || cmd.contains('"')
                    || cmd.contains('`')
                    || cmd.contains('$')
                    || cmd.contains(';')
                    || cmd.contains('|')
                    || cmd.contains('&')
                    || cmd.contains('\n')
                    || cmd.contains("..")
                {
                    anyhow::bail!(
                        "Engine command '{}' contains shell metacharacters or path traversal. \
                         Must be an absolute path or a bare binary name.",
                        cmd
                    );
                }
                // Must be an absolute path (starts with /) or a bare binary name (no slashes at all)
                let has_slash = cmd.contains('/');
                if has_slash && !cmd.starts_with('/') {
                    anyhow::bail!(
                        "Engine command '{}' must be an absolute path (starting with /) \
                         or a bare binary name (no path separators).",
                        cmd
                    );
                }
                Ok(cmd.to_string())
            }
            None => Ok("/tmp/awf-tools/copilot".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Engine, GITHUB_COPILOT_CLI_ENGINE};
    use crate::compile::{extensions::collect_extensions, parse_markdown};

    #[test]
    fn copilot_engine_generates_cli_params() {
        let (front_matter, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let params = GITHUB_COPILOT_CLI_ENGINE
            .generate_cli_params(&front_matter, &collect_extensions(&front_matter))
            .unwrap();
        assert!(params.contains("--model claude-opus-4.5"));
        assert!(params.contains("--disable-builtin-mcps"));
    }

    #[test]
    fn copilot_engine_generates_agent_ado_env() {
        let env = GITHUB_COPILOT_CLI_ENGINE.generate_agent_ado_env(Some("read-sc"));
        assert!(env.contains("AZURE_DEVOPS_EXT_PAT: $(SC_READ_TOKEN)"));
        assert!(env.contains("SYSTEM_ACCESSTOKEN: $(SC_READ_TOKEN)"));
    }

    #[test]
    fn copilot_engine_generates_empty_ado_env_without_service_connection() {
        assert!(GITHUB_COPILOT_CLI_ENGINE.generate_agent_ado_env(None).is_empty());
    }

    // ─── engine.agent ────────────────────────────────────────────────────────

    #[test]
    fn copilot_engine_agent_flag_when_set() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  agent: technical-doc-writer\n---\n",
        )
        .unwrap();
        let params = GITHUB_COPILOT_CLI_ENGINE
            .generate_cli_params(&front_matter, &collect_extensions(&front_matter))
            .unwrap();
        assert!(
            params.contains("--agent technical-doc-writer"),
            "Expected --agent flag, got: {}",
            params
        );
    }

    #[test]
    fn copilot_engine_no_agent_flag_by_default() {
        let (front_matter, _) =
            parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let params = GITHUB_COPILOT_CLI_ENGINE
            .generate_cli_params(&front_matter, &collect_extensions(&front_matter))
            .unwrap();
        assert!(
            !params.contains("--agent"),
            "Should not have --agent flag by default, got: {}",
            params
        );
    }

    #[test]
    fn copilot_engine_agent_rejects_path_separators() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  agent: ../evil/agent\n---\n",
        )
        .unwrap();
        let result = GITHUB_COPILOT_CLI_ENGINE
            .generate_cli_params(&front_matter, &collect_extensions(&front_matter));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }

    #[test]
    fn copilot_engine_agent_rejects_empty() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  agent: \"\"\n---\n",
        )
        .unwrap();
        let result = GITHUB_COPILOT_CLI_ENGINE
            .generate_cli_params(&front_matter, &collect_extensions(&front_matter));
        assert!(result.is_err());
    }

    // ─── engine.version (install steps) ─────────────────────────────────────

    #[test]
    fn install_steps_default_version() {
        let (front_matter, _) =
            parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let steps = GITHUB_COPILOT_CLI_ENGINE
            .generate_install_steps(&front_matter)
            .unwrap();
        assert!(
            steps.contains(&format!("-Version {}", crate::compile::COPILOT_CLI_VERSION)),
            "Should contain default version, got: {}",
            steps
        );
        assert!(steps.contains("NuGetAuthenticate@1"));
        assert!(steps.contains("Install Copilot CLI"));
    }

    #[test]
    fn install_steps_custom_version() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  version: \"1.2.3\"\n---\n",
        )
        .unwrap();
        let steps = GITHUB_COPILOT_CLI_ENGINE
            .generate_install_steps(&front_matter)
            .unwrap();
        assert!(
            steps.contains("-Version 1.2.3"),
            "Should contain custom version, got: {}",
            steps
        );
    }

    #[test]
    fn install_steps_latest_version_omits_version_flag() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  version: latest\n---\n",
        )
        .unwrap();
        let steps = GITHUB_COPILOT_CLI_ENGINE
            .generate_install_steps(&front_matter)
            .unwrap();
        assert!(
            !steps.contains("-Version"),
            "Should not contain -Version flag when 'latest', got: {}",
            steps
        );
        // Still contains install steps
        assert!(steps.contains("NuGetAuthenticate@1"));
    }

    #[test]
    fn install_steps_version_rejects_shell_metacharacters() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  version: \"1.0; evil\"\n---\n",
        )
        .unwrap();
        let result = GITHUB_COPILOT_CLI_ENGINE.generate_install_steps(&front_matter);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }

    // ─── engine.command ─────────────────────────────────────────────────────

    #[test]
    fn install_steps_skipped_when_command_set() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  command: /usr/local/bin/my-copilot\n---\n",
        )
        .unwrap();
        let steps = GITHUB_COPILOT_CLI_ENGINE
            .generate_install_steps(&front_matter)
            .unwrap();
        assert!(
            steps.is_empty(),
            "Install steps should be empty when command is set, got: {}",
            steps
        );
    }

    #[test]
    fn command_path_default() {
        let (front_matter, _) =
            parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let cmd = GITHUB_COPILOT_CLI_ENGINE
            .generate_command_path(&front_matter)
            .unwrap();
        assert_eq!(cmd, "/tmp/awf-tools/copilot");
    }

    #[test]
    fn command_path_custom_absolute() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  command: /usr/local/bin/my-copilot\n---\n",
        )
        .unwrap();
        let cmd = GITHUB_COPILOT_CLI_ENGINE
            .generate_command_path(&front_matter)
            .unwrap();
        assert_eq!(cmd, "/usr/local/bin/my-copilot");
    }

    #[test]
    fn command_path_bare_binary() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  command: my-copilot\n---\n",
        )
        .unwrap();
        let cmd = GITHUB_COPILOT_CLI_ENGINE
            .generate_command_path(&front_matter)
            .unwrap();
        assert_eq!(cmd, "my-copilot");
    }

    #[test]
    fn command_path_rejects_relative_path() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  command: bin/copilot\n---\n",
        )
        .unwrap();
        let result = GITHUB_COPILOT_CLI_ENGINE.generate_command_path(&front_matter);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("absolute path"));
    }

    #[test]
    fn command_path_rejects_shell_metacharacters() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  command: \"/tmp/copilot; rm -rf /\"\n---\n",
        )
        .unwrap();
        let result = GITHUB_COPILOT_CLI_ENGINE.generate_command_path(&front_matter);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("shell metacharacters"));
    }

    #[test]
    fn command_path_rejects_path_traversal() {
        let (front_matter, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  command: /tmp/../etc/evil\n---\n",
        )
        .unwrap();
        let result = GITHUB_COPILOT_CLI_ENGINE.generate_command_path(&front_matter);
        assert!(result.is_err());
    }
}
