//! Typed-IR builder for the 1ES Pipeline Templates compile target.
//!
//! This module replaces `src/data/1es-base.yml` for the 1ES pipeline
//! shape: instead of interpolating values into a YAML string
//! template, [`build_onees_pipeline`] composes a typed [`Pipeline`]
//! programmatically that the [`crate::compile::ir::lower`] pass
//! serialises.
//!
//! ## Shape
//!
//! The 1ES pipeline emits as a top-level
//! `extends: template: v1/1ES.Unofficial.PipelineTemplate.yml@1ESPipelineTemplates`
//! block whose `parameters.stages[0]` is a single `AgentStage`
//! wrapping the canonical 5-job graph (`Setup?`, `Agent`,
//! `Detection`, `SafeOutputs`, `Teardown?`). Job IDs are unprefixed
//! (same as standalone).
//!
//! Differences from [`crate::compile::agentic_pipeline`]:
//!
//! - Top-level [`crate::compile::ir::Resources`] prepends a
//!   `1ESPipelineTemplates` repository resource at the head of the
//!   list. The standalone-style `self` repo + user repos follow.
//! - The agent-pool is hoisted to `extends.parameters.pool`; the
//!   per-job `pool:` keys are suppressed via
//!   [`crate::compile::ir::job::Job::template_context`].
//! - Every job carries
//!   `template_context = Some(JobTemplateContext::default())` so the
//!   lowering pass:
//!     - emits `templateContext: type: buildJob, [outputs:], steps:`
//!       in place of `pool:` + `steps:`,
//!     - lifts any `Step::Publish` in the job's steps into
//!       `templateContext.outputs[]` (the 1ES template owns the
//!       artifact publish).

use anyhow::Result;
use std::path::Path;

use super::agentic_pipeline::build_pipeline_context;
use super::common;
use super::extensions::{CompileContext, Extension};
use super::ir::ids::StageId;
use super::ir::job::{JobTemplateContext, Pool};
use super::ir::{OneEsSdlConfig, Pipeline, PipelineBody, PipelineShape, RepositoryResource};
use super::types::FrontMatter;

/// 1ES Unofficial Pipeline Templates repository identifier used
/// across every 1ES-compiled pipeline.
const ONEES_TEMPLATES_REPO_IDENTIFIER: &str = "1ESPipelineTemplates";
const ONEES_TEMPLATES_REPO_NAME: &str = "1ESPipelineTemplates/1ESPipelineTemplates";
const ONEES_TEMPLATES_REPO_KIND: &str = "git";
const ONEES_TEMPLATES_REPO_REF: &str = "refs/heads/main";

/// Build the typed [`Pipeline`] for the 1ES compile target. See
/// module docs for the shape.
#[allow(clippy::too_many_arguments)]
pub fn build_onees_pipeline(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    ctx: &CompileContext<'_>,
    input_path: &Path,
    output_path: &Path,
    markdown_body: &str,
    skip_integrity: bool,
    debug_pipeline: bool,
) -> Result<Pipeline> {
    let agent_display_name = front_matter.name.clone();
    // 1ES jobs share the same canonical structure as standalone — the
    // builder owns the 5-job graph and per-step bodies. We then wrap
    // it in the 1ES shape and tag each job with `template_context` so
    // the lowering pass emits the `templateContext:` block and lifts
    // `Step::Publish` to `templateContext.outputs[]`.
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

    let top_level_pool =
        common::resolve_pool_typed(front_matter.target.clone(), front_matter.pool.as_ref())?;

    let mut jobs = built.jobs;
    for job in jobs.iter_mut() {
        // Agentless (server) jobs — e.g. the ManualReview `ManualValidation@1`
        // gate — are not build jobs and must not be wrapped in a 1ES
        // `templateContext:` (which would suppress `pool: server` and nest the
        // server task under a build-job step list).
        if job.pool == Pool::Server {
            continue;
        }
        job.template_context = Some(JobTemplateContext::default());
    }

    // Resources: prepend the 1ESPipelineTemplates repo before the
    // standalone-built repo list (which already includes `self` +
    // user-declared repos).
    let mut resources = built.resources;
    resources.repositories.insert(
        0,
        RepositoryResource::Named {
            identifier: ONEES_TEMPLATES_REPO_IDENTIFIER.to_string(),
            kind: ONEES_TEMPLATES_REPO_KIND.to_string(),
            name: ONEES_TEMPLATES_REPO_NAME.to_string(),
            r#ref: Some(ONEES_TEMPLATES_REPO_REF.to_string()),
        },
    );

    Ok(Pipeline {
        name: built.pipeline_name,
        parameters: built.parameters,
        resources,
        triggers: built.triggers,
        variables: front_matter
            .variable_groups
            .iter()
            .cloned()
            .map(super::ir::PipelineVar::Group)
            .collect(),
        body: PipelineBody::Jobs(jobs),
        shape: PipelineShape::OneEs {
            sdl: OneEsSdlConfig::default(),
            top_level_pool,
            stage_id: StageId::new("AgentStage")?,
            stage_display_name: agent_display_name,
        },
    })
}
