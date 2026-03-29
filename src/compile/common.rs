//! Common helper functions shared across all compile targets.

use anyhow::{Context, Result};

use super::types::{FrontMatter, McpConfig, Repository, TriggerConfig};
use crate::fuzzy_schedule;
use crate::mcp_metadata::McpMetadataFile;

/// Check if an MCP name is a built-in (launched via agency mcp)
pub fn is_builtin_mcp(name: &str) -> bool {
    let metadata = McpMetadataFile::bundled();
    metadata.get(name).map(|m| m.builtin).unwrap_or(false)
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

    let metadata = McpMetadataFile::bundled();
    let mut disallowed_mcps: Vec<&str> = metadata.mcp_names();
    disallowed_mcps.sort();

    let mut params = Vec::new();

    params.push(format!("--model {}", front_matter.engine.model()));
    params.push("--disable-builtin-mcps".to_string());
    params.push("--no-ask-user".to_string());

    for tool in allowed_tools {
        if tool.contains('(') || tool.contains(')') || tool.contains(' ') {
            // Use double quotes - the agency_params are embedded inside a single-quoted
            // bash string in the AWF command, so single quotes would break quoting.
            params.push(format!("--allow-tool \"{}\"", tool));
        } else {
            params.push(format!("--allow-tool {}", tool));
        }
    }

    for mcp in disallowed_mcps {
        params.push(format!("--disable-mcp-server {}", mcp));
    }

    for (name, config) in &front_matter.mcp_servers {
        let is_custom = matches!(config, McpConfig::WithOptions(opts) if opts.command.is_some());
        if is_custom {
            continue;
        }

        let is_enabled = match config {
            McpConfig::Enabled(enabled) => *enabled,
            McpConfig::WithOptions(_) => true,
        };

        if is_enabled {
            params.push(format!("--mcp {}", name));
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
pub const AWF_VERSION: &str = "0.25.3";

/// Version of the GitHub Copilot CLI (Microsoft.Copilot.CLI.linux-x64) NuGet package to install.
/// Update this when upgrading to a new Copilot CLI release.
/// See: https://pkgs.dev.azure.com/msazuresphere/_packaging/Guardian1ESPTUpstreamOrgFeed/nuget/v3/index.json
pub const COPILOT_CLI_VERSION: &str = "1.0.6";

/// Generate source path for the execute command.
///
/// Returns a path using `{{ workspace }}` as the base, which gets resolved
/// to the correct ADO working directory before this placeholder is replaced.
pub fn generate_source_path(input_path: &std::path::Path) -> String {
    let filename = input_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("agent.md");

    format!("{{{{ workspace }}}}/agents/{}", filename)
}

/// Generate the pipeline YAML path for integrity checking at ADO runtime.
///
/// Returns a path using `{{ workspace }}` as the base, derived from the
/// output path's filename so it matches whatever `-o` was specified during compilation.
pub fn generate_pipeline_path(output_path: &std::path::Path) -> String {
    let filename = output_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pipeline.yml");

    format!("{{{{ workspace }}}}/{}", filename)
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

/// Safe-output names that require write access to ADO.
const WRITE_REQUIRING_SAFE_OUTPUTS: &[&str] = &[
    "create-pull-request",
    "create-work-item",
    "update-work-item",
    "create-wiki-page",
    "update-wiki-page",
];

/// Validate that write-requiring safe-outputs have a write service connection configured.
pub fn validate_write_permissions(front_matter: &FrontMatter) -> Result<()> {
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
    fn test_copilot_params_custom_mcp_not_added_with_mcp_flag() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                command: Some("node".to_string()),
                ..Default::default()
            }),
        );
        let params = generate_copilot_params(&fm);
        // Custom MCPs (with command) should NOT appear as --mcp flags
        assert!(!params.contains("--mcp my-tool"));
    }

    #[test]
    fn test_copilot_params_builtin_mcp_added_with_mcp_flag() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("ado".to_string(), McpConfig::Enabled(true));
        let params = generate_copilot_params(&fm);
        assert!(params.contains("--mcp ado"));
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
}
