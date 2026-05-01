//! Common helper functions shared across all compile targets.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::types::{FrontMatter, OnConfig, PipelineParameter, Repository};
use super::extensions::{CompilerExtension, Extension, McpgServerConfig, McpgGatewayConfig, McpgConfig, CompileContext};
use crate::compile::types::McpConfig;
use crate::fuzzy_schedule;
use crate::allowed_hosts::{CORE_ALLOWED_HOSTS, mcp_required_hosts};
use crate::ecosystem_domains::{get_ecosystem_domains, is_ecosystem_identifier, is_known_ecosystem};
use crate::validate;

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
/// Generate the top-level `parameters:` YAML block from front matter parameters.
///
/// Returns a YAML block like:
/// ```yaml
/// parameters:
///   - name: clearMemory
///     displayName: "Clear agent memory"
///     type: boolean
///     default: false
/// ```
///
/// Returns an empty string if the parameters list is empty.
/// Returns an error if any parameter name is not a valid ADO identifier.
pub fn generate_parameters(parameters: &[PipelineParameter]) -> Result<String> {
    if parameters.is_empty() {
        return Ok(String::new());
    }

    // Validate parameter names — must be valid ADO identifiers to prevent
    // YAML injection or template expression injection.
    for p in parameters {
        if !validate::is_valid_parameter_name(&p.name) {
            anyhow::bail!(
                "Invalid parameter name '{}': must match [A-Za-z_][A-Za-z0-9_]* (ADO identifier)",
                p.name
            );
        }
        // Reject ADO expressions in string fields to prevent template expression injection.
        // Parameter definitions should only contain literal values.
        if let Some(ref display_name) = p.display_name {
            validate::reject_ado_expressions(display_name, &p.name, "displayName")?;
        }
        if let Some(ref default) = p.default {
            validate::reject_ado_expressions_in_value(default, &p.name, "default")?;
        }
        if let Some(ref values) = p.values {
            for v in values {
                validate::reject_ado_expressions_in_value(v, &p.name, "values")?;
            }
        }
    }

    let yaml = serde_yaml::to_string(&serde_yaml::Value::Sequence(
        parameters
            .iter()
            .map(|p| serde_yaml::to_value(p).context("Failed to serialize pipeline parameter"))
            .collect::<Result<Vec<_>>>()?,
    ))
    .context("Failed to serialize parameters to YAML")?;

    // serde_yaml outputs the sequence without a key; we need to wrap it under `parameters:`
    Ok(format!("parameters:\n{}", yaml))
}

/// Validate front matter `name` and `description` fields.
///
/// These values are substituted directly into the pipeline YAML template and must not
/// contain ADO expressions (`${{`, `$(`, `$[`), the compiler's own template marker
/// delimiter (`{{`), or newlines — any of which could disclose secrets or manipulate
/// pipeline logic via second-order injection.
pub fn validate_front_matter_identity(front_matter: &FrontMatter) -> Result<()> {
    for (field, value) in [("name", &front_matter.name), ("description", &front_matter.description)] {
        validate::reject_pipeline_injection(value, field)?;
    }

    // Validate trigger.pipeline fields for newlines and ADO expressions
    if let Some(trigger_config) = &front_matter.on_config {
        if let Some(pipeline) = &trigger_config.pipeline {
            validate::reject_pipeline_injection(&pipeline.name, "on.pipeline.name")?;
            if let Some(project) = &pipeline.project {
                validate::reject_pipeline_injection(project, "on.pipeline.project")?;
            }
            for branch in &pipeline.branches {
                validate::reject_pipeline_injection(branch, &format!("on.pipeline.branches entry {:?}", branch))?;
            }
        }
    }

    Ok(())
}

/// Build the final parameters list by combining user-defined parameters
/// with auto-injected parameters (e.g., `clearMemory` when memory is enabled).
pub fn build_parameters(user_params: &[PipelineParameter], has_memory: bool) -> Vec<PipelineParameter> {
    let mut params = user_params.to_vec();

    // Auto-inject clearMemory parameter when memory is configured,
    // unless the user already defined one with the same name.
    if has_memory && !params.iter().any(|p| p.name == "clearMemory") {
        params.insert(
            0,
            PipelineParameter {
                name: "clearMemory".to_string(),
                display_name: Some("Clear agent memory".to_string()),
                param_type: Some("boolean".to_string()),
                default: Some(serde_yaml::Value::Bool(false)),
                values: None,
            },
        );
    }

    params
}

/// Generate a schedule YAML block from a fuzzy schedule expression.
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

/// Generate PR trigger configuration.
///
/// When `triggers.pr` is explicitly configured, PR triggers stay enabled regardless
/// of schedule or pipeline triggers (overrides suppression). Native ADO branch/path
/// filters are emitted if configured.
pub fn generate_pr_trigger(on_config: &Option<OnConfig>, has_schedule: bool) -> String {
    let has_pipeline_trigger = on_config
        .as_ref()
        .and_then(|t| t.pipeline.as_ref())
        .is_some();

    let has_pr_trigger = on_config
        .as_ref()
        .and_then(|t| t.pr.as_ref())
        .is_some();

    // Explicit triggers.pr overrides schedule/pipeline suppression
    if has_pr_trigger {
        return super::pr_filters::generate_native_pr_trigger(on_config.as_ref().unwrap().pr.as_ref().unwrap());
    }

    match (has_pipeline_trigger, has_schedule) {
        (true, true) => "# Disable PR triggers - only run on schedule or when upstream pipeline completes\npr: none".to_string(),
        (true, false) => "# Disable PR triggers - only run when upstream pipeline completes\npr: none".to_string(),
        (false, true) => "# Disable PR triggers - only run on schedule\npr: none".to_string(),
        (false, false) => String::new(),
    }
}

/// Generate CI trigger configuration
pub fn generate_ci_trigger(on_config: &Option<OnConfig>, has_schedule: bool) -> String {
    let has_pipeline_trigger = on_config
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
pub fn generate_pipeline_resources(on_config: &Option<OnConfig>) -> Result<String> {
    let Some(trigger_config) = on_config else {
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
    yaml.push_str(&format!("      source: '{}'\n", pipeline.name.replace('\'', "''")));

    if let Some(project) = &pipeline.project {
        yaml.push_str(&format!("      project: '{}'\n", project.replace('\'', "''")));
    }

    // If no branches specified, trigger on any branch
    if pipeline.branches.is_empty() {
        yaml.push_str("      trigger: true\n");
    } else {
        yaml.push_str("      trigger:\n");
        yaml.push_str("        branches:\n");
        yaml.push_str("          include:\n");
        for branch in &pipeline.branches {
            yaml.push_str(&format!("            - '{}'\n", branch.replace('\'', "''")));
        }
    }

    Ok(yaml)
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
                "- repository: {}\n  type: {}\n  name: {}\n  ref: {}",
                repo.repository, repo.repo_type, repo.name, repo.repo_ref
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        .join("\n")
}

/// Generate `checkout: self` step.
pub fn generate_checkout_self() -> String {
    "- checkout: self".to_string()
}

/// Names that are reserved by the `workspace:` resolver and therefore cannot
/// be used as repository aliases / `checkout:` entries. If a user defines a
/// repository named `repo` and writes `workspace: repo`, the special-cased
/// reserved arm would silently win over the alias resolution, producing the
/// wrong working directory. We reject this at compile time instead.
const RESERVED_WORKSPACE_NAMES: &[&str] = &["root", "repo", "self"];

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
        if RESERVED_WORKSPACE_NAMES.contains(&name.as_str()) {
            anyhow::bail!(
                "Checkout entry '{}' uses a name reserved by the 'workspace:' resolver \
                ({:?}). Rename the repository alias to avoid ambiguity with \
                'workspace: {}'.",
                name,
                RESERVED_WORKSPACE_NAMES,
                name
            );
        }
    }

    Ok(())
}

/// Sentinel prefix used to encode a repository-alias workspace selection
/// in the string returned by [`compute_effective_workspace`]. The prefix is
/// only ever produced internally by `compute_effective_workspace` from a
/// user-supplied alias that has just been checked against the `checkout:`
/// list, so the encoded value never round-trips back through user input.
const WORKSPACE_ALIAS_PREFIX: &str = "alias:";

/// Compute the effective workspace based on explicit setting and checkout configuration.
///
/// Accepted values for `explicit_workspace`:
/// - `"root"` — `$(Build.SourcesDirectory)` (the checkout root)
/// - `"repo"` or `"self"` — the trigger repository's subfolder
/// - any repository alias listed in `checkout` — that repository's subfolder
///
/// Returns an encoded string that [`generate_working_directory`] resolves to
/// the actual ADO path expression.
pub fn compute_effective_workspace(
    explicit_workspace: &Option<String>,
    checkout: &[String],
    agent_name: &str,
) -> Result<String> {
    let has_additional_checkouts = !checkout.is_empty();

    match explicit_workspace {
        Some(ws) => {
            let ws = ws.as_str();
            match ws {
                "root" => Ok("root".to_string()),
                "repo" | "self" => {
                    if !has_additional_checkouts {
                        eprintln!(
                            "Warning: Agent '{}' has workspace: {} but no additional repositories in checkout. \
                            When only 'self' is checked out, $(Build.SourcesDirectory) already contains the repository content. \
                            The workspace setting has no effect in this case.",
                            agent_name, ws
                        );
                    }
                    Ok("repo".to_string())
                }
                alias => {
                    // Defense in depth: even though aliases are constrained
                    // by `validate_checkout_list` to match a `repository:`
                    // name, refuse anything that could escape the workspace
                    // root once embedded into the working directory path.
                    if !validate::is_safe_path_segment(alias) {
                        anyhow::bail!(
                            "Agent '{}' has workspace: '{}' which is not a safe path \
                            segment. Repository aliases must not be empty, contain '..', \
                            '/', '\\\\' or start with '.'.",
                            agent_name,
                            alias
                        );
                    }
                    // A single contains() check covers both "alias not in
                    // checkout" and "checkout is empty" — produce one error
                    // message that clearly lists what would have been valid.
                    if !checkout.iter().any(|c| c == alias) {
                        if checkout.is_empty() {
                            anyhow::bail!(
                                "Agent '{}' has workspace: '{}' but no additional repositories are checked out. \
                                A repository alias for workspace is only valid when at least one repository appears in 'checkout:'. \
                                Use 'root', 'repo' (or 'self'), or add the repository to the 'checkout:' list.",
                                agent_name,
                                alias
                            );
                        }
                        anyhow::bail!(
                            "Agent '{}' has workspace: '{}' which does not match any checked-out repository. \
                            Valid values: 'root', 'repo' (or 'self'), or one of {:?}",
                            agent_name,
                            alias,
                            checkout
                        );
                    }
                    Ok(format!("{}{}", WORKSPACE_ALIAS_PREFIX, alias))
                }
            }
        }
        None if has_additional_checkouts => Ok("repo".to_string()),
        None => Ok("root".to_string()),
    }
}

/// Generate the directory where the trigger ("self") repository is checked out.
///
/// This is independent of `workspace:` — it depends only on whether any
/// additional repositories are checked out:
/// - No additional checkouts → `$(Build.SourcesDirectory)` (ADO checks `self`
///   into the root).
/// - One or more additional checkouts → `$(Build.SourcesDirectory)/$(Build.Repository.Name)`
///   (ADO puts each checked-out repo, including `self`, into a subfolder named
///   after the repository).
///
/// Used to anchor paths to files that ship in the trigger repo (e.g. the agent
/// markdown source and the compiled pipeline yaml itself), regardless of where
/// `workspace:` points the agent.
pub fn generate_trigger_repo_directory(checkout: &[String]) -> String {
    if checkout.is_empty() {
        "$(Build.SourcesDirectory)".to_string()
    } else {
        "$(Build.SourcesDirectory)/$(Build.Repository.Name)".to_string()
    }
}

