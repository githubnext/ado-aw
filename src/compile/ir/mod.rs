//! Pipeline IR — typed representation of an Azure DevOps pipeline.
//!
//! This module is the entry point for the new pipeline IR introduced
//! by the "Native ADO Pipeline IR" plan. The full design lives in
//! the plan file (`plan.md` in the session workspace) and will move
//! to `docs/ir.md` as part of the `docs-update` commit.
//!
//! ## Layout
//!
//! - [`ids`] — typed newtype identifiers (`StageId`, `JobId`,
//!   `StepId`).
//! - [`step`] — step types (`Step`, `BashStep`, `TaskStep`,
//!   `CheckoutStep`, `DownloadStep`, `PublishStep`).
//! - [`job`] — `Job` and `Pool`.
//! - [`stage`] — `Stage`.
//! - [`env`] — typed `EnvValue` (incl. `Coalesce` and `StepOutput`).
//! - [`condition`] — typed ADO condition AST (`Condition` + `Expr`).
//! - [`output`] — `OutputDecl` / `OutputRef`.
//! - [`Pipeline`] / [`PipelineBody`] / [`PipelineShape`] — the root
//!   container in this file.
//!
//! ## Status
//!
//! As of the `ir-types` commit the module exports **types only**.
//! The dependency-graph pass, YAML emit, output-reference lowering,
//! and condition codegen are introduced in subsequent commits per
//! the plan.
//!
//! Until the `extension-trait-port` commit wires real callers, every
//! type in this module is unreachable from production code — hence
//! the module-scoped `dead_code` allow. The unit tests in each
//! submodule exercise constructors and would surface accidental
//! breakage. The allow is removed atomically with the trait port.
#![allow(dead_code)]

pub mod condition;
pub mod emit;
pub mod env;
pub mod graph;
pub mod ids;
pub mod job;
pub mod lower;
pub mod output;
pub mod stage;
pub mod step;

use job::Job;
use stage::Stage;

/// Top-level pipeline IR.
#[derive(Debug, Clone)]
pub struct Pipeline {
    /// Top-level `name:` (the ADO build-number format string).
    pub name: String,
    /// Top-level `parameters:` block.
    pub parameters: Vec<Parameter>,
    /// Top-level `resources:` block.
    pub resources: Resources,
    /// `schedules:` / `trigger:` / `pr:` / `resources.pipelines.trigger`.
    pub triggers: Triggers,
    /// Top-level `variables:` block.
    pub variables: Vec<PipelineVar>,
    /// Either a flat list of jobs or a list of stages.
    pub body: PipelineBody,
    /// Wrapping shape (standalone / 1ES / job template / stage template).
    pub shape: PipelineShape,
}

/// Either a flat list of jobs (`Standalone`, `JobTemplate`) or a list
/// of stages (`OneEs`, `StageTemplate`).
#[derive(Debug, Clone)]
pub enum PipelineBody {
    Jobs(Vec<Job>),
    Stages(Vec<Stage>),
}

/// Wrapping shape for the pipeline. Captures the per-target
/// differences (1ES `extends:` block, `target: job` / `target: stage`
/// outer template-parameters) that today live in
/// `src/data/*-base.yml`.
#[derive(Debug, Clone)]
pub enum PipelineShape {
    /// Plain pipeline emitted directly.
    Standalone,
    /// 1ES Pipeline Templates wrapping: top-level `extends:` block
    /// over `1es-pipelines.yaml@1esPipelines`.
    OneEs { sdl: OneEsSdlConfig },
    /// `target: job` — emits a jobs-template with external
    /// `parameters: dependsOn / condition` template params.
    JobTemplate { external_params: TemplateParams },
    /// `target: stage` — emits a single stage as a template.
    StageTemplate { external_params: TemplateParams },
}

/// 1ES SDL configuration. Placeholder shape — filled out by the
/// `compile-target-1es` commit when the actual 1ES wrapping is
/// ported.
#[derive(Debug, Clone, Default)]
pub struct OneEsSdlConfig {
    /// Reserved for future fields (credscan / antimalware / etc.).
    #[allow(dead_code)]
    pub reserved: (),
}

/// External template parameters injected by callers of a
/// `target: job` / `target: stage` template (`parameters.dependsOn`
/// and `parameters.condition`). Placeholder shape — filled out by
/// the `compile-target-job` / `compile-target-stage` commits.
#[derive(Debug, Clone, Default)]
pub struct TemplateParams {
    #[allow(dead_code)]
    pub reserved: (),
}

