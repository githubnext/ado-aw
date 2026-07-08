//! Standalone-target wrapper around the canonical agentic-pipeline
//! shape.
//!
//! Historically this file held ~2,300 lines defining the canonical
//! Setup → Agent → Detection → SafeOutputs → Teardown shape consumed
//! by every target (`standalone`, `1es`, `job`, `stage`). That shape
//! now lives in [`super::agentic_pipeline`] (see issue #987); this
//! module retains only the thin standalone-specific wrapper that
//! lifts the canonical [`super::agentic_pipeline::BuiltPipelineContext`]
//! into a top-level [`Pipeline`] with [`PipelineShape::Standalone`].
//!
//! The `1es` / `job` / `stage` targets have parallel one-screen
//! wrappers in `onees_ir.rs`, `job_ir.rs`, and `stage_ir.rs` that
//! call into the same shared back-end.

use anyhow::Result;
use std::path::Path;

use super::agentic_pipeline::build_pipeline_context;
use super::common::variable_group_vars;
use super::extensions::{CompileContext, Extension};
use super::ir::{Pipeline, PipelineBody, PipelineShape};
use super::types::FrontMatter;

/// Build the typed [`Pipeline`] for the standalone target.
///
/// Thin wrapper over [`build_pipeline_context`]: every validation,
/// scalar computation, extension fanout, and canonical-job
/// construction is performed once by the shared back-end; this
/// function then wraps the result in the standalone-shape pipeline
/// envelope. The lowering pass in [`crate::compile::ir::lower`]
/// serialises it to YAML.
#[allow(clippy::too_many_arguments)]
pub fn build_standalone_pipeline(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    ctx: &CompileContext<'_>,
    input_path: &Path,
    output_path: &Path,
    markdown_body: &str,
    skip_integrity: bool,
    debug_pipeline: bool,
) -> Result<Pipeline> {
    let built = build_pipeline_context(
        front_matter,
        extensions,
        ctx,
        input_path,
        output_path,
        markdown_body,
        skip_integrity,
        debug_pipeline,
        None,
    )?;
    Ok(Pipeline {
        name: built.pipeline_name,
        parameters: built.parameters,
        resources: built.resources,
        triggers: built.triggers,
        variables: variable_group_vars(front_matter),
        body: PipelineBody::Jobs(built.jobs),
        shape: PipelineShape::Standalone,
    })
}

