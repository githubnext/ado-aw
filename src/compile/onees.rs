//! 1ES Pipeline Templates compiler.
//!
//! This compiler generates a pipeline that extends the 1ES Unofficial
//! Pipeline Template with Copilot CLI, AWF network isolation, and MCP
//! Gateway — matching the standalone pipeline model while maintaining
//! 1ES SDL compliance.
//!
//! Thin entry-point that delegates to
//! [`crate::compile::onees_ir::build_onees_pipeline`] for IR
//! construction and [`crate::compile::ir::emit::emit`] for YAML
//! serialisation; mirrors the `standalone.rs` / `stage.rs` / `job.rs`
//! shape.

use anyhow::Result;
use async_trait::async_trait;
use log::info;
use std::path::Path;

use super::Compiler;
use super::common;
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
        imported_prompt_body: &str,
        skip_integrity: bool,
        debug_pipeline: bool,
    ) -> Result<String> {
        info!("Compiling for 1ES target (typed IR)");

        let extensions = super::extensions::collect_extensions(front_matter);
        let mut ctx = super::extensions::CompileContext::new(front_matter, input_path).await?;
        ctx.imported_prompt_body = imported_prompt_body.to_string();

        let pipeline = super::onees_ir::build_onees_pipeline(
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
        let header = common::generate_header_comment(input_path);
        // Mirror standalone.rs: legacy emitter inserts a blank line
        // between the header comment block and the first `name:` key —
        // preserve it so committed lock files stay byte-identical.
        let full = format!("{}\n{}", header, yaml);

        common::atomic_write(output_path, &full).await?;
        Ok(full)
    }
}