/// A pipeline-level `parameters:` entry. Placeholder shape — the
/// `extension-trait-port` commit fills in the runtime / boolean /
/// string distinction once the canonical pipeline skeleton is being
/// built from the IR.
#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: String,
    pub display_name: String,
    pub kind: ParameterKind,
    pub default: ParameterDefault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterKind {
    Boolean,
    String,
    Number,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterDefault {
    Bool(bool),
    String(String),
    Number(i64),
    None,
}

/// `resources:` block — repositories, container images, pipelines.
#[derive(Debug, Clone, Default)]
pub struct Resources {
    pub repositories: Vec<RepositoryResource>,
    pub pipelines: Vec<PipelineResource>,
}

/// A `resources.repositories[]` entry.
///
/// Two distinct shapes:
///
/// - `SelfRepo` — the canonical `- repository: self` block carrying
///   `clean:` and `submodules:` flags. Standalone today always emits
///   one of these at the top of every lock file.
/// - `Named` — a user-declared external repository resource with
///   `type` / `name` / `ref`.
#[derive(Debug, Clone)]
pub enum RepositoryResource {
    SelfRepo {
        clean: bool,
        submodules: bool,
    },
    Named {
        identifier: String,
        kind: String,
        name: String,
        r#ref: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct PipelineResource {
    pub identifier: String,
    pub source: String,
    pub project: Option<String>,
    pub branches: Vec<String>,
    pub trigger: bool,
}

/// `schedules:`, `trigger:`, `pr:`, plus the pipeline-trigger
/// surface on resource pipelines.
#[derive(Debug, Clone, Default)]
pub struct Triggers {
    pub schedules: Vec<Schedule>,
    pub pr: Option<PrTrigger>,
    pub ci: Option<CiTrigger>,
}

/// A single `schedules[]` entry (cron + branches + always).
#[derive(Debug, Clone)]
pub struct Schedule {
    /// Cron expression in ADO's 5-field format
    /// (`minute hour day-of-month month day-of-week`).
    pub cron: String,
    pub display_name: String,
    pub branches_include: Vec<String>,
    /// `always: true` — always run even if the source code hasn't
    /// changed since the previous run. Defaults to true (matches the
    /// legacy `fuzzy_schedule::generate_schedule_yaml` output, which
    /// hard-codes `always: true`).
    pub always: bool,
}

/// `pr:` trigger configuration.
#[derive(Debug, Clone)]
pub struct PrTrigger {
    /// Empty branch list means "default behaviour".
    pub branches_include: Vec<String>,
    pub branches_exclude: Vec<String>,
    pub paths_include: Vec<String>,
    pub paths_exclude: Vec<String>,
    /// `pr: none` short-circuits any branch / path filter and emits
    /// the literal scalar `none` in place of the full block.
    pub disabled: bool,
}

/// `trigger:` (CI) configuration. Today standalone agents always
/// emit `trigger: none` (CI is suppressed when schedules /
/// pipeline-completion triggers are configured, and the default
/// "trigger on any branch" case emits no `trigger:` key at all so
/// callers can rely on ADO's implicit default).
#[derive(Debug, Clone)]
pub struct CiTrigger {
    pub disabled: bool,
}

/// A pipeline-level `variables:` entry.
#[derive(Debug, Clone)]
pub struct PipelineVar {
    pub name: String,
    pub value: String,
    pub is_secret: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::ids::{JobId, StageId};
    use crate::compile::ir::job::Pool;

    fn empty_pipeline() -> Pipeline {
        Pipeline {
            name: "Test-$(BuildID)".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(Vec::new()),
            shape: PipelineShape::Standalone,
        }
    }

    #[test]
    fn pipeline_can_be_constructed_in_isolation() {
        let p = empty_pipeline();
        assert_eq!(p.name, "Test-$(BuildID)");
        assert!(matches!(p.body, PipelineBody::Jobs(_)));
        assert!(matches!(p.shape, PipelineShape::Standalone));
    }

    #[test]
    fn pipeline_body_can_hold_jobs_or_stages() {
        let mut p = empty_pipeline();
        let job = Job::new(
            JobId::new("Agent").unwrap(),
            "Agent",
            Pool::VmImage("ubuntu-22.04".into()),
        );
        if let PipelineBody::Jobs(ref mut js) = p.body {
            js.push(job);
        }
        assert!(matches!(&p.body, PipelineBody::Jobs(js) if js.len() == 1));

        let stage = Stage::new(StageId::new("Main").unwrap(), "Main");
        p.body = PipelineBody::Stages(vec![stage]);
        assert!(matches!(&p.body, PipelineBody::Stages(ss) if ss.len() == 1));
    }

    #[test]
    fn pipeline_shape_variants_are_distinct() {
        let standalone = PipelineShape::Standalone;
        let onees = PipelineShape::OneEs {
            sdl: OneEsSdlConfig::default(),
        };
        // Tag-only equality (no derived PartialEq on PipelineShape
        // because OneEsSdlConfig is not yet PartialEq).
        let tag = |s: &PipelineShape| match s {
            PipelineShape::Standalone => 0,
            PipelineShape::OneEs { .. } => 1,
            PipelineShape::JobTemplate { .. } => 2,
            PipelineShape::StageTemplate { .. } => 3,
        };
        assert_ne!(tag(&standalone), tag(&onees));
    }
}
