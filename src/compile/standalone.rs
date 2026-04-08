//! Standalone pipeline compiler.
//!
//! This compiler generates a self-contained Azure DevOps pipeline with:
//! - Full 3-job pipeline: PerformAgenticTask → AnalyzeSafeOutputs → ProcessSafeOutputs
//! - AWF (Agentic Workflow Firewall) L7 domain whitelisting via Squid proxy + Docker
//! - MCP firewall with tool-level filtering and custom MCP server support
//! - Setup/teardown job support

use anyhow::{Context, Result};
use async_trait::async_trait;
use log::info;
use std::collections::HashMap;
use std::path::Path;

use super::Compiler;
use super::common::{
    self, AWF_VERSION, COPILOT_CLI_VERSION, DEFAULT_POOL, compute_effective_workspace, generate_copilot_params,
    generate_acquire_ado_token, generate_cancel_previous_builds, generate_checkout_self,
    generate_checkout_steps, generate_ci_trigger, generate_copilot_ado_env,
    generate_executor_ado_env, generate_header_comment, generate_job_timeout,
    generate_pipeline_path, generate_pipeline_resources, generate_pr_trigger,
    generate_repositories, generate_schedule, generate_source_path,
    generate_working_directory, replace_with_indent, sanitize_filename,
    validate_write_permissions, validate_comment_target, validate_update_work_item_target,
};
use super::types::{FrontMatter, McpConfig};
use crate::allowed_hosts::{CORE_ALLOWED_HOSTS, mcp_required_hosts};
use crate::mcp_firewall::{FirewallConfig, UpstreamConfig};
use std::collections::HashSet;

/// Standalone pipeline compiler.
pub struct StandaloneCompiler;

