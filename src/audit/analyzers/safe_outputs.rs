//! Safe-output trace analyzer for `ado-aw audit`.

use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::audit::model::{
    CreatedItemReport, Finding, RejectedSafeOutputsRollup, SafeOutputExecution,
    SafeOutputExecutionItem, SafeOutputStatus, SafeOutputSummary, Severity,
};
use crate::ndjson::{EXECUTED_NDJSON_FILENAME, SAFE_OUTPUT_FILENAME, read_ndjson_file};

/// Combined safe-output analysis result.
#[derive(Debug, Clone, Default)]
pub struct SafeOutputAnalysis {
    pub summary: Option<crate::audit::model::SafeOutputSummary>,
    pub execution: Option<crate::audit::model::SafeOutputExecution>,
    pub rollup: Option<crate::audit::model::RejectedSafeOutputsRollup>,
    pub created_items: Vec<crate::audit::model::CreatedItemReport>,
    /// Severity-`high` findings emitted when proposals were rejected by
    /// the aggregate detection gate. At most one finding per audit run.
    pub findings: Vec<crate::audit::model::Finding>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct DetectionVerdict {
    prompt_injection: bool,
    secret_leak: bool,
    malicious_patch: bool,
    reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct ExecutionRecord {
    name: String,
    status: String,
    context: Option<String>,
    result: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct ProposalRecord {
    index: usize,
    name: String,
    context: Option<String>,
    proposal: Value,
}

#[derive(Debug, Clone)]
struct IndexedExecutionRecord {
    index: usize,
    record: ExecutionRecord,
}

/// Analyze safe-output proposal, detection, and execution artifacts under one download root.
pub async fn analyze_safe_outputs(
    download_root: &std::path::Path,
) -> anyhow::Result<SafeOutputAnalysis> {
    let proposals_path = find_proposals_file(download_root).await?;
    let detection_path = find_detection_file(download_root).await?;
    let executions_paths = find_execution_files(download_root).await?;

    let proposals = load_proposals(proposals_path.as_deref()).await?;
    let detection = load_detection_verdict(detection_path.as_deref()).await?;
    let executions = load_execution_records(&executions_paths).await?;
    let detection_gate_fired = detection.as_ref().is_some_and(DetectionVerdict::gate_fired);

    let items = if detection_gate_fired {
        proposals
            .iter()
            .map(|proposal| {
                build_gate_rejected_item(proposal, detection.as_ref().expect("gate-fired verdict"))
            })
            .collect()
    } else {
        build_execution_items(&proposals, &executions)
    };

    let proposed_count = proposals.len() as u64;
    let executed_count = items
        .iter()
        .filter(|item| {
            matches!(
                item.status,
                SafeOutputStatus::Executed | SafeOutputStatus::Warning
            )
        })
        .count() as u64;
    let rejected_by_execution_count = items
        .iter()
        .filter(|item| {
            matches!(
                item.status,
                SafeOutputStatus::RejectedByExecution
                    | SafeOutputStatus::BudgetExhausted
                    | SafeOutputStatus::Skipped
            )
        })
        .count() as u64;
    let not_processed_count = items
        .iter()
        .filter(|item| item.status == SafeOutputStatus::NotProcessedDueToAggregateGate)
        .count() as u64;

    let summary = if proposed_count == 0 && items.is_empty() {
        None
    } else {
        Some(SafeOutputSummary {
            proposed_count,
            executed_count,
            rejected_by_execution_count,
            not_processed_count,
        })
    };

    let execution = (!items.is_empty()).then_some(SafeOutputExecution { items });
    let created_items = execution
        .as_ref()
        .map(|execution| {
            execution
                .items
                .iter()
                .filter_map(created_item_from_execution_item)
                .collect()
        })
        .unwrap_or_default();
    let rollup = build_rollup(summary.as_ref(), execution.as_ref(), detection.as_ref());
    let findings = if detection_gate_fired && proposed_count > 0 {
        vec![build_detection_finding(
            detection.as_ref().expect("gate-fired verdict"),
            proposed_count,
        )]
    } else {
        Vec::new()
    };

    Ok(SafeOutputAnalysis {
        summary,
        execution,
        rollup,
        created_items,
        findings,
    })
}

impl DetectionVerdict {
    fn gate_fired(&self) -> bool {
        self.prompt_injection || self.secret_leak || self.malicious_patch
    }

    fn flags(&self) -> Vec<&'static str> {
        let mut flags = Vec::new();
        if self.prompt_injection {
            flags.push("prompt_injection");
        }
        if self.secret_leak {
            flags.push("secret_leak");
        }
        if self.malicious_patch {
            flags.push("malicious_patch");
        }
        flags
    }
}

async fn load_proposals(path: Option<&Path>) -> anyhow::Result<Vec<ProposalRecord>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };

