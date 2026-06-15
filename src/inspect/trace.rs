//! `ado-aw trace`: runtime audit data joined with typed-IR graph facts.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::audit::model::{AuditData, JobData};
use crate::compile::ir::summary::StepLocationEntry;
use crate::inspect::graph_deps::{self, GraphDepsDirection, StepDependency};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceReport {
    pub build_id: u64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub failing_jobs: Vec<TraceJobReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<TraceStepReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceJobReport {
    pub job: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub upstream: Vec<TraceUpstreamJob>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub downstream: Vec<TraceDownstreamJob>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceUpstreamJob {
    pub job: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceDownstreamJob {
    pub job: String,
    pub classification: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceStepReport {
    pub step: String,
    pub location: TraceStepLocation,
    pub status: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub upstream: Vec<TraceUpstreamJob>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub downstream: Vec<TraceDownstreamJob>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub upstream_steps: Vec<StepDependency>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub downstream_steps: Vec<StepDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraceStepLocation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    pub job: String,
}

pub fn build_trace_report(audit: &AuditData, step: Option<&str>) -> TraceReport {
    let failing_jobs = audit
        .jobs
        .iter()
        .filter(|job| job.failed())
        .map(|job| job_report(audit, job))
        .collect();

    let step_report = step.and_then(|step_id| build_step_report(audit, step_id));

    TraceReport {
        build_id: audit.overview.build_id,
        failing_jobs,
        step: step_report,
    }
}

pub fn render_text(
    audit: &AuditData,
    report: &TraceReport,
    requested_step: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("Trace for build {}\n", report.build_id));
    match &audit.pipeline_graph {
        Some(graph) => out.push_str(&format!("IR graph: {}\n", graph.source_path)),
        None => out.push_str("IR graph: unavailable (runtime-only trace)\n"),
    }
    out.push('\n');

    out.push_str("Failing job chain\n");
    if report.failing_jobs.is_empty() {
        out.push_str("  (no failed jobs)\n");
    } else {
        for job in &report.failing_jobs {
            render_job_report(job, &mut out);
        }
    }

    if requested_step.is_some() {
        out.push('\n');
        out.push_str("Step trace\n");
        match &report.step {
            Some(step) => {
                let stage = step
                    .location
                    .stage
                    .as_deref()
                    .map(|stage| format!("{stage}."))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  {} in {}{}: {}\n",
                    step.step, stage, step.location.job, step.status
                ));
                render_upstream(&step.upstream, &mut out);
                render_downstream(&step.downstream, &mut out);
                render_step_dependencies("upstream steps", &step.upstream_steps, &mut out);
                render_step_dependencies("downstream steps", &step.downstream_steps, &mut out);
            }
            None => out.push_str("  (step not found in local IR graph)\n"),
        }
    }

    out
}

fn render_job_report(job: &TraceJobReport, out: &mut String) {
    let stage = job
        .stage
        .as_deref()
        .map(|stage| format!(" [{stage}]"))
        .unwrap_or_default();
    out.push_str(&format!("  {}{}: {}\n", job.job, stage, job.status));
    render_upstream(&job.upstream, out);
    render_downstream(&job.downstream, out);
}

fn render_upstream(upstream: &[TraceUpstreamJob], out: &mut String) {
    if upstream.is_empty() {
        out.push_str("    upstream: (none)\n");
    } else {
        out.push_str(&format!(
            "    upstream: {}\n",
            upstream
                .iter()
                .map(|job| format!("{} ({})", job.job, job.status))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
}

fn render_downstream(downstream: &[TraceDownstreamJob], out: &mut String) {
    if downstream.is_empty() {
        out.push_str("    downstream: (none)\n");
    } else {
        out.push_str(&format!(
            "    downstream: {}\n",
            downstream
                .iter()
                .map(|job| format!("{} ({})", job.job, job.classification))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
}

fn render_step_dependencies(label: &str, steps: &[StepDependency], out: &mut String) {
    if steps.is_empty() {
        return;
    }
    out.push_str(&format!(
        "    {label}: {}\n",
        steps
            .iter()
            .map(|step| {
                let stage = step
                    .stage
                    .as_deref()
                    .map(|stage| format!("{stage}."))
                    .unwrap_or_default();
                match &step.via_output {
                    Some(via) => format!("{}{}.{} via {}", stage, step.job, step.step, via),
                    None => format!("{}{}.{}", stage, step.job, step.step),
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    ));
}

fn build_step_report(audit: &AuditData, step_id: &str) -> Option<TraceStepReport> {
    let pipeline_graph = audit.pipeline_graph.as_ref()?;
    let location = pipeline_graph
        .summary
        .graph
        .step_locations
        .iter()
        .find(|location| location.step == step_id)?;
    let job = runtime_job_for_location(audit, location);
    Some(TraceStepReport {
        step: step_id.to_string(),
        location: TraceStepLocation {
            stage: location.stage.clone(),
            job: location.job.clone(),
        },
        status: job
            .map(JobData::classification)
            .unwrap_or_else(|| String::from("unknown")),
        upstream: job
            .map(|job| upstream_reports(audit, job))
            .unwrap_or_default(),
        downstream: job
            .map(|job| downstream_reports(audit, job))
            .unwrap_or_default(),
        upstream_steps: graph_deps::analyze(
            &pipeline_graph.summary,
            step_id,
            GraphDepsDirection::Upstream,
        )
        .map(|report| report.transitive_steps)
        .unwrap_or_default(),
        downstream_steps: graph_deps::analyze(
            &pipeline_graph.summary,
            step_id,
            GraphDepsDirection::Downstream,
        )
        .map(|report| report.transitive_steps)
        .unwrap_or_default(),
    })
}

fn job_report(audit: &AuditData, job: &JobData) -> TraceJobReport {
    TraceJobReport {
        job: job.name.clone(),
        stage: stage_for_job(audit, job),
        status: job_status(job),
        upstream: upstream_reports(audit, job),
        downstream: downstream_reports(audit, job),
    }
}

fn upstream_reports(audit: &AuditData, job: &JobData) -> Vec<TraceUpstreamJob> {
    collect_related_jobs(audit, job, Direction::Upstream)
        .into_iter()
        .map(|job_id| TraceUpstreamJob {
            status: find_runtime_job(audit, &job_id)
                .map(JobData::classification)
                .unwrap_or_else(|| String::from("unknown")),
            job: job_id,
        })
        .collect()
}

fn downstream_reports(audit: &AuditData, job: &JobData) -> Vec<TraceDownstreamJob> {
    collect_related_jobs(audit, job, Direction::Downstream)
        .into_iter()
        .map(|job_id| TraceDownstreamJob {
            classification: find_runtime_job(audit, &job_id)
                .map(JobData::classification)
                .unwrap_or_else(|| String::from("expected to skip")),
            job: job_id,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Direction {
    Upstream,
    Downstream,
}

fn collect_related_jobs(audit: &AuditData, job: &JobData, direction: Direction) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    collect_related_jobs_inner(audit, job, direction, &mut seen, &mut ordered);
    ordered
}

fn collect_related_jobs_inner(
    audit: &AuditData,
    job: &JobData,
    direction: Direction,
    seen: &mut BTreeSet<String>,
    ordered: &mut Vec<String>,
) {
    let related = match direction {
        Direction::Upstream => &job.upstream_jobs,
        Direction::Downstream => &job.downstream_jobs,
    };

    for job_id in related {
        if !seen.insert(job_id.clone()) {
            continue;
        }
        ordered.push(job_id.clone());
        if let Some(next) = find_runtime_job(audit, job_id) {
            collect_related_jobs_inner(audit, next, direction, seen, ordered);
        }
    }
}

fn runtime_job_for_location<'a>(
    audit: &'a AuditData,
    location: &StepLocationEntry,
) -> Option<&'a JobData> {
    audit.jobs.iter().find(|job| {
        crate::audit::pipeline_graph::timeline_name_matches_job(
            &job.name,
            &location.job,
            location.stage.as_deref(),
        )
    })
}

fn find_runtime_job<'a>(audit: &'a AuditData, ir_job_id: &str) -> Option<&'a JobData> {
    audit.jobs.iter().find(|job| job.matches_ir_id(ir_job_id))
}

fn stage_for_job(audit: &AuditData, runtime_job: &JobData) -> Option<String> {
    let pipeline_graph = audit.pipeline_graph.as_ref()?;
    pipeline_graph
        .summary
        .all_jobs()
        .find(|job| {
            crate::audit::pipeline_graph::timeline_name_matches_job(
                &runtime_job.name,
                &job.id,
                job.stage.as_deref(),
            )
        })
        .and_then(|job| job.stage.clone())
}

fn job_status(job: &JobData) -> String {
    job.classification()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::model::{AuditData, OverviewData};

    #[test]
    fn build_trace_report_shapes_failed_job_chain_without_network() {
        let audit = AuditData {
            overview: OverviewData {
                build_id: 42,
                ..Default::default()
            },
            jobs: vec![
                JobData {
                    name: String::from("Setup"),
                    status: String::from("completed"),
                    result: Some(String::from("succeeded")),
                    ..Default::default()
                },
                JobData {
                    name: String::from("Agent"),
                    status: String::from("completed"),
                    result: Some(String::from("failed")),
                    upstream_jobs: vec![String::from("Setup")],
                    downstream_jobs: vec![String::from("Detection")],
                    ..Default::default()
                },
                JobData {
                    name: String::from("Detection"),
                    status: String::from("completed"),
                    result: Some(String::from("skipped")),
                    downstream_jobs: vec![String::from("SafeOutputs")],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let report = build_trace_report(&audit, None);

        assert_eq!(report.build_id, 42);
        assert_eq!(report.failing_jobs.len(), 1);
        assert_eq!(report.failing_jobs[0].job, "Agent");
        assert_eq!(report.failing_jobs[0].upstream[0].status, "succeeded");
        assert_eq!(
            report.failing_jobs[0].downstream[0].classification,
            "skipped"
        );
        assert_eq!(
            report.failing_jobs[0].downstream[1].classification,
            "expected to skip"
        );
    }
}
