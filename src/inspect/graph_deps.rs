//! Step-centric dependency traversal for `ado-aw graph deps`.
//!
//! The compiler's public [`PipelineSummary`] already contains the
//! resolved job/stage dependency graph plus per-step output references.
//! This module answers one focused question over that stable summary:
//! what sits upstream or downstream of a single named step?

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use anyhow::{Result, anyhow};
use serde::Serialize;

use crate::compile::ir::summary::{
    EdgeEntry, JobSummary, PipelineBodySummary, PipelineSummary, StepLocationEntry, StepSummary,
};

/// Traversal direction for `ado-aw graph deps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "lower")]
pub enum GraphDepsDirection {
    /// Walk producer-side dependencies.
    Upstream,
    /// Walk consumer-side dependents.
    Downstream,
}

impl GraphDepsDirection {
    /// Stable JSON/text label for the direction.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upstream => "upstream",
            Self::Downstream => "downstream",
        }
    }
}

/// A transitive job reached by the query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobDependency {
    /// Job id.
    pub job: String,
    /// Containing stage id for staged pipelines.
    pub stage: Option<String>,
}

/// A transitive step reached by following output references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepDependency {
    /// Step id, or a stable anonymous label for steps without `id`.
    pub step: String,
    /// Containing job id.
    pub job: String,
    /// Containing stage id for staged pipelines.
    pub stage: Option<String>,
    /// Output edge that caused the step to be reached, when known.
    pub via_output: Option<String>,
}

/// JSON report emitted by `ado-aw graph deps --json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphDepsReport {
    /// Traversal direction: `upstream` or `downstream`.
    pub direction: String,
    /// Input step id.
    pub step: String,
    /// Location of the input step in the pipeline.
    pub step_location: StepLocationEntry,
    /// Transitive jobs reached through job/stage dependencies.
    pub transitive_jobs: Vec<JobDependency>,
    /// Transitive steps reached through output references.
    pub transitive_steps: Vec<StepDependency>,
}

/// Typed errors for graph dependency queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphDepsError {
    /// The requested step id is not present in `summary.graph.step_locations`.
    StepNotFound {
        /// Missing step id.
        step: String,
        /// Closest known step id, if one was available.
        suggestion: Option<String>,
    },
}

impl fmt::Display for GraphDepsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StepNotFound { step, suggestion } => {
                write!(f, "graph deps: step '{step}' not found")?;
                if let Some(s) = suggestion {
                    write!(f, " (closest match: '{s}')")?;
                }
                Ok(())
            }
        }
    }
}

impl Error for GraphDepsError {}

/// Analyze transitive dependencies for a single named step.
///
/// If `step` does not match a step id but does match a job id, the
/// query falls back to job-level traversal. That keeps the command
/// useful for canonical jobs such as `SafeOutputs` that may contain no
/// named step with the same id.
pub fn analyze(
    summary: &PipelineSummary,
    step: &str,
    direction: GraphDepsDirection,
) -> Result<GraphDepsReport> {
    let step_loc = summary
        .graph
        .step_locations
        .iter()
        .find(|loc| loc.step == step)
        .cloned();
    let job_loc = step_loc
        .is_none()
        .then(|| find_job(summary, step))
        .flatten();
    let loc = if let Some(loc) = step_loc {
        loc
    } else if let Some(job) = job_loc {
        StepLocationEntry {
            step: step.to_string(),
            stage: job.stage.clone(),
            job: job.id.clone(),
            outputs: Vec::new(),
        }
    } else {
        return Err(anyhow!(GraphDepsError::StepNotFound {
            step: step.to_string(),
            suggestion: closest(
                step,
                known_step_or_job_ids(summary).iter().map(String::as_str)
            ),
        }));
    };

    let transitive_jobs = transitive_jobs(summary, &loc, direction);
    let transitive_steps = if job_loc.is_some() {
        transitive_steps_for_job(summary, &loc.job, direction)
    } else {
        transitive_steps(summary, step, direction)
    };

    Ok(GraphDepsReport {
        direction: direction.as_str().to_string(),
        step: step.to_string(),
        step_location: loc,
        transitive_jobs,
        transitive_steps,
    })
}

