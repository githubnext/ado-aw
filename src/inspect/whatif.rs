//! Static failure reachability for `ado-aw whatif`.
//!
//! The command does not execute a pipeline. It uses the public
//! [`PipelineSummary`] graph and the already-rendered ADO condition
//! strings to classify downstream jobs that would be skipped after a
//! chosen job or step fails.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use anyhow::{Result, anyhow};
use serde::Serialize;

use crate::compile::ir::summary::{EdgeEntry, JobSummary, PipelineBodySummary, PipelineSummary};

/// JSON report emitted by `ado-aw whatif --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WhatIfReport {
    /// Failing step or job supplied by the user.
    pub failing_node: FailingNode,
    /// Downstream jobs classified by whether their rendered condition bypasses failure.
    pub downstream_jobs: Vec<DownstreamJob>,
}

/// The failing node resolved from `--fail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailingNode {
    /// Node kind: `step` or `job`.
    pub kind: String,
    /// User-supplied id that matched the node.
    pub id: String,
    /// Owning job id.
    pub job: String,
    /// Containing stage id for staged pipelines.
    pub stage: Option<String>,
}

/// Classification for a downstream job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WhatIfClassification {
    /// The job requires success of its dependency chain and would be skipped.
    Skipped,
    /// The job condition explicitly permits running after failure.
    RunsAnyway,
}

impl WhatIfClassification {
    fn label(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::RunsAnyway => "runs_anyway",
        }
    }
}

/// A downstream job and the reason-bearing condition string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownstreamJob {
    /// Job id.
    pub job: String,
    /// Containing stage id for staged pipelines.
    pub stage: Option<String>,
    /// Static classification.
    pub classification: WhatIfClassification,
    /// Lowered ADO condition string, when one was explicitly set.
    pub condition: Option<String>,
}

/// Typed errors for `whatif` queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhatIfError {
    /// The supplied id was neither a known step id nor a known job id.
    UnknownFailId {
        /// Missing id.
        id: String,
        /// Closest known step/job id, if one was available.
        suggestion: Option<String>,
    },
}

impl fmt::Display for WhatIfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFailId { id, suggestion } => {
                write!(f, "whatif: unknown step or job '{id}'")?;
                if let Some(s) = suggestion {
                    write!(f, " (closest match: '{s}')")?;
                }
                Ok(())
            }
        }
    }
}

impl Error for WhatIfError {}

/// Analyze which downstream jobs would skip if `fail_id` failed.
pub fn analyze(summary: &PipelineSummary, fail_id: &str) -> Result<WhatIfReport> {
    let failing_node = resolve_failing_node(summary, fail_id)?;
    let mut downstream = reachable_downstream_jobs(summary, &failing_node);
    downstream.sort_by(|a, b| {
        (a.stage.as_deref(), a.job.as_str()).cmp(&(b.stage.as_deref(), b.job.as_str()))
    });

    Ok(WhatIfReport {
        failing_node,
        downstream_jobs: downstream,
    })
}

/// Render a what-if report as terminal-friendly text.
pub fn render_text(report: &WhatIfReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "What if {} '{}' fails?\n",
        report.failing_node.kind, report.failing_node.id
    ));
    out.push_str(&format!(
        "Failing job: {}\n\n",
        qualified_job(&report.failing_node.stage, &report.failing_node.job)
    ));

    render_group(
        &mut out,
        "Skipped",
        report
            .downstream_jobs
            .iter()
            .filter(|job| job.classification == WhatIfClassification::Skipped),
    );
    out.push('\n');
    render_group(
        &mut out,
        "Runs anyway",
        report
            .downstream_jobs
            .iter()
            .filter(|job| job.classification == WhatIfClassification::RunsAnyway),
    );
    out
}

