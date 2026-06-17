//! Serializable, agent-facing summary of a typed [`Pipeline`].
//!
//! The internal IR (`Pipeline`, `Job`, `Step`, `Graph`, …) is rich and
//! intentionally tied to the compiler's lowering needs. Exposing those
//! shapes directly over MCP / JSON would lock us into every internal
//! field rename. Instead, this module defines a parallel "summary"
//! tree with `#[derive(Serialize)]` that captures the agent-relevant
//! signals (ids, kinds, conditions, output declarations, output
//! references, derived dependency edges) and intentionally **omits**
//! internal-only bookkeeping (template wraps, 1ES templateContext,
//! lowering hints).
//!
//! ## Stability contract
//!
//! [`PipelineSummary::schema_version`] is pinned. Bump it whenever
//! the JSON shape changes in a way a downstream consumer would
//! notice (renamed field, removed variant, changed semantics).
//! Additive changes such as new optional fields do not require a bump.
//! New enum variants currently require a schema-version bump so older
//! consumers fail loudly instead of misinterpreting data.
//!
//! The summary is the **public** schema. The internal IR types
//! (`super::Pipeline` and friends) are NOT public API and may change
//! freely.

use std::collections::BTreeSet;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::condition::{Condition, Expr, codegen::CondCodegenCtx, codegen::lower_condition};
use super::env::EnvValue;
use super::graph::{Graph, build_graph};
use super::output::{OutputDecl, OutputRef};
use super::step::Step;
use super::{Pipeline, PipelineBody, PipelineShape};

/// Current public schema version. Bump when the JSON shape changes
/// in a backwards-incompatible way.
pub const SCHEMA_VERSION: u32 = 1;

/// Public, serializable summary of a compiled pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineSummary {
    /// Public schema version; see [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Top-level `name:` (the ADO build-number format string).
    pub name: String,
    /// Compile target: `"standalone"`, `"1es"`, `"job-template"`,
    /// `"stage-template"`.
    pub shape: String,
    /// Either a flat list of jobs (`standalone`, `job-template`) or
    /// a list of stages (`1es`, `stage-template`).
    pub body: PipelineBodySummary,
    /// Resolved dependency graph.
    pub graph: GraphSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PipelineBodySummary {
    Jobs { jobs: Vec<JobSummary> },
    Stages { stages: Vec<StageSummary> },
}

impl PipelineSummary {
    /// Iterate every job in the pipeline, regardless of whether the
    /// body is `Jobs`-shaped or `Stages`-shaped.
    ///
    /// Single source of truth for body-shape iteration; both
    /// `audit::pipeline_graph` and the `inspect` commands go through
    /// this so that future shape additions (e.g. a new `Templates`
    /// variant) only need to be handled in one place.
    ///
    /// Returns an `impl Iterator` rather than a `Vec` so hot paths
    /// (`populate_job_edges`, `find_matching_job_summary`, the inspect
    /// traversals) avoid a per-call heap allocation. Callers that
    /// need a slice can `.collect::<Vec<_>>()` at the use site.
    pub fn all_jobs(&self) -> impl Iterator<Item = &JobSummary> + '_ {
        match &self.body {
            PipelineBodySummary::Jobs { jobs } => AllJobsIter::Flat(jobs.iter()),
            PipelineBodySummary::Stages { stages } => {
                AllJobsIter::Stages(stages.iter().flat_map(stage_jobs))
            }
        }
    }
}

fn stage_jobs(stage: &StageSummary) -> std::slice::Iter<'_, JobSummary> {
    stage.jobs.iter()
}