    let values = read_ndjson_file(path).await?;
    Ok(values
        .into_iter()
        .enumerate()
        .map(|(index, proposal)| ProposalRecord {
            index,
            name: extract_string_field(&proposal, &["name"]).unwrap_or_default(),
            context: extract_string_field(&proposal, &["context"]),
            proposal,
        })
        .collect())
}

async fn load_detection_verdict(path: Option<&Path>) -> anyhow::Result<Option<DetectionVerdict>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read detection verdict {}", path.display()))?;
    let verdict = serde_json::from_str::<DetectionVerdict>(&contents)
        .with_context(|| format!("Failed to parse detection verdict {}", path.display()))?;
    Ok(Some(verdict))
}

async fn load_execution_records(
    paths: &[PathBuf],
) -> anyhow::Result<Vec<IndexedExecutionRecord>> {
    let mut records = Vec::new();
    for path in paths {
        let values = read_ndjson_file(path).await?;
        for value in values {
            // `index` is assigned across the merged set so records from
            // multiple execution artifacts never collide during matching.
            let index = records.len();
            let mut record =
                serde_json::from_value::<ExecutionRecord>(value).with_context(|| {
                    format!(
                        "Failed to parse execution record {} from {}",
                        index,
                        path.display()
                    )
                })?;
            record.name = record.name.trim().to_string();
            record.status = record.status.trim().to_string();
            record.context = normalize_optional_string(record.context);
            record.error = normalize_optional_string(record.error);
            records.push(IndexedExecutionRecord { index, record });
        }
    }
    Ok(records)
}

fn build_execution_items(
    proposals: &[ProposalRecord],
    executions: &[IndexedExecutionRecord],
) -> Vec<SafeOutputExecutionItem> {
    let mut proposal_to_execution = vec![None; proposals.len()];
    let mut execution_matched = vec![false; executions.len()];
    let mut context_index = BTreeMap::<(String, String), VecDeque<usize>>::new();

    for proposal in proposals {
        let Some(context) = proposal.context.clone() else {
            continue;
        };
        context_index
            .entry((proposal.name.clone(), context))
            .or_default()
            .push_back(proposal.index);
    }

    for execution in executions {
        let Some(context) = execution.record.context.clone() else {
            continue;
        };
        let Some(proposal_indexes) =
            context_index.get_mut(&(execution.record.name.clone(), context))
        else {
            continue;
        };
        while let Some(proposal_index) = proposal_indexes.pop_front() {
            if proposal_to_execution[proposal_index].is_none() {
                proposal_to_execution[proposal_index] = Some(execution.index);
                execution_matched[execution.index] = true;
                break;
            }
        }
    }

    for execution in executions {
        if execution_matched[execution.index] || execution.record.context.is_some() {
            continue;
        }

        let Some(proposal) = proposals.get(execution.index) else {
            continue;
        };
        if proposal_to_execution[proposal.index].is_some() {
            continue;
        }
        if proposal.context.is_none() && proposal.name == execution.record.name {
            proposal_to_execution[proposal.index] = Some(execution.index);
            execution_matched[execution.index] = true;
        }
    }

    let mut items = Vec::with_capacity(proposals.len() + executions.len());
    for proposal in proposals {
        let item = match proposal_to_execution[proposal.index] {
            Some(execution_index) => {
                build_item_from_execution(proposal, &executions[execution_index].record)
            }
            None => build_missing_execution_item(proposal),
        };
        items.push(item);
    }

    for execution in executions {
        if execution_matched[execution.index] {
            continue;
        }
        items.push(build_unmatched_execution_item(&execution.record));
    }

    items
}

