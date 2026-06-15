//! Typed-IR builder for the `target: stage` compile target.
//!
//! This module replaces `src/data/stage-base.yml` for the
//! stage-template pipeline shape: instead of interpolating values
//! into a YAML string template, [`build_stage_pipeline`] composes a
//! typed [`Pipeline`] programmatically that the
//! [`crate::compile::ir::lower`] pass serialises.
//!
//! ## Shape
//!
//! A stage template emits as a single ADO stage that wraps the
//! canonical 5-job graph (`Setup?, <prefix>_Agent,
//! <prefix>_Detection, <prefix>_SafeOutputs, Teardown?`). The
//! outer pipeline carries:
//!
//! - No top-level `name:` / `resources:` / `schedules:` /
//!   `trigger:` / `pr:` keys — the parent pipeline owns those.
//! - A `parameters:` block with the auto-injected `dependsOn`
//!   (`type: object`, `default: []`) and `condition` (`type: string`,
//!   `default: ''`) parameters so callers can pass stage ordering at
//!   the template-invocation site.
//! - A single `stages:` entry whose stage carries
//!   [`crate::compile::ir::stage::Stage::external_params_wrap`] so
//!   the lowering pass emits
//!   `${{ if ne(length(parameters.dependsOn), 0) }}: dependsOn: ${{ parameters.dependsOn }}`
//!   and the matching `condition:` block.
//!
//! Job-id prefixing matches the legacy template (Agent / Detection /
//! SafeOutputs are prefixed; Setup / Teardown are unprefixed). See
//! [`crate::compile::agentic_pipeline::JobPrefix`] for the prefix rule.

use anyhow::Result;
use std::path::Path;

use super::agentic_pipeline::build_pipeline_context;
use super::common;
use super::extensions::{CompileContext, Extension};
use super::ir::ids::StageId;
use super::ir::stage::{Stage, StageExternalParamsWrap};
use super::ir::{Pipeline, PipelineBody, PipelineShape, Resources, TemplateParams, Triggers};
use super::types::FrontMatter;

/// Build the typed [`Pipeline`] for the `target: stage` compile
/// target. See module docs for the shape.
#[allow(clippy::too_many_arguments)]
pub fn build_stage_pipeline(
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
            "on: trigger configuration is ignored for target: stage (triggers are the parent pipeline's concern)"
        );
    }

    let stage_prefix = common::generate_stage_prefix(&front_matter.name);
    let agent_display_name = front_matter.name.clone();

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

    // Wrap the canonical jobs in a single `Stage` carrying the
    // external-params wrap so the lowering emits the
    // `${{ if ne(... }}` keys for caller-supplied dependsOn /
    // condition.
    let mut stage = Stage::new(StageId::new(&stage_prefix)?, agent_display_name);
    stage.jobs = built.jobs;
    stage.external_params_wrap = Some(StageExternalParamsWrap::new("dependsOn", "condition")?);

    // Discard top-level resources / triggers — the lower pass will
    // skip them for `PipelineShape::StageTemplate` anyway, but we
    // null them out so the IR Pipeline reads clean for downstream
    // tooling.
    let _ = built.resources;
    let _ = built.triggers;

    Ok(Pipeline {
        name: String::new(),
        parameters: built.parameters,
        resources: Resources::default(),
        triggers: Triggers::default(),
        variables: Vec::new(),
        body: PipelineBody::Stages(vec![stage]),
        shape: PipelineShape::StageTemplate {
            external_params: TemplateParams::default(),
        },
    })
}