/// Render a dependency report as terminal-friendly text.
pub fn render_text(report: &GraphDepsReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Graph dependencies for step '{}' ({})\n",
        report.step, report.direction
    ));
    out.push_str("Step location\n");
    out.push_str(&format!(
        "  {}\n",
        qualified(
            &report.step_location.stage,
            &report.step_location.job,
            &report.step_location.step
        )
    ));
    out.push('\n');

    out.push_str("Job-level edges\n");
    if report.transitive_jobs.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for job in &report.transitive_jobs {
            out.push_str(&format!("  - {}\n", qualified_job(&job.stage, &job.job)));
        }
    }
    out.push('\n');

    out.push_str("Step-level output edges\n");
    if report.transitive_steps.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for step in &report.transitive_steps {
            let via = step
                .via_output
                .as_deref()
                .map(|v| format!(" via {v}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "  - {}{}\n",
                qualified(&step.stage, &step.job, &step.step),
                via
            ));
        }
    }
    out
}

fn transitive_jobs(
    summary: &PipelineSummary,
    loc: &StepLocationEntry,
    direction: GraphDepsDirection,
) -> Vec<JobDependency> {
    let mut seen: BTreeSet<(Option<String>, String)> = BTreeSet::new();

    for job in reachable_edges(&summary.graph.job_edges, &loc.job, direction) {
        seen.insert((stage_for_job(summary, &job), job));
    }

    if let Some(stage) = &loc.stage {
        for reached_stage in reachable_edges(&summary.graph.stage_edges, stage, direction) {
            for job in jobs_in_stage(summary, &reached_stage) {
                seen.insert((Some(reached_stage.clone()), job));
            }
        }
    }

    seen.into_iter()
        .map(|(stage, job)| JobDependency { job, stage })
        .collect()
}

