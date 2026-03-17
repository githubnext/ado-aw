//! 1ES Pipeline Template compiler.
//!
//! This compiler generates a pipeline that extends the 1ES Unofficial Pipeline Template:
//! - Uses `templateContext.type: agencyJob` for the main agent job
//! - Integrates with 1ES SDL scanning and compliance tools
//! - Custom jobs for threat analysis and safe output processing
//!
//! Limitations:
//! - MCP servers use service connections (no custom `command:` support)
//! - Network isolation is handled by OneBranch (no custom proxy allow-lists)

use anyhow::{Context, Result};
use async_trait::async_trait;
use log::info;
use std::collections::HashMap;
use std::path::Path;

use super::Compiler;
use super::common::{
    self, AWF_VERSION, COPILOT_CLI_VERSION, DEFAULT_POOL, compute_effective_workspace, generate_copilot_params,
    generate_acquire_ado_token, generate_checkout_self, generate_checkout_steps,
    generate_ci_trigger, generate_copilot_ado_env, generate_executor_ado_env,
    generate_pipeline_path, generate_pipeline_resources, generate_pr_trigger,
    generate_repositories, generate_schedule, generate_source_path,
    generate_working_directory, replace_with_indent, validate_write_permissions,
};
use super::types::{FrontMatter, McpConfig};

/// 1ES Pipeline Template compiler.
pub struct OneESCompiler;

#[async_trait]
impl Compiler for OneESCompiler {
    fn target_name(&self) -> &'static str {
        "1ES"
    }

    async fn compile(
        &self,
        input_path: &Path,
        output_path: &Path,
        front_matter: &FrontMatter,
        markdown_body: &str,
    ) -> Result<String> {
        info!("Compiling for 1ES target");

        // Load 1ES template
        let template = include_str!("../../templates/1es-base.yml");

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
        let source_path = generate_source_path(input_path);
        let pipeline_path = generate_pipeline_path(output_path);

        // Pool - for 1ES we need both name and os
        let pool = front_matter
            .pool
            .as_ref()
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| DEFAULT_POOL.to_string());

        // Generate 1ES-specific content
        let agent_context_root = generate_agent_context_root(&effective_workspace);
        let mcp_configuration = generate_mcp_configuration(&front_matter.mcp_servers);
        let prepare_steps = generate_inline_steps(&front_matter.steps);

        // Default finalize step to avoid empty stepList
        let default_finalize_step = serde_yaml::from_str::<serde_yaml::Value>(
            r#"bash: echo "Agent task completed"
displayName: "Finalize""#,
        )
        .expect("default finalize step should be valid YAML");
        let finalize_steps = if front_matter.post_steps.is_empty() {
            generate_inline_steps(&[default_finalize_step])
        } else {
            generate_inline_steps(&front_matter.post_steps)
        };

        let setup_job = generate_setup_job(&front_matter.setup, &front_matter.name);
        let teardown_job = generate_teardown_job(&front_matter.teardown, &front_matter.name);
        let agentic_depends_on = if !front_matter.setup.is_empty() {
            "dependsOn: SetupJob".to_string()
        } else {
            String::new()
        };

        // Load threat analysis prompt template
        let threat_analysis_prompt = include_str!("../../templates/threat-analysis.md");

        // Insert threat analysis prompt first
        let template = replace_with_indent(
            template,
            "{{ threat_analysis_prompt }}",
            threat_analysis_prompt,
        );

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

        // Replace all template markers
        let compiler_version = env!("CARGO_PKG_VERSION");
        let replacements: Vec<(&str, &str)> = vec![
            ("{{ compiler_version }}", compiler_version),
            // No-op for 1ES (template doesn't use AWF), but included for forward-compatibility
            ("{{ firewall_version }}", AWF_VERSION),
            ("{{ copilot_version }}", COPILOT_CLI_VERSION),
            ("{{ pool }}", &pool),
            ("{{ schedule }}", &schedule),
            ("{{ pr_trigger }}", &pr_trigger),
            ("{{ ci_trigger }}", &ci_trigger),
            ("{{ repositories }}", &repositories),
            ("{{ pipeline_resources }}", &pipeline_resources),
            ("{{ checkout_self }}", &checkout_self),
            ("{{ checkout_repositories }}", &checkout_steps),
            ("{{ agent_name }}", &front_matter.name),
            ("{{ agent_description }}", &front_matter.description),
            ("{{ agent_context_root }}", &agent_context_root),
            ("{{ agent_content }}", markdown_body),
            ("{{ prepare_steps }}", &prepare_steps),
            ("{{ finalize_steps }}", &finalize_steps),
            ("{{ global_options }}", ""),
            ("{{ log_level }}", ""),
            ("{{ mcp_configuration }}", &mcp_configuration),
            ("{{ agentic_depends_on }}", &agentic_depends_on),
            ("{{ setup_job }}", &setup_job),
            ("{{ teardown_job }}", &teardown_job),
            ("{{ source_path }}", &source_path),
            ("{{ pipeline_path }}", &pipeline_path),
            ("{{ working_directory }}", &working_directory),
            ("{{ workspace }}", &working_directory),
            ("{{ agency_params }}", &agency_params),
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

        // Warn about custom MCP limitations
        if front_matter
            .mcp_servers
            .iter()
            .any(|(_, c)| matches!(c, McpConfig::WithOptions(o) if o.command.is_some()))
        {
            eprintln!(
                "Warning: Custom MCP servers (with command:) are not supported in 1ES target. \
                They will be ignored. Use standalone target for full MCP support."
            );
        }

        Ok(pipeline_yaml)
    }
}

