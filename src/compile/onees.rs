//! 1ES Pipeline Template compiler.
//!
//! This compiler generates a pipeline that extends the 1ES Unofficial Pipeline Template
//! with Copilot CLI, AWF network isolation, and MCP Gateway — matching the standalone
//! pipeline model while maintaining 1ES SDL compliance.

use anyhow::{Context, Result};
use async_trait::async_trait;
use log::info;
use std::path::Path;

use super::Compiler;
use super::common::{
    AWF_VERSION, MCPG_VERSION, MCPG_IMAGE, MCPG_PORT, MCPG_DOMAIN,
    CompileConfig, compile_shared,
    generate_allowed_domains,
    generate_cancel_previous_builds,
    generate_enabled_tools_args,
    generate_mcpg_config, generate_mcpg_docker_env,
    generate_mcp_client_config,
    format_steps_yaml_indented,
};
use super::types::FrontMatter;

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
        skip_integrity: bool,
    ) -> Result<String> {
        info!("Compiling for 1ES target");

        // Collect extensions (needed for MCPG config and allowed domains)
        let extensions = super::extensions::collect_extensions(front_matter);

        // Build compile context for MCPG config generation
        let input_dir = input_path.parent().unwrap_or(Path::new("."));
        let ctx = super::extensions::CompileContext::new(front_matter, input_dir).await;

        // Generate values shared with standalone that are passed as extra replacements
        let allowed_domains = generate_allowed_domains(front_matter, &extensions)?;
        let enabled_tools_args = generate_enabled_tools_args(front_matter);
        let cancel_previous_builds = generate_cancel_previous_builds(&front_matter.triggers);

        let mcpg_config = generate_mcpg_config(front_matter, &ctx, &extensions)?;
        let mcpg_config_json = serde_json::to_string_pretty(&mcpg_config)
            .context("Failed to serialize MCPG config")?;
        let mcpg_docker_env = generate_mcpg_docker_env(front_matter);
        let mcp_client_config = generate_mcp_client_config(&mcpg_config)?;

        // Generate 1ES-specific setup/teardown jobs (no per-job pool, uses templateContext).
        // These override the shared {{ setup_job }} / {{ teardown_job }} markers via
        // extra_replacements, which are applied before the shared replacements.
        let setup_job = generate_setup_job(&front_matter.setup);
        let teardown_job = generate_teardown_job(&front_matter.teardown);

        let config = CompileConfig {
            template: include_str!("../data/1es-base.yml").to_string(),
            extra_replacements: vec![
                ("{{ firewall_version }}".into(), AWF_VERSION.into()),
                ("{{ mcpg_version }}".into(), MCPG_VERSION.into()),
                ("{{ mcpg_image }}".into(), MCPG_IMAGE.into()),
                ("{{ mcpg_port }}".into(), MCPG_PORT.to_string()),
                ("{{ mcpg_domain }}".into(), MCPG_DOMAIN.into()),
                ("{{ allowed_domains }}".into(), allowed_domains),
                ("{{ enabled_tools_args }}".into(), enabled_tools_args),
                ("{{ cancel_previous_builds }}".into(), cancel_previous_builds),
                ("{{ mcpg_config }}".into(), mcpg_config_json),
                ("{{ mcpg_docker_env }}".into(), mcpg_docker_env),
                ("{{ mcp_client_config }}".into(), mcp_client_config),
                ("{{ setup_job }}".into(), setup_job),
                ("{{ teardown_job }}".into(), teardown_job),
            ],
            skip_integrity,
        };

        compile_shared(input_path, output_path, front_matter, markdown_body, &extensions, &ctx, config).await
    }
}

// ==================== 1ES-specific helpers ====================

/// Generate setup job for 1ES template.
/// Unlike standalone, 1ES jobs don't have per-job `pool:` — the pool is at
/// the top-level `parameters.pool`. Jobs use `templateContext: type: buildJob`.
fn generate_setup_job(setup_steps: &[serde_yaml::Value]) -> String {
    if setup_steps.is_empty() {
        return String::new();
    }

    let steps_yaml = format_steps_yaml_indented(setup_steps, 6);

    format!(
        r#"- job: Setup
  displayName: "Setup"
  templateContext:
    type: buildJob
    steps:
      - checkout: self
{}
"#,
        steps_yaml
    )
}

/// Generate teardown job for 1ES template.
/// Unlike standalone, 1ES jobs don't have per-job `pool:`.
fn generate_teardown_job(teardown_steps: &[serde_yaml::Value]) -> String {
    if teardown_steps.is_empty() {
        return String::new();
    }

    let steps_yaml = format_steps_yaml_indented(teardown_steps, 6);

    format!(
        r#"- job: Teardown
  displayName: "Teardown"
  dependsOn: Execution
  templateContext:
    type: buildJob
    steps:
      - checkout: self
{}
"#,
        steps_yaml
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── generate_setup_job ──────────────────────────────────────────────────

    #[test]
    fn test_generate_setup_job_empty_steps() {
        let result = generate_setup_job(&[]);
        assert!(result.is_empty(), "Empty setup steps should return empty string");
    }

    #[test]
    fn test_generate_setup_job_with_steps() {
        let step: serde_yaml::Value =
            serde_yaml::from_str("bash: echo setup").expect("valid yaml");
        let result = generate_setup_job(&[step]);
        assert!(result.contains("Setup"), "Should define a Setup job");
        assert!(result.contains("displayName: \"Setup\""), "Should use simple display name");
        assert!(result.contains("checkout: self"), "Should include self checkout");
        assert!(result.contains("echo setup"), "Should include the step content");
        assert!(result.contains("templateContext"), "Should include templateContext");
        assert!(result.contains("type: buildJob"), "Should use buildJob type");
        assert!(!result.contains("pool:"), "Should not include per-job pool");
    }

    // ─── generate_teardown_job ───────────────────────────────────────────────

    #[test]
    fn test_generate_teardown_job_empty_steps() {
        let result = generate_teardown_job(&[]);
        assert!(result.is_empty(), "Empty teardown steps should return empty string");
    }

    #[test]
    fn test_generate_teardown_job_with_steps() {
        let step: serde_yaml::Value =
            serde_yaml::from_str("bash: echo teardown").expect("valid yaml");
        let result = generate_teardown_job(&[step]);
        assert!(result.contains("Teardown"), "Should define a Teardown job");
        assert!(
            result.contains("displayName: \"Teardown\""),
            "Should use simple display name"
        );
        assert!(
            result.contains("dependsOn: Execution"),
            "Should depend on Execution"
        );
        assert!(result.contains("checkout: self"), "Should include self checkout");
        assert!(result.contains("echo teardown"), "Should include the step content");
        assert!(result.contains("templateContext"), "Should include templateContext");
        assert!(!result.contains("pool:"), "Should not include per-job pool");
    }
}