/// Either-style iterator that yields the same `&JobSummary` element type
/// for both pipeline body shapes without heap-allocating into a `Vec`.
#[allow(clippy::type_complexity)]
enum AllJobsIter<'a> {
    Flat(std::slice::Iter<'a, JobSummary>),
    Stages(
        std::iter::FlatMap<
            std::slice::Iter<'a, StageSummary>,
            std::slice::Iter<'a, JobSummary>,
            fn(&'a StageSummary) -> std::slice::Iter<'a, JobSummary>,
        >,
    ),
}

impl<'a> Iterator for AllJobsIter<'a> {
    type Item = &'a JobSummary;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Flat(iter) => iter.next(),
            Self::Stages(iter) => iter.next(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageSummary {
    pub id: String,
    pub display_name: String,
    pub depends_on: Vec<String>,
    /// Lowered ADO condition string, when one is set on the stage.
    pub condition: Option<String>,
    pub jobs: Vec<JobSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: String,
    /// `None` for top-level jobs in a flat `Jobs` pipeline.
    pub stage: Option<String>,
    pub display_name: String,
    pub depends_on: Vec<String>,
    /// Lowered ADO condition string, when one is set on the job.
    pub condition: Option<String>,
    pub pool: PoolSummary,
    pub steps: Vec<StepSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PoolSummary {
    VmImage {
        image: String,
    },
    Named {
        name: String,
        image: Option<String>,
        os: Option<String>,
    },
}

/// A single step's public summary.
///
/// `kind` discriminates the step shape and the rest of the fields
/// are populated per kind. `id` is the ADO step `name:` (required
/// when other steps consume this step's outputs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepSummary {
    pub id: Option<String>,
    pub kind: StepKind,
    pub display_name: Option<String>,
    /// For `task` steps: the ADO task identifier (e.g. `"UseNode@1"`).
    pub task: Option<String>,
    /// Lowered ADO condition string, when one is set on the step.
    pub condition: Option<String>,
    /// Step outputs **declared** by this step (`BashStep::outputs`).
    pub outputs: Vec<OutputDeclSummary>,
    /// Other-step outputs **read** by this step's `env:` map.
    pub env_refs: Vec<OutputRefSummary>,
    /// Other-step outputs **read** by this step's `condition:`.
    pub condition_refs: Vec<OutputRefSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Bash,
    Task,
    Checkout,
    Download,
    Publish,
    RawYaml,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputDeclSummary {
    pub name: String,
    pub is_secret: bool,
    /// `true` when at least one cross-step consumer reads this
    /// output; the producer must emit `isOutput=true` in its
    /// `##vso[task.setvariable …]` directive.
    pub auto_is_output: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputRefSummary {
    /// Producer step id.
    pub step: String,
    /// Output variable name (matches an `OutputDecl::name`).
    pub name: String,
}

/// JSON-friendly view of the IR's typed [`Graph`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphSummary {
    /// Every step that carries an id, with its location and declared
    /// outputs.
    pub step_locations: Vec<StepLocationEntry>,
    /// Derived job-level `dependsOn` edges (`consumer → producer`).
    pub job_edges: Vec<EdgeEntry>,
    /// Derived stage-level `dependsOn` edges (`consumer → producer`).
    pub stage_edges: Vec<EdgeEntry>,
    /// Producer-step outputs that need `isOutput=true` because at
    /// least one cross-step consumer reads them.
    pub outputs_needing_is_output: Vec<StepOutputsEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepLocationEntry {
    pub step: String,
    pub stage: Option<String>,
    pub job: String,
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeEntry {
    /// The job/stage that has the `dependsOn` entry.
    pub consumer: String,
    /// The job/stage being depended on.
    pub producer: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepOutputsEntry {
    pub step: String,
    pub outputs: Vec<String>,
}

impl PipelineSummary {
    /// Build a public summary from a typed [`Pipeline`].
    ///
    /// Runs the graph pass to derive `depends_on` and validate
    /// output references — same flow the YAML emit takes. Returns
    /// the graph errors verbatim so summary callers see the same
    /// errors a compile would surface.
    pub fn from_pipeline(p: &Pipeline) -> Result<Self> {
        let graph = build_graph(p)?;
        let body = match &p.body {
            PipelineBody::Jobs(jobs) => PipelineBodySummary::Jobs {
                jobs: jobs
                    .iter()
                    .map(|j| summarize_job(None, j, &graph))
                    .collect(),
            },
            PipelineBody::Stages(stages) => PipelineBodySummary::Stages {
                stages: stages
                    .iter()
                    .map(|s| {
                        let stage_id = s.id.as_str().to_string();
                        StageSummary {
                            id: stage_id.clone(),
                            display_name: s.display_name.clone(),
                            depends_on: s
                                .depends_on
                                .iter()
                                .map(|d| d.as_str().to_string())
                                .collect(),
                            condition: s.condition.as_ref().and_then(|c| {
                                // Conditions on a stage have no
                                // step-output context; render with
                                // an empty graph and a placeholder
                                // job so callers see the lowered
                                // string. Stage-level conditions
                                // today never reference step
                                // outputs.
                                render_condition(c, &graph, None, None)
                            }),
                            jobs: s
                                .jobs
                                .iter()
                                .map(|j| summarize_job(Some(stage_id.clone()), j, &graph))
                                .collect(),
                        }
                    })
                    .collect(),
            },
        };

        Ok(PipelineSummary {
            schema_version: SCHEMA_VERSION,
            name: p.name.clone(),
            shape: shape_label(&p.shape).to_string(),
            body,
            graph: GraphSummary::from_graph(&graph),
        })
    }
}

fn shape_label(shape: &PipelineShape) -> &'static str {
    match shape {
        PipelineShape::Standalone => "standalone",
        PipelineShape::OneEs { .. } => "1es",
        PipelineShape::JobTemplate { .. } => "job-template",
        PipelineShape::StageTemplate { .. } => "stage-template",
    }
}

fn summarize_job(stage: Option<String>, j: &super::job::Job, graph: &Graph) -> JobSummary {
    let job_id_str = j.id.as_str().to_string();
    let stage_clone = stage.clone();
    let stage_for_render = stage_clone.as_deref();
    JobSummary {
        id: job_id_str.clone(),
        stage,
        display_name: j.display_name.clone(),
        depends_on: j
            .depends_on
            .iter()
            .map(|d| d.as_str().to_string())
            .collect(),
        condition: j
            .condition
            .as_ref()
            .and_then(|c| render_condition(c, graph, stage_for_render, Some(&job_id_str))),
        pool: summarize_pool(&j.pool),
        steps: j
            .steps
            .iter()
            .map(|s| summarize_step(s, graph, stage_for_render, &job_id_str))
            .collect(),
    }
}

fn summarize_pool(p: &super::job::Pool) -> PoolSummary {
    match p {
        super::job::Pool::VmImage(image) => PoolSummary::VmImage {
            image: image.clone(),
        },
        super::job::Pool::Named { name, image, os } => PoolSummary::Named {
            name: name.clone(),
            image: image.clone(),
            os: os.clone(),
        },
    }
}

fn summarize_step(step: &Step, graph: &Graph, stage: Option<&str>, job: &str) -> StepSummary {
    let (id, kind, display_name, task, condition, mut outputs, env_refs, condition_refs) =
        match step {
            Step::Bash(b) => {
                let env_refs = collect_env_refs(b.env.values());
                let cond_refs = b
                    .condition
                    .as_ref()
                    .map(collect_condition_refs)
                    .unwrap_or_default();
                (
                    b.id.as_ref().map(|i| i.as_str().to_string()),
                    StepKind::Bash,
                    Some(b.display_name.clone()),
                    None,
                    b.condition
                        .as_ref()
                        .and_then(|c| render_condition(c, graph, stage, Some(job))),
                    b.outputs
                        .iter()
                        .map(summarize_output_decl)
                        .collect::<Vec<_>>(),
                    env_refs.into_iter().map(summarize_output_ref).collect(),
                    cond_refs.into_iter().map(summarize_output_ref).collect(),
                )
            }
            Step::Task(t) => {
                let env_refs = collect_env_refs(t.env.values());
                let cond_refs = t
                    .condition
                    .as_ref()
                    .map(collect_condition_refs)
                    .unwrap_or_default();
                (
                    t.id.as_ref().map(|i| i.as_str().to_string()),
                    StepKind::Task,
                    Some(t.display_name.clone()),
                    Some(t.task.clone()),
                    t.condition
                        .as_ref()
                        .and_then(|c| render_condition(c, graph, stage, Some(job))),
                    Vec::new(),
                    env_refs.into_iter().map(summarize_output_ref).collect(),
                    cond_refs.into_iter().map(summarize_output_ref).collect(),
                )
            }
            Step::Checkout(_) => (
                None,
                StepKind::Checkout,
                None,
                None,
                None,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            Step::Download(d) => {
                let cond_refs = d
                    .condition
                    .as_ref()
                    .map(collect_condition_refs)
                    .unwrap_or_default();
                (
                    None,
                    StepKind::Download,
                    Some(format!("download: {}", d.artifact)),
                    None,
                    d.condition
                        .as_ref()
                        .and_then(|c| render_condition(c, graph, stage, Some(job))),
                    Vec::new(),
                    Vec::new(),
                    cond_refs.into_iter().map(summarize_output_ref).collect(),
                )
            }
            Step::Publish(p) => {
                let cond_refs = p
                    .condition
                    .as_ref()
                    .map(collect_condition_refs)
                    .unwrap_or_default();
                (
                    None,
                    StepKind::Publish,
                    Some(format!("publish: {}", p.artifact)),
                    None,
                    p.condition
                        .as_ref()
                        .and_then(|c| render_condition(c, graph, stage, Some(job))),
                    Vec::new(),
                    Vec::new(),
                    cond_refs.into_iter().map(summarize_output_ref).collect(),
                )
            }
            Step::RawYaml(_) => (
                None,
                StepKind::RawYaml,
                None,
                None,
                None,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
        };
    // Patch auto_is_output from the graph's outputs_needing_is_output
    // index so it's accurate without requiring the caller to mutate
    // the Pipeline via apply_auto_is_output.
    if let Some(step_id) = id.as_deref()
        && !outputs.is_empty()
    {
        let key = super::ids::StepId::new(step_id).ok();
        if let Some(k) = key
            && let Some(needs) = graph.outputs_needing_is_output.get(&k)
        {
            for o in outputs.iter_mut() {
                if needs.contains(&o.name) {
                    o.auto_is_output = true;
                }
            }
        }
    }
    StepSummary {
        id,
        kind,
        display_name,
        task,
        condition,
        outputs,
        env_refs,
        condition_refs,
    }
}

fn summarize_output_decl(d: &OutputDecl) -> OutputDeclSummary {
    OutputDeclSummary {
        name: d.name.clone(),
        is_secret: d.is_secret,
        auto_is_output: d.auto_is_output,
    }
}

fn summarize_output_ref(r: OutputRef) -> OutputRefSummary {
    OutputRefSummary {
        step: r.step.as_str().to_string(),
        name: r.name,
    }
}

fn render_condition(
    c: &Condition,
    graph: &Graph,
    stage: Option<&str>,
    job: Option<&str>,
) -> Option<String> {
    // Build typed stage/job ids for the codegen context. If the
    // caller is rendering a stage-level condition we synthesise a
    // dummy job — stage-level conditions in the canonical pipeline
    // never reference step outputs, but the codegen API still
    // requires a `&JobId`, and a placeholder is fine because no
    // `Expr::StepOutput` should reach this path.
    let job_id = super::ids::JobId::new(job.unwrap_or("_stage_placeholder")).ok()?;
    let stage_id = stage
        .map(super::ids::StageId::new)
        .transpose()
        .ok()
        .flatten();
    let ctx = CondCodegenCtx {
        graph,
        stage: stage_id.as_ref(),
        job: &job_id,
    };
    lower_condition(&ctx, c).ok()
}

fn collect_env_refs<'a, I: IntoIterator<Item = &'a EnvValue>>(values: I) -> Vec<OutputRef> {
    let mut out = Vec::new();
    for v in values {
        walk_env(v, &mut out);
    }
    out
}

fn walk_env(v: &EnvValue, out: &mut Vec<OutputRef>) {
    match v {
        EnvValue::StepOutput(r) => out.push(r.clone()),
        EnvValue::Coalesce(parts) | EnvValue::Concat(parts) => {
            for p in parts {
                walk_env(p, out);
            }
        }
        _ => {}
    }
}

fn collect_condition_refs(c: &Condition) -> Vec<OutputRef> {
    let mut out = Vec::new();
    walk_cond(c, &mut out);
    out
}

fn walk_cond(c: &Condition, out: &mut Vec<OutputRef>) {
    match c {
        Condition::And(parts) | Condition::Or(parts) => {
            for p in parts {
                walk_cond(p, out);
            }
        }
        Condition::Not(inner) => walk_cond(inner, out),
        Condition::Eq(a, b) | Condition::Ne(a, b) => {
            walk_expr(a, out);
            walk_expr(b, out);
        }
        _ => {}
    }
}

fn walk_expr(e: &Expr, out: &mut Vec<OutputRef>) {
    if let Expr::StepOutput(r) = e {
        out.push(r.clone());
    }
}

impl GraphSummary {
    fn from_graph(g: &Graph) -> Self {
        let step_locations = g
            .step_locations
            .iter()
            .map(|(step, loc)| StepLocationEntry {
                step: step.as_str().to_string(),
                stage: loc.stage.as_ref().map(|s| s.as_str().to_string()),
                job: loc.job.as_str().to_string(),
                outputs: loc.outputs.iter().cloned().collect(),
            })
            .collect();
        let job_edges = g
            .job_edges
            .iter()
            .map(|(c, p)| EdgeEntry {
                consumer: c.as_str().to_string(),
                producer: p.as_str().to_string(),
            })
            .collect();
        let stage_edges = g
            .stage_edges
            .iter()
            .map(|(c, p)| EdgeEntry {
                consumer: c.as_str().to_string(),
                producer: p.as_str().to_string(),
            })
            .collect();
        let outputs_needing_is_output = g
            .outputs_needing_is_output
            .iter()
            .map(|(step, outs)| StepOutputsEntry {
                step: step.as_str().to_string(),
                outputs: outs
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            })
            .collect();
        GraphSummary {
            step_locations,
            job_edges,
            stage_edges,
            outputs_needing_is_output,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::condition::{Condition, Expr};
    use crate::compile::ir::env::EnvValue;
    use crate::compile::ir::ids::{JobId, StepId};
    use crate::compile::ir::job::{Job, Pool};
    use crate::compile::ir::output::{OutputDecl, OutputRef};
    use crate::compile::ir::step::{BashStep, Step};
    use crate::compile::ir::{Pipeline, PipelineBody, PipelineShape, Resources, Triggers};

    fn fixture_pipeline() -> Pipeline {
        let producer = Step::Bash(
            BashStep::new("setup", "echo hi")
                .with_id(StepId::new("synthPr").unwrap())
                .with_output(OutputDecl::new("AW_SYNTHETIC_PR_ID")),
        );
        let consumer = Step::Bash(
            BashStep::new("run", "echo bye")
                .with_env(
                    "PR_ID",
                    EnvValue::step_output(OutputRef::new(
                        StepId::new("synthPr").unwrap(),
                        "AW_SYNTHETIC_PR_ID",
                    )),
                )
                .with_condition(Condition::Eq(
                    Expr::StepOutput(OutputRef::new(
                        StepId::new("synthPr").unwrap(),
                        "AW_SYNTHETIC_PR_ID",
                    )),
                    Expr::Literal("42".into()),
                )),
        );

        let setup = {
            let mut j = Job::new(
                JobId::new("Setup").unwrap(),
                "Setup",
                Pool::VmImage("ubuntu-22.04".into()),
            );
            j.steps.push(producer);
            j
        };
        let agent = {
            let mut j = Job::new(
                JobId::new("Agent").unwrap(),
                "Agent",
                Pool::VmImage("ubuntu-22.04".into()),
            );
            j.steps.push(consumer);
            j
        };

        Pipeline {
            name: "T".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(vec![setup, agent]),
            shape: PipelineShape::Standalone,
        }
    }

    #[test]
    fn summary_schema_version_is_pinned() {
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn from_pipeline_round_trips_jobs_and_graph() {
        let p = fixture_pipeline();
        let s = PipelineSummary::from_pipeline(&p).unwrap();
        assert_eq!(s.shape, "standalone");
        let jobs = match s.body {
            PipelineBodySummary::Jobs { jobs } => jobs,
            _ => panic!("expected jobs body"),
        };
        assert_eq!(jobs.len(), 2);
        let agent = jobs.iter().find(|j| j.id == "Agent").unwrap();
        assert_eq!(agent.steps.len(), 1);
        let step = &agent.steps[0];
        assert_eq!(step.env_refs.len(), 1);
        assert_eq!(step.env_refs[0].step, "synthPr");
        assert_eq!(step.env_refs[0].name, "AW_SYNTHETIC_PR_ID");
        assert_eq!(step.condition_refs.len(), 1);
        assert_eq!(step.condition_refs[0].step, "synthPr");
        assert_eq!(step.condition_refs[0].name, "AW_SYNTHETIC_PR_ID");
        // Graph derived a job edge Agent -> Setup
        assert!(
            s.graph
                .job_edges
                .iter()
                .any(|e| e.consumer == "Agent" && e.producer == "Setup"),
            "expected derived edge Agent -> Setup, got {:?}",
            s.graph.job_edges
        );
        // Producer output is marked auto_is_output — verify it's the right output
        let setup = jobs.iter().find(|j| j.id == "Setup").unwrap();
        let prod_step = &setup.steps[0];
        assert_eq!(prod_step.outputs[0].name, "AW_SYNTHETIC_PR_ID");
        assert!(prod_step.outputs[0].auto_is_output);
    }

    #[test]
    fn serialized_json_contains_schema_version_and_shape() {
        let p = fixture_pipeline();
        let s = PipelineSummary::from_pipeline(&p).unwrap();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"schema_version\":1"));
        assert!(json.contains("\"shape\":\"standalone\""));
    }
}
