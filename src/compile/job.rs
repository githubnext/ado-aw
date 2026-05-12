//! Job-level ADO template compiler.
//!
//! This compiler generates a reusable ADO YAML template with `jobs:` at root.
//! Users include it in their existing pipelines via `- template: <path>`.
//!
//! Two inclusion patterns:
//! - Directly in a flat pipeline's `jobs:` list
//! - Inside a user-defined stage's `jobs:` list

use anyhow::{Context, Result};
use async_trait::async_trait;
use log::{info, warn};
use std::path::Path;

use super::Compiler;
use super::common::{
    AWF_VERSION, MCPG_VERSION, MCPG_IMAGE, MCPG_PORT, MCPG_DOMAIN,
    CompileConfig, compile_shared,
    generate_allowed_domains,
    generate_awf_mounts,
    generate_awf_path_step,
    collect_awf_path_prepends,
    generate_enabled_tools_args,
    generate_mcpg_config, generate_mcpg_docker_env, generate_mcpg_step_env,
    generate_stage_prefix, generate_template_parameters,
    generate_header_comment,
};
use super::types::FrontMatter;

/// Job-level template compiler.
pub struct JobCompiler;

#[async_trait]
impl Compiler for JobCompiler {
    fn target_name(&self) -> &'static str {
        "job"
    }

    async fn compile(
        &self,
        input_path: &Path,
        output_path: &Path,
        front_matter: &FrontMatter,
        markdown_body: &str,
        skip_integrity: bool,
        debug_pipeline: bool,
    ) -> Result<String> {
        info!("Compiling for job template target");

        if front_matter.on_config.is_some() {
            warn!("on: trigger configuration is ignored for target: job (triggers are the parent pipeline's concern)");
        }

        // Collect extensions (needed before compile_shared for MCPG config)
        let extensions = super::extensions::collect_extensions(front_matter);

        // Build compile context for MCPG config generation
        let input_dir = input_path.parent().unwrap_or(std::path::Path::new("."));
        let ctx = super::extensions::CompileContext::new(front_matter, input_dir).await?;

        // Generate stage prefix for job-name uniqueness
        let stage_prefix = generate_stage_prefix(&front_matter.name);

        // Generate template-level parameters
        let template_params = generate_template_parameters(front_matter)?;

        // Same AWF/MCPG values as standalone
        let allowed_domains = generate_allowed_domains(front_matter, &extensions)?;
        let awf_mounts = generate_awf_mounts(&extensions);
        let awf_paths = collect_awf_path_prepends(&extensions);
        let awf_path_step = generate_awf_path_step(&awf_paths);
        let enabled_tools_args = generate_enabled_tools_args(front_matter);

        let config_obj = generate_mcpg_config(front_matter, &ctx, &extensions)?;
        let mcpg_config_json =
            serde_json::to_string_pretty(&config_obj).context("Failed to serialize MCPG config")?;
        let mcpg_docker_env = generate_mcpg_docker_env(front_matter, &extensions);
        let mcpg_step_env = generate_mcpg_step_env(&extensions);

        let config = CompileConfig {
            template: include_str!("../data/job-base.yml").to_string(),
            extra_replacements: vec![
                ("{{ stage_prefix }}".into(), stage_prefix),
                ("{{ template_parameters }}".into(), template_params),
                ("{{ firewall_version }}".into(), AWF_VERSION.into()),
                ("{{ mcpg_version }}".into(), MCPG_VERSION.into()),
                ("{{ mcpg_image }}".into(), MCPG_IMAGE.into()),
                ("{{ mcpg_port }}".into(), MCPG_PORT.to_string()),
                ("{{ mcpg_domain }}".into(), MCPG_DOMAIN.into()),
                ("{{ allowed_domains }}".into(), allowed_domains),
                ("{{ awf_mounts }}".into(), awf_mounts),
                ("{{ awf_path_step }}".into(), awf_path_step),
                ("{{ enabled_tools_args }}".into(), enabled_tools_args),
                ("{{ mcpg_config }}".into(), mcpg_config_json),
                ("{{ mcpg_docker_env }}".into(), mcpg_docker_env),
                ("{{ mcpg_step_env }}".into(), mcpg_step_env),
            ],
            skip_integrity,
            debug_pipeline,
            has_awf_paths: !awf_paths.is_empty(),
            skip_header: true,
        };

        let yaml = compile_shared(
            input_path, output_path, front_matter, markdown_body,
            &extensions, &ctx, config,
        ).await?;

        // Prepend custom header with job-template usage instructions
        let header = generate_job_header(input_path, front_matter);
        Ok(format!("{}{}", header, yaml))
    }
}

/// Generate the header comment block for job-level templates.
fn generate_job_header(input_path: &Path, front_matter: &FrontMatter) -> String {
    let base_header = generate_header_comment(input_path);
    let lock_path = input_path
        .with_extension("lock.yml")
        .to_string_lossy()
        .replace('\\', "/");

    let mut header = base_header;
    header.push_str("#\n");
    header.push_str("# Job-level ADO template. Include in your pipeline:\n");
    header.push_str("#\n");
    header.push_str("#   jobs:\n");
    header.push_str(&format!("#     - template: {}\n", lock_path));
    header.push_str("#\n");
    header.push_str("# Or inside a stage in a multi-stage pipeline:\n");
    header.push_str("#\n");
    header.push_str("#   stages:\n");
    header.push_str("#     - stage: AgenticReview\n");
    header.push_str("#       dependsOn: Build\n");
    header.push_str("#       jobs:\n");
    header.push_str(&format!("#         - template: {}\n", lock_path));

    // Document required resources if agent uses repos
    if !front_matter.repositories.is_empty() {
        header.push_str("#\n");
        header.push_str("# Add these repositories to your pipeline's resources: block:\n");
        header.push_str("#\n");
        header.push_str("#   resources:\n");
        header.push_str("#     repositories:\n");
        for repo in &front_matter.repositories {
            header.push_str(&format!("#       - repository: {}\n", repo.repository));
            header.push_str(&format!("#         type: {}\n", repo.repo_type));
            header.push_str(&format!("#         name: {}\n", repo.name));
        }
    }

    header.push('\n');
    header
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::generate_stage_prefix;

    #[test]
    fn test_generate_stage_prefix_basic() {
        assert_eq!(generate_stage_prefix("Daily Code Review"), "DailyCodeReview");
    }

    #[test]
    fn test_generate_stage_prefix_hyphens() {
        assert_eq!(generate_stage_prefix("my-agent-123"), "MyAgent123");
    }

    #[test]
    fn test_generate_stage_prefix_empty() {
        assert_eq!(generate_stage_prefix(""), "Agent");
    }

    #[test]
    fn test_generate_stage_prefix_leading_digit() {
        assert_eq!(generate_stage_prefix("123start"), "_123start");
    }

    #[test]
    fn test_generate_stage_prefix_single_word() {
        assert_eq!(generate_stage_prefix("review"), "Review");
    }

    #[test]
    fn test_generate_stage_prefix_underscores() {
        assert_eq!(generate_stage_prefix("code_review_agent"), "CodeReviewAgent");
    }

    #[test]
    fn test_generate_stage_prefix_unicode_stripped() {
        // ADO identifiers require [A-Za-z0-9_]; non-ASCII chars are split points
        assert_eq!(generate_stage_prefix("über-agent"), "BerAgent");
    }
}