fn render_group<'a>(out: &mut String, title: &str, jobs: impl Iterator<Item = &'a DownstreamJob>) {
    out.push_str(title);
    out.push('\n');
    let mut any = false;
    for job in jobs {
        any = true;
        let condition = job
            .condition
            .as_deref()
            .map(|c| format!(" condition: {c}"))
            .unwrap_or_else(|| " condition: <default succeeded()>".to_string());
        out.push_str(&format!(
            "  - {} ({}){}\n",
            qualified_job(&job.stage, &job.job),
            job.classification.label(),
            condition
        ));
    }
    if !any {
        out.push_str("  (none)\n");
    }
}

fn resolve_failing_node(summary: &PipelineSummary, fail_id: &str) -> Result<FailingNode> {
    if let Some(loc) = summary
        .graph
        .step_locations
        .iter()
        .find(|loc| loc.step == fail_id)
    {
        return Ok(FailingNode {
            kind: "step".to_string(),
            id: fail_id.to_string(),
            job: loc.job.clone(),
            stage: loc.stage.clone(),
        });
    }

    if let Some(job) = find_job(summary, fail_id) {
        return Ok(FailingNode {
            kind: "job".to_string(),
            id: fail_id.to_string(),
            job: job.id.clone(),
            stage: job.stage.clone(),
        });
    }

    Err(anyhow!(WhatIfError::UnknownFailId {
        id: fail_id.to_string(),
        suggestion: closest(fail_id, known_ids(summary).iter().map(String::as_str)),
    }))
}

fn reachable_downstream_jobs(
    summary: &PipelineSummary,
    failing_node: &FailingNode,
) -> Vec<DownstreamJob> {
    let mut keys: BTreeSet<(Option<String>, String)> = BTreeSet::new();

    for job in reachable_edges(&summary.graph.job_edges, &failing_node.job) {
        keys.insert((stage_for_job(summary, &job), job));
    }

    if let Some(stage) = &failing_node.stage {
        for downstream_stage in reachable_edges(&summary.graph.stage_edges, stage) {
            for job in jobs_in_stage(summary, &downstream_stage) {
                keys.insert((Some(downstream_stage.clone()), job));
            }
        }
    }

    keys.into_iter()
        .filter_map(|(stage, job_id)| {
            find_job(summary, &job_id).map(|job| DownstreamJob {
                job: job.id.clone(),
                stage: stage.or_else(|| job.stage.clone()),
                classification: classify_condition(&job.condition),
                condition: job.condition.clone(),
            })
        })
        .collect()
}

fn classify_condition(condition: &Option<String>) -> WhatIfClassification {
    let Some(condition) = condition else {
        return WhatIfClassification::Skipped;
    };
    let normalized = condition.to_ascii_lowercase().replace(' ', "");
    if normalized.contains("always()")
        || normalized.contains("failed()")
        || normalized.contains("succeededorfailed()")
    {
        WhatIfClassification::RunsAnyway
    } else {
        WhatIfClassification::Skipped
    }
}

fn reachable_edges(edges: &[EdgeEntry], start: &str) -> BTreeSet<String> {
    let mut reverse: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in edges {
        reverse
            .entry(edge.producer.clone())
            .or_default()
            .insert(edge.consumer.clone());
    }

    let mut seen = BTreeSet::new();
    let mut queue: VecDeque<String> = reverse
        .get(start)
        .into_iter()
        .flat_map(|next| next.iter().cloned())
        .collect();
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(next) = reverse.get(&node) {
            queue.extend(next.iter().cloned());
        }
    }
    seen
}

fn known_ids(summary: &PipelineSummary) -> Vec<String> {
    let mut ids: Vec<String> = summary
        .graph
        .step_locations
        .iter()
        .map(|loc| loc.step.clone())
        .collect();
    ids.extend(all_jobs(summary).into_iter().map(|job| job.id.clone()));
    ids
}