fn build_item_from_execution(
    proposal: &ProposalRecord,
    execution: &ExecutionRecord,
) -> SafeOutputExecutionItem {
    let status = map_execution_status(&execution.status);
    let error = execution.error.clone();
    SafeOutputExecutionItem {
        context: proposal
            .context
            .clone()
            .or_else(|| execution.context.clone()),
        tool: if proposal.name.is_empty() {
            execution.name.clone()
        } else {
            proposal.name.clone()
        },
        status,
        proposal: proposal.proposal.clone(),
        error: error.clone(),
        result: execution.result.clone(),
        rejection_reason: rejection_reason_for_status(status, error),
        applies_to_whole_batch: false,
    }
}

fn build_missing_execution_item(proposal: &ProposalRecord) -> SafeOutputExecutionItem {
    let error = Some(String::from("no execution record found"));
    SafeOutputExecutionItem {
        context: proposal.context.clone(),
        tool: proposal.name.clone(),
        status: SafeOutputStatus::Skipped,
        proposal: proposal.proposal.clone(),
        error: error.clone(),
        result: None,
        rejection_reason: error,
        applies_to_whole_batch: false,
    }
}

fn build_unmatched_execution_item(execution: &ExecutionRecord) -> SafeOutputExecutionItem {
    let status = map_execution_status(&execution.status);
    let error = execution.error.clone();
    SafeOutputExecutionItem {
        context: execution.context.clone(),
        tool: execution.name.clone(),
        status,
        proposal: Value::Null,
        error: error.clone(),
        result: execution.result.clone(),
        rejection_reason: rejection_reason_for_status(status, error),
        applies_to_whole_batch: false,
    }
}

fn build_gate_rejected_item(
    proposal: &ProposalRecord,
    detection: &DetectionVerdict,
) -> SafeOutputExecutionItem {
    SafeOutputExecutionItem {
        context: proposal.context.clone(),
        tool: proposal.name.clone(),
        status: SafeOutputStatus::NotProcessedDueToAggregateGate,
        proposal: proposal.proposal.clone(),
        error: None,
        result: None,
        rejection_reason: Some(aggregate_reason_key(detection)),
        applies_to_whole_batch: true,
    }
}

fn map_execution_status(status: &str) -> SafeOutputStatus {
    match status.trim().to_ascii_lowercase().as_str() {
        "succeeded" => SafeOutputStatus::Executed,
        "warning" => SafeOutputStatus::Warning,
        "failed" => SafeOutputStatus::RejectedByExecution,
        "skipped" => SafeOutputStatus::Skipped,
        "budget_exhausted" => SafeOutputStatus::BudgetExhausted,
        _ => SafeOutputStatus::Skipped,
    }
}

fn rejection_reason_for_status(status: SafeOutputStatus, error: Option<String>) -> Option<String> {
    match status {
        SafeOutputStatus::Executed | SafeOutputStatus::Warning => None,
        SafeOutputStatus::RejectedByExecution
        | SafeOutputStatus::Skipped
        | SafeOutputStatus::BudgetExhausted => error,
        SafeOutputStatus::NotProcessedDueToAggregateGate => None,
    }
}

fn build_rollup(
    summary: Option<&SafeOutputSummary>,
    execution: Option<&SafeOutputExecution>,
    detection: Option<&DetectionVerdict>,
) -> Option<RejectedSafeOutputsRollup> {
    let summary = summary?;

    let total_rejected = summary.rejected_by_execution_count + summary.not_processed_count;
    if total_rejected == 0 {
        return None;
    }

    let mut by_reason = BTreeMap::new();
    let mut by_threat = BTreeMap::new();

    if summary.not_processed_count > 0 {
        if let Some(detection) = detection {
            by_reason.insert(aggregate_reason_key(detection), summary.not_processed_count);
            for flag in detection.flags() {
                by_threat.insert(flag.to_string(), summary.not_processed_count);
            }
        }
    } else if let Some(execution) = execution {
        for item in &execution.items {
            let reason_key = match item.status {
                SafeOutputStatus::RejectedByExecution => truncate_reason(
                    item.error
                        .clone()
                        .unwrap_or_else(|| String::from("execution_failed")),
                    200,
                ),
                SafeOutputStatus::BudgetExhausted => String::from("budget_exhausted"),
                SafeOutputStatus::Skipped => String::from("skipped"),
                SafeOutputStatus::Executed
                | SafeOutputStatus::Warning
                | SafeOutputStatus::NotProcessedDueToAggregateGate => {
                    continue;
                }
            };
            *by_reason.entry(reason_key).or_insert(0) += 1;
        }
    }

    Some(RejectedSafeOutputsRollup {
        total_rejected,
        by_reason,
        by_threat,
    })
}

