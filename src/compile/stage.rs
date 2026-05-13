//! Stage-level ADO template compiler.
//!
//! This compiler generates a reusable ADO YAML template with `stages:` at root
//! wrapping the 3-job chain (Agent → Detection → Execution).
//!
//! Users include it in their multi-stage pipeline via:
//!
//! ```yaml
//! stages:
//!   - template: agents/review.lock.yml
//!     dependsOn: Build
//!     condition: succeeded()
//! ```
//!
//! ADO natively supports `dependsOn` and `condition` at the template call site,
//! so these don't need to be template parameters.

use anyhow::Result;
use async_trait::async_trait;
use log::warn;
use std::path::Path;

use super::Compiler;
use super::common::{
    compile_template_target, TemplateTargetConfig,
    generate_header_comment,
};
use super::types::FrontMatter;

/// Stage-level template compiler.
pub struct StageCompiler;

#[async_trait]
impl Compiler for StageCompiler {
    fn target_name(&self) -> &'static str {
        "stage"
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
            warn!("on: trigger configuration is ignored for target: stage (triggers are the parent pipeline's concern)");
        }

        compile_template_target(
            input_path,
            output_path,
            front_matter,
            markdown_body,
            TemplateTargetConfig {
                template: include_str!("../data/stage-base.yml"),
                skip_integrity,
                debug_pipeline,
            },
            generate_stage_header,
        ).await
    }
}

/// Generate the header comment block for stage-level templates.
fn generate_stage_header(input_path: &Path, output_path: &Path, front_matter: &FrontMatter) -> String {
    let base_header = generate_header_comment(input_path);
    let mut lock_path = output_path
        .to_string_lossy()
        .replace('\\', "/");
    while lock_path.starts_with("./") {
        lock_path = lock_path[2..].to_string();
    }

    let mut header = base_header;
    header.push_str("#\n");
    header.push_str("# Stage-level ADO template. Include in your pipeline:\n");
    header.push_str("#\n");
    header.push_str("#   stages:\n");
    header.push_str(&format!("#     - template: {}\n", lock_path));
    header.push_str("#       dependsOn: Build\n");
    header.push_str("#       condition: succeeded()\n");

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
