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
//!     parameters:
//!       dependsOn: Build
//!       condition: succeeded()
//! ```
//!
//! ADO's `stages.template` schema only allows `template:` and `parameters:`
//! at the call site, so `dependsOn` / `condition` are surfaced as template
//! parameters and the template applies them inside.

use anyhow::Result;
use async_trait::async_trait;
use log::info;
use std::path::Path;

use super::Compiler;
use super::common::{self, generate_header_comment};
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
        info!("Compiling for stage target (typed IR)");

        let extensions = super::extensions::collect_extensions(front_matter);
        let ctx = super::extensions::CompileContext::new(front_matter, input_path).await?;

        let pipeline = super::stage_ir::build_stage_pipeline(
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
        let header = generate_stage_header(input_path, output_path, front_matter);
        // Mirror standalone.rs: legacy emitter places a blank line
        // between the header comment block and the first key.
        let full = format!("{}{}", header, yaml);

        common::atomic_write(output_path, &full).await?;
        Ok(full)
    }
}

/// Generate the header comment block for stage-level templates.
fn generate_stage_header(
    input_path: &Path,
    output_path: &Path,
    front_matter: &FrontMatter,
) -> String {
    let base_header = generate_header_comment(input_path);
    let mut lock_path = output_path.to_string_lossy().replace('\\', "/");
    while lock_path.starts_with("./") {
        lock_path = lock_path[2..].to_string();
    }

    let mut header = base_header;
    header.push_str("#\n");
    header.push_str("# Stage-level ADO template. Include in your pipeline:\n");
    header.push_str("#\n");
    header.push_str("#   stages:\n");
    header.push_str(&format!("#     - template: {}\n", lock_path));
    header.push_str("#       parameters:\n");
    header.push_str("#         dependsOn: Build              # or [Build, Test]; omit for implicit dep on previous stage\n");
    header
        .push_str("#         condition: succeeded('Build') # omit for ADO's default succeeded()\n");
    header.push_str("#\n");
    header
        .push_str("# ADO's stages.template schema only allows `template:` and `parameters:` at\n");
    header.push_str(
        "# the call site \u{2014} `dependsOn:` / `condition:` are passed via parameters.\n",
    );
    header.push_str(
        "# See https://learn.microsoft.com/azure/devops/pipelines/yaml-schema/stages-template\n",
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