fn build_detection_finding(detection: &DetectionVerdict, proposed_count: u64) -> Finding {
    let flags = detection.flags().join(",");
    let reasons = if detection.reasons.is_empty() {
        String::from("- (none provided)")
    } else {
        detection
            .reasons
            .iter()
            .map(|reason| format!("- {reason}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    Finding {
        severity: Severity::High,
        category: String::from("safe_outputs"),
        title: format!("Detection rejected {proposed_count} safe output(s)"),
        description: format!(
            "The threat-analysis verdict had {flags} set. All {proposed_count} proposed safe outputs were dropped by the aggregate gate.\n\nReasons:\n{reasons}"
        ),
        impact: Some(String::from(
            "No items were created; the agent's work is not visible to downstream consumers.",
        )),
    }
}

fn created_item_from_execution_item(item: &SafeOutputExecutionItem) -> Option<CreatedItemReport> {
    if item.status != SafeOutputStatus::Executed {
        return None;
    }

    let result = item.result.as_ref()?;
    Some(CreatedItemReport {
        kind: item.tool.clone(),
        id: extract_string_field(result, &["id", "work_item_id", "number", "pr_number"]),
        url: extract_string_field(result, &["url", "html_url", "web_url"]),
        title: extract_string_field(result, &["title", "name", "subject"]),
    })
}

fn extract_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|candidate| match candidate {
            Value::String(text) => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(boolean) => Some(boolean.to_string()),
            _ => None,
        })
    })
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn aggregate_reason_key(detection: &DetectionVerdict) -> String {
    let joined = detection
        .reasons
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    if joined.is_empty() {
        format!("detection: {}", detection.flags().join(","))
    } else {
        joined
    }
}

fn truncate_reason(reason: String, max_chars: usize) -> String {
    let mut chars = reason.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        truncated
    } else {
        reason
    }
}