fn all_jobs(summary: &PipelineSummary) -> Vec<&JobSummary> {
    match &summary.body {
        PipelineBodySummary::Jobs { jobs } => jobs.iter().collect(),
        PipelineBodySummary::Stages { stages } => {
            stages.iter().flat_map(|stage| stage.jobs.iter()).collect()
        }
    }
}

fn find_job<'a>(summary: &'a PipelineSummary, job_id: &str) -> Option<&'a JobSummary> {
    all_jobs(summary).into_iter().find(|job| job.id == job_id)
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
        GraphSummary, PipelineBodySummary, PoolSummary, StepKind, StepLocationEntry, StepSummary,
    };

    fn fixture(always_job: Option<&str>) -> PipelineSummary {
        let jobs = ["Setup", "Agent", "Detection", "SafeOutputs"]
            .into_iter()
            .map(|id| JobSummary {
                id: id.to_string(),
                stage: None,
                display_name: id.to_string(),
                depends_on: Vec::new(),
                condition: if Some(id) == always_job {
                    Some("always()".to_string())
                } else {
                    None
                },
                pool: PoolSummary::VmImage {
                    image: "ubuntu-latest".to_string(),
                },
                steps: if id == "Setup" {
                    vec![StepSummary {
                        id: Some("SetupStep".to_string()),
                        kind: StepKind::Bash,
                        display_name: Some("SetupStep".to_string()),
                        task: None,
                        condition: None,
                        outputs: Vec::new(),
                        env_refs: Vec::new(),
                        condition_refs: Vec::new(),
                    }]
                } else {
                    Vec::new()
                },
            })
            .collect::<Vec<_>>();
        PipelineSummary {
            schema_version: 1,
            name: "test".to_string(),
            shape: "standalone".to_string(),
            body: PipelineBodySummary::Jobs { jobs },
            graph: GraphSummary {
                step_locations: vec![StepLocationEntry {
                    step: "SetupStep".to_string(),
                    stage: None,
                    job: "Setup".to_string(),
                    outputs: Vec::new(),
                }],
                job_edges: vec![
                    EdgeEntry {
                        consumer: "Agent".to_string(),
                        producer: "Setup".to_string(),
                    },
                    EdgeEntry {
                        consumer: "Detection".to_string(),
                        producer: "Agent".to_string(),
                    },
                    EdgeEntry {
                        consumer: "SafeOutputs".to_string(),
                        producer: "Detection".to_string(),
                    },
                ],
                stage_edges: Vec::new(),
                outputs_needing_is_output: Vec::new(),
            },
        }
    }

    #[test]
    fn fail_setup_marks_canonical_downstream_jobs_skipped() {
        let report = analyze(&fixture(None), "Setup").unwrap();

        assert_eq!(
            report
                .downstream_jobs
                .iter()
                .map(|job| (job.job.as_str(), job.classification))
                .collect::<Vec<_>>(),
            vec![
                ("Agent", WhatIfClassification::Skipped),
                ("Detection", WhatIfClassification::Skipped),
                ("SafeOutputs", WhatIfClassification::Skipped),
            ]
        );
    }

    #[test]
    fn always_condition_runs_anyway() {
        let report = analyze(&fixture(Some("Detection")), "Setup").unwrap();

        let detection = report
            .downstream_jobs
            .iter()
            .find(|job| job.job == "Detection")
            .unwrap();
        assert_eq!(detection.classification, WhatIfClassification::RunsAnyway);
    }

    #[test]
    fn unknown_fail_id_returns_typed_error() {
        let err = analyze(&fixture(None), "unknown-id").unwrap_err();

        assert!(err.downcast_ref::<WhatIfError>().is_some());
    }

    #[test]
    fn failing_step_in_setup_matches_failing_setup_job() {
        let job_report = analyze(&fixture(None), "Setup").unwrap();
        let step_report = analyze(&fixture(None), "SetupStep").unwrap();

        assert_eq!(job_report.downstream_jobs, step_report.downstream_jobs);
    }
}