#[async_trait]
impl Compiler for StandaloneCompiler {
    fn target_name(&self) -> &'static str {
        "standalone"
    }

    async fn compile(
        &self,
        input_path: &Path,
        output_path: &Path,
        front_matter: &FrontMatter,
        markdown_body: &str,
    ) -> Result<String> {
        info!("Compiling for standalone target");

        // Load base template
        let template = include_str!("../../templates/base.yml");

        // Generate schedule
        let schedule = match &front_matter.schedule {
            Some(s) => generate_schedule(&front_matter.name, s)
                .with_context(|| format!("Failed to parse schedule '{}'", s.expression()))?,
            None => String::new(),
        };

        let repositories = generate_repositories(&front_matter.repositories);
        let checkout_steps = generate_checkout_steps(&front_matter.checkout);
        let checkout_self = generate_checkout_self();
        let agency_params = generate_copilot_params(front_matter);
        let agent_name = sanitize_filename(&front_matter.name);

        // Compute effective workspace
        let effective_workspace = compute_effective_workspace(
            &front_matter.workspace,
            &front_matter.checkout,
            &front_matter.name,
        );
        let working_directory = generate_working_directory(&effective_workspace);
        let pipeline_resources = generate_pipeline_resources(&front_matter.triggers)?;
        let has_schedule = front_matter.schedule.is_some();
        let pr_trigger = generate_pr_trigger(&front_matter.triggers, has_schedule);
        let ci_trigger = generate_ci_trigger(&front_matter.triggers, has_schedule);
        let cancel_previous_builds = generate_cancel_previous_builds(&front_matter.triggers);

        // Generate source path for Stage 2
        let source_path = generate_source_path(input_path);

        // Generate pipeline path for integrity checking
        let pipeline_path = generate_pipeline_path(output_path);

        // Generate comma-separated domain list for AWF
        let allowed_domains = generate_allowed_domains(front_matter);

        // Pool name
        let pool = front_matter
            .pool
            .as_ref()
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| DEFAULT_POOL.to_string());

        // Generate hooks
        let setup_job = generate_setup_job(
            &front_matter.setup,
            &front_matter.name,
            &pool,
        );
        let teardown_job = generate_teardown_job(
            &front_matter.teardown,
            &front_matter.name,
            &pool,
        );
        let has_memory = front_matter.safe_outputs.contains_key("memory");
        let prepare_steps = generate_prepare_steps(&front_matter.steps, has_memory);
        let finalize_steps = generate_finalize_steps(&front_matter.post_steps);
        let agentic_depends_on = generate_agentic_depends_on(&front_matter.setup);
        let job_timeout = generate_job_timeout(front_matter);

        // Generate service connection token acquisition steps and env vars
        let acquire_read_token = generate_acquire_ado_token(
            front_matter.permissions.as_ref().and_then(|p| p.read.as_deref()),
            "SC_READ_TOKEN",
        );
        let copilot_ado_env = generate_copilot_ado_env(
            front_matter.permissions.as_ref().and_then(|p| p.read.as_deref()),
        );
        let acquire_write_token = generate_acquire_ado_token(
            front_matter.permissions.as_ref().and_then(|p| p.write.as_deref()),
            "SC_WRITE_TOKEN",
        );
        let executor_ado_env = generate_executor_ado_env(
            front_matter.permissions.as_ref().and_then(|p| p.write.as_deref()),
        );

        // Validate that write-requiring safe-outputs have a write service connection
        validate_write_permissions(front_matter)?;
        // Validate comment-on-work-item has required target field
        validate_comment_target(front_matter)?;
        // Validate update-work-item has required target field
        validate_update_work_item_target(front_matter)?;

        // Load threat analysis prompt template
        let threat_analysis_prompt = include_str!("../../templates/threat-analysis.md");

        // Insert threat analysis prompt first
        let template = replace_with_indent(
            template,
            "{{ threat_analysis_prompt }}",
            threat_analysis_prompt,
        );

        // Replace template markers
        let compiler_version = env!("CARGO_PKG_VERSION");
        let replacements: Vec<(&str, &str)> = vec![
            ("{{ compiler_version }}", compiler_version),
            ("{{ firewall_version }}", AWF_VERSION),
            ("{{ copilot_version }}", COPILOT_CLI_VERSION),
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
            ("{{ cancel_previous_builds }}", &cancel_previous_builds),
            ("{{ agent }}", &agent_name),
            ("{{ agent_name }}", &front_matter.name),
            ("{{ agent_description }}", &front_matter.description),
            ("{{ agency_params }}", &agency_params),
            ("{{ source_path }}", &source_path),
            ("{{ pipeline_path }}", &pipeline_path),
            ("{{ working_directory }}", &working_directory),
            ("{{ workspace }}", &working_directory),
            ("{{ allowed_domains }}", &allowed_domains),
            ("{{ agent_content }}", markdown_body),
            ("{{ acquire_ado_token }}", &acquire_read_token),
            ("{{ copilot_ado_env }}", &copilot_ado_env),
            ("{{ acquire_write_token }}", &acquire_write_token),
            ("{{ executor_ado_env }}", &executor_ado_env),
        ];

        let pipeline_yaml = replacements
            .into_iter()
            .fold(template, |yaml, (placeholder, replacement)| {
                replace_with_indent(&yaml, placeholder, replacement)
            });

        // Generate MCP firewall config JSON
        let firewall_config_json = if !front_matter.mcp_servers.is_empty() {
            let config = generate_firewall_config(front_matter);
            serde_json::to_string_pretty(&config)
                .unwrap_or_else(|_| r#"{"upstreams":{}}"#.to_string())
        } else {
            r#"{"upstreams":{}}"#.to_string()
        };

        let pipeline_yaml = replace_with_indent(
            &pipeline_yaml,
            "{{ firewall_config }}",
            &firewall_config_json,
        );

        // Prepend header comment for pipeline detection
        let header = generate_header_comment(input_path);
        let pipeline_yaml = format!("{}{}", header, pipeline_yaml);

        Ok(pipeline_yaml)
    }
}

// ==================== Standalone-specific helpers ====================

/// Generate the allowed domains list for AWF network isolation.
///
/// This generates a comma-separated list of domain patterns for AWF's
/// `--allow-domains` flag. The list includes:
/// 1. Core Azure DevOps/GitHub endpoints
/// 2. MCP-specific endpoints for each enabled MCP
/// 3. User-specified additional hosts from network.allow
fn generate_allowed_domains(front_matter: &FrontMatter) -> String {
    // Collect enabled MCP names
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
        .map(|n| n.allow.clone())
        .unwrap_or_default();

    // Generate the allowlist by combining core + MCP + user hosts
    let mut hosts: HashSet<String> = HashSet::new();

    // Add core hosts
    for host in CORE_ALLOWED_HOSTS {
        hosts.insert((*host).to_string());
    }

    // Add MCP-specific hosts
    for mcp in &enabled_mcps {
        for host in mcp_required_hosts(mcp) {
            hosts.insert((*host).to_string());
        }
    }

    // Add user-specified hosts
    for host in &user_hosts {
        hosts.insert(host.clone());
    }

    // Remove blocked hosts
    let blocked_hosts: Vec<String> = front_matter
        .network
        .as_ref()
        .map(|n| n.blocked.clone())
        .unwrap_or_default();
    for blocked in &blocked_hosts {
        hosts.remove(blocked);
    }

    // Sort for deterministic output
    let mut allowlist: Vec<String> = hosts.into_iter().collect();
    allowlist.sort();

    // Format as comma-separated list for AWF --allow-domains
    allowlist.join(",")
}