/// Generate working directory based on workspace setting
pub fn generate_working_directory(effective_workspace: &str) -> String {
    if let Some(alias) = effective_workspace.strip_prefix(WORKSPACE_ALIAS_PREFIX) {
        return format!("$(Build.SourcesDirectory)/{}", alias);
    }
    match effective_workspace {
        "repo" => "$(Build.SourcesDirectory)/$(Build.Repository.Name)".to_string(),
        "root" => "$(Build.SourcesDirectory)".to_string(),
        // compute_effective_workspace only ever returns "root", "repo", or an
        // "alias:<name>" sentinel; any other value indicates a programming
        // error rather than user input. Fall back to the safest path.
        other => {
            debug_assert!(false, "unexpected effective workspace value: {other}");
            "$(Build.SourcesDirectory)".to_string()
        }
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
pub const AWF_VERSION: &str = "0.25.28";

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
pub const MCPG_VERSION: &str = "0.3.0";

/// Docker image for the MCPG container.
pub const MCPG_IMAGE: &str = "ghcr.io/github/gh-aw-mcpg";

/// Default port MCPG listens on inside the container (host network mode).
pub const MCPG_PORT: u16 = 80;

/// Domain that the AWF-sandboxed agent uses to reach MCPG on the host.
/// Docker's `host.docker.internal` resolves to the host loopback from
/// inside containers running with `--network host` or via Docker DNS.
pub const MCPG_DOMAIN: &str = "host.docker.internal";

/// Docker base image for the Azure DevOps MCP container.
pub const ADO_MCP_IMAGE: &str = "node:20-slim";

/// Default entrypoint for the Azure DevOps MCP container.
pub const ADO_MCP_ENTRYPOINT: &str = "npx";

/// Default entrypoint args for the Azure DevOps MCP npm package.
pub const ADO_MCP_PACKAGE: &str = "@azure-devops/mcp";

/// Reserved MCPG server name for the auto-configured ADO MCP.
pub const ADO_MCP_SERVER_NAME: &str = "azure-devops";

/// Generate the agent markdown source path for Stage 3 execution.
///
/// Returns a path using `{{ trigger_repo_directory }}` as the base. The agent
/// markdown lives in the trigger ("self") repo, so this anchor is independent
/// of the user's `workspace:` setting (which may point at a different
/// checked-out repo where the agent runs).
///
/// The full relative path of the input file is preserved so that agents compiled
/// from subdirectories (e.g. `ado-aw compile agents/ctf.md`) produce a correct
/// runtime path rather than one that drops the directory component.
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

    format!("{{{{ trigger_repo_directory }}}}/{}", relative)
}

/// Generate the "Verify pipeline integrity" step for the pipeline YAML.
///
/// When `skip` is `false` (the default), returns the full bash step that
/// downloads the ado-aw compiler and runs `ado-aw check` against the
/// pipeline path.
///
/// The step sets `workingDirectory: {{ trigger_repo_directory }}` so that:
/// 1. The relative `{{ pipeline_path }}` argument resolves correctly when
///    `checkout:` produces a multi-repo `$(Build.SourcesDirectory)` layout.
/// 2. `ado-aw check`'s recompile step has access to the trigger repo's
///    `.git` directory, which is required to infer the ADO org from the
///    git remote (used by `tools.azure-devops`).
///
/// When `skip` is `true` (developer builds with `--skip-integrity`),
/// returns an empty string and the step is omitted from the pipeline.
pub fn generate_integrity_check(skip: bool) -> String {
    if skip {
        return String::new();
    }

    // Indentation is handled by replace_with_indent at the call site.
    r#"- bash: |
    AGENTIC_PIPELINES_PATH="$(Pipeline.Workspace)/agentic-pipeline-compiler/ado-aw"
    chmod +x "$AGENTIC_PIPELINES_PATH"
    $AGENTIC_PIPELINES_PATH check "{{ pipeline_path }}"
  workingDirectory: {{ trigger_repo_directory }}
  displayName: "Verify pipeline integrity""#
        .to_string()
}

/// Generate debug pipeline replacement values for template markers.
///
/// When `debug` is `true`, returns content for MCPG debug diagnostics:
/// - `{{ mcpg_debug_flags }}`: `-e DEBUG="*"` env, stderr tee redirect, and
///   stderr dump on health-check failure
/// - `{{ verify_mcp_backends }}`: full pipeline step that probes each MCPG
///   backend with MCP initialize + tools/list
///
/// When `debug` is `false`, both markers resolve to empty strings.
pub fn generate_debug_pipeline_replacements(debug: bool) -> Vec<(String, String)> {
    if !debug {
        return vec![
            // Emit `\` to maintain bash line continuation (same pattern as
            // generate_mcpg_docker_env when no env flags are needed).
            ("{{ mcpg_debug_flags }}".into(), "\\".into()),
            ("{{ verify_mcp_backends }}".into(), String::new()),
        ];
    }

    let mcpg_debug_flags = r##"-e DEBUG="*" \"##.to_string();

    let verify_mcp_backends = r###"# Probe all MCPG backends to force eager launch and surface failures.
# MCPG lazily starts stdio backends on first tool call — without this
# step, a broken backend (e.g., npx timeout) only surfaces as a silent
# missing-tool error during the agent run.
- bash: |
    echo "=== Probing MCP backends ==="
    PROBE_FAILED=false
    for server in $(jq -r '.mcpServers | keys[]' /tmp/awf-tools/mcp-config.json); do
      echo ""
      echo "--- Probing: $server ---"
      # MCP requires initialize handshake before tools/list.
      # Send initialize first, then tools/list in a second request
      # using the session ID from the initialize response.
      INIT_RESPONSE=$(curl -s -D /tmp/probe-headers.txt -o /tmp/probe-init.json -w "%{http_code}" --max-time 120 -X POST \
        -H "Authorization: $MCPG_API_KEY" \
        -H "Content-Type: application/json" \
        -H "Accept: application/json, text/event-stream" \
        -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"ado-aw-probe","version":"1.0"}}}' \
        "http://localhost:{{ mcpg_port }}/mcp/$server" 2>&1)
      SESSION_ID=$(grep -i "mcp-session-id" /tmp/probe-headers.txt 2>/dev/null | tr -d '\r' | awk '{print $2}')
      echo "Initialize: HTTP $INIT_RESPONSE, session=$SESSION_ID"

      if [ -z "$SESSION_ID" ]; then
        echo "##vso[task.logissue type=warning]MCP backend '$server' did not return a session ID"
        cat /tmp/probe-init.json 2>/dev/null || true
        PROBE_FAILED=true
        continue
      fi

      # Now send tools/list with the session
      HTTP_CODE=$(curl -s -o /tmp/probe-response.json -w "%{http_code}" --max-time 120 -X POST \
        -H "Authorization: $MCPG_API_KEY" \
        -H "Content-Type: application/json" \
        -H "Accept: application/json, text/event-stream" \
        -H "Mcp-Session-Id: $SESSION_ID" \
        -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
        "http://localhost:{{ mcpg_port }}/mcp/$server" 2>&1)
      BODY=$(cat /tmp/probe-response.json 2>/dev/null || echo "(empty)")
      # Extract tool count from SSE data line
      TOOL_COUNT=$(echo "$BODY" | grep '^data:' | sed 's/^data: //' | jq -r '.result.tools | length' 2>/dev/null || echo "?")
      echo "tools/list: HTTP $HTTP_CODE"
      if [ "$HTTP_CODE" -ge 200 ] && [ "$HTTP_CODE" -lt 300 ] && [ "$TOOL_COUNT" != "?" ]; then
        echo "✓ $server: $TOOL_COUNT tools available"
      else
        echo "##vso[task.logissue type=warning]MCP backend '$server' tools/list returned HTTP $HTTP_CODE"
        echo "Response: $BODY"
        PROBE_FAILED=true
      fi
    done

    echo ""
    echo "=== MCPG health after probes ==="
    curl -sf "http://localhost:{{ mcpg_port }}/health" | jq . || true

    if [ "$PROBE_FAILED" = "true" ]; then
      echo "##vso[task.logissue type=warning]One or more MCP backends failed to initialize — check logs above"
    fi
  displayName: "Verify MCP backends"
  env:
    MCPG_API_KEY: $(MCP_GATEWAY_API_KEY)"###
        .to_string();

    vec![
        ("{{ mcpg_debug_flags }}".into(), mcpg_debug_flags),
        ("{{ verify_mcp_backends }}".into(), verify_mcp_backends),
    ]
}

