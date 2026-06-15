//! Structural lint rules over [`PipelineSummary`].
//!
//! `build_pipeline_ir()` and [`PipelineSummary::from_pipeline`] already run the
//! compile-time IR graph validation pass. These lint rules are intentionally
//! lighter-weight, user-facing quality checks; a few are defensive guards for
//! callers that might construct summaries without the normal graph pass.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::compile::ir::summary::{JobSummary, PipelineSummary, StepSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LintSeverity {
    Error,
    Warning,
    Info,
}

impl LintSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintLocation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintFinding {
    pub severity: LintSeverity,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<LintLocation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintSummary {
    pub errors: u32,
    pub warnings: u32,
    pub infos: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintReport {
    pub findings: Vec<LintFinding>,
    pub summary: LintSummary,
}

/// Run every lint rule over a public pipeline summary.
pub fn lint(summary: &PipelineSummary) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    rule_unused_output(summary, &mut findings);
    rule_missing_is_output(summary, &mut findings);
    rule_anonymous_producer(summary, &mut findings);
    rule_step_id_collisions(summary, &mut findings);
    findings
}

pub fn report(summary: &PipelineSummary) -> LintReport {
    let findings = lint(summary);
    // Rename the local to avoid shadowing the `PipelineSummary`
    // parameter with a `LintSummary` of the same name in the same
    // scope; the struct field is still called `summary` below.
    let tally = summarize_findings(&findings);
    LintReport {
        findings,
        summary: tally,
    }
}

pub fn summarize_findings(findings: &[LintFinding]) -> LintSummary {
    let mut summary = LintSummary {
        errors: 0,
        warnings: 0,
        infos: 0,
    };
    for finding in findings {
        match finding.severity {
            LintSeverity::Error => summary.errors += 1,
            LintSeverity::Warning => summary.warnings += 1,
            LintSeverity::Info => summary.infos += 1,
        }
    }
    summary
}

pub fn render_text(report: &LintReport) -> String {
    let mut out = String::new();
    render_group(&mut out, LintSeverity::Error, "Errors", &report.findings);
    render_group(
        &mut out,
        LintSeverity::Warning,
        "Warnings",
        &report.findings,
    );
    render_group(&mut out, LintSeverity::Info, "Infos", &report.findings);
    out
}

fn render_group(out: &mut String, severity: LintSeverity, heading: &str, findings: &[LintFinding]) {
    out.push_str(heading);
    out.push('\n');
    let mut any = false;
    for finding in findings.iter().filter(|f| f.severity == severity) {
        any = true;
        out.push_str(&format!(
            "{} {}{}: {}\n",
            finding.severity.as_str(),
            finding.code,
            format_location(finding.location.as_ref()),
            finding.message
        ));
    }
    if !any {
        out.push_str("  (none)\n");
    }
}

fn format_location(location: Option<&LintLocation>) -> String {
    let Some(location) = location else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(stage) = &location.stage {
        parts.push(format!("stage={stage}"));
    }
    if let Some(job) = &location.job {
        parts.push(format!("job={job}"));
    }
    if let Some(step) = &location.step {
        parts.push(format!("step={step}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(" "))
    }
}

fn rule_unused_output(summary: &PipelineSummary, findings: &mut Vec<LintFinding>) {
    let consumed = consumed_outputs(summary);
    for (job, step) in all_steps(summary) {
        let Some(step_id) = step.id.as_deref() else {
            continue;
        };
        for output in &step.outputs {
            let key = (step_id.to_string(), output.name.clone());
            if !consumed.contains(&key) {
                findings.push(LintFinding {
                    severity: LintSeverity::Warning,
                    code: "unused-output".to_string(),
                    message: format!(
                        "output '{}.{}' is declared but never read",
                        step_id, output.name
                    ),
                    location: Some(location_for(job, Some(step_id))),
                });
            }
        }
    }
}

/// Lint rule: every output consumed across step boundaries must be
/// declared with `isOutput=true` so ADO publishes it as a step output.
///
/// In the normal compile path `PipelineSummary::from_pipeline` already
/// patches `auto_is_output = true` on every affected declaration based
/// on the graph's `outputs_needing_is_output` set, so this rule will
/// stay quiet for well-formed inputs. We still emit a finding when the
/// flag is unset so that:
///
/// - Summaries constructed without going through `from_pipeline` (e.g.
///   deserialised straight from disk) are still validated.
/// - Future drift between the summary patcher and graph codegen — for
///   instance a new declaration kind that the patcher forgets to touch
///   — produces a real, surfaced finding instead of silently skipping.
fn rule_missing_is_output(summary: &PipelineSummary, findings: &mut Vec<LintFinding>) {
    let declarations = output_declarations(summary);
    for needed in &summary.graph.outputs_needing_is_output {
        for output_name in &needed.outputs {
            if let Some((job, step, decl)) =
                declarations.get(&(needed.step.clone(), output_name.clone()))
                && !decl.auto_is_output
            {
                findings.push(LintFinding {
                    severity: LintSeverity::Info,
                    code: "missing-is-output".to_string(),
                    message: format!(
                        "output '{}.{}' is consumed across steps but is not marked isOutput=true",
                        needed.step, output_name
                    ),
                    location: Some(location_for(job, step.id.as_deref())),
                });
            }
        }
    }
}

fn rule_anonymous_producer(summary: &PipelineSummary, findings: &mut Vec<LintFinding>) {
    for (job, step) in all_steps(summary) {
        if step.id.is_none() && !step.outputs.is_empty() {
            // The normal graph pass rejects this before lint runs. This
            // defensive rule also protects callers that lint a PipelineSummary
            // produced without build_graph validation.
            findings.push(LintFinding {
                severity: LintSeverity::Error,
                code: "anonymous-producer".to_string(),
                message: "step declares outputs but has no step id/name".to_string(),
                location: Some(location_for(job, None)),
            });
        }
    }
}

fn rule_step_id_collisions(summary: &PipelineSummary, findings: &mut Vec<LintFinding>) {
    // Track first-seen job for each step id, then emit one finding per
    // collision that names BOTH the original producer location and the
    // colliding consumer — otherwise the finding only points at the
    // second occurrence and operators have to grep the rest of the
    // pipeline to find the duplicate.
    let mut first_seen: BTreeMap<String, &JobSummary> = BTreeMap::new();
    for (job, step) in all_steps(summary) {
        let Some(step_id) = step.id.as_deref() else {
            continue;
        };
        if let Some(producer) = first_seen.get(step_id) {
            // The normal graph pass rejects pipeline-wide duplicate step ids.
            // Keep this defensive check for summaries that bypassed the graph.
            let producer_location = match &producer.stage {
                Some(stage) => format!("{stage}.{}", producer.id),
                None => producer.id.clone(),
            };
            findings.push(LintFinding {
                severity: LintSeverity::Error,
                code: "step-id-collisions".to_string(),
                message: format!(
                    "step id '{step_id}' is used more than once in the pipeline (also seen at {producer_location})"
                ),
                location: Some(location_for(job, Some(step_id))),
            });
        } else {
            first_seen.insert(step_id.to_string(), job);
        }
    }
}

fn consumed_outputs(summary: &PipelineSummary) -> BTreeSet<(String, String)> {
    // Cross-step / cross-job consumers are surfaced through
    // `outputs_needing_is_output` (the set the compiler patches with
    // `isOutput=true`). That set deliberately omits same-job consumers
    // because ADO does not require `isOutput=true` for those, so we
    // additionally walk every step's `env_refs` and `condition_refs`
    // to count references that stay inside one job. Matches
    // `graph_deps::step_refs`, which already treats both sets
    // uniformly regardless of job boundary.
    let mut consumed: BTreeSet<(String, String)> = summary
        .graph
        .outputs_needing_is_output
        .iter()
        .flat_map(|entry| {
            entry
                .outputs
                .iter()
                .map(|output| (entry.step.clone(), output.clone()))
        })
        .collect();
    for (_, step) in all_steps(summary) {
        for r in step.env_refs.iter().chain(step.condition_refs.iter()) {
            consumed.insert((r.step.clone(), r.name.clone()));
        }
    }
    consumed
}

fn output_declarations(
    summary: &PipelineSummary,
) -> BTreeMap<
    (String, String),
    (
        &JobSummary,
        &StepSummary,
        &crate::compile::ir::summary::OutputDeclSummary,
    ),
> {
    let mut declarations = BTreeMap::new();
    for (job, step) in all_steps(summary) {
        if let Some(step_id) = step.id.as_deref() {
            for decl in &step.outputs {
                declarations.insert((step_id.to_string(), decl.name.clone()), (job, step, decl));
            }
        }
    }
    declarations
}

fn all_steps(summary: &PipelineSummary) -> Vec<(&JobSummary, &StepSummary)> {
    summary
        .all_jobs()
        .flat_map(|job| job.steps.iter().map(move |step| (job, step)))
        .collect()
}

fn location_for(job: &JobSummary, step: Option<&str>) -> LintLocation {
    LintLocation {
        stage: job.stage.clone(),
        job: Some(job.id.clone()),
        step: step.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::summary::{
        GraphSummary, OutputDeclSummary, OutputRefSummary, PipelineBodySummary, PoolSummary,
        StepKind, StepOutputsEntry,
    };

    #[test]
    fn unused_output_produces_exactly_one_inspect_lint_finding() {
        let summary =
            summary_with_steps(vec![step_with_output("producer", "value", false)], vec![]);
        let findings = lint(&summary);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "unused-output");
        assert_eq!(findings[0].severity, LintSeverity::Warning);
    }

    #[test]
    fn no_findings_inspect_lint_emits_empty_list_and_zero_errors() {
        let summary = summary_with_steps(vec![plain_step("only")], vec![]);
        let report = report(&summary);
        assert!(report.findings.is_empty());
        assert_eq!(report.summary.errors, 0);
    }

    #[test]
    fn consumed_outputs_do_not_emit_unused_output_inspect_lint() {
        let summary = summary_with_steps(
            vec![step_with_output("producer", "pull_request_id", true)],
            vec![StepOutputsEntry {
                step: "producer".to_string(),
                outputs: vec!["pull_request_id".to_string()],
            }],
        );
        let findings = lint(&summary);
        assert!(!findings.iter().any(|f| f.code == "unused-output"));
    }

    #[test]
    fn same_job_env_ref_does_not_emit_unused_output_inspect_lint() {
        // Regression: outputs consumed by a peer step **within the
        // same job** (via env_refs / condition_refs) do not appear in
        // graph.outputs_needing_is_output — ADO does not require
        // isOutput=true for same-job reads. consumed_outputs must
        // still treat them as consumed so we do not emit a
        // false-positive `unused-output` finding.
        let mut producer = step_with_output("producer", "value", false);
        producer.id = Some("producer".to_string());
        let mut consumer = plain_step("consumer");
        consumer.env_refs.push(OutputRefSummary {
            step: "producer".to_string(),
            name: "value".to_string(),
        });

        let summary = summary_with_steps(vec![producer, consumer], vec![]);
        let findings = lint(&summary);
        assert!(
            !findings.iter().any(|f| f.code == "unused-output"),
            "same-job env_ref consumer must suppress unused-output, got {findings:?}"
        );
    }

    #[test]
    fn same_job_condition_ref_does_not_emit_unused_output_inspect_lint() {
        let producer = step_with_output("producer", "value", false);
        let mut consumer = plain_step("consumer");
        consumer.condition_refs.push(OutputRefSummary {
            step: "producer".to_string(),
            name: "value".to_string(),
        });

        let summary = summary_with_steps(vec![producer, consumer], vec![]);
        let findings = lint(&summary);
        assert!(
            !findings.iter().any(|f| f.code == "unused-output"),
            "same-job condition_ref consumer must suppress unused-output, got {findings:?}"
        );
    }

    #[tokio::test]
    async fn create_pull_request_fixture_has_no_unused_output_inspect_lint() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("safe-outputs")
            .join("create-pull-request.md");
        let (_fm, pipeline) = crate::compile::build_pipeline_ir(&fixture)
            .await
            .unwrap();
        let summary = PipelineSummary::from_pipeline(&pipeline).unwrap();
        let findings = lint(&summary);
        assert!(!findings.iter().any(|f| f.code == "unused-output"));
    }

    #[test]
    fn lint_finding_json_serialization_round_trips_for_inspect() {
        let finding = LintFinding {
            severity: LintSeverity::Info,
            code: "no-condition-references".to_string(),
            message: "example".to_string(),
            location: Some(LintLocation {
                stage: Some("Stage".to_string()),
                job: Some("Job".to_string()),
                step: None,
            }),
        };
        let json = serde_json::to_string(&finding).unwrap();
        let round_trip: LintFinding = serde_json::from_str(&json).unwrap();
        assert_eq!(round_trip, finding);
    }

    fn summary_with_steps(
        steps: Vec<StepSummary>,
        outputs_needing_is_output: Vec<StepOutputsEntry>,
    ) -> PipelineSummary {
        PipelineSummary {
            schema_version: 1,
            name: "test".to_string(),
            shape: "standalone".to_string(),
            body: PipelineBodySummary::Jobs {
                jobs: vec![JobSummary {
                    id: "Job".to_string(),
                    stage: None,
                    display_name: "Job".to_string(),
                    depends_on: vec![],
                    condition: None,
                    pool: PoolSummary::VmImage {
                        image: "ubuntu-latest".to_string(),
                    },
                    steps,
                }],
            },
            graph: GraphSummary {
                step_locations: vec![],
                job_edges: vec![],
                stage_edges: vec![],
                outputs_needing_is_output,
            },
        }
    }

    fn plain_step(id: &str) -> StepSummary {
        StepSummary {
            id: Some(id.to_string()),
            kind: StepKind::Bash,
            display_name: Some(id.to_string()),
            task: None,
            condition: None,
            outputs: vec![],
            env_refs: vec![],
            condition_refs: vec![],
        }
    }

    fn step_with_output(id: &str, output: &str, auto_is_output: bool) -> StepSummary {
        let mut step = plain_step(id);
        step.outputs.push(OutputDeclSummary {
            name: output.to_string(),
            is_secret: false,
            auto_is_output,
        });
        step
    }
}
