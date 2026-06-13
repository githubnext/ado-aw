//! Typed-IR builder for the `target: job` compile target.
//!
//! This module replaces `src/data/job-base.yml` for the
//! job-template pipeline shape: instead of interpolating values
//! into a YAML string template, [`build_job_pipeline`] composes a
//! typed [`Pipeline`] programmatically that the
//! [`crate::compile::ir::lower`] pass serialises.
//!
//! ## Shape
//!
//! A job template emits as a flat top-level `jobs:` block holding
//! the canonical 5-job graph (`Setup?, <prefix>_Agent,
//! <prefix>_Detection, <prefix>_SafeOutputs, Teardown?`). The
//! outer pipeline carries:
//!
//! - No top-level `name:` / `resources:` / `schedules:` /
//!   `trigger:` / `pr:` keys — the parent pipeline owns those.
//! - A `parameters:` block with the auto-injected `dependsOn`
//!   (`type: object`, `default: []`) and `condition` (`type: string`,
//!   `default: ''`) parameters so callers can pass job ordering at
//!   the template-invocation site.
//! - The `<prefix>_Agent` job carries
//!   [`crate::compile::ir::job::TemplateDependsOnWrap`] so the
//!   lowering emits dual-branch
//!   `${{ if eq(length(parameters.dependsOn), 0) }}` /
//!   `${{ if ne(...) }}` blocks that merge the internal `Setup`
//!   dependency with the caller-supplied `dependsOn` list (and
//!   appends `${{ parameters.condition }}` into the internal
//!   condition's `and(…)` body).
//!
//! Job-id prefixing matches the legacy template (Agent / Detection /
//! SafeOutputs are prefixed; Setup / Teardown are unprefixed). See
//! [`crate::compile::agentic_pipeline::JobPrefix`] for the prefix rule.

use anyhow::Result;
use std::path::Path;

use super::common;
use super::extensions::{CompileContext, Extension};
use super::ir::ids::JobId;
use super::ir::job::TemplateDependsOnWrap;
use super::ir::{Pipeline, PipelineBody, PipelineShape, Resources, TemplateParams, Triggers};
use super::agentic_pipeline::build_pipeline_context;
use super::types::FrontMatter;

/// Build the typed [`Pipeline`] for the `target: job` compile target.
/// See module docs for the shape.
#[allow(clippy::too_many_arguments)]
pub fn build_job_pipeline(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    ctx: &CompileContext<'_>,
    input_path: &Path,
    output_path: &Path,
    markdown_body: &str,
    skip_integrity: bool,
    debug_pipeline: bool,
) -> Result<Pipeline> {
    if front_matter.on_config.is_some() {
        log::warn!(
            "on: trigger configuration is ignored for target: job (triggers are the parent pipeline's concern)"
        );
    }

    let stage_prefix = common::generate_stage_prefix(&front_matter.name);

    let built = build_pipeline_context(
        front_matter,
        extensions,
        ctx,
        input_path,
        output_path,
        markdown_body,
        skip_integrity,
        debug_pipeline,
        Some(&stage_prefix),
    )?;

    // Locate the Agent job (prefixed when stage_prefix is set) and
    // attach the template-parameter dual-branch wrap so the lowering
    // emits the `${{ if eq(... }}` / `${{ if ne(... }}` blocks.
    let agent_id = JobId::new(format!("{}_Agent", stage_prefix))?;
    let mut jobs = built.jobs;
    for job in jobs.iter_mut() {
        if job.id == agent_id {
            job.template_dependson_wrap =
                Some(TemplateDependsOnWrap::new("dependsOn", "condition")?);
        }
    }

    let _ = built.resources;
    let _ = built.triggers;

    Ok(Pipeline {
        name: String::new(),
        parameters: built.parameters,
        resources: Resources::default(),
        triggers: Triggers::default(),
        variables: Vec::new(),
        body: PipelineBody::Jobs(jobs),
        shape: PipelineShape::JobTemplate {
            external_params: TemplateParams::default(),
        },
    })
}