/// Generate the setup job YAML
fn generate_setup_job(
    setup_steps: &[serde_yaml::Value],
    agent_name: &str,
    pool: &str,
) -> String {
    if setup_steps.is_empty() {
        return String::new();
    }

    let steps_yaml = common::format_steps_yaml_indented(setup_steps, 4);

    format!(
        r#"- job: SetupJob
  displayName: "{} - Setup"
  pool:
    name: {}
  steps:
    - checkout: self
{}
"#,
        agent_name, pool, steps_yaml
    )
}

/// Generate the teardown job YAML
fn generate_teardown_job(
    teardown_steps: &[serde_yaml::Value],
    agent_name: &str,
    pool: &str,
) -> String {
    if teardown_steps.is_empty() {
        return String::new();
    }

    let steps_yaml = common::format_steps_yaml(teardown_steps);

    format!(
        r#"  - job: TeardownJob
    displayName: "{} - Teardown"
    dependsOn: ProcessSafeOutputs
    pool:
      name: {}
    steps:
      - checkout: self
{}
"#,
        agent_name, pool, steps_yaml
    )
}

/// Generate prepare steps (inline), including memory download/restore/prompt if enabled
fn generate_prepare_steps(prepare_steps: &[serde_yaml::Value], has_memory: bool) -> String {
    let mut parts = Vec::new();

    // Memory steps run before user prepare steps
    if has_memory {
        parts.push(generate_memory_download());
        parts.push(generate_memory_prompt());
    }

    if !prepare_steps.is_empty() {
        parts.push(common::format_steps_yaml_indented(prepare_steps, 0));
    }

    parts.join("\n\n")
}

/// Generate finalize steps (inline)
fn generate_finalize_steps(finalize_steps: &[serde_yaml::Value]) -> String {
    if finalize_steps.is_empty() {
        return String::new();
    }

    common::format_steps_yaml_indented(finalize_steps, 0)
}

/// Generate dependsOn clause for setup job
fn generate_agentic_depends_on(setup_steps: &[serde_yaml::Value]) -> String {
    if !setup_steps.is_empty() {
        "dependsOn: SetupJob".to_string()
    } else {
        String::new()
    }
}

/// Generate MCP firewall configuration from front matter
pub fn generate_firewall_config(front_matter: &FrontMatter) -> FirewallConfig {
    let mut upstreams = HashMap::new();

    for (name, config) in &front_matter.mcp_servers {
        let (is_enabled, options) = match config {
            McpConfig::Enabled(enabled) => (*enabled, None),
            McpConfig::WithOptions(opts) => (true, Some(opts)),
        };

        if !is_enabled {
            continue;
        }

        let upstream = if let Some(opts) = options {
            if let Some(command) = &opts.command {
                // Custom MCP with explicit command
                UpstreamConfig {
                    command: command.clone(),
                    args: opts.args.clone(),
                    env: opts.env.clone(),
                    allowed: if opts.allowed.is_empty() {
                        vec!["*".to_string()]
                    } else {
                        opts.allowed.clone()
                    },
                    spawn_timeout_secs: 30,
                }
            } else if common::is_builtin_mcp(name) {
                // Built-in MCP with options
                let mut args = vec!["mcp".to_string(), name.clone()];
                args.extend(opts.args.clone());

                UpstreamConfig {
                    command: "agency".to_string(),
                    args,
                    env: opts.env.clone(),
                    allowed: if opts.allowed.is_empty() {
                        vec!["*".to_string()]
                    } else {
                        opts.allowed.clone()
                    },
                    spawn_timeout_secs: 30,
                }
            } else {
                log::warn!(
                    "MCP '{}' has no command and is not a built-in - skipping",
                    name
                );
                continue;
            }
        } else if common::is_builtin_mcp(name) {
            // Built-in MCP with simple enablement
            UpstreamConfig {
                command: "agency".to_string(),
                args: vec!["mcp".to_string(), name.clone()],
                env: HashMap::new(),
                allowed: vec!["*".to_string()],
                spawn_timeout_secs: 30,
            }
        } else {
            log::warn!(
                "MCP '{}' is not a built-in and has no command - skipping",
                name
            );
            continue;
        };

        upstreams.insert(name.clone(), upstream);
    }

    FirewallConfig {
        upstreams,
        metadata_path: None,
    }
}

