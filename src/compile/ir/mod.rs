//! Pipeline IR — typed representation of an Azure DevOps pipeline.
//!
//! See `docs/ir.md` for the full reference.
//!
//! ## Layout
//!
//! - [`ids`] — typed newtype identifiers (`StageId`, `JobId`,
//!   `StepId`).
//! - [`step`] — step types (`Step`, `BashStep`, `TaskStep`,
//!   `CheckoutStep`, `DownloadStep`, `PublishStep`).
//! - [`job`] — `Job` and `Pool`.
//! - [`stage`] — `Stage`.
//! - [`mod@env`] — typed `EnvValue` (incl. `Coalesce` and `StepOutput`).
//! - [`condition`] — typed ADO condition AST (`Condition` + `Expr`).
//! - [`output`] — `OutputDecl` / `OutputRef`.
//! - [`Pipeline`] / [`PipelineBody`] / [`PipelineShape`] — the root
//!   container in this file.
//!
//! ## Dead-code allow rationale
//!
//! Several constructor helpers (`Condition::and`/`or`/`not`,
//! `EnvValue::secret`/`concat`, `Job::push_step`, `Stage::push_job`,
//! …) plus the convenience [`graph::resolve`] pipeline-mutation entry
//! point and its sub-passes (`apply_edges`, `apply_auto_is_output`,
//! `merge_job_deps`) are intentionally **API surface** rather than
//! production callers. The compile path threads
//! [`graph::build_graph`] → [`graph::detect_cycles`] → emit, wiring
//! `depends_on` explicitly inside the canonical-jobs builder
//! (`agentic_pipeline.rs`, shared by every target wrapper); the
//! `apply_*` helpers are kept for any future caller that wants the
//! documented "build → derive → validate → emit" flow (e.g. tooling
//! that lints or transforms a Pipeline before emit).
//!
//! Per-item `#[allow(dead_code)]` annotations would be churn; the
//! module-level allow is the pragmatic line.
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

use ids::StageId;
use job::{Job, Pool};
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
    /// over `v1/1ES.Unofficial.PipelineTemplate.yml@1ESPipelineTemplates`.
    ///
    /// `top_level_pool` is hoisted to `extends.parameters.pool` (no
    /// per-job pool is emitted; the contained [`Job`]s carry
    /// `template_context = Some(_)` so the lowering pass suppresses
    /// `pool:` and wraps `steps:` under `templateContext:`).
    ///
    /// `stage_id` / `stage_display_name` name the single `AgentStage`
    /// that wraps the canonical 5-job graph under
    /// `extends.parameters.stages[0]`.
    OneEs {
        sdl: OneEsSdlConfig,
        top_level_pool: Pool,
        stage_id: StageId,
        stage_display_name: String,
    },
    /// `target: job` — emits a jobs-template with external
    /// `parameters: dependsOn / condition` template params.
    JobTemplate { external_params: TemplateParams },
    /// `target: stage` — emits a single stage as a template.
    StageTemplate { external_params: TemplateParams },
}

/// 1ES SDL configuration.
///
/// `source_analysis_pool` populates `sdl.sourceAnalysisPool` (the
/// pool that hosts the SDL credential/secret scan stage). The 1ES
/// template requires a Windows pool here; today we hard-code
/// `AZS-1ES-W-MMS2022` to match the legacy `1es-base.yml` output.
///
/// `feature_flags` populates `sdl.featureFlags` —
/// `disableNetworkIsolation` is `true` because AWF handles isolation
/// at the application layer, and `runPrerequisitesOnImage` is `false`
/// because the agent pool image already has 1ES prerequisites
/// preinstalled.
#[derive(Debug, Clone, Default)]
pub struct OneEsSdlConfig {
    pub source_analysis_pool: OneEsSourceAnalysisPool,
    pub feature_flags: OneEsFeatureFlags,
}

/// `extends.parameters.sdl.sourceAnalysisPool` — the pool that hosts
/// the 1ES SDL credential / secret scan stage. Must be a Windows pool
/// per 1ES template requirements.
#[derive(Debug, Clone)]
pub struct OneEsSourceAnalysisPool {
    pub name: String,
    pub os: String,
}

impl Default for OneEsSourceAnalysisPool {
    fn default() -> Self {
        Self {
            name: "AZS-1ES-W-MMS2022".to_string(),
            os: "windows".to_string(),
        }
    }
}

/// `extends.parameters.sdl.featureFlags` — toggles that we set
/// uniformly today; carried as a struct so the shape stays open for
/// future per-agent customisation.
#[derive(Debug, Clone)]
pub struct OneEsFeatureFlags {
    /// AWF handles network isolation at the application layer; the
    /// 1ES template-level isolation is mutually exclusive with the
    /// Docker-based AWF launch.
    pub disable_network_isolation: bool,
    /// The agent pool image already has 1ES prerequisites
    /// preinstalled, so re-running them during the buildJob is wasted
    /// time.
    pub run_prerequisites_on_image: bool,
}

impl Default for OneEsFeatureFlags {
    fn default() -> Self {
        Self {
            disable_network_isolation: true,
            run_prerequisites_on_image: false,
        }
    }
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
    /// When `None`, the parameter is emitted without a `displayName:`
    /// key. Used for auto-injected template parameters (`dependsOn`,
    /// `condition`) that surface only as plumbing — they don't appear
    /// in the ADO UI parameter dropdown.
    pub display_name: Option<String>,
    pub kind: ParameterKind,
    pub default: ParameterDefault,
    /// Optional `values:` enumeration — restricts the parameter to a
    /// finite set of strings/numbers; surfaced as a dropdown in the
    /// ADO pipeline UI.
    pub values: Vec<serde_yaml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterKind {
    Boolean,
    String,
    Number,
    /// ADO `object` type — accepts arbitrary YAML structures (lists,
    /// mappings, scalars). Used by template targets for
    /// `parameters.dependsOn` which defaults to `[]`.
    Object,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterDefault {
    Bool(bool),
    String(String),
    Number(i64),
    /// YAML sequence default (e.g. the empty list `[]` for
    /// `parameters.dependsOn`). Emitted as a flow / block sequence
    /// by the lowering pass.
    Sequence(Vec<serde_yaml::Value>),
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
            top_level_pool: Pool::Named {
                name: "AzurePipelines-EO".into(),
                image: None,
                os: Some("linux".into()),
            },
            stage_id: StageId::new("AgentStage").unwrap(),
            stage_display_name: "Agent".into(),
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