fn reachable_edges(
    edges: &[EdgeEntry],
    start: &str,
    direction: GraphDepsDirection,
) -> BTreeSet<String> {
    let mut adjacency: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in edges {
        match direction {
            GraphDepsDirection::Upstream => {
                adjacency
                    .entry(e.consumer.clone())
                    .or_default()
                    .insert(e.producer.clone());
            }
            GraphDepsDirection::Downstream => {
                adjacency
                    .entry(e.producer.clone())
                    .or_default()
                    .insert(e.consumer.clone());
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut queue: VecDeque<String> = adjacency
        .get(start)
        .into_iter()
        .flat_map(|next| next.iter().cloned())
        .collect();
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(next) = adjacency.get(&node) {
            queue.extend(next.iter().cloned());
        }
    }
    seen
}

fn transitive_steps(
    summary: &PipelineSummary,
    step: &str,
    direction: GraphDepsDirection,
) -> Vec<StepDependency> {
    let nodes = step_nodes(summary);
    let node_by_step: BTreeMap<String, StepNode> = nodes
        .iter()
        .map(|node| (node.step.clone(), node.clone()))
        .collect();

    match direction {
        GraphDepsDirection::Upstream => upstream_steps(step, &node_by_step),
        GraphDepsDirection::Downstream => downstream_steps(step, &nodes),
    }
}

fn transitive_steps_for_job(
    summary: &PipelineSummary,
    job: &str,
    direction: GraphDepsDirection,
) -> Vec<StepDependency> {
    let nodes = step_nodes(summary);
    let node_by_step: BTreeMap<String, StepNode> = nodes
        .iter()
        .map(|node| (node.step.clone(), node.clone()))
        .collect();

    match direction {
        GraphDepsDirection::Upstream => {
            let refs = nodes
                .iter()
                .filter(|node| node.job == job)
                .flat_map(|node| node.refs.iter().cloned())
                .collect();
            upstream_from_refs(refs, &node_by_step)
        }
        GraphDepsDirection::Downstream => {
            let start_steps: Vec<String> = summary
                .graph
                .step_locations
                .iter()
                .filter(|loc| loc.job == job)
                .map(|loc| loc.step.clone())
                .collect();
            let mut seen = BTreeSet::new();
            let mut out = Vec::new();
            for start_step in start_steps {
                for dep in downstream_steps(&start_step, &nodes) {
                    if seen.insert(dep.step.clone()) {
                        out.push(dep);
                    }
                }
            }
            out
        }
    }
}

fn upstream_steps(step: &str, node_by_step: &BTreeMap<String, StepNode>) -> Vec<StepDependency> {
    let Some(node) = node_by_step.get(step) else {
        return Vec::new();
    };
    upstream_from_refs(node.refs.clone(), node_by_step)
}

fn upstream_from_refs(
    refs: Vec<StepReference>,
    node_by_step: &BTreeMap<String, StepNode>,
) -> Vec<StepDependency> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut queue: VecDeque<StepReference> = refs.into();

    while let Some(reference) = queue.pop_front() {
        let producer = reference.producer_step.clone();
        if !seen.insert(producer.clone()) {
            continue;
        }
        if let Some(producer_node) = node_by_step.get(&producer) {
            out.push(StepDependency {
                step: producer.clone(),
                job: producer_node.job.clone(),
                stage: producer_node.stage.clone(),
                via_output: Some(format!("{}.{}", producer, reference.output_name)),
            });
            queue.extend(producer_node.refs.iter().cloned());
        }
    }
    out
}

fn downstream_steps(step: &str, nodes: &[StepNode]) -> Vec<StepDependency> {
    let mut reverse: BTreeMap<String, Vec<(StepNode, String)>> = BTreeMap::new();
    for node in nodes {
        for reference in &node.refs {
            reverse
                .entry(reference.producer_step.clone())
                .or_default()
                .push((node.clone(), reference.output_name.clone()));
        }
    }

    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut queue = VecDeque::from([step.to_string()]);
    while let Some(producer) = queue.pop_front() {
        let Some(consumers) = reverse.get(&producer) else {
            continue;
        };
        for (consumer, output_name) in consumers {
            if !seen.insert(consumer.step.clone()) {
                continue;
            }
            out.push(StepDependency {
                step: consumer.step.clone(),
                job: consumer.job.clone(),
                stage: consumer.stage.clone(),
                via_output: Some(format!("{}.{}", producer, output_name)),
            });
            queue.push_back(consumer.step.clone());
        }
    }
    out
}

#[derive(Debug, Clone)]
struct StepNode {
    step: String,
    job: String,
    stage: Option<String>,
    refs: Vec<StepReference>,
}

#[derive(Debug, Clone)]
struct StepReference {
    producer_step: String,
    output_name: String,
}

fn step_nodes(summary: &PipelineSummary) -> Vec<StepNode> {
    let mut nodes = Vec::new();
    match &summary.body {
        PipelineBodySummary::Jobs { jobs } => {
            for job in jobs {
                push_job_step_nodes(&mut nodes, job);
            }
        }
        PipelineBodySummary::Stages { stages } => {
            for stage in stages {
                for job in &stage.jobs {
                    push_job_step_nodes(&mut nodes, job);
                }
            }
        }
    }
    nodes
}

fn push_job_step_nodes(nodes: &mut Vec<StepNode>, job: &JobSummary) {
    for (idx, step) in job.steps.iter().enumerate() {
        let step_label = step_label(step, job, idx);
        nodes.push(StepNode {
            step: step_label,
            job: job.id.clone(),
            stage: job.stage.clone(),
            refs: step_refs(step),
        });
    }
}

fn step_refs(step: &StepSummary) -> Vec<StepReference> {
    step.env_refs
        .iter()
        .chain(step.condition_refs.iter())
        .map(|r| StepReference {
            producer_step: r.step.clone(),
            output_name: r.name.clone(),
        })
        .collect()
}

fn step_label(step: &StepSummary, job: &JobSummary, idx: usize) -> String {
    step.id
        .clone()
        .unwrap_or_else(|| format!("{}#{}", job.id, idx + 1))
}

fn stage_for_job(summary: &PipelineSummary, job_id: &str) -> Option<String> {
    find_job(summary, job_id).and_then(|job| job.stage.clone())
}

fn jobs_in_stage(summary: &PipelineSummary, stage_id: &str) -> Vec<String> {
    match &summary.body {
        PipelineBodySummary::Jobs { .. } => Vec::new(),
        PipelineBodySummary::Stages { stages } => stages
            .iter()
            .find(|stage| stage.id == stage_id)
            .map(|stage| stage.jobs.iter().map(|job| job.id.clone()).collect())
            .unwrap_or_default(),
    }
}

fn find_job<'a>(summary: &'a PipelineSummary, job_id: &str) -> Option<&'a JobSummary> {
    match &summary.body {
        PipelineBodySummary::Jobs { jobs } => jobs.iter().find(|job| job.id == job_id),
        PipelineBodySummary::Stages { stages } => stages
            .iter()
            .flat_map(|stage| stage.jobs.iter())
            .find(|job| job.id == job_id),
    }
}