async fn find_proposals_file(download_root: &Path) -> anyhow::Result<Option<PathBuf>> {
    for directory in top_level_dirs_with_prefix(download_root, "agent_outputs_").await? {
        for candidate in [
            directory.join("staging").join(SAFE_OUTPUT_FILENAME),
            directory.join(SAFE_OUTPUT_FILENAME),
        ] {
            if fs::metadata(&candidate)
                .await
                .map(|m| m.is_file())
                .unwrap_or(false)
            {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

async fn find_detection_file(download_root: &Path) -> anyhow::Result<Option<PathBuf>> {
    for directory in top_level_dirs_with_prefix(download_root, "analyzed_outputs_").await? {
        let candidate = directory.join("threat-analysis.json");
        if fs::metadata(&candidate)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

async fn find_execution_files(download_root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    // With manual review, execution splits across multiple artifacts
    // (`safe_outputs/` for the automatic path and `safe_outputs_reviewed/`
    // for the approval-gated path), each with its own `executed.ndjson`.
    // Collect them all so the audit reflects the complete set of actions.
    let mut matches = Vec::new();
    collect_named_files(download_root, EXECUTED_NDJSON_FILENAME, &mut matches).await?;
    matches.sort();
    matches.dedup();
    Ok(matches)
}

async fn top_level_dirs_with_prefix(root: &Path, prefix: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut entries = match fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read directory {}", root.display()));
        }
    };

    let mut matches: Vec<(String, PathBuf)> = Vec::new();
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(error) => {
                return Err(error).with_context(|| format!("Failed to iterate {}", root.display()));
            }
        };

        let file_type = entry
            .file_type()
            .await
            .with_context(|| format!("Failed to inspect {}", entry.path().display()))?;
        if !file_type.is_dir() {
            continue;
        }

        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with(prefix) {
            matches.push((name, entry.path()));
        }
    }
    // Sort by numeric suffix so `agent_outputs_10` outranks
    // `agent_outputs_9` (lexicographic sort gets this wrong).
    matches.sort_by(|(a, _), (b, _)| crate::audit::cmp_numeric_suffix(a, b));
    Ok(matches.into_iter().map(|(_, path)| path).collect())
}

fn collect_named_files<'a>(
    root: &'a Path,
    file_name: &'a str,
    matches: &'a mut Vec<PathBuf>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = match fs::read_dir(root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read directory {}", root.display()));
            }
        };

        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Failed to iterate {}", root.display()));
                }
            };
            let path = entry.path();
            let file_type = entry
                .file_type()
                .await
                .with_context(|| format!("Failed to inspect {}", path.display()))?;
            if file_type.is_dir() {
                collect_named_files(&path, file_name, matches).await?;
            } else if file_type.is_file()
                && path.file_name().and_then(|name| name.to_str()) == Some(file_name)
            {
                matches.push(path);
            }
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CreatedItemReport, EXECUTED_NDJSON_FILENAME, SAFE_OUTPUT_FILENAME, SafeOutputStatus,
        Severity, analyze_safe_outputs,
    };
    use serde_json::{Value, json};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    #[tokio::test]
    async fn empty_download_root_returns_default_analysis() {
        let temp_dir = TempDir::new().expect("create temp dir");

        let analysis = analyze_safe_outputs(temp_dir.path())
            .await
            .expect("analyze empty root");

        assert!(analysis.summary.is_none());
        assert!(analysis.execution.is_none());
        assert!(analysis.rollup.is_none());
        assert!(analysis.created_items.is_empty());
        assert!(analysis.findings.is_empty());
    }

    #[tokio::test]
    async fn proposals_with_successful_executions_are_reported_as_executed() {
        let temp_dir = TempDir::new().expect("create temp dir");
        write_ndjson(
            &temp_dir
                .path()
                .join("agent_outputs_42")
                .join("staging")
                .join(SAFE_OUTPUT_FILENAME),
            &[
                json!({"name": "noop", "context": "noop-1"}),
                json!({"name": "create_pull_request", "context": "pr-1"}),
            ],
        );
        write_ndjson(
            &temp_dir
                .path()
                .join("safe_outputs")
                .join(EXECUTED_NDJSON_FILENAME),
            &[
                json!({"name": "noop", "status": "succeeded", "context": "noop-1", "result": {"status": "ok"}}),
                json!({"name": "create_pull_request", "status": "succeeded", "context": "pr-1", "result": {"number": 7}}),
            ],
        );

        let analysis = analyze_safe_outputs(temp_dir.path())
            .await
            .expect("analyze successful safe outputs");

        let summary = analysis.summary.expect("summary");
        assert_eq!(summary.proposed_count, 2);
        assert_eq!(summary.executed_count, 2);
        assert_eq!(summary.rejected_by_execution_count, 0);
        assert_eq!(summary.not_processed_count, 0);

        let execution = analysis.execution.expect("execution");
        assert_eq!(execution.items.len(), 2);
        assert!(
            execution
                .items
                .iter()
                .all(|item| item.status == SafeOutputStatus::Executed)
        );
        assert!(analysis.rollup.is_none());
        assert!(analysis.findings.is_empty());
    }

    #[tokio::test]
    async fn execution_records_aggregate_across_split_artifacts() {
        // Manual-review split: automatic outputs land in `safe_outputs/`,
        // reviewed (approval-gated) outputs in `safe_outputs_reviewed/`. The
        // audit must reflect both.
        let temp_dir = TempDir::new().expect("create temp dir");
        write_ndjson(
            &temp_dir
                .path()
                .join("agent_outputs_99")
                .join("staging")
                .join(SAFE_OUTPUT_FILENAME),
            &[
                json!({"name": "add_pr_comment", "context": "c-1"}),
                json!({"name": "create_pull_request", "context": "pr-1"}),
            ],
        );
        write_ndjson(
            &temp_dir
                .path()
                .join("safe_outputs")
                .join(EXECUTED_NDJSON_FILENAME),
            &[json!({"name": "add_pr_comment", "status": "succeeded", "context": "c-1", "result": {"status": "ok"}})],
        );
        write_ndjson(
            &temp_dir
                .path()
                .join("safe_outputs_reviewed")
                .join(EXECUTED_NDJSON_FILENAME),
            &[json!({"name": "create_pull_request", "status": "succeeded", "context": "pr-1", "result": {"number": 9}})],
        );

        let analysis = analyze_safe_outputs(temp_dir.path())
            .await
            .expect("analyze split safe outputs");

        let summary = analysis.summary.expect("summary");
        assert_eq!(summary.proposed_count, 2);
        // Both the automatic and the reviewed execution are counted.
        assert_eq!(summary.executed_count, 2);

        let execution = analysis.execution.expect("execution");
        assert_eq!(execution.items.len(), 2);
        assert!(
            execution
                .items
                .iter()
                .all(|item| item.status == SafeOutputStatus::Executed)
        );
    }

    #[tokio::test]
    async fn aggregate_detection_gate_rejects_all_proposals() {
        let temp_dir = TempDir::new().expect("create temp dir");
        write_ndjson(
            &temp_dir
                .path()
                .join("agent_outputs_77")
                .join("staging")
                .join(SAFE_OUTPUT_FILENAME),
            &[
                json!({"name": "noop", "context": "noop-1"}),
                json!({"name": "create_pull_request", "context": "pr-1"}),
            ],
        );
        write_json(
            &temp_dir
                .path()
                .join("analyzed_outputs_77")
                .join("threat-analysis.json"),
            &json!({
                "prompt_injection": true,
                "secret_leak": false,
                "malicious_patch": false,
                "reasons": ["evil"]
            }),
        );
        write_ndjson(
            &temp_dir
                .path()
                .join("safe_outputs")
                .join(EXECUTED_NDJSON_FILENAME),
            &[json!({"name": "noop", "status": "succeeded", "context": "noop-1"})],
        );

        let analysis = analyze_safe_outputs(temp_dir.path())
            .await
            .expect("analyze gate-rejected safe outputs");

        let summary = analysis.summary.expect("summary");
        assert_eq!(summary.proposed_count, 2);
        assert_eq!(
            summary.not_processed_count, 2,
            "all proposals must be counted as not_processed when the gate fires"
        );
        assert_eq!(summary.executed_count, 0);
        assert_eq!(summary.rejected_by_execution_count, 0);

        let execution = analysis.execution.expect("execution");
        assert_eq!(execution.items.len(), 2);
        assert!(execution.items.iter().all(|item| {
            item.status == SafeOutputStatus::NotProcessedDueToAggregateGate
                && item.applies_to_whole_batch
        }));

        let rollup = analysis.rollup.expect("rollup");
        assert_eq!(rollup.total_rejected, 2);
        assert_eq!(rollup.by_reason.get("evil"), Some(&2));
        assert_eq!(rollup.by_threat.get("prompt_injection"), Some(&2));

        assert_eq!(analysis.findings.len(), 1);
        assert_eq!(analysis.findings[0].severity, Severity::High);
        assert_eq!(analysis.findings[0].category, "safe_outputs");
        assert_eq!(
            analysis.findings[0].title,
            "Detection rejected 2 safe output(s)"
        );
    }

    #[tokio::test]
    async fn mixed_execution_outcomes_are_rolled_up() {
        let temp_dir = TempDir::new().expect("create temp dir");
        write_ndjson(
            &temp_dir
                .path()
                .join("agent_outputs_11")
                .join("staging")
                .join(SAFE_OUTPUT_FILENAME),
            &[
                json!({"name": "noop"}),
                json!({"name": "create_pull_request", "context": "pr-ctx"}),
                json!({"name": "create_issue"}),
            ],
        );
        write_ndjson(
            &temp_dir
                .path()
                .join("safe_outputs")
                .join(EXECUTED_NDJSON_FILENAME),
            &[
                json!({"name": "noop", "status": "succeeded", "result": {"status": "ok"}}),
                json!({
                    "name": "create_pull_request",
                    "status": "failed",
                    "context": "pr-ctx",
                    "error": "permission denied"
                }),
            ],
        );

        let analysis = analyze_safe_outputs(temp_dir.path())
            .await
            .expect("analyze mixed execution outcomes");

        let summary = analysis.summary.expect("summary");
        assert_eq!(summary.proposed_count, 3);
        assert_eq!(summary.executed_count, 1);
        assert_eq!(summary.rejected_by_execution_count, 2);
        assert_eq!(summary.not_processed_count, 0);

        let execution = analysis.execution.expect("execution");
        assert_eq!(
            execution
                .items
                .iter()
                .map(|item| item.status)
                .collect::<Vec<_>>(),
            vec![
                SafeOutputStatus::Executed,
                SafeOutputStatus::RejectedByExecution,
                SafeOutputStatus::Skipped,
            ]
        );

        let rollup = analysis.rollup.expect("rollup");
        assert_eq!(rollup.total_rejected, 2);
        assert_eq!(rollup.by_reason.get("permission denied"), Some(&1));
        assert_eq!(rollup.by_reason.get("skipped"), Some(&1));
    }

    #[tokio::test]
    async fn warning_status_counts_as_executed_not_rejected() {
        // A `noop` proposal that succeeds with a warning (e.g. missing ADO
        // credentials) is written to the executed manifest with
        // status: "warning". It should be counted toward executed_count and
        // must not show up in the rejection rollup.
        let temp_dir = TempDir::new().expect("create temp dir");
        write_ndjson(
            &temp_dir
                .path()
                .join("agent_outputs_77")
                .join("staging")
                .join(SAFE_OUTPUT_FILENAME),
            &[json!({"name": "noop", "context": "noop-warn"})],
        );
        write_ndjson(
            &temp_dir
                .path()
                .join("safe_outputs")
                .join(EXECUTED_NDJSON_FILENAME),
            &[json!({
                "name": "noop",
                "status": "warning",
                "context": "noop-warn",
                "result": {"status": "ok"}
            })],
        );

        let analysis = analyze_safe_outputs(temp_dir.path())
            .await
            .expect("analyze warning execution outcome");

        let summary = analysis.summary.expect("summary");
        assert_eq!(summary.proposed_count, 1);
        assert_eq!(
            summary.executed_count, 1,
            "warning status should count toward executed_count"
        );
        assert_eq!(
            summary.rejected_by_execution_count, 0,
            "warning status must not be counted as a rejection"
        );

        let execution = analysis.execution.expect("execution");
        assert_eq!(execution.items.len(), 1);
        assert_eq!(execution.items[0].status, SafeOutputStatus::Warning);
        assert!(
            execution.items[0].rejection_reason.is_none(),
            "warning items should have no rejection_reason"
        );

        assert!(
            analysis.rollup.is_none(),
            "no rejection rollup should be emitted for a warning-only run"
        );
    }

    #[tokio::test]
    async fn created_item_report_uses_field_fallbacks() {
        let temp_dir = TempDir::new().expect("create temp dir");
        write_ndjson(
            &temp_dir
                .path()
                .join("agent_outputs_1")
                .join("staging")
                .join(SAFE_OUTPUT_FILENAME),
            &[json!({"name": "create_pull_request", "context": "pr-42"})],
        );
        write_ndjson(
            &temp_dir
                .path()
                .join("safe_outputs")
                .join(EXECUTED_NDJSON_FILENAME),
            &[json!({
                "name": "create_pull_request",
                "status": "succeeded",
                "context": "pr-42",
                "result": {
                    "url": "https://example.invalid/pr/42",
                    "number": 42,
                    "title": "Fix"
                }
            })],
        );

        let analysis = analyze_safe_outputs(temp_dir.path())
            .await
            .expect("analyze created item report");

        assert_eq!(analysis.created_items.len(), 1);
        assert_eq!(
            analysis.created_items[0],
            CreatedItemReport {
                kind: String::from("create_pull_request"),
                url: Some(String::from("https://example.invalid/pr/42")),
                id: Some(String::from("42")),
                title: Some(String::from("Fix")),
            }
        );
    }

    #[tokio::test]
    async fn created_item_report_falls_back_to_work_item_id() {
        let temp_dir = TempDir::new().expect("create temp dir");
        write_ndjson(
            &temp_dir
                .path()
                .join("agent_outputs_2")
                .join("staging")
                .join(SAFE_OUTPUT_FILENAME),
            &[json!({"name": "create_work_item", "context": "wi-99"})],
        );
        write_ndjson(
            &temp_dir
                .path()
                .join("safe_outputs")
                .join(EXECUTED_NDJSON_FILENAME),
            &[json!({
                "name": "create_work_item",
                "status": "succeeded",
                "context": "wi-99",
                "result": {"work_item_id": 99}
            })],
        );

        let analysis = analyze_safe_outputs(temp_dir.path())
            .await
            .expect("analyze created work item report");

        assert_eq!(analysis.created_items.len(), 1);
        assert_eq!(analysis.created_items[0].id.as_deref(), Some("99"));
    }

    fn write_ndjson(path: &Path, values: &[Value]) {
        let contents = values
            .iter()
            .map(|value| serde_json::to_string(value).expect("serialize ndjson value"))
            .collect::<Vec<_>>()
            .join("\n");
        write_text(path, &(contents + "\n"));
    }

    fn write_json(path: &Path, value: &Value) {
        write_text(
            path,
            &serde_json::to_string(value).expect("serialize json value"),
        );
    }

    fn write_text(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directories");
        }
        fs::write(path, contents).expect("write test file");
    }
}
