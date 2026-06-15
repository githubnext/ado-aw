//! Static failure reachability for `ado-aw whatif`.
//!
//! The command does not execute a pipeline. It uses the public
//! [`PipelineSummary`] graph and the already-rendered ADO condition
//! strings to classify downstream jobs that would be skipped after a
//! chosen job or step fails.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use anyhow::{Result, anyhow};
use serde::Serialize;

use crate::compile::ir::summary::{EdgeEntry, JobSummary, PipelineBodySummary, PipelineSummary};
use crate::inspect::graph_deps;

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
            find_job(summary, &job_id).map(|job| {
                let job_classification = classify_condition(&job.condition);
                // Inherit the stage's bypass classification when the
                // containing stage carries a `condition: always()` /
                // `succeededOrFailed()` / similar. Without this the
                // job branch alone reads as Skipped — wrong for the
                // common cleanup-stage pattern where the stage
                // bypasses failure but its inner jobs keep the
                // default `succeeded()` condition.
                let stage_classification = stage
                    .as_deref()
                    .and_then(|stage_id| find_stage(summary, stage_id))
                    .map(|stage_summary| classify_condition(&stage_summary.condition))
                    .unwrap_or(WhatIfClassification::Skipped);
                let classification =
                    stronger_classification(job_classification, stage_classification);
                DownstreamJob {
                    job: job.id.clone(),
                    stage: stage.or_else(|| job.stage.clone()),
                    classification,
                    condition: job.condition.clone(),
                }
            })
        })
        .collect()
}

/// Return `RunsAnyway` if either side asserts the job runs after
/// upstream failure; otherwise `Skipped`. Used to lift a stage's
/// bypass classification through to its contained jobs.
fn stronger_classification(
    a: WhatIfClassification,
    b: WhatIfClassification,
) -> WhatIfClassification {
    match (a, b) {
        (WhatIfClassification::RunsAnyway, _) | (_, WhatIfClassification::RunsAnyway) => {
            WhatIfClassification::RunsAnyway
        }
        _ => WhatIfClassification::Skipped,
    }
}

fn find_stage<'a>(
    summary: &'a PipelineSummary,
    stage_id: &str,
) -> Option<&'a crate::compile::ir::summary::StageSummary> {
    match &summary.body {
        PipelineBodySummary::Jobs { .. } => None,
        PipelineBodySummary::Stages { stages } => stages.iter().find(|stage| stage.id == stage_id),
    }
}

/// Classify a rendered ADO `condition:` string for what-if analysis.
///
/// Returns [`WhatIfClassification::RunsAnyway`] if the condition
/// contains a recognised failure-bypass marker (`always()`, `failed()`,
/// `succeededOrFailed()`) that is **not** inside an odd number of
/// `not(...)` wrappers. Negation is handled by
/// [`is_negated_call`], so `not(failed())` is treated as `Skipped` and
/// `not(not(failed()))` resolves back to `RunsAnyway`.
///
/// ## Coverage limitations
///
/// The classifier is a best-effort static analyser over the rendered
/// condition string, not a semantic ADO expression evaluator. Known
/// limitations, in order of "most likely to surprise an author":
///
/// - **`not(succeeded())` is misclassified as `Skipped`**. The
///   classifier's bypass list contains only `always()`, `failed()`,
///   and `succeededOrFailed()`; `succeeded()` is not recognised, so
///   negating it does not flip the classification. In practice
///   `not(succeeded())` (run when the parent did **not** succeed —
///   typically a cleanup job) would execute after an upstream
///   failure but is conservatively reported as `Skipped`. Treat
///   that result as a lower bound for any cleanup job using this
///   form.
/// - **Scoped predicate forms are not recognised**. ADO accepts
///   arguments such as `failed('Setup')`,
///   `succeededOrFailed('Stage.Job')`, or `always('Stage1')` to
///   scope the predicate to specific upstream jobs/stages. The
///   classifier searches for the bare `failed()` /
///   `succeededorfailed()` / `always()` tokens (parens immediately
///   closed), so any argumented form drops through to `Skipped`.
/// - **`canceled()` is not recognised**. A condition of `canceled()`
///   alone classifies as `Skipped`. The common
///   `or(failed(), canceled())` form is still classified as
///   `RunsAnyway` because `failed()` is in the list, so this only
///   matters for cancellation-only bypass jobs.
/// - **Variable-based and dependency-result conditions** such as
///   `eq(variables['Agent.JobStatus'], 'Failed')`,
///   `eq(dependencies.Setup.result, 'Failed')`, or
///   `in(dependencies.Agent.result, 'Failed', 'Canceled')` are
///   conservatively reported as `Skipped`. Treat that result as a
///   lower bound — a job may still execute at runtime via a
///   variable-based escape hatch we cannot statically detect.
/// - **Templated `${{ }}` expressions** that survived compile-time
///   substitution (e.g. `eq('${{ parameters.runAnyway }}', 'true')`)
///   are opaque to the classifier and report `Skipped`.
/// - **Boolean composition is ignored**. `and(failed(), eq(...))`
///   classifies as `RunsAnyway` because the unnegated `failed()`
///   marker is enough — the `eq(...)` half is not evaluated for
///   short-circuit semantics.
/// - **Multi-line `not(...)` wraps** can defeat the negation
///   detector. The normaliser strips spaces but not tabs or
///   newlines, so `not\n(failed())` would not satisfy the `not(`
///   lookbehind and the marker would be treated as un-negated.
///   ADO emits compact single-line conditions in practice.
/// - **Step-level conditions are ignored**. `classify_condition` is
///   only called for job/stage `condition:` strings; a step inside a
///   job with its own bypass does not affect the job's
///   classification.
/// - **String literals containing marker syntax** trigger a
///   false-positive `RunsAnyway`: a condition like
///   `eq(variables['result'], 'failed()')` would match the literal
///   `failed()` substring even though the call is never invoked. ADO
///   conditions are compiler-generated rather than raw user input, so
///   this is an accepted residual gap.
///
/// The authoritative source for any classification disagreement
/// remains the live ADO pipeline run (or
/// [`crate::inspect::trace`] over a real build's timeline).
fn classify_condition(condition: &Option<String>) -> WhatIfClassification {
    let Some(condition) = condition else {
        return WhatIfClassification::Skipped;
    };
    let normalized = condition.to_ascii_lowercase().replace(' ', "");
    if contains_unnegated_call(&normalized, "always()")
        || contains_unnegated_call(&normalized, "failed()")
        || contains_unnegated_call(&normalized, "succeededorfailed()")
    {
        WhatIfClassification::RunsAnyway
    } else {
        WhatIfClassification::Skipped
    }
}