fn known_step_or_job_ids(summary: &PipelineSummary) -> Vec<String> {
    let mut ids: Vec<String> = summary
        .graph
        .step_locations
        .iter()
        .map(|loc| loc.step.clone())
        .collect();
    match &summary.body {
        PipelineBodySummary::Jobs { jobs } => ids.extend(jobs.iter().map(|job| job.id.clone())),
        PipelineBodySummary::Stages { stages } => ids.extend(
            stages
                .iter()
                .flat_map(|stage| stage.jobs.iter().map(|job| job.id.clone())),
        ),
    }
    ids
}

fn qualified(stage: &Option<String>, job: &str, step: &str) -> String {
    match stage {
        Some(stage) => format!("{stage}.{job}.{step}"),
        None => format!("{job}.{step}"),
    }
}

fn qualified_job(stage: &Option<String>, job: &str) -> String {
    match stage {
        Some(stage) => format!("{stage}.{job}"),
        None => job.to_string(),
    }
}

fn closest<'a>(needle: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    candidates
        .map(|candidate| (levenshtein(needle, candidate), candidate))
        .min_by_key(|(distance, candidate)| (*distance, (*candidate).to_string()))
        .map(|(_, candidate)| candidate.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let mut prev: Vec<usize> = (0..=b.chars().count()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut curr = vec![i + 1];
        for (j, cb) in b.chars().enumerate() {
            let cost = usize::from(ca != cb);
            curr.push((curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost));
        }
        prev = curr;
    }
    prev[b.chars().count()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::summary::{
        GraphSummary, OutputDeclSummary, OutputRefSummary, PipelineBodySummary, PoolSummary,
        StepKind,
    };

    fn summary(jobs: Vec<JobSummary>, edges: Vec<(&str, &str)>) -> PipelineSummary {
        let step_locations = jobs
            .iter()
            .flat_map(|job| {
                job.steps.iter().filter_map(|step| {
                    step.id.as_ref().map(|id| StepLocationEntry {
                        step: id.clone(),
                        stage: job.stage.clone(),
                        job: job.id.clone(),
                        outputs: step.outputs.iter().map(|o| o.name.clone()).collect(),
                    })
                })
            })
            .collect();
        PipelineSummary {
            schema_version: 1,
            name: "test".to_string(),
            shape: "standalone".to_string(),
            body: PipelineBodySummary::Jobs { jobs },
            graph: GraphSummary {
                step_locations,
                job_edges: edges
                    .into_iter()
                    .map(|(consumer, producer)| EdgeEntry {
                        consumer: consumer.to_string(),
                        producer: producer.to_string(),
                    })
                    .collect(),
                stage_edges: Vec::new(),
                outputs_needing_is_output: Vec::new(),
            },
        }
    }

    fn job(id: &str, steps: Vec<StepSummary>) -> JobSummary {
        JobSummary {
            id: id.to_string(),
            stage: None,
            display_name: id.to_string(),
            depends_on: Vec::new(),
            condition: None,
            pool: PoolSummary::VmImage {
                image: "ubuntu-latest".to_string(),
            },
            steps,
        }
    }

    fn step(id: &str, outputs: &[&str], refs: &[(&str, &str)]) -> StepSummary {
        StepSummary {
            id: Some(id.to_string()),
            kind: StepKind::Bash,
            display_name: Some(id.to_string()),
            task: None,
            condition: None,
            outputs: outputs
                .iter()
                .map(|name| OutputDeclSummary {
                    name: (*name).to_string(),
                    is_secret: false,
                    auto_is_output: false,
                })
                .collect(),
            env_refs: refs
                .iter()
                .map(|(producer, name)| OutputRefSummary {
                    step: (*producer).to_string(),
                    name: (*name).to_string(),
                })
                .collect(),
            condition_refs: Vec::new(),
        }
    }

    #[test]
    fn no_upstream_or_downstream_returns_empty_lists() {
        let s = summary(vec![job("Solo", vec![step("A", &[], &[])])], vec![]);

        let upstream = analyze(&s, "A", GraphDepsDirection::Upstream).unwrap();
        let downstream = analyze(&s, "A", GraphDepsDirection::Downstream).unwrap();

        assert!(upstream.transitive_jobs.is_empty());
        assert!(upstream.transitive_steps.is_empty());
        assert!(downstream.transitive_jobs.is_empty());
        assert!(downstream.transitive_steps.is_empty());
    }

    #[test]
    fn transitive_walk_crosses_multiple_hops() {
        let s = summary(
            vec![
                job("Setup", vec![step("A", &["one"], &[])]),
                job("Build", vec![step("B", &["two"], &[("A", "one")])]),
                job("Test", vec![step("C", &[], &[("B", "two")])]),
            ],
            vec![("Build", "Setup"), ("Test", "Build")],
        );

        let report = analyze(&s, "C", GraphDepsDirection::Upstream).unwrap();

        assert_eq!(
            report
                .transitive_jobs
                .iter()
                .map(|j| j.job.as_str())
                .collect::<Vec<_>>(),
            vec!["Build", "Setup"]
        );
        assert_eq!(
            report
                .transitive_steps
                .iter()
                .map(|s| s.step.as_str())
                .collect::<Vec<_>>(),
            vec!["B", "A"]
        );
    }

    #[test]
    fn step_not_found_returns_typed_error() {
        let s = summary(vec![job("Solo", vec![step("A", &[], &[])])], vec![]);

        let err = analyze(&s, "Missing", GraphDepsDirection::Upstream).unwrap_err();
        assert!(err.downcast_ref::<GraphDepsError>().is_some());
    }

    #[test]
    fn bidirectional_symmetry_for_step_edges() {
        let s = summary(
            vec![
                job("Setup", vec![step("A", &["one"], &[])]),
                job("Build", vec![step("B", &[], &[("A", "one")])]),
            ],
            vec![("Build", "Setup")],
        );

        let b_upstream = analyze(&s, "B", GraphDepsDirection::Upstream).unwrap();
        let a_downstream = analyze(&s, "A", GraphDepsDirection::Downstream).unwrap();

        assert!(b_upstream.transitive_steps.iter().any(|s| s.step == "A"));
        assert!(a_downstream.transitive_steps.iter().any(|s| s.step == "B"));
    }
}