/// Generate the pipeline YAML path for integrity checking at ADO runtime.
///
/// Returns the path **relative** to the trigger repository root. The integrity
/// check step itself sets `workingDirectory: {{ trigger_repo_directory }}` so
/// that the path resolves correctly and so that `ado-aw check`'s recompile
/// step has access to the trigger repo's `.git` directory (needed to infer
/// the ADO org for `tools.azure-devops`).
///
/// The full relative path is preserved so that pipelines compiled into
/// subdirectories (e.g. `agents/ctf.yml`) produce a correct runtime path
/// rather than one that drops the directory component.
///
/// Absolute paths fall back to using only the filename to avoid embedding
/// machine-specific paths in the generated pipeline.
pub fn generate_pipeline_path(output_path: &std::path::Path) -> String {
    normalize_relative_path(output_path).unwrap_or_else(|| {
        output_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("pipeline.yml")
            .to_string()
    })
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
            lines.push(format!("    azureSubscription: '{}'", sc.replace('\'', "''")));
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

/// Generate the env block entries for the executor step (Stage 3 Execution).
/// Uses the write token from the write service connection.
/// When not configured, omits ADO access tokens entirely.
pub fn generate_executor_ado_env(write_service_connection: Option<&str>) -> String {
    match write_service_connection {
        Some(_) => "SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)".to_string(),
        None => String::new(),
    }
}

/// Generate `--enabled-tools` CLI args for the SafeOutputs MCP server.
///
/// Derives the tool list from `safe-outputs:` front matter keys plus always-on
/// diagnostic tools. If `safe-outputs:` is empty, returns an empty string
/// (all tools enabled for backward compatibility).
///
/// Tool names are validated to contain only ASCII alphanumerics and hyphens
/// to prevent shell injection when the args are embedded in bash commands.
/// Unrecognized tool names emit a compile-time warning and are skipped.
pub fn generate_enabled_tools_args(front_matter: &FrontMatter) -> String {
    use crate::safeoutputs::{ALL_KNOWN_SAFE_OUTPUTS, ALWAYS_ON_TOOLS, NON_MCP_SAFE_OUTPUT_KEYS};
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
        if !validate::is_safe_tool_name(key) {
            eprintln!(
                "Warning: skipping invalid safe-output tool name '{}' (must be ASCII alphanumeric/hyphens only)",
                key
            );
            continue;
        }
        if NON_MCP_SAFE_OUTPUT_KEYS.contains(&key.as_str()) {
            continue;
        }
        if key == "memory" {
            eprintln!(
                "Warning: Agent '{}': 'safe-outputs: memory:' has moved to \
                 'tools: cache-memory:'. Update your front matter to restore memory support.",
                front_matter.name
            );
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
        // Every user-specified key was either invalid or unrecognized.
        // Return empty to keep all tools available (backward compat).
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
    use crate::safeoutputs::WRITE_REQUIRING_SAFE_OUTPUTS;

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
/// An empty `allowed-votes` list when vote is enabled would always fail at Stage 3 with a
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
                         'allowed-votes' list. This would reject all votes at Stage 3. \
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

/// Validate that resolve-pr-thread has a required `allowed-statuses` field when configured.
///
/// An empty or missing `allowed-statuses` list would let agents set any thread status,
/// including "fixed" or "wontFix" on security-critical review threads. Operators must
/// explicitly opt in to each allowed status transition.
pub fn validate_resolve_pr_thread_statuses(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("resolve-pr-thread") {
        if let Some(obj) = config_value.as_object() {
            let allowed_statuses = obj.get("allowed-statuses");
            let is_empty = match allowed_statuses {
                None => true,
                Some(v) => v.as_array().map_or(true, |a| a.is_empty()),
            };
            if is_empty {
                anyhow::bail!(
                    "safe-outputs.resolve-pr-thread requires a non-empty \
                     'allowed-statuses' list to prevent agents from manipulating thread \
                     statuses without explicit operator consent. Example:\n\n  \
                     safe-outputs:\n    resolve-pr-thread:\n      allowed-statuses:\n\
                     \x20       - fixed\n\n\
                     Valid statuses: active, fixed, wont-fix, closed, by-design\n"
                );
            }
        } else {
            anyhow::bail!(
                "safe-outputs.resolve-pr-thread must be a configuration object \
                 with an 'allowed-statuses' list. Example:\n\n  \
                 safe-outputs:\n    resolve-pr-thread:\n      allowed-statuses:\n\
                 \x20       - fixed\n"
            );
        }
    }
    Ok(())
}

/// Generate the setup job YAML.
///
/// Extension `setup_steps()` are injected first (download + gate steps for
/// Tier 2/3 filters). For Tier-1-only filters (no extension activated), the
/// inline gate step is generated directly. User `setup_steps` are appended
/// last, conditioned on the gate if filters are active.
pub fn generate_setup_job(
    setup_steps: &[serde_yaml::Value],
    pool: &str,
    pr_filters: Option<&super::types::PrFilters>,
    pipeline_filters: Option<&super::types::PipelineFilters>,
    extensions: &[super::extensions::Extension],
    ctx: &super::extensions::CompileContext,
) -> anyhow::Result<String> {
    use super::extensions::CompilerExtension;

    let has_filters = pr_filters.is_some() || pipeline_filters.is_some();

    // Collect setup_steps from ALL extensions
    let ext_setup_steps: Vec<String> = extensions
        .iter()
        .flat_map(|ext| ext.setup_steps(ctx))
        .collect();
    let has_ext_setup = !ext_setup_steps.is_empty();

    if setup_steps.is_empty() && !has_filters && !has_ext_setup {
        return Ok(String::new());
    }

    let mut steps_parts = Vec::new();

    // Extension setup steps (any extension can contribute — includes gate steps)
    for step in ext_setup_steps {
        steps_parts.push(step);
    }

    let has_gate = has_filters;

    // User setup steps (conditioned on gate passing when filters are active)
    if !setup_steps.is_empty() {
        if has_gate {
            let condition = match (pr_filters.is_some(), pipeline_filters.is_some()) {
                (true, true) => {
                    "and(eq(variables['prGate.SHOULD_RUN'], 'true'), eq(variables['pipelineGate.SHOULD_RUN'], 'true'))".to_string()
                }
                (true, false) => "eq(variables['prGate.SHOULD_RUN'], 'true')".to_string(),
                (false, true) => "eq(variables['pipelineGate.SHOULD_RUN'], 'true')".to_string(),
                (false, false) => unreachable!(),
            };
            let conditioned = super::pr_filters::add_condition_to_steps(
                setup_steps,
                &condition,
            );
            steps_parts.push(format_steps_yaml_indented(&conditioned, 4));
        } else {
            steps_parts.push(format_steps_yaml_indented(setup_steps, 4));
        }
    }

    if steps_parts.is_empty() {
        return Ok(String::new());
    }

    let combined_steps = steps_parts.join("\n\n");

    Ok(format!(
        r#"- job: Setup
  displayName: "Setup"
  pool:
    name: {}
  steps:
    - checkout: self
{}
"#,
        pool, combined_steps
    ))
}

/// Generate the teardown job YAML
pub fn generate_teardown_job(
    teardown_steps: &[serde_yaml::Value],
    pool: &str,
) -> String {
    if teardown_steps.is_empty() {
        return String::new();
    }

    let steps_yaml = format_steps_yaml_indented(teardown_steps, 4);

    format!(
        r#"- job: Teardown
  displayName: "Teardown"
  dependsOn: Execution
  pool:
    name: {}
  steps:
    - checkout: self
{}
"#,
        pool, steps_yaml
    )
}

/// Generate prepare steps (inline), including extension steps and user-defined steps.
pub fn generate_prepare_steps(
    prepare_steps: &[serde_yaml::Value],
    extensions: &[super::extensions::Extension],
) -> Result<String> {
    let mut parts = Vec::new();

    // Extension prepare steps and prompt supplements (runtimes + first-party tools)
    for ext in extensions {
        for step in ext.prepare_steps() {
            parts.push(step);
        }
        if let Some(prompt) = ext.prompt_supplement() {
            parts.push(super::extensions::wrap_prompt_append(&prompt, ext.name())?);
        }
    }

    if !prepare_steps.is_empty() {
        parts.push(format_steps_yaml_indented(prepare_steps, 0));
    }

    Ok(parts.join("\n\n"))
}

/// Generate finalize steps (inline)
pub fn generate_finalize_steps(finalize_steps: &[serde_yaml::Value]) -> String {
    if finalize_steps.is_empty() {
        return String::new();
    }

    format_steps_yaml_indented(finalize_steps, 0)
}

/// Generate dependsOn clause and condition for setup/gate dependencies.
///
/// When PR or pipeline filters are active, adds a condition that allows
/// non-matching trigger types to proceed unconditionally, while matching
/// builds require the gate to pass.
/// When `expression` is provided, it's ANDed into the condition as an escape hatch.
pub fn generate_agentic_depends_on(
    setup_steps: &[serde_yaml::Value],
    has_pr_filters: bool,
    has_pipeline_filters: bool,
    expression: Option<&str>,
) -> String {
    let has_gate = has_pr_filters || has_pipeline_filters;
    let has_setup = !setup_steps.is_empty() || has_gate;

    if !has_setup && expression.is_none() {
        return String::new();
    }

    let depends = if has_setup {
        "dependsOn: Setup\n"
    } else {
        ""
    };

    if has_gate || expression.is_some() {
        let mut parts = Vec::new();
        parts.push("succeeded()".to_string());

        if has_pr_filters {
            parts.push(
                r"or(
         ne(variables['Build.Reason'], 'PullRequest'),
         eq(dependencies.Setup.outputs['prGate.SHOULD_RUN'], 'true')
       )"
                .to_string(),
            );
        }

        if has_pipeline_filters {
            parts.push(
                r"or(
         ne(variables['Build.Reason'], 'ResourceTrigger'),
         eq(dependencies.Setup.outputs['pipelineGate.SHOULD_RUN'], 'true')
       )"
                .to_string(),
            );
        }

        if let Some(expr) = expression {
            parts.push(expr.to_string());
        }

        let condition_body = parts.join(",\n       ");
        format!("{depends}condition: |\n and(\n   {condition_body}\n )")
    } else {
        "dependsOn: Setup".to_string()
    }
}

/// Returns `Some(v.to_vec())` when `v` is non-empty, otherwise `None`.
fn nonempty_vec<T: Clone>(v: &[T]) -> Option<Vec<T>> {
    if v.is_empty() { None } else { Some(v.to_vec()) }
}

/// Returns `Some(BTreeMap from m)` when `m` is non-empty, otherwise `None`.
///
/// Converts a `HashMap` source to a `BTreeMap` so JSON serialization is
/// deterministic (keys are emitted in sorted order).
fn nonempty_map<K, V>(m: &HashMap<K, V>) -> Option<std::collections::BTreeMap<K, V>>
where
    K: Clone + Eq + std::hash::Hash + Ord,
    V: Clone,
{
    if m.is_empty() {
        None
    } else {
        Some(m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
}

/// Validate a container-based MCP entry and emit any warnings.
fn validate_stdio_mcp(name: &str, container: &str, opts: &crate::compile::types::McpOptions) {
    for w in validate::validate_container_image(container, name) { eprintln!("{}", w); }
    for mount in &opts.mounts {
        for w in validate::validate_mount_source(mount, name) { eprintln!("{}", w); }
    }
    for w in validate::validate_docker_args(&opts.args, name) { eprintln!("{}", w); }
    for w in validate::warn_potential_secrets(name, &opts.env, &opts.headers) { eprintln!("{}", w); }
}

/// Build a stdio `McpgServerConfig` from a container-based MCP options block.
fn build_stdio_mcpg_server(container: &str, opts: &crate::compile::types::McpOptions) -> McpgServerConfig {
    McpgServerConfig {
        server_type: "stdio".to_string(),
        container: Some(container.to_string()),
        entrypoint: opts.entrypoint.clone(),
        entrypoint_args: nonempty_vec(&opts.entrypoint_args),
        mounts: nonempty_vec(&opts.mounts),
        args: nonempty_vec(&opts.args),
        url: None,
        headers: None,
        env: nonempty_map(&opts.env),
        tools: nonempty_vec(&opts.allowed),
    }
}

/// Build an HTTP `McpgServerConfig` from a URL-based MCP options block.
fn build_http_mcpg_server(url: &str, opts: &crate::compile::types::McpOptions) -> McpgServerConfig {
    McpgServerConfig {
        server_type: "http".to_string(),
        container: None,
        entrypoint: None,
        entrypoint_args: None,
        mounts: None,
        args: None,
        url: Some(url.to_string()),
        headers: nonempty_map(&opts.headers),
        env: None,
        tools: nonempty_vec(&opts.allowed),
    }
}

/// Validate and insert a single user-defined MCP server into `servers`.
///
/// Returns `Ok(())` on success. Returns `Err` for invalid server names.
/// Silently skips reserved names, disabled entries, and unconfigured entries.
fn try_add_user_mcp(
    name: &str,
    config: &McpConfig,
    servers: &mut std::collections::BTreeMap<String, McpgServerConfig>,
) -> Result<()> {
    // Prevent user-defined MCPs from overwriting the reserved safeoutputs backend
    if name.eq_ignore_ascii_case("safeoutputs") {
        log::warn!(
            "MCP name 'safeoutputs' is reserved for the safe outputs HTTP backend — skipping"
        );
        return Ok(());
    }

    // Validate server name for URL safety — names are embedded in MCPG routed
    // endpoints (/mcp/{name}) and must be safe URL path segments.
    // Leading dots are rejected to prevent path normalization issues (e.g., ".." → parent).
    if name.is_empty()
        || name.starts_with('.')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        anyhow::bail!(
            "MCP server name '{}' is invalid — must be non-empty, not start with '.', and contain only ASCII alphanumerics, hyphens, underscores, and dots",
            name
        );
    }

    // Skip if already auto-configured by an extension (e.g., tools.azure-devops)
    if servers.contains_key(name) {
        return Ok(());
    }

    let opts = match config {
        McpConfig::Enabled(false) => return Ok(()),
        McpConfig::Enabled(true) => {
            log::warn!("MCP '{}' has no container or url — skipping", name);
            return Ok(());
        }
        McpConfig::WithOptions(opts) => {
            if !opts.enabled.unwrap_or(true) {
                return Ok(());
            }
            opts
        }
    };

    if opts.container.is_some() && opts.url.is_some() {
        log::warn!(
            "MCP '{}': both 'container' and 'url' are set — using 'container' (stdio). \
            Remove 'url' to silence this warning.",
            name
        );
    }

    if let Some(container) = &opts.container {
        validate_stdio_mcp(name, container, opts);
        servers.insert(name.to_string(), build_stdio_mcpg_server(container, opts));
    } else if let Some(url) = &opts.url {
        // HTTP-based MCP (remote server)
        for w in validate::validate_mcp_url(url, name) { eprintln!("{}", w); }
        for w in validate::warn_potential_secrets(name, &HashMap::new(), &opts.headers) { eprintln!("{}", w); }
        if !opts.env.is_empty() {
            eprintln!(
                "Warning: MCP '{}': env vars are not supported for HTTP MCPs — they will be ignored. \
                Use headers for authentication instead.",
                name
            );
        }
        servers.insert(name.to_string(), build_http_mcpg_server(url, opts));
    } else {
        log::warn!("MCP '{}' has no container or url — skipping", name);
    }

    Ok(())
}

/// Generate MCPG configuration from front matter.
///
/// Converts the front matter `mcp-servers` definitions into MCPG-compatible JSON.
/// SafeOutputs is always included as an HTTP backend. Extension-contributed MCPG
/// entries (e.g., azure-devops) are included via the `extensions` parameter.
pub fn generate_mcpg_config(
    front_matter: &FrontMatter,
    ctx: &CompileContext,
    extensions: &[super::extensions::Extension],
) -> Result<McpgConfig> {
    let mut mcp_servers = std::collections::BTreeMap::new();

    // Add extension-contributed MCPG server entries (safeoutputs, azure-devops, etc.)
    for ext in extensions {
        for (name, config) in ext.mcpg_servers(ctx)? {
            mcp_servers.insert(name, config);
        }
    }

    for (name, config) in &front_matter.mcp_servers {
        try_add_user_mcp(name, config, &mut mcp_servers)?;
    }

    Ok(McpgConfig {
        mcp_servers,
        gateway: McpgGatewayConfig {
            port: MCPG_PORT,
            domain: MCPG_DOMAIN.to_string(),
            api_key: "${MCP_GATEWAY_API_KEY}".to_string(),
            payload_dir: "/tmp/gh-aw/mcp-payloads".to_string(),
        },
    })
}

/// Generate additional `-e` flags for the MCPG Docker run command.
///
/// Two sources of env flags:
/// 1. **Extension pipeline var mappings** — extensions declare `required_pipeline_vars()`
///    which map container env vars to pipeline variables (typically secrets).
///    These become `-e CONTAINER_VAR="$PIPELINE_VAR"` flags referencing bash vars
///    (the companion `generate_mcpg_step_env` provides the ADO `env:` mapping).
/// 2. **User-configured MCP passthrough** — front matter `mcp-servers:` entries with
///    `env: { VAR: "" }` become bare `-e VAR` flags (MCPG passthrough from host env).
///
/// Returns flags formatted for inline insertion in the `docker run` command.
pub fn generate_mcpg_docker_env(
    front_matter: &FrontMatter,
    extensions: &[super::extensions::Extension],
) -> String {
    let mut env_flags: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. Extension pipeline var mappings (e.g., AZURE_DEVOPS_EXT_PAT -> SC_READ_TOKEN)
    for ext in extensions {
        for mapping in ext.required_pipeline_vars() {
            if seen.contains(&mapping.container_var) {
                continue;
            }
            env_flags.push(format!(
                "-e {}=\"${}\"",
                mapping.container_var, mapping.pipeline_var
            ));
            seen.insert(mapping.container_var.clone());
        }
    }

    // 2. User-configured MCP passthrough env vars (empty value = passthrough from host)
    for (mcp_name, config) in &front_matter.mcp_servers {
        let opts = match config {
            McpConfig::WithOptions(opts) if opts.enabled.unwrap_or(true) => opts,
            _ => continue,
        };

        if opts.container.is_none() {
            continue;
        }

        for (var_name, var_value) in &opts.env {
            if !validate::is_valid_env_var_name(var_name) {
                log::warn!(
                    "MCP '{}': skipping invalid env var name '{}' — must match [A-Za-z_][A-Za-z0-9_]*",
                    mcp_name, var_name
                );
                continue;
            }
            if seen.contains(var_name) {
                continue;
            }
            if var_value.is_empty() {
                env_flags.push(format!("-e {}", var_name));
                seen.insert(var_name.clone());
            }
        }
    }

    env_flags.sort();
    if env_flags.is_empty() {
        "\\".to_string()
    } else {
        let flags = env_flags.join(" \\\n");
        format!("{} \\", flags)
    }
}

/// Generate the ADO step-level `env:` block for the MCPG start step.
///
/// ADO secret variables (set via `##vso[task.setvariable;issecret=true]`) must
/// be explicitly mapped via the step's `env:` block to be available as bash
/// environment variables. This function collects all pipeline variable mappings
/// from extensions and generates the corresponding `env:` entries.
///
/// Returns YAML `env:` entries (e.g., `SC_READ_TOKEN: $(SC_READ_TOKEN)`),
/// or an empty string if no mappings are needed.
pub fn generate_mcpg_step_env(
    extensions: &[super::extensions::Extension],
) -> String {
    let mut entries: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for ext in extensions {
        for mapping in ext.required_pipeline_vars() {
            if seen.contains(&mapping.pipeline_var) {
                continue;
            }
            entries.push(format!(
                "{}: $({})",
                mapping.pipeline_var, mapping.pipeline_var
            ));
            seen.insert(mapping.pipeline_var.clone());
        }
    }

    if entries.is_empty() {
        return String::new();
    }

    // Return full `env:` block so the template marker can be cleanly omitted when empty
    let indented = entries
        .iter()
        .map(|e| format!("  {}", e))
        .collect::<Vec<_>>()
        .join("\n");
    format!("env:\n{}", indented)
}

// ==================== Domain allowlist ====================

/// Generate the allowed domains list for AWF network isolation.
///
/// This generates a comma-separated list of domain patterns for AWF's
/// `--allow-domains` flag. The list includes:
/// 1. Core Azure DevOps/GitHub endpoints
/// 2. MCP-specific endpoints for each enabled MCP
/// 3. User-specified additional hosts from network.allowed
pub fn generate_allowed_domains(
    front_matter: &FrontMatter,
    extensions: &[super::extensions::Extension],
) -> Result<String> {
    // Collect enabled MCP names (user-defined MCPs, not first-party tools)
    let enabled_mcps: Vec<String> = front_matter
        .mcp_servers
        .iter()
        .filter_map(|(name, config)| {
            let is_enabled = match config {
                McpConfig::Enabled(enabled) => *enabled,
                McpConfig::WithOptions(_) => true,
            };
            if is_enabled { Some(name.clone()) } else { None }
        })
        .collect();

    // Get user-specified hosts
    let user_hosts: Vec<String> = front_matter
        .network
        .as_ref()
        .map(|n| n.allowed.clone())
        .unwrap_or_default();

    // Generate the allowlist by combining core + MCP + extension + user hosts
    let mut hosts: HashSet<String> = HashSet::new();

    // Add core hosts
    for host in CORE_ALLOWED_HOSTS {
        hosts.insert((*host).to_string());
    }

    // Add host.docker.internal — required for the AWF container to reach
    // MCPG and SafeOutputs on the host.
    hosts.insert("host.docker.internal".to_string());

    // Add MCP-specific hosts (user-defined MCPs via mcp_required_hosts lookup)
    for mcp in &enabled_mcps {
        for host in mcp_required_hosts(mcp) {
            hosts.insert((*host).to_string());
        }
    }

    // Add extension-declared hosts (runtimes + first-party tools).
    // Extensions may return ecosystem identifiers (e.g., "lean") which are
    // expanded to their domain lists, or raw domain names.
    for ext in extensions {
        for host in ext.required_hosts() {
            if is_ecosystem_identifier(&host) {
                let domains = get_ecosystem_domains(&host);
                if domains.is_empty() {
                    eprintln!(
                        "warning: extension '{}' requires unknown ecosystem '{}'; \
                         no domains added",
                        ext.name(),
                        host
                    );
                }
                for domain in domains {
                    hosts.insert(domain);
                }
            } else {
                hosts.insert(host);
            }
        }
    }

    // Add engine-required hosts (e.g., GHES/GHEC api-target hostname).
    // The engine resolves its config and returns additional hosts that AWF must allow.
    let engine = crate::engine::get_engine(front_matter.engine.engine_id())?;
    for host in engine.required_hosts(&front_matter.engine) {
        hosts.insert(host);
    }

    // Add user-specified hosts (validated against DNS-safe characters)
    // Entries may be ecosystem identifiers (e.g., "python", "rust") which
    // expand to their domain lists, or raw domain names.
    for host in &user_hosts {
        if is_ecosystem_identifier(host) {
            let domains = get_ecosystem_domains(host);
            if domains.is_empty() && !is_known_ecosystem(host) {
                eprintln!(
                    "warning: network.allowed contains unknown ecosystem identifier '{}'. \
                     Known ecosystems: python, rust, node, go, java, etc. \
                     If this is a domain name, it should contain a dot.",
                    host
                );
            }
            for domain in domains {
                hosts.insert(domain);
            }
        } else {
            validate::validate_dns_domain(host)?;
            hosts.insert(host.clone());
        }
    }

    // Remove blocked hosts (supports both ecosystem identifiers and raw domains)
    let blocked_hosts: Vec<String> = front_matter
        .network
        .as_ref()
        .map(|n| n.blocked.clone())
        .unwrap_or_default();
    for blocked in &blocked_hosts {
        if is_ecosystem_identifier(blocked) {
            for domain in get_ecosystem_domains(blocked) {
                hosts.remove(&domain);
            }
        } else {
            hosts.remove(blocked);
        }
    }

    // Sort for deterministic output
    let mut allowlist: Vec<String> = hosts.into_iter().collect();
    allowlist.sort();

    // Format as comma-separated list for AWF --allow-domains
    Ok(allowlist.join(","))
}

/// Generate AWF `--mount` flags from extension-declared volume mounts.
///
/// Collects `required_awf_mounts()` from all extensions and formats them
/// as `--mount "spec"` CLI flags for the AWF invocation.
///
/// Each mount spec is rendered using its [`Display`][std::fmt::Display] impl
/// (Docker bind-mount format: `host_path:container_path[:mode]`).
///
/// When no extensions require mounts, returns `\` (a bare bash continuation
/// marker) so the surrounding `\`-continuation chain in the template is
/// preserved.  When mounts are present, each flag occupies its own line
/// (`--mount "spec" \`); indentation is handled by [`replace_with_indent`]
/// at the call site.
pub fn generate_awf_mounts(extensions: &[super::extensions::Extension]) -> String {
    let mounts: Vec<super::extensions::AwfMount> = extensions
        .iter()
        .flat_map(|ext| ext.required_awf_mounts())
        .collect();

    if mounts.is_empty() {
        return "\\".to_string();
    }

    mounts
        .iter()
        .map(|m| format!("--mount \"{}\" \\", m))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generates a dedicated pipeline step that writes a `GITHUB_PATH` file
/// containing directories collected from `CompilerExtension::awf_path_prepends()`.
///
/// AWF reads the `$GITHUB_PATH` environment variable (a path to a file) at
/// startup and merges its entries into the chroot PATH. This mechanism was
/// designed for GitHub Actions `setup-*` actions but works identically on
/// ADO when we compose the file ourselves.
///
/// The generated step uses `##vso[task.setvariable]` to set `GITHUB_PATH`
/// as a pipeline variable visible to subsequent steps (including the AWF
/// invocation step that runs under `sudo`). This bypasses the `sudo`
/// `secure_path` reset that strips custom PATH entries.
///
/// When no extensions declare path prepends, returns an empty string and
/// the step is omitted from the pipeline.
pub fn generate_awf_path_step(awf_paths: &[String]) -> String {
    if awf_paths.is_empty() {
        return String::new();
    }

    let path_lines = awf_paths
        .iter()
        .map(|p| format!("    {p}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "\
- bash: |
    AWF_PATH_FILE=\"/tmp/awf-tools/ado-path-entries\"
    cat > \"$AWF_PATH_FILE\" << AWF_PATH_EOF
{path_lines}
    AWF_PATH_EOF
    echo \"##vso[task.setvariable variable=GITHUB_PATH]$AWF_PATH_FILE\"
  displayName: \"Generate GITHUB_PATH file\""
    )
}

/// Generates the `env:` block entry that passes `GITHUB_PATH` to the AWF
/// invocation step.
///
/// ADO pipeline variables set via `##vso[task.setvariable]` are auto-mapped
/// as environment variables in subsequent steps, but we explicitly pass
/// `GITHUB_PATH` via the `env:` block for clarity and robustness.
///
/// When no path prepends exist, returns an empty string.
pub fn generate_awf_path_env(has_awf_paths: bool) -> String {
    if !has_awf_paths {
        return String::new();
    }

    "GITHUB_PATH: $(GITHUB_PATH)".to_string()
}

/// Collects `awf_path_prepends()` from all extensions into a single `Vec`.
pub fn collect_awf_path_prepends(extensions: &[super::extensions::Extension]) -> Vec<String> {
    extensions
        .iter()
        .flat_map(|ext| ext.awf_path_prepends())
        .collect()
}

// ==================== Shared compile flow ====================

/// Target-specific overrides for the shared compile flow.
pub struct CompileConfig {
    /// The base YAML template content (the template string itself).
    pub template: String,
    /// Additional placeholder→value replacements beyond the shared set.
    /// These are applied **before** the shared replacements, allowing
    /// target-specific overrides of shared markers (e.g., 1ES-specific
    /// setup/teardown jobs that differ from the standalone defaults).
    pub extra_replacements: Vec<(String, String)>,
    /// When true, the "Verify pipeline integrity" step is omitted from the
    /// generated pipeline. This is a developer-only option gated behind
    /// `cfg(debug_assertions)` at the CLI level.
    pub skip_integrity: bool,
    /// When true, MCPG debug diagnostics (debug logging, stderr streaming,
    /// backend probe step) are included in the generated pipeline.
    /// Gated behind `cfg(debug_assertions)` at the CLI level.
    pub debug_pipeline: bool,
    /// Whether any extension declared AWF path prepends. Used by `compile_shared`
    /// to append `GITHUB_PATH: $(GITHUB_PATH)` to the engine env block without
    /// re-collecting path prepends from extensions.
    pub has_awf_paths: bool,
}

/// Shared compilation flow used by both standalone and 1ES compilers.
///
/// This function handles the common pipeline compilation steps:
/// 1. Validates front matter
/// 2. Generates all shared placeholder values
/// 3. Runs extension validations
/// 4. Applies replacements to the template
/// 5. Prepends the header comment
///
/// Target-specific values are provided via `CompileConfig.extra_replacements`,
/// which are applied before the shared replacements so that targets can
/// override shared markers (e.g., `{{ setup_job }}`, `{{ teardown_job }}`).
pub async fn compile_shared(
    input_path: &Path,
    output_path: &Path,
    front_matter: &FrontMatter,
    markdown_body: &str,
    extensions: &[Extension],
    ctx: &CompileContext<'_>,
    config: CompileConfig,
) -> Result<String> {
    // 1. Validate
    validate_front_matter_identity(front_matter)?;

    // 2. Generate schedule
    let schedule = match front_matter.schedule() {
        Some(s) => generate_schedule(&front_matter.name, s)
            .with_context(|| format!("Failed to parse schedule '{}'", s.expression()))?,
        None => String::new(),
    };

    let repositories = generate_repositories(&front_matter.repositories);
    let checkout_steps = generate_checkout_steps(&front_matter.checkout);
    let checkout_self = generate_checkout_self();
    let agent_name = sanitize_filename(&front_matter.name);

    // 3. Run extension validations
    for ext in extensions {
        for warning in ext.validate(ctx)? {
            eprintln!("Warning: {}", warning);
        }
    }

    // 4. Generate engine invocations and install steps
    let engine_run = ctx.engine.invocation(
        ctx.front_matter,
        extensions,
        "/tmp/awf-tools/agent-prompt.md",
        Some("/tmp/awf-tools/mcp-config.json"),
    )?;
    let engine_run_detection = ctx.engine.invocation(
        ctx.front_matter,
        extensions,
        "/tmp/awf-tools/threat-analysis-prompt.md",
        None,
    )?;
    let engine_install_steps = ctx.engine.install_steps(&front_matter.engine)?;

    // 5. Compute workspace, working directory, triggers
    let effective_workspace = compute_effective_workspace(
        &front_matter.workspace,
        &front_matter.checkout,
        &front_matter.name,
    )?;
    let working_directory = generate_working_directory(&effective_workspace);
    let trigger_repo_directory = generate_trigger_repo_directory(&front_matter.checkout);
    let pipeline_resources = generate_pipeline_resources(&front_matter.on_config)?;
    let has_schedule = front_matter.has_schedule();
    let pr_trigger = generate_pr_trigger(&front_matter.on_config, has_schedule);
    let ci_trigger = generate_ci_trigger(&front_matter.on_config, has_schedule);

    // 6. Generate source path and pipeline path
    let source_path = generate_source_path(input_path);
    let pipeline_path = generate_pipeline_path(output_path);

    // 7. Pool name
    let pool = front_matter
        .pool
        .as_ref()
        .map(|p| p.name().to_string())
        .unwrap_or_else(|| DEFAULT_POOL.to_string());

    // 8. Setup/teardown jobs, parameters, prepare/finalize steps
    let pr_filters = front_matter.pr_filters();
    let has_pr_filters = pr_filters.is_some();
    let pipeline_filters = front_matter.pipeline_filters();
    let has_pipeline_filters = pipeline_filters.is_some();
    let setup_job = generate_setup_job(&front_matter.setup, &pool, pr_filters, pipeline_filters, extensions, ctx)?;
    let teardown_job = generate_teardown_job(&front_matter.teardown, &pool);
    let has_memory = front_matter
        .tools
        .as_ref()
        .and_then(|t| t.cache_memory.as_ref())
        .is_some_and(|cm| cm.is_enabled());
    let parameters = build_parameters(&front_matter.parameters, has_memory);
    let parameters_yaml = generate_parameters(&parameters)?;
    let prepare_steps = generate_prepare_steps(&front_matter.steps, extensions)?;
    let finalize_steps = generate_finalize_steps(&front_matter.post_steps);
    let pr_expression = pr_filters.and_then(|f| f.expression.as_deref());
    let pipeline_expression = pipeline_filters.and_then(|f| f.expression.as_deref());
    let expression = pr_expression.or(pipeline_expression);

    // Validate expression escape hatch against injection
    if let Some(expr) = expression {
        if crate::validate::contains_template_marker(expr) {
            anyhow::bail!(
                "Filter expression contains template marker '{{{{' which could cause injection. Found: '{}'",
                expr
            );
        }
        if crate::validate::contains_pipeline_command(expr) {
            anyhow::bail!(
                "Filter expression contains pipeline command ('##vso[' or '##[') which is not allowed. Found: '{}'",
                expr
            );
        }
    }

    let agentic_depends_on = generate_agentic_depends_on(
        &front_matter.setup,
        has_pr_filters,
        has_pipeline_filters,
        expression,
    );
    let job_timeout = generate_job_timeout(front_matter);

    // 9. Token acquisition and env vars
    let acquire_read_token = generate_acquire_ado_token(
        front_matter
            .permissions
            .as_ref()
            .and_then(|p| p.read.as_deref()),
        "SC_READ_TOKEN",
    );
    let mut engine_env = ctx.engine.env(&front_matter.engine)?;

    // Append GITHUB_PATH env mapping when extensions declare path prepends
    let awf_path_env = generate_awf_path_env(config.has_awf_paths);
    if !awf_path_env.is_empty() {
        engine_env = format!("{engine_env}\n{awf_path_env}");
    }
    let engine_log_dir = ctx.engine.log_dir();
    let acquire_write_token = generate_acquire_ado_token(
        front_matter
            .permissions
            .as_ref()
            .and_then(|p| p.write.as_deref()),
        "SC_WRITE_TOKEN",
    );
    let executor_ado_env = generate_executor_ado_env(
        front_matter
            .permissions
            .as_ref()
            .and_then(|p| p.write.as_deref()),
    );

    // 10. Validations
    validate_write_permissions(front_matter)?;
    validate_comment_target(front_matter)?;
    validate_update_work_item_target(front_matter)?;
    validate_submit_pr_review_events(front_matter)?;
    validate_update_pr_votes(front_matter)?;
    validate_resolve_pr_thread_statuses(front_matter)?;

    // 11. Threat analysis prompt
    let threat_analysis_prompt = include_str!("../data/threat-analysis.md");
    let template = replace_with_indent(
        &config.template,
        "{{ threat_analysis_prompt }}",
        threat_analysis_prompt,
    );

    // 12. Debug pipeline replacements (MUST run before extra_replacements
    //     because the probe step content contains {{ mcpg_port }} which is
    //     resolved by extra_replacements).
    let debug_replacements = generate_debug_pipeline_replacements(config.debug_pipeline);
    let mut template = template;
    for (placeholder, replacement) in &debug_replacements {
        template = replace_with_indent(&template, placeholder, replacement);
    }

    // 13. Apply extra replacements (target-specific overrides like {{ mcpg_port }})
    // These run before shared replacements so targets can override shared
    // markers like {{ setup_job }} and {{ teardown_job }}.
    for (placeholder, replacement) in &config.extra_replacements {
        template = replace_with_indent(&template, placeholder, replacement);
    }

    // 14. Shared replacements
    let compiler_version = env!("CARGO_PKG_VERSION");
    let integrity_check = generate_integrity_check(config.skip_integrity);
    let replacements: Vec<(&str, &str)> = vec![
        ("{{ parameters }}", &parameters_yaml),
        ("{{ compiler_version }}", compiler_version),
        ("{{ engine_install_steps }}", &engine_install_steps),
        ("{{ pool }}", &pool),
        ("{{ setup_job }}", &setup_job),
        ("{{ teardown_job }}", &teardown_job),
        ("{{ prepare_steps }}", &prepare_steps),
        ("{{ finalize_steps }}", &finalize_steps),
        ("{{ agentic_depends_on }}", &agentic_depends_on),
        ("{{ job_timeout }}", &job_timeout),
        ("{{ repositories }}", &repositories),
        ("{{ schedule }}", &schedule),
        ("{{ pipeline_resources }}", &pipeline_resources),
        ("{{ pr_trigger }}", &pr_trigger),
        ("{{ ci_trigger }}", &ci_trigger),
        ("{{ checkout_self }}", &checkout_self),
        ("{{ checkout_repositories }}", &checkout_steps),
        ("{{ agent }}", &agent_name),
        ("{{ agent_name }}", &front_matter.name),
        ("{{ agent_description }}", &front_matter.description),
        ("{{ engine_run }}", &engine_run),
        ("{{ engine_run_detection }}", &engine_run_detection),
        ("{{ source_path }}", &source_path),
        // integrity_check must come before pipeline_path because the
        // integrity step content itself contains {{ pipeline_path }}.
        ("{{ integrity_check }}", &integrity_check),
        ("{{ pipeline_path }}", &pipeline_path),
        // trigger_repo_directory must come after source_path / pipeline_path
        // because those expansions embed the placeholder.
        ("{{ trigger_repo_directory }}", &trigger_repo_directory),
        ("{{ working_directory }}", &working_directory),
        ("{{ workspace }}", &working_directory),
        ("{{ agent_content }}", markdown_body),
        ("{{ acquire_ado_token }}", &acquire_read_token),
        ("{{ engine_env }}", &engine_env),
        ("{{ engine_log_dir }}", engine_log_dir),
        ("{{ acquire_write_token }}", &acquire_write_token),
        ("{{ executor_ado_env }}", &executor_ado_env),
    ];

    let pipeline_yaml = replacements
        .into_iter()
        .fold(template, |yaml, (placeholder, replacement)| {
            replace_with_indent(&yaml, placeholder, replacement)
        });

    // 15. Prepend header
    let header = generate_header_comment(input_path);
    Ok(format!("{}{}", header, pipeline_yaml))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::types::{McpConfig, McpOptions, Repository};
    use crate::compile::extensions::{CompileContext, collect_extensions};
    use std::collections::HashMap;

    /// Helper: create a minimal FrontMatter by parsing YAML
    fn minimal_front_matter() -> FrontMatter {
        let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
        fm
    }

    // ─── compute_effective_workspace ─────────────────────────────────────────

    #[test]
    fn test_workspace_explicit_root() {
        let ws = compute_effective_workspace(&Some("root".to_string()), &[], "agent").unwrap();
        assert_eq!(ws, "root");
        assert_eq!(generate_working_directory(&ws), "$(Build.SourcesDirectory)");
    }

    #[test]
    fn test_workspace_explicit_repo_with_checkouts() {
        let checkouts = vec!["other-repo".to_string()];
        let ws =
            compute_effective_workspace(&Some("repo".to_string()), &checkouts, "agent").unwrap();
        assert_eq!(ws, "repo");
        assert_eq!(
            generate_working_directory(&ws),
            "$(Build.SourcesDirectory)/$(Build.Repository.Name)"
        );
    }

    #[test]
    fn test_workspace_explicit_self_alias_for_repo() {
        let checkouts = vec!["other-repo".to_string()];
        let ws =
            compute_effective_workspace(&Some("self".to_string()), &checkouts, "agent").unwrap();
        // 'self' is a synonym for 'repo' (the trigger repository).
        assert_eq!(ws, "repo");
        assert_eq!(
            generate_working_directory(&ws),
            "$(Build.SourcesDirectory)/$(Build.Repository.Name)"
        );
    }

    #[test]
    fn test_workspace_implicit_root_no_checkouts() {
        let ws = compute_effective_workspace(&None, &[], "agent").unwrap();
        assert_eq!(ws, "root");
    }

    #[test]
    fn test_workspace_implicit_repo_with_checkouts() {
        let checkouts = vec!["other-repo".to_string()];
        let ws = compute_effective_workspace(&None, &checkouts, "agent").unwrap();
        assert_eq!(ws, "repo");
    }

    #[test]
    fn test_workspace_explicit_repo_no_checkouts_still_returns_repo() {
        // Emits a warning but still returns "repo"
        let ws = compute_effective_workspace(&Some("repo".to_string()), &[], "agent").unwrap();
        assert_eq!(ws, "repo");
    }

    #[test]
    fn test_workspace_explicit_self_no_checkouts_still_returns_repo() {
        // 'self' takes the same code path as 'repo'; it should also warn
        // and still resolve to the repo subfolder.
        let ws = compute_effective_workspace(&Some("self".to_string()), &[], "agent").unwrap();
        assert_eq!(ws, "repo");
    }

    #[test]
    fn test_workspace_explicit_alias_with_traversal_fails() {
        let checkouts = vec!["../sibling".to_string()];
        let err = compute_effective_workspace(
            &Some("../sibling".to_string()),
            &checkouts,
            "agent",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not a safe path"), "msg: {msg}");
    }

    #[test]
    fn test_workspace_explicit_alias_with_slash_fails() {
        let checkouts = vec!["foo/bar".to_string()];
        let err = compute_effective_workspace(
            &Some("foo/bar".to_string()),
            &checkouts,
            "agent",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not a safe path"), "msg: {msg}");
    }

    #[test]
    fn test_workspace_explicit_alias_resolves_to_repo_subdir() {
        let checkouts = vec!["exp23-a7-nw".to_string(), "another-repo".to_string()];
        let ws = compute_effective_workspace(
            &Some("exp23-a7-nw".to_string()),
            &checkouts,
            "agent",
        )
        .unwrap();
        assert_eq!(
            generate_working_directory(&ws),
            "$(Build.SourcesDirectory)/exp23-a7-nw"
        );
    }

    #[test]
    fn test_workspace_explicit_alias_not_in_checkout_fails() {
        let checkouts = vec!["other-repo".to_string()];
        let err = compute_effective_workspace(
            &Some("exp23-a7-nw".to_string()),
            &checkouts,
            "agent",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exp23-a7-nw"), "msg: {msg}");
        assert!(msg.contains("does not match"), "msg: {msg}");
    }

    #[test]
    fn test_workspace_explicit_alias_no_checkouts_fails() {
        let err =
            compute_effective_workspace(&Some("exp23-a7-nw".to_string()), &[], "agent")
                .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exp23-a7-nw"), "msg: {msg}");
        assert!(
            msg.contains("no additional repositories are checked out"),
            "msg: {msg}"
        );
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

    #[test]
    fn test_validate_checkout_list_reserved_name_fails() {
        // A repo aliased "repo" would silently shadow `workspace: repo`, so
        // reject it at compile time.
        let repos = vec![Repository {
            repository: "repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["repo".to_string()];
        let err = validate_checkout_list(&repos, &checkout).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("reserved"), "msg: {msg}");
        assert!(msg.contains("'repo'"), "msg: {msg}");
    }

    // ─── Engine::args (copilot params) ──────────────────────────────────────

    #[test]
    fn test_engine_args_bash_wildcard() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![":*".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(params.contains("--allow-all-tools"), "wildcard bash should emit --allow-all-tools");
        assert!(!params.contains("--allow-tool"), "no individual --allow-tool flags with --allow-all-tools");
    }

    #[test]
    fn test_engine_args_bash_star_wildcard() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["*".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(params.contains("--allow-all-tools"), "\"*\" should behave same as \":*\"");
        assert!(!params.contains("--allow-tool"), "no individual --allow-tool flags with --allow-all-tools");
    }

    #[test]
    fn test_engine_args_bash_disabled() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(!params.contains("shell("));
    }

    #[test]
    fn test_engine_args_allow_all_paths_when_edit_enabled() {
        let fm = minimal_front_matter(); // edit defaults to true, bash defaults to wildcard
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(params.contains("--allow-all-paths"), "edit enabled (default) should emit --allow-all-paths");
        assert!(params.contains("--allow-all-tools"), "default (no bash) should emit --allow-all-tools");
        assert!(!params.contains("--allow-tool"), "no individual --allow-tool flags with --allow-all-tools");
    }

    #[test]
    fn test_engine_args_no_allow_all_paths_when_edit_disabled() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: None,
            edit: Some(false),
            cache_memory: None,
            azure_devops: None,
        });
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(!params.contains("--allow-all-paths"), "edit disabled should NOT emit --allow-all-paths");
        assert!(!params.contains("--allow-tool write"), "edit disabled should NOT emit --allow-tool write");
    }

    #[test]
    fn test_engine_args_allow_all_tools_with_allow_all_paths() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![":*".to_string()]),
            edit: Some(true),
            cache_memory: None,
            azure_devops: None,
        });
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(params.contains("--allow-all-tools"), "wildcard bash should emit --allow-all-tools");
        assert!(params.contains("--allow-all-paths"), "edit enabled should still emit --allow-all-paths");
        assert!(!params.contains("--allow-tool"), "no individual --allow-tool flags");
    }

    #[test]
    fn test_engine_args_lean_adds_bash_commands() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.runtimes = Some(crate::compile::types::RuntimesConfig {
            lean: Some(crate::runtimes::lean::LeanRuntimeConfig::Enabled(true)),
        });
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(params.contains("shell(lean)"), "lean command should be allowed");
        assert!(params.contains("shell(lake)"), "lake command should be allowed");
        assert!(params.contains("shell(elan)"), "elan command should be allowed");
        // Explicit bash commands should still be present
        assert!(params.contains("shell(cat)"), "explicit commands should remain");
    }

    #[test]
    fn test_engine_args_lean_with_unrestricted_bash() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![":*".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.runtimes = Some(crate::compile::types::RuntimesConfig {
            lean: Some(crate::runtimes::lean::LeanRuntimeConfig::Enabled(true)),
        });
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(params.contains("--allow-all-tools"), "wildcard should use --allow-all-tools");
        // Should NOT add individual tool flags when --allow-all-tools is active
        assert!(!params.contains("--allow-tool"), "no individual tool flags with --allow-all-tools");
    }

    #[test]
    fn test_engine_args_custom_mcp_no_mcp_flag() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                ..Default::default()
            }),
        );
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(
            !params.contains("--allow-tool my-tool"),
            "default (all-tools) mode should not emit individual --allow-tool for MCPs"
        );
    }

    #[test]
    fn test_engine_args_allow_tool_for_container_mcp() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                ..Default::default()
            }),
        );
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(params.contains("--allow-tool my-tool"), "container MCP should get --allow-tool");
    }

    #[test]
    fn test_engine_args_allow_tool_for_url_mcp() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.mcp_servers.insert(
            "remote-ado".to_string(),
            McpConfig::WithOptions(McpOptions {
                url: Some("https://mcp.dev.azure.com/myorg".to_string()),
                ..Default::default()
            }),
        );
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(params.contains("--allow-tool remote-ado"), "URL MCP should get --allow-tool");
    }

    #[test]
    fn test_engine_args_no_allow_tool_for_enabled_only_mcp() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::Enabled(true),
        );
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(!params.contains("--allow-tool my-tool"), "Enabled(true) with no container/url should not get --allow-tool");
    }

    #[test]
    fn test_engine_args_allow_tool_mcps_sorted() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.mcp_servers.insert(
            "z-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("alpine".to_string()),
                ..Default::default()
            }),
        );
        fm.mcp_servers.insert(
            "a-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("alpine".to_string()),
                ..Default::default()
            }),
        );
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        let a_pos = params.find("--allow-tool a-tool").expect("a-tool should be present");
        let z_pos = params.find("--allow-tool z-tool").expect("z-tool should be present");
        assert!(a_pos < z_pos, "MCPs should be sorted alphabetically: a-tool before z-tool");
    }

    #[test]
    fn test_engine_args_builtin_mcp_no_mcp_flag() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("ado".to_string(), McpConfig::Enabled(true));
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        // Copilot CLI has no built-in MCPs — all MCPs are handled via the MCP firewall
        assert!(!params.contains("--mcp ado"));
    }

    #[test]
    fn test_engine_args_no_max_timeout() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  timeout-minutes: 30\n---\n",
        )
        .unwrap();
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(!params.contains("--max-timeout"), "timeout-minutes should not be emitted as a CLI arg");
    }

    #[test]
    fn test_engine_args_no_max_timeout_when_simple_engine() {
        let fm = minimal_front_matter();
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
        assert!(!params.contains("--max-timeout"));
    }

    #[test]
    fn test_engine_args_max_timeout_zero_not_emitted() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  timeout-minutes: 0\n---\n",
        )
        .unwrap();
        let params = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm)).unwrap();
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
        let triggers = Some(crate::compile::types::OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(result.contains("pr: none"));
        assert!(result.contains("upstream pipeline"));
    }

    #[test]
    fn test_generate_pr_trigger_both_pipeline_and_schedule() {
        let triggers = Some(crate::compile::types::OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
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
        let triggers = Some(crate::compile::types::OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = generate_ci_trigger(&triggers, false);
        assert_eq!(result, "trigger: none");
    }

    #[test]
    fn test_generate_ci_trigger_both_pipeline_and_schedule() {
        let triggers = Some(crate::compile::types::OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
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
        let triggers = Some(crate::compile::types::OnConfig { schedule: None, pipeline: None, pr: None });
        let result = generate_pipeline_resources(&triggers).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_generate_pipeline_resources_with_branches() {
        let triggers = Some(crate::compile::types::OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".into(),
                project: Some("OtherProject".into()),
                branches: vec!["main".into(), "release/*".into()],
            filters: None,
            }),
        pr: None,
        schedule: None,
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
        let triggers = Some(crate::compile::types::OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "My Pipeline".into(),
                project: None,
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = generate_pipeline_resources(&triggers).unwrap();
        assert!(result.contains("source: 'My Pipeline'"));
        assert!(result.contains("trigger: true"));
        // No project when not specified
        assert!(!result.contains("project:"));
    }

    #[test]
    fn test_generate_pipeline_resources_resource_id_is_snake_case() {
        let triggers = Some(crate::compile::types::OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "My Build Pipeline".into(),
                project: None,
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = generate_pipeline_resources(&triggers).unwrap();
        // The pipeline resource ID should be snake_case derived from the name
        assert!(result.contains("pipeline: my_build_pipeline"));
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
        // Compiling agents/ctf.md should produce the trigger-repo-anchored
        // path so the integrity check / Stage 3 executor find the file in the
        // self repo regardless of the user's workspace setting.
        let path = std::path::Path::new("agents/ctf.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ trigger_repo_directory }}/agents/ctf.md");
    }

    #[test]
    fn test_generate_source_path_nested_directory() {
        let path = std::path::Path::new("pipelines/production/review.md");
        let result = generate_source_path(path);
        assert_eq!(
            result,
            "{{ trigger_repo_directory }}/pipelines/production/review.md"
        );
    }

    #[test]
    fn test_generate_source_path_strips_dot_slash() {
        let path = std::path::Path::new("./agents/my-agent.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ trigger_repo_directory }}/agents/my-agent.md");
    }

    #[test]
    fn test_generate_source_path_filename_only() {
        let path = std::path::Path::new("my-agent.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ trigger_repo_directory }}/my-agent.md");
    }

    // ─── generate_pipeline_path ──────────────────────────────────────────────

    #[test]
    fn test_generate_pipeline_path_preserves_directory() {
        // The original bug: compiling agents/ctf.md produced agents/ctf.yml as
        // output, but the embedded path was only ctf.yml (missing agents/).
        // Pipeline path is relative to the integrity check's workingDirectory
        // ({{ trigger_repo_directory }}), so no prefix is embedded here.
        let path = std::path::Path::new("agents/ctf.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "agents/ctf.yml");
    }

    #[test]
    fn test_generate_pipeline_path_nested_directory() {
        let path = std::path::Path::new("pipelines/production/review.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "pipelines/production/review.yml");
    }

    #[test]
    fn test_generate_pipeline_path_strips_dot_slash() {
        let path = std::path::Path::new("./agents/my-agent.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "agents/my-agent.yml");
    }

    #[test]
    fn test_generate_pipeline_path_filename_only() {
        let path = std::path::Path::new("pipeline.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "pipeline.yml");
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
        assert_eq!(result, "{{ trigger_repo_directory }}/ctf.md");
    }

    #[test]
    fn test_generate_pipeline_path_absolute_falls_back_to_filename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let abs_path = tmp.path().join("agents").join("ctf.yml");
        let result = generate_pipeline_path(&abs_path);
        assert_eq!(result, "ctf.yml");
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
        assert_eq!(result, "{{ trigger_repo_directory }}/agents/ctf.md");
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
        assert_eq!(result, "agents/ctf.yml");
    }

    // ─── generate_trigger_repo_directory ─────────────────────────────────────

    #[test]
    fn test_generate_trigger_repo_directory_no_additional_checkouts() {
        // With only `self` checked out, ADO places the repository content
        // directly into $(Build.SourcesDirectory).
        let result = generate_trigger_repo_directory(&[]);
        assert_eq!(result, "$(Build.SourcesDirectory)");
    }

    #[test]
    fn test_generate_trigger_repo_directory_with_additional_checkouts() {
        // As soon as any additional repo is checked out, ADO places every
        // checked-out repo (including `self`) into a subdirectory named
        // after the repository.
        let result =
            generate_trigger_repo_directory(&["exp23-a7-nw".to_string()]);
        assert_eq!(
            result,
            "$(Build.SourcesDirectory)/$(Build.Repository.Name)"
        );
    }

    #[test]
    fn test_trigger_repo_directory_independent_of_workspace_alias() {
        // Regression: when workspace points at a checked-out alias, the
        // trigger-repo directory must still anchor at the self repo, NOT at
        // the alias subfolder. This is what makes the integrity check
        // (and Stage 3 --source) find the pipeline yaml / agent markdown.
        let checkout = vec!["exp23-a7-nw".to_string()];
        let trigger = generate_trigger_repo_directory(&checkout);
        let workspace = compute_effective_workspace(
            &Some("exp23-a7-nw".to_string()),
            &checkout,
            "ctf",
        )
        .unwrap();
        let working_dir = generate_working_directory(&workspace);

        assert_eq!(
            trigger,
            "$(Build.SourcesDirectory)/$(Build.Repository.Name)"
        );
        assert_eq!(working_dir, "$(Build.SourcesDirectory)/exp23-a7-nw");
        assert_ne!(
            trigger, working_dir,
            "trigger repo dir must differ from working dir when workspace points at an alias"
        );
    }

    // ─── generate_integrity_check ────────────────────────────────────────────

    #[test]
    fn test_generate_integrity_check_default_produces_step() {
        let result = generate_integrity_check(false);
        assert!(
            result.contains("Verify pipeline integrity"),
            "Should contain the displayName"
        );
        assert!(
            result.contains("ado-aw"),
            "Should reference the ado-aw binary"
        );
        assert!(
            result.contains("{{ pipeline_path }}"),
            "Should contain the pipeline_path placeholder for later resolution"
        );
        assert!(
            result.contains("workingDirectory: {{ trigger_repo_directory }}"),
            "Should set workingDirectory to the trigger repo so `ado-aw check` \
             can recompile from a directory that contains .git (needed for \
             ADO org inference when tools.azure-devops is enabled)"
        );
    }

    #[test]
    fn test_generate_integrity_check_skip_produces_empty() {
        let result = generate_integrity_check(true);
        assert!(
            result.is_empty(),
            "Should produce empty string when skipping"
        );
    }

    // ─── generate_debug_pipeline_replacements ────────────────────────────────

    #[test]
    fn test_debug_pipeline_replacements_disabled() {
        let replacements = generate_debug_pipeline_replacements(false);
        assert_eq!(replacements.len(), 2);
        // mcpg_debug_flags returns `\` for bash line continuation
        let flags = replacements.iter().find(|(m, _)| m == "{{ mcpg_debug_flags }}").unwrap();
        assert_eq!(flags.1, "\\", "mcpg_debug_flags should be a bare backslash when disabled");
        // verify_mcp_backends should be empty
        let probe = replacements.iter().find(|(m, _)| m == "{{ verify_mcp_backends }}").unwrap();
        assert!(probe.1.is_empty(), "verify_mcp_backends should be empty when disabled");
    }

    #[test]
    fn test_debug_pipeline_replacements_enabled() {
        let replacements = generate_debug_pipeline_replacements(true);
        assert_eq!(replacements.len(), 2);

        let flags = replacements.iter().find(|(m, _)| m == "{{ mcpg_debug_flags }}");
        assert!(flags.is_some(), "Should have mcpg_debug_flags marker");
        let flags_value = &flags.unwrap().1;
        assert!(flags_value.contains("DEBUG"), "Should contain DEBUG env var");

        let probe = replacements.iter().find(|(m, _)| m == "{{ verify_mcp_backends }}");
        assert!(probe.is_some(), "Should have verify_mcp_backends marker");
        let probe_value = &probe.unwrap().1;
        assert!(probe_value.contains("Verify MCP backends"), "Should contain displayName");
        assert!(probe_value.contains("tools/list"), "Should contain tools/list probe");
        assert!(probe_value.contains("initialize"), "Should contain initialize handshake");
        assert!(probe_value.contains("MCPG_API_KEY"), "Should contain API key env mapping");
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
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-thread:\n    allowed-repositories:\n      - self\n---\n"
        ).unwrap();
        let result = validate_resolve_pr_thread_statuses(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-statuses"), "message: {msg}");
    }

    #[test]
    fn test_resolve_pr_thread_fails_when_allowed_statuses_empty() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-thread:\n    allowed-statuses: []\n---\n"
        ).unwrap();
        let result = validate_resolve_pr_thread_statuses(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-statuses"), "message: {msg}");
    }

    #[test]
    fn test_resolve_pr_thread_fails_when_value_is_scalar() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-thread: true\n---\n"
        ).unwrap();
        let result = validate_resolve_pr_thread_statuses(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_pr_thread_passes_when_statuses_provided() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-thread:\n    allowed-statuses:\n      - fixed\n      - wont-fix\n---\n"
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
        assert!(validate::is_safe_tool_name("create-pull-request"));
        assert!(validate::is_safe_tool_name("noop"));
        assert!(validate::is_safe_tool_name("my-tool-123"));
        assert!(!validate::is_safe_tool_name(""));
        assert!(!validate::is_safe_tool_name("$(curl evil.com)"));
        assert!(!validate::is_safe_tool_name("foo; rm -rf /"));
        assert!(!validate::is_safe_tool_name("tool name"));
        assert!(!validate::is_safe_tool_name("tool\ttab"));
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
    fn test_generate_enabled_tools_args_memory_no_longer_safe_output() {
        // `memory` is no longer a safe-output key — it moved to `tools: cache-memory:`.
        // If someone still puts it in safe-outputs, it should be treated as unrecognized
        // and the real MCP tool should still be present.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  create-pull-request:\n    target-branch: main\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(args.contains("--enabled-tools create-pull-request"), "Real MCP tool should be present");
    }

    #[test]
    fn test_generate_enabled_tools_args_empty_safe_outputs_no_filter() {
        // When safe-outputs is empty, no --enabled-tools args should be generated
        // so all tools remain available.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(args.is_empty(), "empty safe-outputs should produce no args (all tools available)");
    }

    // ─── parameter name validation ──────────────────────────────────────────

    #[test]
    fn test_is_valid_parameter_name() {
        assert!(validate::is_valid_parameter_name("clearMemory"));
        assert!(validate::is_valid_parameter_name("myParam"));
        assert!(validate::is_valid_parameter_name("_private"));
        assert!(validate::is_valid_parameter_name("param123"));
        assert!(!validate::is_valid_parameter_name(""));
        assert!(!validate::is_valid_parameter_name("has space"));
        assert!(!validate::is_valid_parameter_name("has-dash"));
        assert!(!validate::is_valid_parameter_name("${{inject}}"));
        assert!(!validate::is_valid_parameter_name("123startsWithDigit"));
    }

    #[test]
    fn test_generate_parameters_rejects_invalid_name() {
        let params = vec![PipelineParameter {
            name: "${{evil}}".to_string(),
            display_name: None,
            param_type: None,
            default: None,
            values: None,
        }];
        let result = generate_parameters(&params);
        assert!(result.is_err(), "Should reject invalid parameter name");
        assert!(
            result.unwrap_err().to_string().contains("Invalid parameter name"),
            "Error should mention invalid parameter name"
        );
    }

    #[test]
    fn test_build_parameters_auto_injects_clear_memory() {
        let params = build_parameters(&[], true);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "clearMemory");
    }

    #[test]
    fn test_build_parameters_no_inject_without_memory() {
        let params = build_parameters(&[], false);
        assert!(params.is_empty());
    }

    #[test]
    fn test_build_parameters_no_duplicate_clear_memory() {
        let user = vec![PipelineParameter {
            name: "clearMemory".to_string(),
            display_name: Some("Custom".to_string()),
            param_type: Some("boolean".to_string()),
            default: Some(serde_yaml::Value::Bool(true)),
            values: None,
        }];
        let params = build_parameters(&user, true);
        assert_eq!(params.len(), 1, "Should not duplicate clearMemory");
        assert_eq!(params[0].display_name.as_deref(), Some("Custom"), "Should keep user's definition");
    }

    #[test]
    fn test_generate_parameters_rejects_expression_in_display_name() {
        let params = vec![PipelineParameter {
            name: "myParam".to_string(),
            display_name: Some("Test ${{ variables.evil }}".to_string()),
            param_type: None,
            default: None,
            values: None,
        }];
        let result = generate_parameters(&params);
        assert!(result.is_err(), "Should reject ADO expression in displayName");
    }

    #[test]
    fn test_generate_parameters_rejects_expression_in_default() {
        let params = vec![PipelineParameter {
            name: "myParam".to_string(),
            display_name: None,
            param_type: None,
            default: Some(serde_yaml::Value::String("$(secretVar)".to_string())),
            values: None,
        }];
        let result = generate_parameters(&params);
        assert!(result.is_err(), "Should reject ADO macro expression in default");
    }

    #[test]
    fn test_generate_parameters_rejects_expression_in_values() {
        let params = vec![PipelineParameter {
            name: "myParam".to_string(),
            display_name: None,
            param_type: None,
            default: None,
            values: Some(vec![
                serde_yaml::Value::String("safe".to_string()),
                serde_yaml::Value::String("${{ parameters.inject }}".to_string()),
            ]),
        }];
        let result = generate_parameters(&params);
        assert!(result.is_err(), "Should reject ADO expression in values");
    }

    #[test]
    fn test_generate_parameters_allows_literal_values() {
        let params = vec![PipelineParameter {
            name: "region".to_string(),
            display_name: Some("Target Region".to_string()),
            param_type: Some("string".to_string()),
            default: Some(serde_yaml::Value::String("us-east".to_string())),
            values: Some(vec![
                serde_yaml::Value::String("us-east".to_string()),
                serde_yaml::Value::String("eu-west".to_string()),
            ]),
        }];
        let result = generate_parameters(&params);
        assert!(result.is_ok(), "Should accept literal values");
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

    // ─── engine env / generate_executor_ado_env ────────────────────────────

    #[test]
    fn test_engine_env() {
        let fm = minimal_front_matter();
        let ctx = CompileContext::for_test(&fm);
        let result = ctx.engine.env(&fm.engine).unwrap();
        assert!(
            result.contains("GITHUB_TOKEN: $(GITHUB_TOKEN)"),
            "Should include GITHUB_TOKEN"
        );
        assert!(
            !result.contains("AZURE_DEVOPS_EXT_PAT"),
            "ADO token is handled by MCPG, not engine env"
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

    // ─── Security validation tests ────────────────────────────────────────────

    #[test]
    fn test_model_name_rejects_single_quote() {
        let mut fm = minimal_front_matter();
        fm.engine = crate::compile::types::EngineConfig::Full(crate::compile::types::EngineOptions {
            id: Some("copilot".to_string()),
            model: Some("model' && echo pwned".to_string()),
            version: None, agent: None, api_target: None,
            args: vec![], env: None, command: None,
            timeout_minutes: None,
        });
        let result = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }

    #[test]
    fn test_model_name_rejects_space() {
        let mut fm = minimal_front_matter();
        fm.engine = crate::compile::types::EngineConfig::Full(crate::compile::types::EngineOptions {
            id: Some("copilot".to_string()),
            model: Some("model && curl evil.com".to_string()),
            version: None, agent: None, api_target: None,
            args: vec![], env: None, command: None,
            timeout_minutes: None,
        });
        let result = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm));
        assert!(result.is_err());
    }

    #[test]
    fn test_model_name_allows_valid_names() {
        for name in &["claude-opus-4.5", "gpt-5.2-codex", "gemini-3-pro-preview", "my_model:latest"] {
            let mut fm = minimal_front_matter();
            fm.engine = crate::compile::types::EngineConfig::Full(crate::compile::types::EngineOptions {
                id: Some("copilot".to_string()),
                model: Some(name.to_string()),
                version: None, agent: None, api_target: None,
                args: vec![], env: None, command: None,
                timeout_minutes: None,
            });
            let result = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm));
            assert!(result.is_ok(), "Model name '{}' should be valid", name);
        }
    }

    #[test]
    fn test_bash_command_rejects_single_quote() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat'".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        let result = CompileContext::for_test(&fm).engine.args(&fm, &crate::compile::extensions::collect_extensions(&fm));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("single quote"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_name() {
        let mut fm = minimal_front_matter();
        fm.name = "My Agent ${{ variables['System.AccessToken'] }}".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_macro_in_description() {
        let mut fm = minimal_front_matter();
        fm.description = "Agent $(System.AccessToken)".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_name() {
        let mut fm = minimal_front_matter();
        fm.name = "My Agent\ninjected: true".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("single line"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_template_marker_in_name() {
        let mut fm = minimal_front_matter();
        fm.name = "{{ agent_content }}".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("template marker"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_template_marker_in_description() {
        let mut fm = minimal_front_matter();
        fm.description = "{{ copilot_params }}".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("template marker"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_trigger_pipeline_name() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build\ninjected: true".to_string(),
                project: None,
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("on.pipeline.name"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_trigger_pipeline_project() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: Some("OtherProject\ninjected: true".to_string()),
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("on.pipeline.project"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_trigger_pipeline_branch() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: None,
                branches: vec!["main\ninjected: true".to_string()],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("on.pipeline.branches"));
    }

    #[test]
    fn test_validate_front_matter_identity_allows_valid_name_and_description() {
        let mut fm = minimal_front_matter();
        fm.name = "Daily Code Review Agent".to_string();
        fm.description = "Reviews code daily for quality issues".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_front_matter_identity_allows_valid_trigger_pipeline_fields() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: Some("OtherProject".to_string()),
                branches: vec!["main".to_string(), "release/*".to_string()],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_runtime_expression() {
        let mut fm = minimal_front_matter();
        fm.name = "Agent $[variables['System.AccessToken']]".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_trigger_pipeline_name() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build $(System.AccessToken)".to_string(),
                project: None,
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_trigger_pipeline_project() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: Some("$(System.AccessToken)".to_string()),
                branches: vec![],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_trigger_pipeline_branch() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: None,
                branches: vec!["$[variables['token']]".to_string()],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_pipeline_resources_escapes_single_quotes() {
        let triggers = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build's Pipeline".to_string(),
                project: Some("My'Project".to_string()),
                branches: vec!["main".to_string(), "it's-branch".to_string()],
            filters: None,
            }),
        pr: None,
        schedule: None,
        });
        let result = generate_pipeline_resources(&triggers).unwrap();
        assert!(result.contains("source: 'Build''s Pipeline'"));
        assert!(result.contains("project: 'My''Project'"));
        assert!(result.contains("- 'it''s-branch'"));
    }

    // ─── generate_prepare_steps ──────────────────────────────────────────────

    #[test]
    fn test_generate_prepare_steps_with_memory_includes_memory_preamble() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  cache-memory: true\n---\n",
        ).unwrap();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let result = generate_prepare_steps(&[], &exts).unwrap();
        assert!(
            !result.is_empty(),
            "memory steps must be emitted when cache-memory enabled"
        );
        assert!(
            result.contains("agent_memory"),
            "should reference memory directory"
        );
    }

    #[test]
    fn test_generate_prepare_steps_without_memory_and_no_steps_has_safeoutputs_prompt() {
        let fm = minimal_front_matter();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let result = generate_prepare_steps(&[], &exts).unwrap();
        // SafeOutputs always contributes a prompt supplement
        assert!(
            result.contains("Safe Outputs"),
            "should include SafeOutputs prompt supplement"
        );
    }

    #[test]
    fn test_generate_prepare_steps_with_memory_includes_download_and_prompt() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  cache-memory: true\n---\n",
        ).unwrap();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let result = generate_prepare_steps(&[], &exts).unwrap();
        assert!(
            result.contains("DownloadPipelineArtifact"),
            "memory steps must include the artifact download task"
        );
        assert!(
            result.contains("Agent Memory"),
            "memory steps must include the memory prompt"
        );
    }

    #[test]
    fn test_generate_prepare_steps_without_memory_with_user_steps() {
        let fm = minimal_front_matter();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let step: serde_yaml::Value =
            serde_yaml::from_str("bash: echo hello\ndisplayName: greet").unwrap();
        let result = generate_prepare_steps(&[step], &exts).unwrap();
        assert!(!result.is_empty(), "user steps should be present");
        assert!(
            !result.contains("agent_memory"),
            "no memory reference when cache-memory not enabled"
        );
    }

    #[test]
    fn test_generate_prepare_steps_with_memory_and_user_steps() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  cache-memory: true\n---\n",
        ).unwrap();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let step: serde_yaml::Value =
            serde_yaml::from_str("bash: echo hello\ndisplayName: greet").unwrap();
        let result = generate_prepare_steps(&[step], &exts).unwrap();
        assert!(
            result.contains("agent_memory"),
            "memory reference must be present"
        );
        assert!(
            result.contains("echo hello"),
            "user step must also be present"
        );
    }

    #[test]
    fn test_generate_prepare_steps_with_lean() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  lean: true\n---\n",
        ).unwrap();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let result = generate_prepare_steps(&[], &exts).unwrap();
        assert!(result.contains("elan-init.sh"), "should include elan installer");
        assert!(result.contains("Lean 4"), "should include Lean prompt");
        assert!(result.contains("--default-toolchain stable"), "should default to stable");
        assert!(result.contains("/tmp/awf-tools/"), "should symlink into awf-tools for AWF chroot");
    }

    #[test]
    fn test_generate_prepare_steps_with_lean_custom_toolchain() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  lean:\n    toolchain: \"leanprover/lean4:v4.29.1\"\n---\n",
        ).unwrap();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let result = generate_prepare_steps(&[], &exts).unwrap();
        assert!(
            result.contains("--default-toolchain leanprover/lean4:v4.29.1"),
            "should use specified toolchain"
        );
    }

    #[test]
    fn test_generate_prepare_steps_with_lean_and_memory() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  lean: true\ntools:\n  cache-memory: true\n---\n",
        ).unwrap();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let result = generate_prepare_steps(&[], &exts).unwrap();
        assert!(result.contains("agent_memory"), "memory steps present");
        assert!(result.contains("elan-init.sh"), "lean install present");
        assert!(result.contains("Lean 4"), "lean prompt present");
    }

    // ─── generate_awf_mounts ──────────────────────────────────────────────

    #[test]
    fn test_generate_awf_mounts_no_extensions() {
        let fm = minimal_front_matter();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let result = generate_awf_mounts(&exts);
        assert_eq!(result, "\\", "no mounts should produce bare continuation");
    }

    #[test]
    fn test_generate_awf_mounts_with_lean() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  lean: true\n---\n",
        ).unwrap();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let result = generate_awf_mounts(&exts);
        assert!(result.contains("--mount"), "should contain --mount flag");
        assert!(result.contains(".elan"), "should reference .elan directory");
        assert!(result.contains(":ro"), "should be read-only");
        // Each mount line ends with ` \` continuation
        assert!(result.ends_with(" \\"), "last mount should end with continuation");
        // No embedded indent — replace_with_indent handles indentation
        assert!(!result.contains("            "), "should not contain hard-coded indent");
    }

    // ─── generate_awf_path_step ──────────────────────────────────────────────

    #[test]
    fn test_generate_awf_path_step_no_paths() {
        let result = generate_awf_path_step(&[]);
        assert!(result.is_empty(), "no path prepends should produce empty string");
    }

    #[test]
    fn test_generate_awf_path_step_with_lean() {
        let paths = collect_awf_path_prepends(
            &crate::compile::extensions::collect_extensions(
                &parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  lean: true\n---\n").unwrap().0,
            ),
        );
        let result = generate_awf_path_step(&paths);
        assert!(result.contains("ado-path-entries"), "should reference path entries file");
        assert!(result.contains(".elan/bin"), "should include elan bin path");
        assert!(result.contains("GITHUB_PATH"), "should set GITHUB_PATH variable");
        assert!(result.contains("displayName"), "should be a complete pipeline step");
        assert!(result.contains("AWF_PATH_EOF"), "should use heredoc markers");
    }

    #[test]
    fn test_generate_awf_path_step_multi_path_indentation() {
        let paths = vec![
            "$HOME/.elan/bin".to_string(),
            "$HOME/.other-tool/bin".to_string(),
        ];
        let result = generate_awf_path_step(&paths);
        // Every path line must have consistent 4-space indentation
        for path in &paths {
            assert!(
                result.contains(&format!("    {path}")),
                "path '{path}' should have 4-space indentation"
            );
        }
    }

    // ─── generate_awf_path_env ──────────────────────────────────────────────

    #[test]
    fn test_generate_awf_path_env_no_paths() {
        let result = generate_awf_path_env(false);
        assert!(result.is_empty(), "no path prepends should produce empty string");
    }

    #[test]
    fn test_generate_awf_path_env_with_paths() {
        let result = generate_awf_path_env(true);
        assert_eq!(result, "GITHUB_PATH: $(GITHUB_PATH)");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Tests moved from standalone.rs — MCPG config, docker env, validation
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_generate_firewall_config_custom_mcp() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("node".to_string()),
                entrypoint_args: vec!["server.js".to_string()],
                allowed: vec!["do_thing".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let server = config.mcp_servers.get("my-tool").unwrap();
        assert_eq!(server.server_type, "stdio");
        assert_eq!(server.container.as_ref().unwrap(), "node:20-slim");
        assert_eq!(server.entrypoint.as_ref().unwrap(), "node");
        assert_eq!(
            server.entrypoint_args.as_ref().unwrap(),
            &vec!["server.js"]
        );
        assert_eq!(
            server.tools.as_ref().unwrap(),
            &vec!["do_thing".to_string()]
        );
    }

    #[test]
    fn test_generate_mcpg_config_mcp_without_transport_skipped() {
        let mut fm = minimal_front_matter();
        // An MCP with no container or url should be skipped
        fm.mcp_servers
            .insert("phantom".to_string(), McpConfig::Enabled(true));
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        assert!(!config.mcp_servers.contains_key("phantom"));
        // safeoutputs is always present
        assert!(config.mcp_servers.contains_key("safeoutputs"));
    }

    #[test]
    fn test_generate_mcpg_config_disabled_mcp_skipped() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("my-tool".to_string(), McpConfig::Enabled(false));
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        assert!(!config.mcp_servers.contains_key("my-tool"));
    }

    #[test]
    fn test_generate_mcpg_config_empty_mcp_servers() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        // Only safeoutputs should be present
        assert_eq!(config.mcp_servers.len(), 1);
        assert!(config.mcp_servers.contains_key("safeoutputs"));
    }

    #[test]
    fn test_generate_mcpg_config_gateway_defaults() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        assert_eq!(config.gateway.port, 80);
        assert_eq!(config.gateway.domain, "host.docker.internal");
        assert_eq!(config.gateway.api_key, "${MCP_GATEWAY_API_KEY}");
        assert_eq!(config.gateway.payload_dir, "/tmp/gh-aw/mcp-payloads");
    }

    #[test]
    fn test_generate_mcpg_config_json_roundtrip() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("python:3.12-slim".to_string()),
                entrypoint: Some("python".to_string()),
                entrypoint_args: vec!["-m".to_string(), "server".to_string()],
                allowed: vec!["query".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let json = serde_json::to_string_pretty(&config).expect("Config should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("Serialized JSON should parse back");

        // Verify top-level structure matches MCPG expectation
        assert!(
            parsed.get("mcpServers").is_some(),
            "Should have mcpServers key"
        );
        assert!(parsed.get("gateway").is_some(), "Should have gateway key");

        let gw = parsed.get("gateway").unwrap();
        assert!(gw.get("port").is_some(), "Gateway should have port");
        assert!(gw.get("domain").is_some(), "Gateway should have domain");
        assert!(gw.get("apiKey").is_some(), "Gateway should have apiKey");
        assert!(
            gw.get("payloadDir").is_some(),
            "Gateway should have payloadDir"
        );
    }

    #[test]
    fn test_generate_mcpg_config_safeoutputs_variable_placeholders() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let so = config.mcp_servers.get("safeoutputs").unwrap();

        // URL should reference the runtime-substituted port
        let url = so.url.as_ref().unwrap();
        assert!(
            url.contains("${SAFE_OUTPUTS_PORT}"),
            "SafeOutputs URL should use ${{SAFE_OUTPUTS_PORT}} placeholder, got: {url}"
        );

        // Auth header should reference the runtime-substituted API key
        let headers = so.headers.as_ref().unwrap();
        let auth = headers.get("Authorization").unwrap();
        assert!(
            auth.contains("${SAFE_OUTPUTS_API_KEY}"),
            "SafeOutputs auth header should use ${{SAFE_OUTPUTS_API_KEY}} placeholder, got: {auth}"
        );
    }

    #[test]
    fn test_generate_mcpg_config_safeoutputs_is_http_type() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let so = config.mcp_servers.get("safeoutputs").unwrap();
        assert_eq!(so.server_type, "http");
        assert!(
            so.container.is_none(),
            "HTTP backend should have no container"
        );
        assert!(so.args.is_none(), "HTTP backend should have no args");
        assert!(so.url.is_some(), "HTTP backend must have a URL");
    }

    #[test]
    fn test_generate_mcpg_config_container_mcp_is_stdio_type() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "runner".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("node".to_string()),
                entrypoint_args: vec!["srv.js".to_string()],
                allowed: vec!["run".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let srv = config.mcp_servers.get("runner").unwrap();
        assert_eq!(srv.server_type, "stdio");
        assert!(
            srv.container.is_some(),
            "stdio server must have a container"
        );
        assert!(srv.url.is_none(), "stdio server should have no URL");
    }

    #[test]
    fn test_generate_mcpg_config_container_with_env() {
        let mut fm = minimal_front_matter();
        let mut env = HashMap::new();
        env.insert("TOKEN".to_string(), "secret".to_string());
        fm.mcp_servers.insert(
            "with-env".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                env,
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let srv = config.mcp_servers.get("with-env").unwrap();
        let e = srv.env.as_ref().unwrap();
        assert_eq!(e.get("TOKEN").unwrap(), "secret");
    }

    #[test]
    fn test_generate_mcpg_config_reserved_safeoutputs_name_rejected() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "safeoutputs".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("evil:latest".to_string()),
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        // The reserved entry should still be the HTTP backend, not the user's container
        let so = config.mcp_servers.get("safeoutputs").unwrap();
        assert_eq!(
            so.server_type, "http",
            "safeoutputs should remain HTTP backend"
        );
        assert!(
            so.container.is_none(),
            "User container should not overwrite safeoutputs"
        );
    }

    #[test]
    fn test_generate_mcpg_config_safeoutputs_reserved_name_skipped() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "SafeOutputs".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("node".to_string()),
                entrypoint_args: vec!["evil.js".to_string()],
                allowed: vec!["hijack".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        // The user-defined "SafeOutputs" must not overwrite the built-in entry
        let so = config.mcp_servers.get("safeoutputs").unwrap();
        assert_eq!(so.server_type, "http");
        assert!(so.url.as_ref().unwrap().contains("localhost"));
        // No stdio entry should have been added under any casing
        assert_eq!(config.mcp_servers.len(), 1);
    }

    #[test]
    fn test_generate_mcpg_config_http_mcp() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "remote".to_string(),
            McpConfig::WithOptions(McpOptions {
                url: Some("https://mcp.example.com/api".to_string()),
                headers: {
                    let mut h = HashMap::new();
                    h.insert("X-Custom".to_string(), "value".to_string());
                    h
                },
                allowed: vec!["query".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let srv = config.mcp_servers.get("remote").unwrap();
        assert_eq!(srv.server_type, "http");
        assert_eq!(
            srv.url.as_ref().unwrap(),
            "https://mcp.example.com/api"
        );
        assert_eq!(
            srv.headers.as_ref().unwrap().get("X-Custom").unwrap(),
            "value"
        );
        assert!(srv.container.is_none(), "HTTP server should have no container");
    }

    #[test]
    fn test_generate_mcpg_config_container_with_entrypoint() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "ado".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("npx".to_string()),
                entrypoint_args: vec!["-y".to_string(), "@azure-devops/mcp".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let srv = config.mcp_servers.get("ado").unwrap();
        assert_eq!(srv.server_type, "stdio");
        assert_eq!(srv.container.as_ref().unwrap(), "node:20-slim");
        assert_eq!(srv.entrypoint.as_ref().unwrap(), "npx");
        assert_eq!(
            srv.entrypoint_args.as_ref().unwrap(),
            &vec!["-y", "@azure-devops/mcp"]
        );
    }

    #[test]
    fn test_generate_mcpg_config_container_with_mounts() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "data-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("data-tool:latest".to_string()),
                mounts: vec!["/host/data:/app/data:ro".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let srv = config.mcp_servers.get("data-tool").unwrap();
        assert_eq!(
            srv.mounts.as_ref().unwrap(),
            &vec!["/host/data:/app/data:ro"]
        );
    }

    #[test]
    fn test_generate_mcpg_config_no_transport_skipped() {
        let mut fm = minimal_front_matter();
        // MCP with options but no container or url should be skipped
        fm.mcp_servers.insert(
            "no-transport".to_string(),
            McpConfig::WithOptions(McpOptions {
                allowed: vec!["tool".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        assert!(!config.mcp_servers.contains_key("no-transport"));
    }

    #[test]
    fn test_generate_mcpg_docker_env_with_permissions_read() {
        // When ADO tool is enabled with permissions.read, the extension's
        // required_pipeline_vars should produce the -e flag
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\npermissions:\n  read: my-read-sc\n---\n",
        ).unwrap();
        let extensions = collect_extensions(&fm);
        let env = generate_mcpg_docker_env(&fm, &extensions);
        assert!(
            env.contains("-e ADO_MCP_AUTH_TOKEN=\"$SC_READ_TOKEN\""),
            "Should map ADO token via extension pipeline var"
        );
    }

    #[test]
    fn test_generate_mcpg_docker_env_no_extensions() {
        // No tools enabled — no extension pipeline vars — only user MCP passthrough
        let fm = minimal_front_matter();
        let extensions = collect_extensions(&fm);
        let env = generate_mcpg_docker_env(&fm, &extensions);
        assert!(
            !env.contains("ADO_MCP_AUTH_TOKEN"),
            "Should not have ADO token when no extension needs it"
        );
    }

    #[test]
    fn test_generate_mcpg_docker_env_dedup_extension_and_user_passthrough() {
        // Extension provides ADO_MCP_AUTH_TOKEN mapping, user MCP also has it as passthrough.
        // Extension mapping should win (deduplicated).
        let (mut fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\npermissions:\n  read: my-read-sc\n---\n",
        ).unwrap();
        fm.mcp_servers.insert(
            "ado-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                env: {
                    let mut e = HashMap::new();
                    e.insert("ADO_MCP_AUTH_TOKEN".to_string(), "".to_string());
                    e
                },
                ..Default::default()
            }),
        );
        let extensions = collect_extensions(&fm);
        let env = generate_mcpg_docker_env(&fm, &extensions);
        let count = env.matches("ADO_MCP_AUTH_TOKEN").count();
        assert_eq!(count, 1, "ADO_MCP_AUTH_TOKEN should appear exactly once, got {}", count);
    }

    #[test]
    fn test_generate_mcpg_docker_env_passthrough_vars() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("img:latest".to_string()),
                env: {
                    let mut e = HashMap::new();
                    e.insert("PASS_THROUGH".to_string(), "".to_string());
                    e.insert("STATIC".to_string(), "value".to_string());
                    e
                },
                ..Default::default()
            }),
        );
        let extensions = collect_extensions(&fm);
        let env = generate_mcpg_docker_env(&fm, &extensions);
        assert!(env.contains("-e PASS_THROUGH"), "Should include passthrough var");
        assert!(!env.contains("-e STATIC"), "Should NOT include static var");
    }

    #[test]
    fn test_generate_mcpg_docker_env_rejects_invalid_names() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "evil".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("img:latest".to_string()),
                env: {
                    let mut e = HashMap::new();
                    e.insert("MY_VAR --privileged".to_string(), "".to_string());
                    e.insert("GOOD_VAR".to_string(), "".to_string());
                    e
                },
                ..Default::default()
            }),
        );
        let extensions = collect_extensions(&fm);
        let env = generate_mcpg_docker_env(&fm, &extensions);
        assert!(
            !env.contains("--privileged"),
            "Should reject invalid env var name with Docker flag injection"
        );
        assert!(
            env.contains("-e GOOD_VAR"),
            "Should include valid env var"
        );
    }

    // ─── generate_mcpg_step_env ──────────────────────────────────────────────

    #[test]
    fn test_generate_mcpg_step_env_with_ado_extension() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n",
        ).unwrap();
        let extensions = collect_extensions(&fm);
        let env = generate_mcpg_step_env(&extensions);
        assert!(
            env.starts_with("env:\n"),
            "Should emit full env: block header"
        );
        assert!(
            env.contains("SC_READ_TOKEN: $(SC_READ_TOKEN)"),
            "Should map SC_READ_TOKEN for ADO extension"
        );
    }

    #[test]
    fn test_generate_mcpg_step_env_no_extensions() {
        let fm = minimal_front_matter();
        let extensions = collect_extensions(&fm);
        let env = generate_mcpg_step_env(&extensions);
        assert!(env.is_empty(), "Should be empty when no extensions need pipeline vars");
    }

    #[test]
    fn test_is_valid_env_var_name() {
        assert!(validate::is_valid_env_var_name("MY_VAR"));
        assert!(validate::is_valid_env_var_name("_PRIVATE"));
        assert!(validate::is_valid_env_var_name("A"));
        assert!(validate::is_valid_env_var_name("VAR123"));
        assert!(!validate::is_valid_env_var_name(""));
        assert!(!validate::is_valid_env_var_name("123ABC"));
        assert!(!validate::is_valid_env_var_name("MY-VAR"));
        assert!(!validate::is_valid_env_var_name("MY VAR"));
        assert!(!validate::is_valid_env_var_name("X --privileged"));
        assert!(!validate::is_valid_env_var_name("X -v /etc:/etc:rw"));
    }

    #[test]
    fn test_generate_mcpg_config_rejects_invalid_server_name() {
        let yaml = "---\nname: test-agent\ndescription: test\nmcp-servers:\n  bad/name:\n    container: python:3\n    entrypoint: python\n---\n";
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm));
        assert!(result.is_err(), "Should reject server name with /");
    }

    #[test]
    fn test_generate_mcpg_config_rejects_dot_leading_server_name() {
        // ".." would resolve to /mcp via path normalization, bypassing routing
        let yaml = "---\nname: test-agent\ndescription: test\nmcp-servers:\n  ..:\n    container: python:3\n    entrypoint: python\n---\n";
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm));
        assert!(result.is_err(), "Should reject server name starting with dot");

        // ".hidden" would produce /mcp/.hidden
        let yaml2 = "---\nname: test-agent\ndescription: test\nmcp-servers:\n  .hidden:\n    container: python:3\n    entrypoint: python\n---\n";
        let (fm2, _) = parse_markdown(yaml2).unwrap();
        let result2 = generate_mcpg_config(&fm2, &CompileContext::for_test(&fm2), &collect_extensions(&fm2));
        assert!(result2.is_err(), "Should reject server name starting with dot");
    }

    // ─── tools.azure-devops MCPG integration ────────────────────────────────

    #[test]
    fn test_ado_tool_generates_mcpg_entry() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n",
        )
        .unwrap();
        // Pass inferred org since no explicit org is set
        let config = generate_mcpg_config(&fm, &CompileContext::for_test_with_org(&fm, "inferred-org"), &collect_extensions(&fm)).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        assert_eq!(ado.server_type, "stdio");
        assert_eq!(ado.container.as_deref(), Some(ADO_MCP_IMAGE));
        assert_eq!(ado.entrypoint.as_deref(), Some(ADO_MCP_ENTRYPOINT));
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"-y".to_string()));
        assert!(args.contains(&ADO_MCP_PACKAGE.to_string()));
        assert!(args.contains(&"inferred-org".to_string()));
        // Should have ADO_MCP_AUTH_TOKEN in env (for bearer token via envvar auth)
        let env = ado.env.as_ref().unwrap();
        assert!(env.contains_key("ADO_MCP_AUTH_TOKEN"));
    }

    #[test]
    fn test_ado_tool_with_toolsets() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    toolsets: [repos, wit, core]\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test_with_org(&fm, "myorg"), &collect_extensions(&fm)).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"-d".to_string()));
        assert!(args.contains(&"repos".to_string()));
        assert!(args.contains(&"wit".to_string()));
        assert!(args.contains(&"core".to_string()));
    }

    #[test]
    fn test_ado_tool_with_org_override() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: myorg\n---\n",
        )
        .unwrap();
        // Explicit org should be used even when inferred_org is None
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"myorg".to_string()));
    }

    #[test]
    fn test_ado_tool_explicit_org_overrides_inferred() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: explicit-org\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test_with_org(&fm, "inferred-org"), &collect_extensions(&fm)).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"explicit-org".to_string()));
        assert!(!args.contains(&"inferred-org".to_string()));
    }

    #[test]
    fn test_ado_tool_no_org_fails() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n",
        )
        .unwrap();
        // No explicit org and no inferred org — should fail
        let result = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm));
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("no ADO organization"),
            "Error should mention missing org"
        );
    }

    #[test]
    fn test_ado_tool_invalid_org_fails() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: \"my org/bad\"\n---\n",
        )
        .unwrap();
        let result = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm));
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Invalid ADO org name"),
            "Error should mention invalid org"
        );
    }

    #[test]
    fn test_ado_tool_invalid_toolset_fails() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: myorg\n    toolsets: [\"repos\", \"bad toolset\"]\n---\n",
        )
        .unwrap();
        let result = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm));
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Invalid ADO toolset name"),
            "Error should mention invalid toolset"
        );
    }

    #[test]
    fn test_ado_tool_with_allowed_tools() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: myorg\n    allowed:\n      - wit_get_work_item\n      - core_list_projects\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let tools = ado.tools.as_ref().unwrap();
        assert_eq!(tools, &["wit_get_work_item", "core_list_projects"]);
    }

    #[test]
    fn test_ado_tool_disabled_not_generated() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: false\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        assert!(!config.mcp_servers.contains_key("azure-devops"));
    }

    #[test]
    fn test_ado_tool_not_set_not_generated() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        assert!(!config.mcp_servers.contains_key("azure-devops"));
    }

    #[test]
    fn test_ado_tool_skips_manual_mcp_entry() {
        // When tools.azure-devops is enabled AND mcp-servers also has azure-devops,
        // the tools config takes precedence and the manual entry is skipped.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: auto-org\nmcp-servers:\n  azure-devops:\n    container: \"node:20-slim\"\n    entrypoint: \"npx\"\n    entrypoint-args: [\"-y\", \"@azure-devops/mcp\", \"manual-org\"]\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, &CompileContext::for_test(&fm), &collect_extensions(&fm)).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        // Should use the auto-configured org, not the manual one
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"auto-org".to_string()));
        assert!(!args.contains(&"manual-org".to_string()));
    }

    #[test]
    fn test_ado_tool_docker_env_passthrough() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\npermissions:\n  read: my-read-sc\n---\n",
        )
        .unwrap();
        let extensions = collect_extensions(&fm);
        let env = generate_mcpg_docker_env(&fm, &extensions);
        assert!(
            env.contains("ADO_MCP_AUTH_TOKEN"),
            "Should include ADO token passthrough when permissions.read is set"
        );
    }

    // ─── validate_docker_args ────────────────────────────────────────────────

    #[test]
    fn test_validate_docker_args_privileged_flag() {
        let warnings = validate::validate_docker_args(&["--privileged".to_string()], "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("--privileged"), "should warn about --privileged");
    }

    #[test]
    fn test_validate_docker_args_entrypoint_in_args_warns() {
        let warnings = validate::validate_docker_args(
            &[
                "--entrypoint".to_string(),
                "/bin/sh".to_string(),
            ],
            "my-mcp",
        );
        assert!(warnings.iter().any(|w| w.contains("--entrypoint") && w.contains("entrypoint:")),
            "should warn about --entrypoint with hint to use entrypoint: field");
    }

    #[test]
    fn test_validate_docker_args_volume_flag_calls_mount_validation() {
        // -v docker.sock in args bypasses `mounts:` validation; should produce warnings
        let warnings = validate::validate_docker_args(
            &[
                "-v".to_string(),
                "/var/run/docker.sock:/var/run/docker.sock".to_string(),
            ],
            "my-mcp",
        );
        assert!(warnings.iter().any(|w| w.contains("bypasses mounts validation")),
            "should warn about volume mount in args");
        assert!(warnings.iter().any(|w| w.contains("Docker socket")),
            "should propagate mount source warning for docker.sock");
    }

    #[test]
    fn test_validate_docker_args_volume_equals_form() {
        // --volume=source:dest form should also be detected
        let warnings = validate::validate_docker_args(
            &["--volume=/var/run/docker.sock:/var/run/docker.sock".to_string()],
            "my-mcp",
        );
        assert!(warnings.iter().any(|w| w.contains("bypasses mounts validation")),
            "should warn about --volume= form");
    }

    #[test]
    fn test_validate_docker_args_safe_args_no_warnings() {
        // A legitimate arg like --read-only should produce no warnings
        let warnings = validate::validate_docker_args(&["--read-only".to_string()], "my-mcp");
        assert!(warnings.is_empty(), "safe args should not produce warnings");
    }

    #[test]
    fn test_validate_docker_args_empty_no_warnings() {
        let warnings = validate::validate_docker_args(&[], "my-mcp");
        assert!(warnings.is_empty(), "empty args should not produce warnings");
    }

    #[test]
    fn test_validate_docker_args_volume_flag_trailing_warns() {
        // -v as the last arg with no mount spec is malformed
        let warnings = validate::validate_docker_args(&["-v".to_string()], "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("malformed"), "trailing -v with no mount spec should warn");
    }

    #[test]
    fn test_validate_docker_args_long_volume_flag_trailing_warns() {
        // --volume as the last arg with no mount spec is malformed
        let warnings = validate::validate_docker_args(&["--volume".to_string()], "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("malformed"), "trailing --volume with no mount spec should warn");
    }

    // ─── validate_mcp_url ────────────────────────────────────────────────────

    #[test]
    fn test_validate_mcp_url_https_no_warnings() {
        let warnings = validate::validate_mcp_url("https://mcp.dev.azure.com/myorg", "my-mcp");
        assert!(warnings.is_empty(), "https URL should not produce warnings");
    }

    #[test]
    fn test_validate_mcp_url_http_no_warnings() {
        let warnings = validate::validate_mcp_url("http://localhost:8100/mcp", "my-mcp");
        assert!(warnings.is_empty(), "http URL should not produce warnings");
    }

    #[test]
    fn test_validate_mcp_url_bad_scheme_warns() {
        let warnings = validate::validate_mcp_url("ftp://files.example.com", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("does not use http://"), "non-HTTP scheme should warn");
    }

    #[test]
    fn test_validate_mcp_url_no_scheme_warns() {
        let warnings = validate::validate_mcp_url("mcp.dev.azure.com/myorg", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("does not use http://"), "URL without scheme should warn");
    }

    // ─── validate_mount_source ───────────────────────────────────────────────

    #[test]
    fn test_validate_mount_source_docker_sock() {
        let warnings = validate::validate_mount_source("/var/run/docker.sock:/var/run/docker.sock:rw", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Docker socket"), "should warn about Docker socket exposure");
    }

    #[test]
    fn test_validate_mount_source_sensitive_path_etc() {
        let warnings = validate::validate_mount_source("/etc/passwd:/data/passwd:ro", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("sensitive host path"), "should warn about /etc mount");
    }

    #[test]
    fn test_validate_mount_source_sensitive_path_proc() {
        let warnings = validate::validate_mount_source("/proc:/host/proc:ro", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("sensitive host path"), "should warn about /proc mount");
    }

    #[test]
    fn test_validate_mount_source_case_insensitive() {
        // /ETC/shadow should match sensitive /etc prefix (lowercased comparison)
        let warnings = validate::validate_mount_source("/ETC/shadow:/data/shadow:ro", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("sensitive host path"), "case-insensitive match should trigger warning");
    }

    #[test]
    fn test_validate_mount_source_no_false_positive_on_etc_configs() {
        // /etc-configs should NOT match the /etc prefix (path boundary check requires trailing /)
        let warnings = validate::validate_mount_source("/etc-configs:/app/config:ro", "my-mcp");
        assert!(warnings.is_empty(), "/etc-configs must not match /etc prefix due to path boundary check");
    }

    #[test]
    fn test_validate_mount_source_safe_path_no_warnings() {
        // /app/data is not a sensitive path; should produce no warnings
        let warnings = validate::validate_mount_source("/app/data:/app/data:ro", "my-mcp");
        assert!(warnings.is_empty(), "safe path should not produce warnings");
    }

    // ─── validate_container_image ────────────────────────────────────────────

    #[test]
    fn test_validate_container_image_empty_string() {
        let warnings = validate::validate_container_image("", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("empty"), "should warn about empty image name");
    }

    #[test]
    fn test_validate_container_image_shell_metacharacters() {
        let warnings = validate::validate_container_image("node:20-slim; rm -rf /", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unexpected characters"), "should warn about shell metacharacters");
    }

    #[test]
    fn test_validate_container_image_valid_name_no_warnings() {
        // Standard image references should produce no warnings
        assert!(validate::validate_container_image("node:20-slim", "my-mcp").is_empty());
        assert!(validate::validate_container_image("ghcr.io/org/image:latest", "my-mcp").is_empty());
        assert!(validate::validate_container_image("python:3.12-slim", "my-mcp").is_empty());
    }

    // ─── warn_potential_secrets ──────────────────────────────────────────────

    #[test]
    fn test_warn_potential_secrets_token_env_var_triggers() {
        let env = HashMap::from([("API_TOKEN".to_string(), "secret123".to_string())]);
        let headers = HashMap::new();
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("API_TOKEN"), "should warn about secret-looking env var");
    }

    #[test]
    fn test_warn_potential_secrets_empty_passthrough_no_warnings() {
        // Empty string = passthrough; should NOT trigger a warning
        let env = HashMap::from([("API_TOKEN".to_string(), "".to_string())]);
        let headers = HashMap::new();
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert!(warnings.is_empty(), "empty passthrough value must not trigger a warning");
    }

    #[test]
    fn test_warn_potential_secrets_authorization_header_triggers() {
        let env = HashMap::new();
        let headers =
            HashMap::from([("Authorization".to_string(), "Bearer abc".to_string())]);
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Authorization"), "should warn about Authorization header");
    }

    #[test]
    fn test_warn_potential_secrets_bearer_value_triggers() {
        // A header whose value starts with "Bearer " should also warn
        let env = HashMap::new();
        let headers =
            HashMap::from([("X-Custom-Auth".to_string(), "Bearer token123".to_string())]);
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("X-Custom-Auth"), "should warn about header with Bearer value");
    }

    #[test]
    fn test_warn_potential_secrets_safe_env_no_warnings() {
        // Env keys with non-secret names and non-empty values should produce no warnings
        let env = HashMap::from([("MY_CONFIG".to_string(), "value".to_string())]);
        let headers = HashMap::new();
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert!(warnings.is_empty(), "non-secret env var should not produce warnings");
    }

    // ─── standalone setup/teardown/finalize/checkout/repositories generators ───

    #[test]
    fn test_generate_setup_job_empty_returns_empty() {
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        assert!(generate_setup_job(&[], "MyPool", None, None, &[], &ctx).unwrap().is_empty());
    }

    #[test]
    fn test_generate_setup_job_with_steps() {
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let step: serde_yaml::Value = serde_yaml::from_str("bash: echo setup").unwrap();
        let out = generate_setup_job(&[step], "MyPool", None, None, &[], &ctx).unwrap();
        assert!(out.contains("- job: Setup"), "out: {out}");
        assert!(out.contains("displayName: \"Setup\""), "out: {out}");
        assert!(out.contains("name: MyPool"), "out: {out}");
        assert!(out.contains("- checkout: self"), "out: {out}");
        assert!(out.contains("echo setup"), "out: {out}");
    }

    #[test]
    fn test_generate_teardown_job_empty_returns_empty() {
        assert!(generate_teardown_job(&[], "MyPool").is_empty());
    }

    #[test]
    fn test_generate_teardown_job_with_steps() {
        let step: serde_yaml::Value = serde_yaml::from_str("bash: echo td").unwrap();
        let out = generate_teardown_job(&[step], "MyPool");
        assert!(out.contains("- job: Teardown"), "out: {out}");
        assert!(out.contains("dependsOn: Execution"), "out: {out}");
        assert!(out.contains("name: MyPool"), "out: {out}");
        assert!(out.contains("echo td"), "out: {out}");
    }

    #[test]
    fn test_generate_agentic_depends_on_empty_steps() {
        assert!(generate_agentic_depends_on(&[], false, false, None).is_empty());
    }

    #[test]
    fn test_generate_agentic_depends_on_with_steps() {
        let step: serde_yaml::Value = serde_yaml::from_str("bash: x").unwrap();
        assert_eq!(generate_agentic_depends_on(&[step], false, false, None), "dependsOn: Setup");
    }

    #[test]
    fn test_generate_finalize_steps_empty() {
        assert!(generate_finalize_steps(&[]).is_empty());
    }

    #[test]
    fn test_generate_finalize_steps_with_step() {
        let step: serde_yaml::Value = serde_yaml::from_str("bash: echo done").unwrap();
        let out = generate_finalize_steps(&[step]);
        assert!(out.contains("echo done"), "out: {out}");
    }

    #[test]
    fn test_generate_checkout_steps_empty() {
        assert!(generate_checkout_steps(&[]).is_empty());
    }

    #[test]
    fn test_generate_checkout_steps_multiple() {
        let aliases = vec!["repo-a".to_string(), "repo-b".to_string()];
        let out = generate_checkout_steps(&aliases);
        assert!(out.contains("- checkout: repo-a"), "out: {out}");
        assert!(out.contains("- checkout: repo-b"), "out: {out}");
    }

    #[test]
    fn test_generate_repositories_empty() {
        assert!(generate_repositories(&[]).is_empty());
    }

    #[test]
    fn test_generate_repositories_single() {
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let out = generate_repositories(&repos);
        assert!(out.contains("- repository: my-repo"), "out: {out}");
        assert!(out.contains("type: git"), "out: {out}");
        assert!(out.contains("name: org/my-repo"), "out: {out}");
        assert!(out.contains("ref: refs/heads/main"), "out: {out}");
    }

    #[test]
    fn test_generate_repositories_multiple() {
        let repos = vec![
            Repository {
                repository: "repo-a".to_string(),
                repo_type: "git".to_string(),
                name: "org/repo-a".to_string(),
                repo_ref: "refs/heads/main".to_string(),
            },
            Repository {
                repository: "repo-b".to_string(),
                repo_type: "git".to_string(),
                name: "org/repo-b".to_string(),
                repo_ref: "refs/heads/develop".to_string(),
            },
        ];
        let out = generate_repositories(&repos);
        assert!(out.contains("- repository: repo-a"), "out: {out}");
        assert!(out.contains("- repository: repo-b"), "out: {out}");
        assert!(out.contains("name: org/repo-a"), "out: {out}");
        assert!(out.contains("ref: refs/heads/develop"), "out: {out}");
    }
}
