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
use log::info;
use std::path::Path;

use super::Compiler;
use super::common::{self, generate_header_comment};
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
        imported_prompt_body: &str,
        skip_integrity: bool,
        debug_pipeline: bool,
    ) -> Result<String> {
        info!("Compiling for job target (typed IR)");

        let extensions = super::extensions::collect_extensions(front_matter);
        let mut ctx = super::extensions::CompileContext::new(front_matter, input_path).await?;
        ctx.imported_prompt_body = imported_prompt_body.to_string();

        let pipeline = super::job_ir::build_job_pipeline(
            front_matter,
            &extensions,
            &ctx,
            input_path,
            output_path,
            markdown_body,
            skip_integrity,
            debug_pipeline,
        )?;

        let yaml = super::ir::emit::emit(&pipeline)?;
        let yaml = common::normalize_yaml(&yaml)?;
        let header = generate_job_header(input_path, output_path, front_matter);
        let full = format!("{}{}", header, yaml);

        common::atomic_write(output_path, &full).await?;
        Ok(full)
    }
}

/// Generate the header comment block for job-level templates.
fn generate_job_header(
    input_path: &Path,
    output_path: &Path,
    front_matter: &FrontMatter,
) -> String {
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
    header.push_str("#       parameters:\n");
    header.push_str("#         dependsOn: [Build]            # list of upstream job names; omit for implicit dep on previous job\n");
    header
        .push_str("#         condition: succeeded('Build') # omit for ADO's default succeeded()\n");
    header.push_str("#\n");
    header.push_str("# Or inside a user-defined stage in a multi-stage pipeline:\n");
    header.push_str("#\n");
    header.push_str("#   stages:\n");
    header.push_str("#     - stage: AgenticReview\n");
    header.push_str("#       dependsOn: Build\n");
    header.push_str("#       jobs:\n");
    header.push_str(&format!("#         - template: {}\n", lock_path));
    header.push_str("#\n");
    header.push_str("# ADO's jobs.template schema only allows `template:` and `parameters:` at\n");
    header.push_str(
        "# the call site \u{2014} `dependsOn:` / `condition:` on a `- template:` call are\n",
    );
    header
        .push_str("# rejected. Pass them via `parameters:` so the template applies them inside.\n");
    header.push_str(
        "# When the agent has a Setup job (e.g. PR/pipeline filters), `dependsOn` MUST\n",
    );
    header.push_str("# be a list so the template can merge `Setup` with the caller's deps.\n");
    header.push_str(
        "# See https://learn.microsoft.com/azure/devops/pipelines/yaml-schema/jobs-template\n",
    );

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
        assert_eq!(
            generate_stage_prefix("Daily Code Review"),
            "DailyCodeReview"
        );
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
        assert_eq!(
            generate_stage_prefix("code_review_agent"),
            "CodeReviewAgent"
        );
    }

    #[test]
    fn test_generate_stage_prefix_unicode_stripped() {
        // ADO identifiers require [A-Za-z0-9_]; non-ASCII chars are split points
        assert_eq!(generate_stage_prefix("über-agent"), "BerAgent");
    }
}