/// Generate the steps to download agent memory from the previous successful run
/// and restore it to the staging directory.
fn generate_memory_download() -> String {
    r#"- task: DownloadPipelineArtifact@2
  displayName: "Download previous agent memory"
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
  continueOnError: true"#
        .to_string()
}

/// Generate the prompt append step to inform the agent about its memory location.
fn generate_memory_prompt() -> String {
    r#"- bash: |
    cat >> "/tmp/awf-tools/agent-prompt.md" << 'MEMORY_PROMPT_EOF'

    ---

    ## Agent Memory

    You have persistent memory across runs. Your memory directory is located at `/tmp/awf-tools/staging/agent_memory/`.

    - **Read** previous memory files from this directory to recall context from prior runs.
    - **Write** new files or update existing ones in this directory to persist knowledge for future runs.
    - Use this memory to track patterns, accumulate findings, remember decisions, and improve over time.
    - The memory directory is yours to organize as you see fit (files, subdirectories, any structure).
    - Memory files are sanitized between runs for security; avoid including pipeline commands or secrets.
    MEMORY_PROMPT_EOF

    echo "Agent memory prompt appended"
  displayName: "Append memory prompt""#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::parse_markdown;
    use crate::compile::types::{McpConfig, McpOptions};

    fn minimal_front_matter() -> FrontMatter {
        let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
        fm
    }

    #[test]
    fn test_generate_firewall_config_builtin_simple_enabled() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("ado".to_string(), McpConfig::Enabled(true));
        let config = generate_firewall_config(&fm);
        let upstream = config.upstreams.get("ado").unwrap();
        assert_eq!(upstream.command, "agency");
        assert_eq!(upstream.args, vec!["mcp", "ado"]);
        assert_eq!(upstream.allowed, vec!["*"]);
    }

    #[test]
    fn test_generate_firewall_config_builtin_with_allowed_list() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "icm".to_string(),
            McpConfig::WithOptions(McpOptions {
                allowed: vec!["create_incident".to_string(), "get_incident".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_firewall_config(&fm);
        let upstream = config.upstreams.get("icm").unwrap();
        assert_eq!(upstream.command, "agency");
        assert_eq!(upstream.args, vec!["mcp", "icm"]);
        assert_eq!(
            upstream.allowed,
            vec!["create_incident".to_string(), "get_incident".to_string()]
        );
    }

    #[test]
    fn test_generate_firewall_config_custom_mcp() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                command: Some("node".to_string()),
                args: vec!["server.js".to_string()],
                allowed: vec!["do_thing".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_firewall_config(&fm);
        let upstream = config.upstreams.get("my-tool").unwrap();
        assert_eq!(upstream.command, "node");
        assert_eq!(upstream.args, vec!["server.js"]);
        assert_eq!(upstream.allowed, vec!["do_thing"]);
    }

    #[test]
    fn test_generate_firewall_config_custom_mcp_empty_allowed_defaults_to_wildcard() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                command: Some("python".to_string()),
                allowed: vec![],
                ..Default::default()
            }),
        );
        let config = generate_firewall_config(&fm);
        let upstream = config.upstreams.get("my-tool").unwrap();
        assert_eq!(upstream.allowed, vec!["*"]);
    }

    #[test]
    fn test_generate_firewall_config_unknown_non_builtin_skipped() {
        // An MCP that is neither built-in nor has a command should be skipped
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("phantom".to_string(), McpConfig::Enabled(true));
        let config = generate_firewall_config(&fm);
        assert!(!config.upstreams.contains_key("phantom"));
    }

    #[test]
    fn test_generate_firewall_config_disabled_mcp_skipped() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("ado".to_string(), McpConfig::Enabled(false));
        let config = generate_firewall_config(&fm);
        assert!(!config.upstreams.contains_key("ado"));
    }

    #[test]
    fn test_generate_firewall_config_empty_mcp_servers() {
        let fm = minimal_front_matter();
        let config = generate_firewall_config(&fm);
        assert!(config.upstreams.is_empty());
    }
}