fn contains_unnegated_call(normalized_condition: &str, call: &str) -> bool {
    let mut from = 0;
    while let Some(offset) = normalized_condition[from..].find(call) {
        let idx = from + offset;
        // Word-boundary guard so `failed()` does not match inside
        // `succeededorfailed()` (which starts at offset 11 within that
        // larger call). Without this the negation logic also misfires
        // because the four chars before the inner match are `edor`,
        // not `not(`, so `not(succeededOrFailed())` was wrongly
        // classified as RunsAnyway.
        if is_word_boundary_before(normalized_condition, idx)
            && !is_negated_call(normalized_condition, idx)
        {
            return true;
        }
        from = idx + call.len();
    }
    false
}

fn is_word_boundary_before(s: &str, idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    s.as_bytes()
        .get(idx - 1)
        .is_none_or(|&b| !b.is_ascii_alphanumeric())
}

fn is_negated_call(normalized_condition: &str, call_idx: usize) -> bool {
    // Compare via the underlying byte slice instead of
    // `normalized_condition[idx - NOT_PREFIX_LEN..idx]` so that a
    // multi-byte UTF-8 sequence ending immediately before the call
    // cannot land on a non-char-boundary and panic. `call_idx` is on a
    // boundary (it comes from `str::find`) but `idx - 4` is not
    // guaranteed to be. ADO normally emits ASCII-only conditions, but
    // a leaked display name could carry non-ASCII bytes — this keeps
    // us crash-safe regardless.
    const NOT_PREFIX: &[u8] = b"not(";
    const NOT_PREFIX_LEN: usize = NOT_PREFIX.len();
    let bytes = normalized_condition.as_bytes();
    let mut idx = call_idx;
    let mut negated = false;
    while idx >= NOT_PREFIX_LEN && bytes.get(idx - NOT_PREFIX_LEN..idx) == Some(NOT_PREFIX) {
        negated = !negated;
        idx -= NOT_PREFIX_LEN;
    }
    negated
}

fn reachable_edges(edges: &[EdgeEntry], start: &str) -> BTreeSet<String> {
    // whatif always walks downstream (producer → consumers); the
    // shared helper in graph_deps owns the BFS so the two failure
    // reachability tools cannot drift apart on traversal semantics.
    graph_deps::reachable_edges(edges, start, graph_deps::GraphDepsDirection::Downstream)
}

fn known_ids(summary: &PipelineSummary) -> Vec<String> {
    let mut ids: Vec<String> = summary
        .graph
        .step_locations
        .iter()
        .map(|loc| loc.step.clone())
        .collect();
    ids.extend(summary.all_jobs().map(|job| job.id.clone()));
    ids
}