// ==================== 1ES-specific helpers ====================

/// Generate agent context root for 1ES templates
fn generate_agent_context_root(effective_workspace: &str) -> String {
    match effective_workspace {
        "repo" => "$(Build.Repository.Name)".to_string(),
        "root" | _ => ".".to_string(),
    }
}

/// Generate MCP configuration for 1ES templates
fn generate_mcp_configuration(mcps: &HashMap<String, McpConfig>) -> String {
    let mut mcp_entries: Vec<_> = mcps
        .iter()
        .filter_map(|(name, config)| {
            let (is_enabled, opts) = match config {
                McpConfig::Enabled(enabled) => (*enabled, None),
                McpConfig::WithOptions(o) => (o.command.is_none(), Some(o)), // Custom MCPs not supported
            };

            if !is_enabled || !common::is_builtin_mcp(name) {
                return None;
            }

            // Use explicit service connection or generate default
            let service_connection = opts
                .and_then(|o| o.service_connection.clone())
                .unwrap_or_else(|| format!("mcp-{}-service-connection", name));

            Some((name.clone(), service_connection))
        })
        .collect();

    if mcp_entries.is_empty() {
        return "{}".to_string();
    }

    // Sort for deterministic output
    mcp_entries.sort_by(|a, b| a.0.cmp(&b.0));

    mcp_entries
        .iter()
        .map(|(name, sc)| format!("{}:\n  serviceConnection: {}", name, sc))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate inline steps YAML (for adding to existing step list)
/// Returns empty string when no steps (blank lines are valid in YAML)
fn generate_inline_steps(steps: &[serde_yaml::Value]) -> String {
    if steps.is_empty() {
        return String::new();
    }

    common::format_steps_yaml_indented(steps, 0)
}

/// Generate setup job for 1ES template
fn generate_setup_job(setup_steps: &[serde_yaml::Value], agent_name: &str) -> String {
    if setup_steps.is_empty() {
        return String::new();
    }

    let steps_yaml: Vec<_> = setup_steps
        .iter()
        .filter_map(|step| {
            serde_yaml::to_string(step).ok().map(|yaml| {
                yaml.trim()
                    .lines()
                    .enumerate()
                    .map(|(i, line)| {
                        if i == 0 {
                            format!("- {}", line.trim_start_matches("---").trim())
                        } else {
                            format!("  {}", line)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        })
        .collect();

    format!(
        r#"- job: SetupJob
  displayName: "{} - Setup"
  templateContext:
    type: buildJob
  steps:
    - checkout: self
    {}"#,
        agent_name,
        steps_yaml.join("\n    ")
    )
}

/// Generate teardown job for 1ES template
fn generate_teardown_job(teardown_steps: &[serde_yaml::Value], agent_name: &str) -> String {
    if teardown_steps.is_empty() {
        return String::new();
    }

    let steps_yaml: Vec<_> = teardown_steps
        .iter()
        .filter_map(|step| {
            serde_yaml::to_string(step).ok().map(|yaml| {
                yaml.trim()
                    .lines()
                    .enumerate()
                    .map(|(i, line)| {
                        if i == 0 {
                            format!("- {}", line.trim_start_matches("---").trim())
                        } else {
                            format!("  {}", line)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        })
        .collect();

    format!(
        r#"- job: TeardownJob
  displayName: "{} - Teardown"
  dependsOn: ProcessSafeOutputs
  templateContext:
    type: buildJob
  steps:
    - checkout: self
    {}"#,
        agent_name,
        steps_yaml.join("\n    ")
    )
}
