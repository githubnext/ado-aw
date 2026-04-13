//! Common helper functions shared across all compile targets.

use anyhow::{Context, Result};

use super::types::{FrontMatter, Repository, TriggerConfig};
use crate::compile::types::McpConfig;
use crate::fuzzy_schedule;

/// Check if an MCP has a transport configuration (container or URL).
/// MCPs with a container are containerized stdio servers; MCPs with a URL
/// are HTTP servers. Both are routed through the MCP Gateway (MCPG).
pub fn is_custom_mcp(config: &McpConfig) -> bool {
    matches!(config, McpConfig::WithOptions(opts) if opts.container.is_some() || opts.url.is_some())
}

/// Parse the markdown file and extract front matter and body
pub fn parse_markdown(content: &str) -> Result<(FrontMatter, String)> {
    let content = content.trim();

    if !content.starts_with("---") {
        anyhow::bail!("Markdown file must start with YAML front matter (---)");
    }

    // Find the closing ---
    let rest = &content[3..];
    let end_idx = rest
        .find("\n---")
        .context("Could not find closing --- for front matter")?;

    let yaml_content = &rest[..end_idx];
    let markdown_body = rest[end_idx + 4..].trim();

    let front_matter: FrontMatter =
        serde_yaml::from_str(yaml_content).context("Failed to parse YAML front matter")?;

    Ok((front_matter, markdown_body.to_string()))
}