fn find_job<'a>(summary: &'a PipelineSummary, job_id: &str) -> Option<&'a JobSummary> {
    summary.all_jobs().find(|job| job.id == job_id)
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
    // Reject low-quality matches: a completely unrelated input like
    // `xyzzy` should not get a suggestion just because some
    // candidate happens to be lexicographically nearest. The
    // threshold is half the needle length plus 2 so single typos in
    // short ids (e.g. `Aget` → `Agent`) still suggest while genuinely
    // unrelated inputs return `None`.
    let needle_len = needle.chars().count();
    let max_distance = needle_len / 2 + 2;
    candidates
        .map(|candidate| (levenshtein(needle, candidate), candidate))
        .filter(|(distance, _)| *distance <= max_distance)
        .min_by_key(|(distance, candidate)| (*distance, (*candidate).to_string()))
        .map(|(_, candidate)| candidate.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b_len = b.chars().count();
    let mut prev: Vec<usize> = (0..=b_len).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut curr = vec![i + 1];
        for (j, cb) in b.chars().enumerate() {
            let cost = usize::from(ca != cb);
            curr.push((curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost));
        }
        prev = curr;
    }
    prev[b_len]
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
    fn negated_failed_condition_is_skipped() {
        let mut summary = fixture(None);
        let PipelineBodySummary::Jobs { jobs } = &mut summary.body else {
            unreachable!("fixture uses jobs body");
        };
        let detection = jobs.iter_mut().find(|job| job.id == "Detection").unwrap();
        detection.condition = Some("not(failed())".to_string());

        let report = analyze(&summary, "Setup").unwrap();
        let detection = report
            .downstream_jobs
            .iter()
            .find(|job| job.job == "Detection")
            .unwrap();
        assert_eq!(detection.classification, WhatIfClassification::Skipped);
    }

    #[test]
    fn double_negated_failed_condition_runs_anyway() {
        let mut summary = fixture(None);
        let PipelineBodySummary::Jobs { jobs } = &mut summary.body else {
            unreachable!("fixture uses jobs body");
        };
        let detection = jobs.iter_mut().find(|job| job.id == "Detection").unwrap();
        detection.condition = Some("not(not(failed()))".to_string());

        let report = analyze(&summary, "Setup").unwrap();
        let detection = report
            .downstream_jobs
            .iter()
            .find(|job| job.job == "Detection")
            .unwrap();
        assert_eq!(detection.classification, WhatIfClassification::RunsAnyway);
    }

    #[test]
    fn negated_always_condition_is_skipped() {
        let mut summary = fixture(None);
        let PipelineBodySummary::Jobs { jobs } = &mut summary.body else {
            unreachable!("fixture uses jobs body");
        };
        let detection = jobs.iter_mut().find(|job| job.id == "Detection").unwrap();
        detection.condition = Some("not(always())".to_string());

        let report = analyze(&summary, "Setup").unwrap();
        let detection = report
            .downstream_jobs
            .iter()
            .find(|job| job.job == "Detection")
            .unwrap();
        assert_eq!(detection.classification, WhatIfClassification::Skipped);
    }

    #[test]
    fn negated_succeeded_or_failed_condition_is_skipped() {
        // Regression for the substring-match bug: `failed()` appears
        // inside `succeededorfailed()` at byte offset 11, and the four
        // chars before it are `edor` (not `not(`), so the old logic
        // wrongly classified `not(succeededOrFailed())` as RunsAnyway.
        let mut summary = fixture(None);
        let PipelineBodySummary::Jobs { jobs } = &mut summary.body else {
            unreachable!("fixture uses jobs body");
        };
        let detection = jobs.iter_mut().find(|job| job.id == "Detection").unwrap();
        detection.condition = Some("not(succeededOrFailed())".to_string());

        let report = analyze(&summary, "Setup").unwrap();
        let detection = report
            .downstream_jobs
            .iter()
            .find(|job| job.job == "Detection")
            .unwrap();
        assert_eq!(detection.classification, WhatIfClassification::Skipped);
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

    #[test]
    fn variable_based_condition_is_conservatively_skipped() {
        // Documented limitation: variable-based conditions are not
        // statically recognised and conservatively classify as Skipped.
        let mut summary = fixture(None);
        let PipelineBodySummary::Jobs { jobs } = &mut summary.body else {
            unreachable!("fixture uses jobs body");
        };
        let detection = jobs.iter_mut().find(|job| job.id == "Detection").unwrap();
        detection.condition = Some("eq(variables['Agent.JobStatus'], 'Failed')".to_string());

        let report = analyze(&summary, "Setup").unwrap();
        let detection = report
            .downstream_jobs
            .iter()
            .find(|job| job.job == "Detection")
            .unwrap();
        assert_eq!(detection.classification, WhatIfClassification::Skipped);
    }

    #[test]
    fn classifier_does_not_panic_on_multibyte_chars_adjacent_to_call() {
        // Regression: the old `is_negated_call` indexed the str
        // directly with `[idx - 4..idx]`, which panics if the four
        // bytes before `failed()` straddle a UTF-8 char boundary. An
        // emoji / accented character leaked into a display-name
        // segment of the condition could trigger that crash.
        //
        // The accented `é` is two bytes (0xC3 0xA9), so prepending it
        // makes the offset before `failed()` land *inside* the
        // multi-byte sequence. The byte-slice comparison must handle
        // that gracefully and not match `not(`.
        let mut summary = fixture(None);
        let PipelineBodySummary::Jobs { jobs } = &mut summary.body else {
            unreachable!("fixture uses jobs body");
        };
        let detection = jobs.iter_mut().find(|job| job.id == "Detection").unwrap();
        detection.condition = Some("éfailed()".to_string());

        let report = analyze(&summary, "Setup").expect("must not panic on multi-byte input");
        let detection = report
            .downstream_jobs
            .iter()
            .find(|job| job.job == "Detection")
            .unwrap();
        // `é` is not part of `not(`, so the call is treated as
        // un-negated and the job classifies as RunsAnyway.
        assert_eq!(detection.classification, WhatIfClassification::RunsAnyway);
    }

    #[test]
    fn closest_returns_none_for_unrelated_input() {
        // Regression: without the Levenshtein threshold, an input
        // like `xyzzy` would always be suggested the
        // lexicographically nearest candidate. That's noise.
        let candidates = ["Setup", "Agent", "Detection", "SafeOutputs"];
        assert_eq!(
            closest("xyzzy", candidates.iter().copied()),
            None,
            "unrelated input must not get a 'did you mean' hint"
        );
    }

    #[test]
    fn closest_suggests_single_typo_within_threshold() {
        let candidates = ["Setup", "Agent", "Detection", "SafeOutputs"];
        assert_eq!(
            closest("Aget", candidates.iter().copied()),
            Some("Agent".to_string()),
        );
    }

    #[test]
    fn stage_always_condition_propagates_to_inner_jobs_runs_anyway() {
        // Regression: in a Stages-bodied pipeline, when a downstream
        // stage carries `condition: always()` but its inner jobs
        // keep the default `succeeded()`, the jobs should classify
        // as RunsAnyway. Previously only `job.condition` was checked
        // and the stage-level bypass was dropped on the floor.
        use crate::compile::ir::summary::StageSummary;
        let stage_a = StageSummary {
            id: "BuildStage".to_string(),
            display_name: "BuildStage".to_string(),
            depends_on: Vec::new(),
            condition: None,
            jobs: vec![JobSummary {
                id: "Build".to_string(),
                stage: Some("BuildStage".to_string()),
                display_name: "Build".to_string(),
                depends_on: Vec::new(),
                condition: None,
                pool: PoolSummary::VmImage {
                    image: "ubuntu-latest".to_string(),
                },
                steps: Vec::new(),
            }],
        };
        let stage_b = StageSummary {
            id: "Cleanup".to_string(),
            display_name: "Cleanup".to_string(),
            depends_on: vec!["BuildStage".to_string()],
            // Stage-level always() — common cleanup pattern.
            condition: Some("always()".to_string()),
            jobs: vec![JobSummary {
                id: "CleanupJob".to_string(),
                stage: Some("Cleanup".to_string()),
                display_name: "CleanupJob".to_string(),
                depends_on: Vec::new(),
                // No job-level condition → defaults to succeeded()
                // semantics on its own.
                condition: None,
                pool: PoolSummary::VmImage {
                    image: "ubuntu-latest".to_string(),
                },
                steps: Vec::new(),
            }],
        };

        let summary = PipelineSummary {
            schema_version: 1,
            name: "stages-test".to_string(),
            shape: "1es".to_string(),
            body: PipelineBodySummary::Stages {
                stages: vec![stage_a, stage_b],
            },
            graph: GraphSummary {
                step_locations: Vec::new(),
                job_edges: Vec::new(),
                stage_edges: vec![EdgeEntry {
                    consumer: "Cleanup".to_string(),
                    producer: "BuildStage".to_string(),
                }],
                outputs_needing_is_output: Vec::new(),
            },
        };

        let report = analyze(&summary, "Build").expect("analyze Build failure");
        let cleanup = report
            .downstream_jobs
            .iter()
            .find(|job| job.job == "CleanupJob")
            .expect("CleanupJob must appear via stage-edge traversal");
        assert_eq!(
            cleanup.classification,
            WhatIfClassification::RunsAnyway,
            "stage-level always() must propagate to inner jobs"
        );
    }
}
