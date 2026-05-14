//! Job-level ADO template compiler.
//!
//! This compiler generates a reusable ADO YAML template with `jobs:` at root.
//! Users include it in their existing pipelines via `- template: <path>`.
//!
//! Two inclusion patterns:
//! - Directly in a flat pipeline's `jobs:` list
//! - Inside a user-defined stage's `jobs:` list

use anyhow::Result;
use async_trait::async_trait;
use log::warn;
use std::path::Path;

use super::Compiler;
use super::common::{
    compile_template_target, generate_header_comment, TemplateTargetConfig,
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
        if front_matter.on_config.is_some() {
            warn!("on: trigger configuration is ignored for target: job (triggers are the parent pipeline's concern)");
        }

        compile_template_target(
            input_path,
            output_path,
            front_matter,
            markdown_body,
            TemplateTargetConfig {
                template: include_str!("../data/job-base.yml"),
                skip_integrity,
                debug_pipeline,
            },
            generate_job_header,
        )
        .await
    }
}

/// Generate the header comment block for job-level templates.
fn generate_job_header(input_path: &Path, output_path: &Path, front_matter: &FrontMatter) -> String {
    let base_header = generate_header_comment(input_path);
    let mut lock_path = output_path.to_string_lossy().replace('\\', "/");
    // Strip redundant leading "./" (same normalization as generate_header_comment)
    while lock_path.starts_with("./") {
        lock_path = lock_path[2..].to_string();
    }

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
