//! `ado-aw trace`: runtime audit data joined with typed-IR graph facts.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::audit::model::{AdoProxyEventSummary, AdoProxyReasonStat, AuditData, JobData};
use crate::compile::ir::summary::StepLocationEntry;
use crate::inspect::graph_deps::{self, GraphDepsDirection, StepDependency};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceReport {
    pub build_id: u64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub failing_jobs: Vec<TraceJobReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ado_proxy: Option<TraceAdoProxySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<TraceStepReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceAdoProxySummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthy_before_teardown: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_before_teardown: Option<String>,
    pub total_requests: u64,
    pub allow_count: u64,
    pub deny_count: u64,
    pub error_count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub top_reasons: Vec<AdoProxyReasonStat>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub recent_problem_events: Vec<AdoProxyEventSummary>,
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
    let ado_proxy = audit.ado_proxy_analysis.as_ref().map(|analysis| {
        let recent_start = analysis.recent_problem_events.len().saturating_sub(5);
        TraceAdoProxySummary {
            healthy_before_teardown: analysis
                .lifecycle
                .as_ref()
                .map(|lifecycle| lifecycle.healthy_before_teardown),
            state_before_teardown: analysis
                .lifecycle
                .as_ref()
                .and_then(|lifecycle| lifecycle.state_before_teardown.clone()),
            total_requests: analysis.total_requests,
            allow_count: analysis.allow_count,
            deny_count: analysis.deny_count,
            error_count: analysis.error_count,
            top_reasons: analysis.reasons.iter().take(5).cloned().collect(),
            recent_problem_events: analysis.recent_problem_events[recent_start..].to_vec(),
        }
    });

    TraceReport {
        build_id: audit.overview.build_id,
        failing_jobs,
        ado_proxy,
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

    if let Some(proxy) = &report.ado_proxy {
        out.push('\n');
        out.push_str("ADO proxy diagnostics\n");
        if let Some(healthy) = proxy.healthy_before_teardown {
            out.push_str(&format!(
                "  lifecycle: {}",
                if healthy { "healthy" } else { "unhealthy" }
            ));
            if let Some(state) = proxy.state_before_teardown.as_deref() {
                out.push_str(&format!(" (state before teardown: {state})"));
            }
            out.push('\n');
        }
        out.push_str(&format!(
            "  requests: {} total, {} allowed, {} denied, {} errors\n",
            proxy.total_requests, proxy.allow_count, proxy.deny_count, proxy.error_count
        ));
        if !proxy.top_reasons.is_empty() {
            out.push_str(&format!(
                "  top reasons: {}\n",
                proxy
                    .top_reasons
                    .iter()
                    .map(|reason| {
                        format!("{}/{} ({})", reason.decision, reason.reason, reason.count)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for event in &proxy.recent_problem_events {
            out.push_str(&format!(
                "  - {} {} {} {} [{}]{}\n",
                event.timestamp.as_deref().unwrap_or("(unknown time)"),
                event.method.as_deref().unwrap_or("(unknown method)"),
                event.host.as_deref().unwrap_or("(unknown host)"),
                event.operation.as_deref().unwrap_or("(unmatched)"),
                event.reason.as_deref().unwrap_or(&event.decision),
                event
                    .detail
                    .as_deref()
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default()
            ));
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
    use crate::audit::model::{
        AdoProxyAnalysis, AdoProxyEventSummary, AdoProxyLifecycle, AdoProxyReasonStat, AuditData,
        OverviewData,
    };

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

    #[test]
    fn trace_projects_bounded_run_level_proxy_diagnostics() {
        let reasons = (0..7)
            .map(|index| AdoProxyReasonStat {
                reason: format!("reason-{index}"),
                decision: String::from("deny"),
                count: 7 - index,
            })
            .collect();
        let events = (0..7)
            .map(|index| AdoProxyEventSummary {
                request_id: Some(index.to_string()),
                method: Some(String::from("GET")),
                operation: Some(String::from("core.project.get")),
                decision: String::from("deny"),
                reason: Some(String::from("out-of-scope")),
                ..Default::default()
            })
            .collect();
        let audit = AuditData {
            overview: OverviewData {
                build_id: 42,
                ..Default::default()
            },
            ado_proxy_analysis: Some(AdoProxyAnalysis {
                lifecycle: Some(AdoProxyLifecycle {
                    state_before_teardown: Some(String::from("running")),
                    healthy_before_teardown: true,
                    ..Default::default()
                }),
                total_requests: 10,
                allow_count: 3,
                deny_count: 7,
                reasons,
                recent_problem_events: events,
                ..Default::default()
            }),
            ..Default::default()
        };

        let report = build_trace_report(&audit, None);
        let proxy = report.ado_proxy.as_ref().expect("proxy trace summary");
        assert_eq!(proxy.top_reasons.len(), 5);
        assert_eq!(proxy.recent_problem_events.len(), 5);
        assert_eq!(
            proxy.recent_problem_events[0].request_id.as_deref(),
            Some("2")
        );

        let rendered = render_text(&audit, &report, None);
        assert!(rendered.contains("ADO proxy diagnostics"));
        assert!(rendered.contains("10 total, 3 allowed, 7 denied, 0 errors"));
    }

    #[test]
    fn trace_omits_proxy_section_when_analysis_is_absent() {
        let audit = AuditData {
            overview: OverviewData {
                build_id: 42,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = build_trace_report(&audit, None);

        assert!(report.ado_proxy.is_none());
        assert!(!render_text(&audit, &report, None).contains("ADO proxy diagnostics"));
    }
}