/// Replace a placeholder in the template, preserving the indentation for multi-line content.
pub fn replace_with_indent(template: &str, placeholder: &str, replacement: &str) -> String {
    let mut result = String::new();
    let mut remaining = template;

    while let Some(pos) = remaining.find(placeholder) {
        // Find the start of the current line to determine indentation
        let line_start = remaining[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let indent = &remaining[line_start..pos];

        // Only use indent if it's all whitespace
        let indent = if indent.chars().all(|c| c.is_whitespace()) {
            indent
        } else {
            ""
        };

        // Add everything before the placeholder
        result.push_str(&remaining[..pos]);

        // Add the replacement with proper indentation for each line
        let mut first_line = true;
        for line in replacement.lines() {
            if first_line {
                result.push_str(line);
                first_line = false;
            } else {
                result.push('\n');
                result.push_str(indent);
                result.push_str(line);
            }
        }
        // Handle case where replacement ends with newline
        if replacement.ends_with('\n') {
            result.push('\n');
        }

        remaining = &remaining[pos + placeholder.len()..];
    }

    result.push_str(remaining);
    result
}

/// Generate a schedule YAML block from a ScheduleConfig.
/// When no explicit schedule branches are configured, defaults to `main`.
pub fn generate_schedule(name: &str, config: &super::types::ScheduleConfig) -> Result<String> {
    let branches = config.branches();
    let fallback;
    let effective_branches = if branches.is_empty() {
        fallback = vec!["main".to_string()];
        &fallback
    } else {
        branches
    };
    fuzzy_schedule::generate_schedule_yaml(config.expression(), name, effective_branches)
}

/// Generate PR trigger configuration
pub fn generate_pr_trigger(triggers: &Option<TriggerConfig>, has_schedule: bool) -> String {
    let has_pipeline_trigger = triggers
        .as_ref()
        .and_then(|t| t.pipeline.as_ref())
        .is_some();

    match (has_pipeline_trigger, has_schedule) {
        (true, true) => "# Disable PR triggers - only run on schedule or when upstream pipeline completes\npr: none".to_string(),
        (true, false) => "# Disable PR triggers - only run when upstream pipeline completes\npr: none".to_string(),
        (false, true) => "# Disable PR triggers - only run on schedule\npr: none".to_string(),
        (false, false) => String::new(),
    }
}

/// Generate CI trigger configuration
pub fn generate_ci_trigger(triggers: &Option<TriggerConfig>, has_schedule: bool) -> String {
    let has_pipeline_trigger = triggers
        .as_ref()
        .and_then(|t| t.pipeline.as_ref())
        .is_some();

    if has_pipeline_trigger || has_schedule {
        "trigger: none".to_string()
    } else {
        String::new()
    }
}

/// Generate pipeline resource YAML for pipeline completion triggers
pub fn generate_pipeline_resources(triggers: &Option<TriggerConfig>) -> Result<String> {
    let Some(trigger_config) = triggers else {
        return Ok(String::new());
    };

    let Some(pipeline) = &trigger_config.pipeline else {
        return Ok(String::new());
    };

    // Generate a valid resource identifier (snake_case) from the pipeline name
    let resource_id: String = pipeline
        .name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();

    let mut yaml = String::from("pipelines:\n");

    yaml.push_str(&format!("    - pipeline: {}\n", resource_id));
    yaml.push_str(&format!("      source: '{}'\n", pipeline.name));

    if let Some(project) = &pipeline.project {
        yaml.push_str(&format!("      project: '{}'\n", project));
    }

    // If no branches specified, trigger on any branch
    if pipeline.branches.is_empty() {
        yaml.push_str("      trigger: true\n");
    } else {
        yaml.push_str("      trigger:\n");
        yaml.push_str("        branches:\n");
        yaml.push_str("          include:\n");
        for branch in &pipeline.branches {
            yaml.push_str(&format!("            - {}\n", branch));
        }
    }

    Ok(yaml)
}

/// Generate a step to cancel previous queued/running builds
pub fn generate_cancel_previous_builds(triggers: &Option<TriggerConfig>) -> String {
    let has_pipeline_trigger = triggers
        .as_ref()
        .and_then(|t| t.pipeline.as_ref())
        .is_some();

    if !has_pipeline_trigger {
        return String::new();
    }

    r#"- bash: |
    CURRENT_BUILD_ID=$(Build.BuildId)

    # Get queued or running builds for THIS pipeline definition only
    BUILDS=$(curl -s -u ":$SYSTEM_ACCESSTOKEN" \
    "$(System.CollectionUri)$(System.TeamProject)/_apis/build/builds?definitions=$(System.DefinitionId)&statusFilter=notStarted,inProgress&api-version=7.1" \
    | jq -r --arg current "$CURRENT_BUILD_ID" '.value[] | select(.id != ($current | tonumber)) | .id')

    if [ -z "$BUILDS" ]; then
    echo "No other queued/running builds to cancel"
    else
    for BUILD_ID in $BUILDS; do
        echo "Cancelling build $BUILD_ID"
        curl -s -X PATCH -u ":$SYSTEM_ACCESSTOKEN" \
        -H "Content-Type: application/json" \
        -d '{"status": "cancelling"}' \
        "$(System.CollectionUri)$(System.TeamProject)/_apis/build/builds/$BUILD_ID?api-version=7.1"
    done
    fi
  displayName: "Cancel previous queued builds"
  env:
    SYSTEM_ACCESSTOKEN: $(System.AccessToken)"#.to_string()
}

/// Generate repository resources YAML
pub fn generate_repositories(repositories: &[Repository]) -> String {
    if repositories.is_empty() {
        return String::new();
    }

    repositories
        .iter()
        .map(|repo| {
            format!(
                r#"- repository: {}
      type: {}
      name: {}
      ref: {}"#,
                repo.repository, repo.repo_type, repo.name, repo.repo_ref
            )
        })
        .collect::<Vec<_>>()
        .join("\n    ")
}

/// Generate checkout steps YAML
pub fn generate_checkout_steps(checkout: &[String]) -> String {
    if checkout.is_empty() {
        return String::new();
    }

    checkout
        .iter()
        .map(|name| format!("- checkout: {}", name))
        .collect::<Vec<_>>()
        .join("\n              ")
}

/// Generate `checkout: self` step.
pub fn generate_checkout_self() -> String {
    "- checkout: self".to_string()
}

/// Validate that all entries in checkout list exist in repositories
pub fn validate_checkout_list(repositories: &[Repository], checkout: &[String]) -> Result<()> {
    if checkout.is_empty() {
        return Ok(());
    }

    let repo_names: std::collections::HashSet<_> =
        repositories.iter().map(|r| r.repository.as_str()).collect();

    for name in checkout {
        if !repo_names.contains(name.as_str()) {
            anyhow::bail!(
                "Checkout entry '{}' not found in repositories. Available: {:?}",
                name,
                repo_names
            );
        }
    }

    Ok(())
}

/// Default bash commands allowed for agents (matches gh-aw defaults + yq)
const DEFAULT_BASH_COMMANDS: &[&str] = &[
    "cat", "date", "echo", "grep", "head", "ls", "pwd", "sort", "tail", "uniq", "wc", "yq",
];

/// Generate copilot CLI params from front matter configuration
pub fn generate_copilot_params(front_matter: &FrontMatter) -> String {
    let mut allowed_tools: Vec<String> = vec!["github".to_string(), "safeoutputs".to_string()];

    // Edit tool: enabled by default, can be disabled with `edit: false`
    let edit_enabled = front_matter
        .tools
        .as_ref()
        .and_then(|t| t.edit)
        .unwrap_or(true);
    if edit_enabled {
        allowed_tools.push("write".to_string());
    }

    // Bash tool: use configured list, or defaults if not specified
    let bash_commands: Vec<&str> = match front_matter.tools.as_ref().and_then(|t| t.bash.as_ref()) {
        Some(cmds) if cmds.len() == 1 && cmds[0] == ":*" => {
            // Unrestricted: single wildcard entry
            allowed_tools.push("shell(:*)".to_string());
            vec![]
        }
        Some(cmds) if cmds.is_empty() => {
            // Explicitly disabled: no bash commands
            vec![]
        }
        Some(cmds) => {
            // Explicit list of commands
            cmds.iter().map(|s| s.as_str()).collect()
        }
        None => {
            // Default safe commands
            DEFAULT_BASH_COMMANDS.to_vec()
        }
    };
    for cmd in bash_commands {
        allowed_tools.push(format!("shell({})", cmd));
    }

    let mut params = Vec::new();

    params.push(format!("--model {}", front_matter.engine.model()));
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

    for tool in allowed_tools {
        if tool.contains('(') || tool.contains(')') || tool.contains(' ') {
            // Use double quotes - the copilot_params are embedded inside a single-quoted
            // bash string in the AWF command, so single quotes would break quoting.
            params.push(format!("--allow-tool \"{}\"", tool));
        } else {
            params.push(format!("--allow-tool {}", tool));
        }
    }

    params.join(" ")
}

/// Compute the effective workspace based on explicit setting and checkout configuration.
pub fn compute_effective_workspace(
    explicit_workspace: &Option<String>,
    checkout: &[String],
    agent_name: &str,
) -> String {
    let has_additional_checkouts = !checkout.is_empty();

    match explicit_workspace {
        Some(ws) if ws == "repo" && !has_additional_checkouts => {
            eprintln!(
                "Warning: Agent '{}' has workspace: repo but no additional repositories in checkout. \
                When only 'self' is checked out, $(Build.SourcesDirectory) already contains the repository content. \
                The workspace setting has no effect in this case.",
                agent_name
            );
            "repo".to_string()
        }
        Some(ws) => ws.clone(),
        None if has_additional_checkouts => "repo".to_string(),
        None => "root".to_string(),
    }
}

/// Generate working directory based on workspace setting
pub fn generate_working_directory(effective_workspace: &str) -> String {
    match effective_workspace {
        "repo" => "$(Build.SourcesDirectory)/$(Build.Repository.Name)".to_string(),
        "root" | _ => "$(Build.SourcesDirectory)".to_string(),
    }
}

/// Generate `timeoutInMinutes` job property from `engine.timeout-minutes`.
/// Returns an empty string when timeout is not configured.
pub fn generate_job_timeout(front_matter: &FrontMatter) -> String {
    match front_matter.engine.timeout_minutes() {
        Some(minutes) => format!("timeoutInMinutes: {}", minutes),
        None => String::new(),
    }
}

/// Format a single step's YAML string with proper indentation
pub fn format_step_yaml(step_yaml: &str) -> String {
    let trimmed = step_yaml.trim();
    trimmed
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                format!("  - {}", line.trim_start_matches("---").trim())
            } else {
                format!("        {}", line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a single step's YAML string with custom base indentation
pub fn format_step_yaml_indented(step_yaml: &str, base_indent: usize) -> String {
    let trimmed = step_yaml.trim();
    let indent = " ".repeat(base_indent);
    let cont_indent = " ".repeat(base_indent + 2);
    trimmed
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                format!("{}- {}", indent, line.trim_start_matches("---").trim())
            } else {
                format!("{}{}", cont_indent, line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format multiple steps to YAML with proper indentation for jobs
pub fn format_steps_yaml(steps: &[serde_yaml::Value]) -> String {
    steps
        .iter()
        .filter_map(|step| serde_yaml::to_string(step).ok())
        .map(|s| format_step_yaml(&s))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format multiple steps to YAML with custom base indentation
pub fn format_steps_yaml_indented(steps: &[serde_yaml::Value], base_indent: usize) -> String {
    steps
        .iter()
        .filter_map(|step| serde_yaml::to_string(step).ok())
        .map(|s| format_step_yaml_indented(&s, base_indent))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sanitize a string to be used as a filename.
///
/// Converts to lowercase, replaces non-alphanumeric characters with dashes,
/// and collapses consecutive dashes into a single dash.
pub fn sanitize_filename(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Default pool name
pub const DEFAULT_POOL: &str = "AZS-1ES-L-MMS-ubuntu-22.04";

/// Version of the AWF (Agentic Workflow Firewall) binary to download from GitHub Releases.
/// Update this when upgrading to a new AWF release.
/// See: https://github.com/github/gh-aw-firewall/releases
pub const AWF_VERSION: &str = "0.25.20";

/// Version of the GitHub Copilot CLI (Microsoft.Copilot.CLI.linux-x64) NuGet package to install.
/// Update this when upgrading to a new Copilot CLI release.
/// See: https://pkgs.dev.azure.com/msazuresphere/_packaging/Guardian1ESPTUpstreamOrgFeed/nuget/v3/index.json
pub const COPILOT_CLI_VERSION: &str = "1.0.24";

/// Prefix used to identify agentic pipeline YAML files generated by ado-aw.
pub const HEADER_MARKER: &str = "# @ado-aw";

/// Generate the header comment block prepended to all compiled pipeline YAML.
///
/// The header includes:
/// - A human-readable "do not edit" warning
/// - A machine-readable `@ado-aw` marker with source path and compiler version
///
/// The source path is the input path as provided to the compiler (e.g., `agents/my-agent.md`,
/// `.azdo/pipelines/review.md`, or any other location the user chose). Path separators
/// are normalized to forward slashes for cross-platform consistency.
pub fn generate_header_comment(input_path: &std::path::Path) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut source_path = input_path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\n', "")
        .replace('\r', "")
        .replace('"', "\\\"");

    // Strip redundant leading "./" prefixes to prevent accumulation when
    // compile_all_pipelines re-joins paths through Path::new(".").join(source).
    while source_path.starts_with("./") {
        source_path = source_path[2..].to_string();
    }

    format!(
        "# This file is auto-generated by ado-aw. Do not edit manually.\n\
         # @ado-aw source=\"{}\" version={}\n",
        source_path, version
    )
}

/// Docker image and version for the MCP Gateway (gh-aw-mcpg).
/// Update this when upgrading to a new MCPG release.
/// See: https://github.com/github/gh-aw-mcpg/releases
pub const MCPG_VERSION: &str = "0.2.18";

/// Default port MCPG listens on inside the container (host network mode).
pub const MCPG_PORT: u16 = 80;

/// Generate source path for the execute command.
///
/// Returns a path using `{{ workspace }}` as the base, which gets resolved
/// to the correct ADO working directory before this placeholder is replaced.
///
/// The full relative path of the input file is preserved so that agents compiled
/// from subdirectories (e.g. `ado-aw compile agents/ctf.md`) produce a correct
/// runtime path (`$(Build.SourcesDirectory)/agents/ctf.md`) rather than a path
/// that drops the directory component.
///
/// Absolute paths fall back to using only the filename to avoid embedding
/// machine-specific paths in the generated pipeline.
pub fn generate_source_path(input_path: &std::path::Path) -> String {
    let relative = normalize_relative_path(input_path).unwrap_or_else(|| {
        input_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("agent.md")
            .to_string()
    });

    format!("{{{{ workspace }}}}/{}", relative)
}

/// Generate the pipeline YAML path for integrity checking at ADO runtime.
///
/// Returns a path using `{{ workspace }}` as the base, derived from the
/// output path so it matches whatever `-o` was specified during compilation.
///
/// The full relative path is preserved so that pipelines compiled into
/// subdirectories (e.g. `agents/ctf.yml`) produce a correct runtime path
/// (`$(Build.SourcesDirectory)/agents/ctf.yml`) rather than a path that
/// drops the directory component.
///
/// Absolute paths fall back to using only the filename to avoid embedding
/// machine-specific paths in the generated pipeline.
pub fn generate_pipeline_path(output_path: &std::path::Path) -> String {
    let relative = normalize_relative_path(output_path).unwrap_or_else(|| {
        output_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("pipeline.yml")
            .to_string()
    });

    format!("{{{{ workspace }}}}/{}", relative)
}

/// Normalize a path for embedding in a generated pipeline.
///
/// Returns `Some(String)` when `path` is relative, with:
/// - Backslashes converted to forward slashes
/// - Redundant leading `./` prefixes stripped
///
/// For absolute paths the function first tries to compute a relative path from
/// the nearest git repository root (found by walking up the directory tree
/// looking for a `.git` entry).  This preserves the directory structure when
/// the user passes an absolute path — e.g.
/// `/home/user/repo/agents/ctf.md` → `agents/ctf.md`.
///
/// Falls back to `None` (callers use filename-only) only when no git root is
/// found, to avoid embedding machine-specific absolute paths in the generated
/// pipeline YAML.
///
/// Note: `..` components in relative paths are passed through unchanged.
/// Callers are responsible for ensuring the path does not traverse outside the
/// repository checkout.
fn normalize_relative_path(path: &std::path::Path) -> Option<String> {
    if path.is_absolute() {
        // Try to make the path relative to the nearest git repo root so that
        // directory structure (e.g. `agents/ctf.md`) is preserved even when
        // the user invokes the compiler with an absolute path.
        if let Some(git_root) = find_git_root(path) {
            if let Ok(rel) = path.strip_prefix(&git_root) {
                let s = rel.to_string_lossy().replace('\\', "/");
                return Some(s);
            }
        }
        return None;
    }

    let mut s = path.to_string_lossy().replace('\\', "/");
    while let Some(stripped) = s.strip_prefix("./") {
        s = stripped.to_string();
    }
    Some(s)
}

/// Walk up the directory tree from `path` looking for a `.git` entry.
///
/// Returns the first ancestor directory that contains `.git`, or `None` if the
/// traversal reaches the filesystem root without finding one.
fn find_git_root(path: &std::path::Path) -> Option<std::path::PathBuf> {
    // Start from the file's parent directory (or the path itself if it is a dir).
    let start: &std::path::Path = if path.is_dir() { path } else { path.parent()? };

    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return None,
        }
    }
}

// ==================== Permission helpers ====================

/// ADO resource ID for minting ADO-scoped tokens via Azure CLI.
const ADO_RESOURCE_ID: &str = "499b84ac-1321-427f-aa17-267ca6975798";

/// Generate an AzureCLI@2 step to acquire an ADO-scoped token from an ARM service connection.
/// The `variable_name` parameter controls which pipeline variable the token is stored in
/// (e.g. "SC_READ_TOKEN" for the agent, "SC_WRITE_TOKEN" for the executor).
/// Returns empty string if no service connection is provided.
pub fn generate_acquire_ado_token(service_connection: Option<&str>, variable_name: &str) -> String {
    match service_connection {
        Some(sc) => {
            let mut lines = Vec::new();
            lines.push("- task: AzureCLI@2".to_string());
            lines.push(format!(
                r#"  displayName: "Acquire ADO token ({variable_name})""#
            ));
            lines.push("  inputs:".to_string());
            lines.push(format!("    azureSubscription: '{}'", sc));
            lines.push("    scriptType: 'bash'".to_string());
            lines.push("    scriptLocation: 'inlineScript'".to_string());
            lines.push("    addSpnToEnvironment: true".to_string());
            lines.push("    inlineScript: |".to_string());
            lines.push("      ADO_TOKEN=$(az account get-access-token \\".to_string());
            lines.push(format!("        --resource {} \\", ADO_RESOURCE_ID));
            lines.push("        --query accessToken -o tsv)".to_string());
            lines.push(format!(
                "      echo \"##vso[task.setvariable variable={variable_name};issecret=true]$ADO_TOKEN\""
            ));
            lines.join("\n")
        }
        None => String::new(),
    }
}

/// Generate the env block entries for the copilot AWF step (Stage 1 agent).
/// Uses the read-only token from the read service connection.
/// When not configured, omits ADO access tokens entirely.
pub fn generate_copilot_ado_env(read_service_connection: Option<&str>) -> String {
    match read_service_connection {
        Some(_) => "AZURE_DEVOPS_EXT_PAT: $(SC_READ_TOKEN)\nSYSTEM_ACCESSTOKEN: $(SC_READ_TOKEN)"
            .to_string(),
        None => String::new(),
    }
}

/// Generate the env block entries for the executor step (Stage 2 ProcessSafeOutputs).
/// Uses the write token from the write service connection.
/// When not configured, omits ADO access tokens entirely.
pub fn generate_executor_ado_env(write_service_connection: Option<&str>) -> String {
    match write_service_connection {
        Some(_) => "SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)".to_string(),
        None => String::new(),
    }
}

/// Returns true if the name contains only ASCII alphanumerics and hyphens.
/// This value is embedded inline in a shell command, so control characters
/// (including newlines) and whitespace must be rejected to prevent corruption.
fn is_safe_tool_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Generate `--enabled-tools` CLI args for the SafeOutputs MCP server.
///
/// Derives the tool list from `safe-outputs:` front matter keys plus always-on
/// diagnostic tools. If `safe-outputs:` is empty, returns an empty string
/// (all tools enabled for backward compatibility).
///
/// Non-MCP keys (like `memory`) are silently skipped — they are handled by the
/// executor and have no corresponding MCP route.
///
/// Tool names are validated to contain only ASCII alphanumerics and hyphens
/// to prevent shell injection when the args are embedded in bash commands.
/// Unrecognized tool names emit a compile-time warning and are skipped.
pub fn generate_enabled_tools_args(front_matter: &FrontMatter) -> String {
    use crate::tools::{ALL_KNOWN_SAFE_OUTPUTS, ALWAYS_ON_TOOLS, NON_MCP_SAFE_OUTPUT_KEYS};
    use std::collections::HashSet;

    if front_matter.safe_outputs.is_empty() {
        return String::new();
    }

    // `seen` deduplicates across user keys and ALWAYS_ON_TOOLS (e.g. if the user
    // configures `noop` explicitly, it shouldn't appear twice in the output).
    let mut seen = HashSet::new();
    let mut tools: Vec<String> = Vec::new();
    let mut effective_mcp_tool_count = 0usize;
    for key in front_matter.safe_outputs.keys() {
        if !is_safe_tool_name(key) {
            eprintln!(
                "Warning: skipping invalid safe-output tool name '{}' (must be ASCII alphanumeric/hyphens only)",
                key
            );
            continue;
        }
        if NON_MCP_SAFE_OUTPUT_KEYS.contains(&key.as_str()) {
            continue;
        }
        if !ALL_KNOWN_SAFE_OUTPUTS.contains(&key.as_str()) {
            eprintln!(
                "Warning: unrecognized safe-output tool '{}' — skipping (no registered tool matches this name)",
                key
            );
            continue;
        }
        effective_mcp_tool_count += 1;
        if seen.insert(key.clone()) {
            tools.push(key.clone());
        }
    }

    if effective_mcp_tool_count == 0 {
        // Every user-specified key was either invalid, unrecognized, or non-MCP
        // (e.g. memory-only). Return empty to keep all tools available (backward compat).
        return String::new();
    }

    // Always include diagnostic tools
    for tool in ALWAYS_ON_TOOLS {
        let name = tool.to_string();
        if seen.insert(name.clone()) {
            tools.push(name);
        }
    }

    tools.sort();

    let args = tools
        .iter()
        .map(|t| format!("--enabled-tools {}", t))
        .collect::<Vec<_>>()
        .join(" ");

    // Trailing space so the args don't concatenate with the next positional
    // argument when embedded inline in the shell template.
    // `args` is never empty here because ALWAYS_ON_TOOLS always contributes entries.
    args + " "
}

/// Validate that write-requiring safe-outputs have a write service connection configured.
pub fn validate_write_permissions(front_matter: &FrontMatter) -> Result<()> {
    use crate::tools::WRITE_REQUIRING_SAFE_OUTPUTS;

    let has_write_sc = front_matter
        .permissions
        .as_ref()
        .is_some_and(|p| p.write.is_some());

    if has_write_sc {
        return Ok(());
    }

    let missing: Vec<&str> = WRITE_REQUIRING_SAFE_OUTPUTS
        .iter()
        .filter(|name| front_matter.safe_outputs.contains_key(**name))
        .copied()
        .collect();

    if !missing.is_empty() {
        anyhow::bail!(
            "Safe outputs [{}] require write access to ADO, but no write service connection \
             is configured. Add a 'permissions.write' field to the front matter:\n\n  \
             permissions:\n    write: <your-write-arm-service-connection>\n",
            missing.join(", ")
        );
    }

    Ok(())
}

/// Validate that comment-on-work-item has a required `target` field when configured.
pub fn validate_comment_target(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("comment-on-work-item") {
        // Check that "target" key is present in the config
        if let Some(obj) = config_value.as_object() {
            if !obj.contains_key("target") {
                anyhow::bail!(
                    "safe-outputs.comment-on-work-item requires a 'target' field to scope \
                     which work items the agent can comment on. Options:\n\n  \
                     target: \"*\"           # any work item (unrestricted)\n  \
                     target: 12345          # specific work item ID\n  \
                     target: [12345, 67890] # list of work item IDs\n  \
                     target: \"Path\\\\Sub\"   # work items under area path prefix\n"
                );
            }
        } else {
            // If the value is not an object (e.g., `comment-on-work-item: true`), that's invalid
            anyhow::bail!(
                "safe-outputs.comment-on-work-item must be a configuration object with at \
                 least a 'target' field. Example:\n\n  \
                 safe-outputs:\n    comment-on-work-item:\n      target: \"*\"\n"
            );
        }
    }
    Ok(())
}

/// Validate that update-work-item has a required `target` field when configured.
pub fn validate_update_work_item_target(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("update-work-item") {
        if let Some(obj) = config_value.as_object() {
            if !obj.contains_key("target") {
                anyhow::bail!(
                    "safe-outputs.update-work-item requires a 'target' field to scope \
                     which work items the agent can update. Options:\n\n  \
                     target: \"*\"   # any work item (unrestricted)\n  \
                     target: 42    # specific work item ID\n"
                );
            }
        } else {
            anyhow::bail!(
                "safe-outputs.update-work-item must be a configuration object with at \
                 least a 'target' field. Example:\n\n  \
                 safe-outputs:\n    update-work-item:\n      target: \"*\"\n      title: true\n"
            );
        }
    }
    Ok(())
}

/// Validate that submit-pr-review has a required `allowed-events` field when configured.
///
/// An empty or missing `allowed-events` list would allow agents to cast any review vote,
/// including auto-approvals. Operators must explicitly opt in to each allowed event.
pub fn validate_submit_pr_review_events(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("submit-pr-review") {
        if let Some(obj) = config_value.as_object() {
            let allowed_events = obj.get("allowed-events");
            let is_empty = match allowed_events {
                None => true,
                Some(v) => v.as_array().map_or(true, |a| a.is_empty()),
            };
            if is_empty {
                anyhow::bail!(
                    "safe-outputs.submit-pr-review requires a non-empty 'allowed-events' list \
                     to prevent agents from casting unrestricted review votes. Example:\n\n  \
                     safe-outputs:\n    submit-pr-review:\n      allowed-events:\n        \
                     - comment\n        - approve-with-suggestions\n\n\
                     Valid events: approve, approve-with-suggestions, request-changes, comment\n"
                );
            }
        } else {
            anyhow::bail!(
                "safe-outputs.submit-pr-review must be a configuration object with an \
                 'allowed-events' list. Example:\n\n  \
                 safe-outputs:\n    submit-pr-review:\n      allowed-events:\n        - comment\n"
            );
        }
    }
    Ok(())
}

/// Validate that update-pr has a required `allowed-votes` field when the `vote` operation
/// is enabled (i.e., `allowed-operations` is empty — meaning all ops — or explicitly contains
/// "vote").
///
/// An empty `allowed-votes` list when vote is enabled would always fail at Stage 2 with a
/// runtime error. Catching this at compile time is consistent with how
/// `validate_submit_pr_review_events` handles the analogous case.
pub fn validate_update_pr_votes(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("update-pr") {
        if let Some(obj) = config_value.as_object() {
            // Determine whether the vote operation is reachable:
            // - allowed-operations absent or empty → all operations allowed (includes vote)
            // - allowed-operations non-empty → vote is allowed only if explicitly listed
            let vote_reachable = match obj.get("allowed-operations") {
                None => true,
                Some(v) => v
                    .as_array()
                    .map_or(true, |a| a.is_empty() || a.iter().any(|x| x == "vote")),
            };

            if vote_reachable {
                let allowed_votes_empty = match obj.get("allowed-votes") {
                    None => true,
                    Some(v) => v.as_array().map_or(true, |a| a.is_empty()),
                };
                if allowed_votes_empty {
                    anyhow::bail!(
                        "safe-outputs.update-pr enables the 'vote' operation but has no \
                         'allowed-votes' list. This would reject all votes at Stage 2. \
                         Either restrict 'allowed-operations' to exclude 'vote', or add an \
                         explicit 'allowed-votes' list:\n\n  \
                         safe-outputs:\n    update-pr:\n      allowed-votes:\n        \
                         - approve-with-suggestions\n        - wait-for-author\n\n\
                         Valid votes: approve, approve-with-suggestions, reject, \
                         wait-for-author, reset\n"
                    );
                }
            }
        }
        // If the value is a scalar (e.g. `update-pr: true`) we don't error here —
        // the config will default to empty allowed-votes, which is safe (vote always rejected).
    }
    Ok(())
}

/// Validate that resolve-pr-review-thread has a required `allowed-statuses` field when configured.
///
/// An empty or missing `allowed-statuses` list would let agents set any thread status,
/// including "fixed" or "wontFix" on security-critical review threads. Operators must
/// explicitly opt in to each allowed status transition.
pub fn validate_resolve_pr_thread_statuses(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("resolve-pr-review-thread") {
        if let Some(obj) = config_value.as_object() {
            let allowed_statuses = obj.get("allowed-statuses");
            let is_empty = match allowed_statuses {
                None => true,
                Some(v) => v.as_array().map_or(true, |a| a.is_empty()),
            };
            if is_empty {
                anyhow::bail!(
                    "safe-outputs.resolve-pr-review-thread requires a non-empty \
                     'allowed-statuses' list to prevent agents from manipulating thread \
                     statuses without explicit operator consent. Example:\n\n  \
                     safe-outputs:\n    resolve-pr-review-thread:\n      allowed-statuses:\n\
                     \x20       - fixed\n\n\
                     Valid statuses: active, fixed, wont-fix, closed, by-design\n"
                );
            }
        } else {
            anyhow::bail!(
                "safe-outputs.resolve-pr-review-thread must be a configuration object \
                 with an 'allowed-statuses' list. Example:\n\n  \
                 safe-outputs:\n    resolve-pr-review-thread:\n      allowed-statuses:\n\
                 \x20       - fixed\n"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::types::{McpConfig, McpOptions, Repository};

    /// Helper: create a minimal FrontMatter by parsing YAML
    fn minimal_front_matter() -> FrontMatter {
        let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
        fm
    }

    // ─── compute_effective_workspace ─────────────────────────────────────────

    #[test]
    fn test_workspace_explicit_root() {
        let ws = compute_effective_workspace(&Some("root".to_string()), &[], "agent");
        assert_eq!(ws, "root");
    }

    #[test]
    fn test_workspace_explicit_repo_with_checkouts() {
        let checkouts = vec!["other-repo".to_string()];
        let ws = compute_effective_workspace(&Some("repo".to_string()), &checkouts, "agent");
        assert_eq!(ws, "repo");
    }

    #[test]
    fn test_workspace_implicit_root_no_checkouts() {
        let ws = compute_effective_workspace(&None, &[], "agent");
        assert_eq!(ws, "root");
    }

    #[test]
    fn test_workspace_implicit_repo_with_checkouts() {
        let checkouts = vec!["other-repo".to_string()];
        let ws = compute_effective_workspace(&None, &checkouts, "agent");
        assert_eq!(ws, "repo");
    }

    #[test]
    fn test_workspace_explicit_repo_no_checkouts_still_returns_repo() {
        // Emits a warning but still returns "repo"
        let ws = compute_effective_workspace(&Some("repo".to_string()), &[], "agent");
        assert_eq!(ws, "repo");
    }

    // ─── validate_checkout_list ───────────────────────────────────────────────

    #[test]
    fn test_validate_checkout_list_empty_is_ok() {
        let result = validate_checkout_list(&[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_checkout_list_valid_alias_passes() {
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["my-repo".to_string()];
        let result = validate_checkout_list(&repos, &checkout);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_checkout_list_unknown_alias_fails() {
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["unknown-alias".to_string()];
        let result = validate_checkout_list(&repos, &checkout);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown-alias"));
    }

    #[test]
    fn test_validate_checkout_list_empty_checkout_of_nonempty_repos_ok() {
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let result = validate_checkout_list(&repos, &[]);
        assert!(result.is_ok());
    }

    // ─── generate_copilot_params ──────────────────────────────────────────────

    #[test]
    fn test_copilot_params_bash_wildcard() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![":*".to_string()]),
            edit: None,
        });
        let params = generate_copilot_params(&fm);
        assert!(params.contains("--allow-tool \"shell(:*)\""));
    }

    #[test]
    fn test_copilot_params_bash_disabled() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![]),
            edit: None,
        });
        let params = generate_copilot_params(&fm);
        assert!(!params.contains("shell("));
    }

    #[test]
    fn test_copilot_params_custom_mcp_no_mcp_flag() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                ..Default::default()
            }),
        );
        let params = generate_copilot_params(&fm);
        assert!(!params.contains("--mcp my-tool"));
    }

    #[test]
    fn test_copilot_params_builtin_mcp_no_mcp_flag() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("ado".to_string(), McpConfig::Enabled(true));
        let params = generate_copilot_params(&fm);
        // Copilot CLI has no built-in MCPs — all MCPs are handled via the MCP firewall
        assert!(!params.contains("--mcp ado"));
    }

    #[test]
    fn test_copilot_params_max_turns_ignored() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  max-turns: 50\n---\n",
        )
        .unwrap();
        let params = generate_copilot_params(&fm);
        assert!(!params.contains("--max-turns"), "max-turns should not be emitted as a CLI arg");
    }

    #[test]
    fn test_copilot_params_no_max_turns_when_simple_engine() {
        let fm = minimal_front_matter();
        let params = generate_copilot_params(&fm);
        assert!(!params.contains("--max-turns"));
    }

    #[test]
    fn test_copilot_params_no_max_timeout() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  timeout-minutes: 30\n---\n",
        )
        .unwrap();
        let params = generate_copilot_params(&fm);
        assert!(!params.contains("--max-timeout"), "timeout-minutes should not be emitted as a CLI arg");
    }

    #[test]
    fn test_copilot_params_no_max_timeout_when_simple_engine() {
        let fm = minimal_front_matter();
        let params = generate_copilot_params(&fm);
        assert!(!params.contains("--max-timeout"));
    }

    #[test]
    fn test_copilot_params_max_turns_zero_not_emitted() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  max-turns: 0\n---\n",
        )
        .unwrap();
        let params = generate_copilot_params(&fm);
        assert!(!params.contains("--max-turns"), "max-turns should not be emitted as a CLI arg");
    }

    #[test]
    fn test_copilot_params_max_timeout_zero_not_emitted() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  timeout-minutes: 0\n---\n",
        )
        .unwrap();
        let params = generate_copilot_params(&fm);
        assert!(!params.contains("--max-timeout"), "timeout-minutes should not be emitted as a CLI arg");
    }

    #[test]
    fn test_job_timeout_with_value() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  timeout-minutes: 30\n---\n",
        )
        .unwrap();
        assert_eq!(generate_job_timeout(&fm), "timeoutInMinutes: 30");
    }

    #[test]
    fn test_job_timeout_without_value() {
        let fm = minimal_front_matter();
        assert_eq!(generate_job_timeout(&fm), "");
    }

    #[test]
    fn test_job_timeout_zero() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  timeout-minutes: 0\n---\n",
        )
        .unwrap();
        assert_eq!(generate_job_timeout(&fm), "timeoutInMinutes: 0");
    }

    // ─── sanitize_filename ────────────────────────────────────────────────────

    #[test]
    fn test_sanitize_filename_basic() {
        assert_eq!(sanitize_filename("Daily Code Review"), "daily-code-review");
        assert_eq!(sanitize_filename("My Agent!"), "my-agent");
    }

    #[test]
    fn test_sanitize_filename_collapses_dashes() {
        assert_eq!(
            sanitize_filename("Test  Multiple   Spaces"),
            "test-multiple-spaces"
        );
        assert_eq!(sanitize_filename("a---b"), "a-b");
    }

    #[test]
    fn test_sanitize_filename_trims_dashes() {
        assert_eq!(sanitize_filename("--leading"), "leading");
        assert_eq!(sanitize_filename("trailing--"), "trailing");
        assert_eq!(sanitize_filename("--both--"), "both");
    }

    #[test]
    fn test_sanitize_filename_special_chars() {
        assert_eq!(sanitize_filename("agent@v1.0"), "agent-v1-0");
        assert_eq!(sanitize_filename("test_case"), "test-case");
    }

    // ─── generate_pr_trigger ─────────────────────────────────────────────────

    #[test]
    fn test_generate_pr_trigger_no_triggers_no_schedule() {
        let result = generate_pr_trigger(&None, false);
        assert!(
            result.is_empty(),
            "Should be empty when no triggers configured"
        );
    }

    #[test]
    fn test_generate_pr_trigger_schedule_only() {
        let result = generate_pr_trigger(&None, true);
        assert!(result.contains("pr: none"));
        assert!(result.contains("only run on schedule"));
    }

    #[test]
    fn test_generate_pr_trigger_pipeline_only() {
        let triggers = Some(crate::compile::types::TriggerConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            }),
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(result.contains("pr: none"));
        assert!(result.contains("upstream pipeline"));
    }

    #[test]
    fn test_generate_pr_trigger_both_pipeline_and_schedule() {
        let triggers = Some(crate::compile::types::TriggerConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            }),
        });
        let result = generate_pr_trigger(&triggers, true);
        assert!(result.contains("pr: none"));
        // Contains text indicating both reasons
        assert!(result.contains("schedule") || result.contains("upstream pipeline"));
    }

    // ─── generate_ci_trigger ─────────────────────────────────────────────────

    #[test]
    fn test_generate_ci_trigger_no_triggers_no_schedule() {
        let result = generate_ci_trigger(&None, false);
        assert!(
            result.is_empty(),
            "Should be empty when no triggers configured"
        );
    }

    #[test]
    fn test_generate_ci_trigger_schedule_only() {
        let result = generate_ci_trigger(&None, true);
        assert_eq!(result, "trigger: none");
    }

    #[test]
    fn test_generate_ci_trigger_pipeline_only() {
        let triggers = Some(crate::compile::types::TriggerConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            }),
        });
        let result = generate_ci_trigger(&triggers, false);
        assert_eq!(result, "trigger: none");
    }

    #[test]
    fn test_generate_ci_trigger_both_pipeline_and_schedule() {
        let triggers = Some(crate::compile::types::TriggerConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            }),
        });
        let result = generate_ci_trigger(&triggers, true);
        assert_eq!(result, "trigger: none");
    }

    // ─── generate_pipeline_resources ─────────────────────────────────────────

    #[test]
    fn test_generate_pipeline_resources_no_triggers() {
        let result = generate_pipeline_resources(&None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_generate_pipeline_resources_empty_trigger_config() {
        let triggers = Some(crate::compile::types::TriggerConfig { pipeline: None });
        let result = generate_pipeline_resources(&triggers).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_generate_pipeline_resources_with_branches() {
        let triggers = Some(crate::compile::types::TriggerConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".into(),
                project: Some("OtherProject".into()),
                branches: vec!["main".into(), "release/*".into()],
            }),
        });
        let result = generate_pipeline_resources(&triggers).unwrap();
        assert!(result.contains("source: 'Build Pipeline'"));
        assert!(result.contains("OtherProject"));
        assert!(result.contains("main"));
        assert!(result.contains("release/*"));
        // Should use branch include list, not `trigger: true`
        assert!(result.contains("branches:"));
        assert!(!result.contains("trigger: true"));
    }

    #[test]
    fn test_generate_pipeline_resources_without_branches_triggers_on_any() {
        let triggers = Some(crate::compile::types::TriggerConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "My Pipeline".into(),
                project: None,
                branches: vec![],
            }),
        });
        let result = generate_pipeline_resources(&triggers).unwrap();
        assert!(result.contains("source: 'My Pipeline'"));
        assert!(result.contains("trigger: true"));
        // No project when not specified
        assert!(!result.contains("project:"));
    }

    #[test]
    fn test_generate_pipeline_resources_resource_id_is_snake_case() {
        let triggers = Some(crate::compile::types::TriggerConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "My Build Pipeline".into(),
                project: None,
                branches: vec![],
            }),
        });
        let result = generate_pipeline_resources(&triggers).unwrap();
        // The pipeline resource ID should be snake_case derived from the name
        assert!(result.contains("pipeline: my_build_pipeline"));
    }

    // ─── generate_cancel_previous_builds ─────────────────────────────────────

    #[test]
    fn test_generate_cancel_previous_builds_no_triggers() {
        let result = generate_cancel_previous_builds(&None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_generate_cancel_previous_builds_no_pipeline_trigger() {
        let triggers = Some(crate::compile::types::TriggerConfig { pipeline: None });
        let result = generate_cancel_previous_builds(&triggers);
        assert!(result.is_empty());
    }

    #[test]
    fn test_generate_cancel_previous_builds_with_pipeline_trigger() {
        let triggers = Some(crate::compile::types::TriggerConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            }),
        });
        let result = generate_cancel_previous_builds(&triggers);
        assert!(!result.is_empty());
        assert!(result.contains("Cancel previous queued builds"));
        assert!(result.contains("SYSTEM_ACCESSTOKEN"));
        assert!(result.contains("cancelling"));
    }

    // ─── generate_header_comment ────────────────────────────────────────────

    #[test]
    fn test_generate_header_comment_escapes_quotes() {
        let path = std::path::Path::new("agents/my \"agent\".md");
        let header = generate_header_comment(path);
        assert!(
            header.contains(r#"source="agents/my \"agent\".md""#),
            "Quotes in path should be escaped: {}",
            header
        );
    }

    #[test]
    fn test_generate_header_comment_round_trip_with_quotes() {
        let path = std::path::Path::new("agents/my \"agent\".md");
        let header = generate_header_comment(path);
        let marker_line = header.lines().nth(1).expect("Should have second line");
        let meta = crate::detect::parse_header_line(marker_line)
            .expect("Should parse header with escaped quotes");
        assert_eq!(meta.source, r#"agents/my "agent".md"#);
    }

    #[test]
    fn test_generate_header_comment_strips_dot_slash_prefixes() {
        let path = std::path::Path::new("././././agents/release-readiness.md");
        let header = generate_header_comment(path);
        assert!(
            header.contains(r#"source="agents/release-readiness.md""#),
            "Redundant ./ prefixes should be stripped: {}",
            header
        );
    }

    #[test]
    fn test_generate_header_comment_strips_single_dot_slash() {
        let path = std::path::Path::new("./agents/my-agent.md");
        let header = generate_header_comment(path);
        assert!(
            header.contains(r#"source="agents/my-agent.md""#),
            "Single ./ prefix should be stripped: {}",
            header
        );
    }

    // ─── generate_source_path ────────────────────────────────────────────────

    #[test]
    fn test_generate_source_path_preserves_directory() {
        // Compiling agents/ctf.md should produce {{ workspace }}/agents/ctf.md,
        // not {{ workspace }}/agents/ctf.md with a hardcoded agents/ prefix.
        let path = std::path::Path::new("agents/ctf.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ workspace }}/agents/ctf.md");
    }

    #[test]
    fn test_generate_source_path_nested_directory() {
        let path = std::path::Path::new("pipelines/production/review.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ workspace }}/pipelines/production/review.md");
    }

    #[test]
    fn test_generate_source_path_strips_dot_slash() {
        let path = std::path::Path::new("./agents/my-agent.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ workspace }}/agents/my-agent.md");
    }

    #[test]
    fn test_generate_source_path_filename_only() {
        let path = std::path::Path::new("my-agent.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ workspace }}/my-agent.md");
    }

    // ─── generate_pipeline_path ──────────────────────────────────────────────

    #[test]
    fn test_generate_pipeline_path_preserves_directory() {
        // The original bug: compiling agents/ctf.md produced agents/ctf.yml as
        // output, but the embedded path was only ctf.yml (missing agents/).
        let path = std::path::Path::new("agents/ctf.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "{{ workspace }}/agents/ctf.yml");
    }

    #[test]
    fn test_generate_pipeline_path_nested_directory() {
        let path = std::path::Path::new("pipelines/production/review.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "{{ workspace }}/pipelines/production/review.yml");
    }

    #[test]
    fn test_generate_pipeline_path_strips_dot_slash() {
        let path = std::path::Path::new("./agents/my-agent.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "{{ workspace }}/agents/my-agent.yml");
    }

    #[test]
    fn test_generate_pipeline_path_filename_only() {
        let path = std::path::Path::new("pipeline.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "{{ workspace }}/pipeline.yml");
    }

    #[test]
    fn test_generate_source_path_absolute_falls_back_to_filename() {
        // An absolute path that is NOT inside a git repo should fall back
        // to filename-only to avoid embedding a machine-specific absolute path.
        // Use a real temp dir so the path is genuinely absolute on any OS.
        let tmp = tempfile::TempDir::new().unwrap();
        let abs_path = tmp.path().join("agents").join("ctf.md");
        // No .git marker — find_git_root will walk up and find nothing
        // (temp dirs are outside any repo).
        let result = generate_source_path(&abs_path);
        assert_eq!(result, "{{ workspace }}/ctf.md");
    }

    #[test]
    fn test_generate_pipeline_path_absolute_falls_back_to_filename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let abs_path = tmp.path().join("agents").join("ctf.yml");
        let result = generate_pipeline_path(&abs_path);
        assert_eq!(result, "{{ workspace }}/ctf.yml");
    }

    #[test]
    fn test_generate_source_path_absolute_with_git_root_preserves_directory() {
        // When the absolute path is inside a git repo, the directory structure
        // relative to the repo root must be preserved.
        use std::fs;
        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        // A `.git` file (as used in worktrees) satisfies `.exists()` just like
        // a `.git` directory, so either form is a valid marker.
        fs::write(tmp.path().join(".git"), "gitdir: fake").unwrap();
        let abs_path = agents_dir.join("ctf.md");
        let result = generate_source_path(&abs_path);
        assert_eq!(result, "{{ workspace }}/agents/ctf.md");
    }

    #[test]
    fn test_generate_pipeline_path_absolute_with_git_root_preserves_directory() {
        use std::fs;
        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(tmp.path().join(".git"), "gitdir: fake").unwrap();
        let abs_path = agents_dir.join("ctf.yml");
        let result = generate_pipeline_path(&abs_path);
        assert_eq!(result, "{{ workspace }}/agents/ctf.yml");
    }

    // ─── validate_submit_pr_review_events ────────────────────────────────────

    #[test]
    fn test_submit_pr_review_events_passes_when_not_configured() {
        let fm = minimal_front_matter();
        assert!(validate_submit_pr_review_events(&fm).is_ok());
    }

    #[test]
    fn test_submit_pr_review_events_fails_when_allowed_events_missing() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  submit-pr-review:\n    allowed-repositories:\n      - self\n---\n"
        ).unwrap();
        let result = validate_submit_pr_review_events(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-events"), "message: {msg}");
    }

    #[test]
    fn test_submit_pr_review_events_fails_when_allowed_events_empty() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  submit-pr-review:\n    allowed-events: []\n---\n"
        ).unwrap();
        let result = validate_submit_pr_review_events(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-events"), "message: {msg}");
    }

    #[test]
    fn test_submit_pr_review_events_fails_when_value_is_scalar() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  submit-pr-review: true\n---\n"
        ).unwrap();
        let result = validate_submit_pr_review_events(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_submit_pr_review_events_passes_when_events_provided() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  submit-pr-review:\n    allowed-events:\n      - comment\n      - approve\n---\n"
        ).unwrap();
        assert!(validate_submit_pr_review_events(&fm).is_ok());
    }

    // ─── validate_update_pr_votes ─────────────────────────────────────────────

    #[test]
    fn test_update_pr_votes_passes_when_not_configured() {
        let fm = minimal_front_matter();
        assert!(validate_update_pr_votes(&fm).is_ok());
    }

    #[test]
    fn test_update_pr_votes_fails_when_vote_reachable_and_no_allowed_votes() {
        // allowed-operations absent → vote is reachable; no allowed-votes → should fail
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-repositories:\n      - self\n---\n"
        ).unwrap();
        let result = validate_update_pr_votes(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-votes"), "message: {msg}");
    }

    #[test]
    fn test_update_pr_votes_fails_when_vote_explicit_and_no_allowed_votes() {
        // allowed-operations contains "vote"; no allowed-votes → should fail
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-operations:\n      - vote\n---\n"
        ).unwrap();
        let result = validate_update_pr_votes(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-votes"), "message: {msg}");
    }

    #[test]
    fn test_update_pr_votes_fails_when_allowed_votes_empty() {
        // allowed-operations absent; allowed-votes is empty list → should fail
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-votes: []\n---\n"
        ).unwrap();
        let result = validate_update_pr_votes(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_pr_votes_passes_when_vote_excluded_from_allowed_operations() {
        // allowed-operations is non-empty and does not contain "vote" → safe, no error
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-operations:\n      - add-reviewers\n      - set-auto-complete\n---\n"
        ).unwrap();
        assert!(validate_update_pr_votes(&fm).is_ok());
    }

    #[test]
    fn test_update_pr_votes_passes_when_vote_reachable_and_allowed_votes_set() {
        // allowed-operations absent; allowed-votes non-empty → OK
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-votes:\n      - approve-with-suggestions\n---\n"
        ).unwrap();
        assert!(validate_update_pr_votes(&fm).is_ok());
    }

    #[test]
    fn test_update_pr_votes_passes_when_vote_explicit_and_allowed_votes_set() {
        // allowed-operations contains "vote"; allowed-votes non-empty → OK
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-operations:\n      - vote\n    allowed-votes:\n      - wait-for-author\n---\n"
        ).unwrap();
        assert!(validate_update_pr_votes(&fm).is_ok());
    }

    // ─── validate_resolve_pr_thread_statuses ──────────────────────────────────

    #[test]
    fn test_resolve_pr_thread_passes_when_not_configured() {
        let fm = minimal_front_matter();
        assert!(validate_resolve_pr_thread_statuses(&fm).is_ok());
    }

    #[test]
    fn test_resolve_pr_thread_fails_when_allowed_statuses_missing() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-review-thread:\n    allowed-repositories:\n      - self\n---\n"
        ).unwrap();
        let result = validate_resolve_pr_thread_statuses(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-statuses"), "message: {msg}");
    }

    #[test]
    fn test_resolve_pr_thread_fails_when_allowed_statuses_empty() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-review-thread:\n    allowed-statuses: []\n---\n"
        ).unwrap();
        let result = validate_resolve_pr_thread_statuses(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-statuses"), "message: {msg}");
    }

    #[test]
    fn test_resolve_pr_thread_fails_when_value_is_scalar() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-review-thread: true\n---\n"
        ).unwrap();
        let result = validate_resolve_pr_thread_statuses(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_pr_thread_passes_when_statuses_provided() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-review-thread:\n    allowed-statuses:\n      - fixed\n      - wont-fix\n---\n"
        ).unwrap();
        assert!(validate_resolve_pr_thread_statuses(&fm).is_ok());
    }

    // ─── Enabled tools args generation ──────────────────────────────────

    #[test]
    fn test_generate_enabled_tools_args_empty_safe_outputs() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(args.is_empty(), "Empty safe-outputs should produce no args");
    }

    #[test]
    fn test_generate_enabled_tools_args_with_configured_tools() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  create-pull-request:\n    target-branch: main\n  create-work-item:\n    work-item-type: Task\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(args.contains("--enabled-tools create-pull-request"));
        assert!(args.contains("--enabled-tools create-work-item"));
        // Always-on tools should also be included
        assert!(args.contains("--enabled-tools noop"));
        assert!(args.contains("--enabled-tools missing-data"));
        assert!(args.contains("--enabled-tools missing-tool"));
        assert!(args.contains("--enabled-tools report-incomplete"));
    }

    #[test]
    fn test_generate_enabled_tools_args_no_duplicates() {
        // If a diagnostic tool is also in safe-outputs, it shouldn't appear twice
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  noop:\n    max: 5\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        let noop_count = args.matches("--enabled-tools noop").count();
        assert_eq!(noop_count, 1, "noop should appear exactly once");
    }

    #[test]
    fn test_is_safe_tool_name() {
        assert!(is_safe_tool_name("create-pull-request"));
        assert!(is_safe_tool_name("noop"));
        assert!(is_safe_tool_name("my-tool-123"));
        assert!(!is_safe_tool_name(""));
        assert!(!is_safe_tool_name("$(curl evil.com)"));
        assert!(!is_safe_tool_name("foo; rm -rf /"));
        assert!(!is_safe_tool_name("tool name"));
        assert!(!is_safe_tool_name("tool\ttab"));
    }

    #[test]
    fn test_generate_enabled_tools_args_skips_unknown_tool() {
        // An unrecognized (but safe-formatted) tool name should be skipped.
        // When no valid MCP tools remain, return empty (all tools available).
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  crate-pull-request:\n    target-branch: main\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(!args.contains("crate-pull-request"), "Unrecognized tool should be skipped");
        assert!(args.is_empty(), "All-unrecognized safe-outputs should produce no args (all tools available)");
    }

    #[test]
    fn test_generate_enabled_tools_args_skips_memory_key() {
        // `memory` is a non-MCP executor-only key. It must not appear in
        // --enabled-tools or it would cause real MCP tools to be filtered out.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  memory:\n    allowed-extensions:\n      - .md\n  create-pull-request:\n    target-branch: main\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(!args.contains("--enabled-tools memory"), "Non-MCP key 'memory' should be skipped");
        assert!(args.contains("--enabled-tools create-pull-request"), "Real MCP tool should be present");
    }

    #[test]
    fn test_generate_enabled_tools_args_memory_only_does_not_filter() {
        // When `memory` is the only safe-output key, no --enabled-tools args should
        // be generated so all tools remain available (backward compat).
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  memory:\n    allowed-extensions:\n      - .md\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(args.is_empty(), "memory-only safe-outputs should produce no args (all tools available)");
    }

    // ─── replace_with_indent ─────────────────────────────────────────────────

    #[test]
    fn test_replace_with_indent_multiline_replacement() {
        let template = "steps:\n    {{ my_marker }}\n";
        let replacement = "- bash: echo hello\n  displayName: Hello";
        let result = replace_with_indent(template, "{{ my_marker }}", replacement);
        // The 4-space indent on the placeholder line is inherited by continuation lines
        assert_eq!(result, "steps:\n    - bash: echo hello\n      displayName: Hello\n");
    }

    #[test]
    fn test_replace_with_indent_not_at_line_start_no_indent() {
        // When the placeholder is not at the start of a line (preceded by non-whitespace),
        // no extra indentation is added to continuation lines.
        let template = "prefix {{ marker }} suffix";
        let result = replace_with_indent(template, "{{ marker }}", "VALUE");
        assert_eq!(result, "prefix VALUE suffix");
    }

    #[test]
    fn test_replace_with_indent_single_line_replacement_preserves_trailing_newline() {
        let template = "    {{ placeholder }}\n";
        let result = replace_with_indent(template, "{{ placeholder }}", "value");
        assert_eq!(result, "    value\n");
    }

    #[test]
    fn test_replace_with_indent_replacement_ending_with_newline() {
        let template = "    {{ placeholder }}\n";
        let result = replace_with_indent(template, "{{ placeholder }}", "line1\nline2\n");
        // The trailing \n in the replacement should be preserved
        assert!(result.contains("line1"));
        assert!(result.contains("line2"));
        assert!(result.ends_with('\n'));
    }

    // ─── format_step_yaml / format_step_yaml_indented ────────────────────────

    #[test]
    fn test_format_step_yaml_single_line() {
        let result = format_step_yaml("bash: echo hi");
        assert_eq!(result, "  - bash: echo hi");
    }

    #[test]
    fn test_format_step_yaml_multiline() {
        let result = format_step_yaml("bash: |\n  echo hi\n  echo bye");
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines[0], "  - bash: |");
        // Continuation lines get 8 spaces prepended (existing indent is preserved)
        assert_eq!(lines[1], "          echo hi");
        assert_eq!(lines[2], "          echo bye");
    }

    #[test]
    fn test_format_step_yaml_strips_yaml_document_separator() {
        let result = format_step_yaml("--- bash: echo hi");
        assert_eq!(result, "  - bash: echo hi");
    }

    #[test]
    fn test_format_step_yaml_indented_custom_base() {
        let result = format_step_yaml_indented("bash: echo hi", 6);
        assert_eq!(result, "      - bash: echo hi");
    }

    #[test]
    fn test_format_step_yaml_indented_zero_base() {
        let result = format_step_yaml_indented("bash: echo hi", 0);
        assert_eq!(result, "- bash: echo hi");
    }

    // ─── generate_acquire_ado_token ──────────────────────────────────────────

    #[test]
    fn test_generate_acquire_ado_token_with_sc() {
        let result = generate_acquire_ado_token(Some("my-arm-sc"), "SC_READ_TOKEN");
        assert!(result.contains("AzureCLI@2"), "Should use AzureCLI@2 task");
        assert!(
            result.contains("azureSubscription: 'my-arm-sc'"),
            "Should embed service connection name"
        );
        assert!(
            result.contains("variable=SC_READ_TOKEN;issecret=true"),
            "Should set correct pipeline variable as secret"
        );
        assert!(
            result.contains("az account get-access-token"),
            "Should call az CLI to get access token"
        );
    }

    #[test]
    fn test_generate_acquire_ado_token_none_returns_empty() {
        let result = generate_acquire_ado_token(None, "SC_READ_TOKEN");
        assert!(result.is_empty(), "None service connection should return empty string");
    }

    #[test]
    fn test_generate_acquire_ado_token_write_token_variable() {
        let result = generate_acquire_ado_token(Some("write-sc"), "SC_WRITE_TOKEN");
        assert!(result.contains("variable=SC_WRITE_TOKEN;issecret=true"));
        assert!(!result.contains("SC_READ_TOKEN"));
    }

    // ─── generate_copilot_ado_env / generate_executor_ado_env ────────────────

    #[test]
    fn test_generate_copilot_ado_env_with_connection() {
        let result = generate_copilot_ado_env(Some("my-sc"));
        assert!(
            result.contains("AZURE_DEVOPS_EXT_PAT: $(SC_READ_TOKEN)"),
            "Should set AZURE_DEVOPS_EXT_PAT to SC_READ_TOKEN"
        );
        assert!(
            result.contains("SYSTEM_ACCESSTOKEN: $(SC_READ_TOKEN)"),
            "Should set SYSTEM_ACCESSTOKEN to SC_READ_TOKEN"
        );
    }

    #[test]
    fn test_generate_copilot_ado_env_none_empty() {
        assert!(
            generate_copilot_ado_env(None).is_empty(),
            "None service connection should produce empty env block"
        );
    }

    #[test]
    fn test_generate_executor_ado_env_with_connection() {
        let result = generate_executor_ado_env(Some("my-sc"));
        assert!(
            result.contains("SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)"),
            "Executor should use SC_WRITE_TOKEN"
        );
        // Must NOT expose the read token in the executor env
        assert!(
            !result.contains("SC_READ_TOKEN"),
            "Executor env must not contain SC_READ_TOKEN"
        );
    }

    #[test]
    fn test_generate_executor_ado_env_none_empty() {
        assert!(
            generate_executor_ado_env(None).is_empty(),
            "None service connection should produce empty env block"
        );
    }
}
