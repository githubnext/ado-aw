//! Sanitized `ado-proxy` decision and lifecycle log analyzer.

use std::collections::{BTreeMap, VecDeque};
use std::io::ErrorKind;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::audit::model::{
    AdoProxyAnalysis, AdoProxyEventSummary, AdoProxyLatencyStats, AdoProxyLifecycle,
    AdoProxyOperationStat, AdoProxyReasonStat,
};

const DECISION_LOG_SCHEMA: &str = "ado-aw/ado-proxy-decisions/v1";
const DECISION_LOG_FILE: &str = "ado-proxy-decisions.jsonl";
const CONTAINER_LOG_FILE: &str = "container.log";
const CONTAINER_STATE_FILE: &str = "container-state.txt";
const MAX_PROBLEM_EVENTS: usize = 20;
const MAX_LIFECYCLE_DIAGNOSTICS: usize = 20;
const MAX_DETAIL_CHARS: usize = 512;

/// Result of analyzing one `logs/ado-proxy` directory.
#[derive(Debug, Default)]
pub struct AdoProxyAnalysisResult {
    pub analysis: Option<AdoProxyAnalysis>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionLogHeader {
    schema: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DecisionKind {
    Allow,
    Deny,
    Error,
}

impl DecisionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionRecord {
    ts: String,
    request_id: String,
    host: String,
    method: String,
    operation: Option<String>,
    decision: DecisionKind,
    reason: Option<String>,
    detail: Option<String>,
    upstream_status_class: Option<String>,
    latency_ms: Option<u64>,
    response_bytes: Option<u64>,
    #[serde(default)]
    stripped_credentials: Vec<String>,
}

#[derive(Debug, Default)]
struct LatencyAccumulator {
    observed_count: u64,
    total_ms: u64,
    max_ms: u64,
}

impl LatencyAccumulator {
    fn record(&mut self, latency_ms: Option<u64>) {
        let Some(latency_ms) = latency_ms else {
            return;
        };
        self.observed_count += 1;
        self.total_ms = self.total_ms.saturating_add(latency_ms);
        self.max_ms = self.max_ms.max(latency_ms);
    }

    fn finish(self) -> Option<AdoProxyLatencyStats> {
        if self.observed_count == 0 {
            return None;
        }
        Some(AdoProxyLatencyStats {
            observed_count: self.observed_count,
            total_ms: self.total_ms,
            average_ms: self.total_ms as f64 / self.observed_count as f64,
            max_ms: self.max_ms,
        })
    }
}

#[derive(Debug, Default)]
struct OperationAccumulator {
    request_count: u64,
    allow_count: u64,
    deny_count: u64,
    error_count: u64,
    latency: LatencyAccumulator,
    response_bytes: u64,
}

impl OperationAccumulator {
    fn record(&mut self, record: &DecisionRecord) {
        self.request_count += 1;
        match record.decision {
            DecisionKind::Allow => self.allow_count += 1,
            DecisionKind::Deny => self.deny_count += 1,
            DecisionKind::Error => self.error_count += 1,
        }
        self.latency.record(record.latency_ms);
        self.response_bytes = self
            .response_bytes
            .saturating_add(record.response_bytes.unwrap_or_default());
    }

    fn finish(self, operation: Option<String>) -> AdoProxyOperationStat {
        AdoProxyOperationStat {
            operation,
            request_count: self.request_count,
            allow_count: self.allow_count,
            deny_count: self.deny_count,
            error_count: self.error_count,
            latency: self.latency.finish(),
            response_bytes: self.response_bytes,
        }
    }
}

#[derive(Debug, Default)]
struct DecisionAccumulator {
    total_requests: u64,
    allow_count: u64,
    deny_count: u64,
    error_count: u64,
    operations: BTreeMap<Option<String>, OperationAccumulator>,
    reasons: BTreeMap<(String, String), u64>,
    upstream_status_classes: BTreeMap<String, u64>,
    latency: LatencyAccumulator,
    response_bytes: u64,
    stripped_credentials: BTreeMap<String, u64>,
    recent_problem_events: VecDeque<AdoProxyEventSummary>,
}

impl DecisionAccumulator {
    fn record(&mut self, record: DecisionRecord) {
        self.total_requests += 1;
        match record.decision {
            DecisionKind::Allow => self.allow_count += 1,
            DecisionKind::Deny => self.deny_count += 1,
            DecisionKind::Error => self.error_count += 1,
        }

        self.operations
            .entry(record.operation.clone())
            .or_default()
            .record(&record);

        if let Some(reason) = record.reason.as_deref().filter(|reason| !reason.is_empty()) {
            *self
                .reasons
                .entry((record.decision.as_str().to_string(), reason.to_string()))
                .or_default() += 1;
        }

        if let Some(status_class) = record
            .upstream_status_class
            .as_deref()
            .filter(|status_class| !status_class.is_empty())
        {
            *self
                .upstream_status_classes
                .entry(status_class.to_string())
                .or_default() += 1;
        }

        self.latency.record(record.latency_ms);
        self.response_bytes = self
            .response_bytes
            .saturating_add(record.response_bytes.unwrap_or_default());

        for header in &record.stripped_credentials {
            let header = header.trim().to_ascii_lowercase();
            if !header.is_empty() {
                *self.stripped_credentials.entry(header).or_default() += 1;
            }
        }

        if matches!(record.decision, DecisionKind::Deny | DecisionKind::Error) {
            if self.recent_problem_events.len() == MAX_PROBLEM_EVENTS {
                self.recent_problem_events.pop_front();
            }
            self.recent_problem_events.push_back(AdoProxyEventSummary {
                timestamp: non_empty(record.ts),
                request_id: non_empty(record.request_id),
                host: non_empty(record.host),
                method: non_empty(record.method),
                operation: record.operation.and_then(non_empty),
                decision: record.decision.as_str().to_string(),
                reason: record.reason.and_then(non_empty),
                detail: record.detail.and_then(|detail| normalize_text(&detail)),
                upstream_status_class: record.upstream_status_class.and_then(non_empty),
                latency_ms: record.latency_ms,
            });
        }
    }

    fn apply(self, analysis: &mut AdoProxyAnalysis) {
        analysis.total_requests = self.total_requests;
        analysis.allow_count = self.allow_count;
        analysis.deny_count = self.deny_count;
        analysis.error_count = self.error_count;
        analysis.upstream_status_classes = self.upstream_status_classes;
        analysis.latency = self.latency.finish();
        analysis.response_bytes = self.response_bytes;
        analysis.stripped_credentials = self.stripped_credentials;
        analysis.recent_problem_events = self.recent_problem_events.into_iter().collect();

        analysis.operations = self
            .operations
            .into_iter()
            .map(|(operation, stats)| stats.finish(operation))
            .collect();
        analysis.operations.sort_by(|left, right| {
            right.request_count.cmp(&left.request_count).then_with(|| {
                match (&left.operation, &right.operation) {
                    (Some(left), Some(right)) => left.cmp(right),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            })
        });

        analysis.reasons = self
            .reasons
            .into_iter()
            .map(|((decision, reason), count)| AdoProxyReasonStat {
                reason,
                decision,
                count,
            })
            .collect();
        analysis.reasons.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.decision.cmp(&right.decision))
                .then_with(|| left.reason.cmp(&right.reason))
        });
    }
}

/// Analyze sanitized proxy diagnostics under `<agent_outputs>/logs/ado-proxy`.
pub async fn analyze_ado_proxy_logs(logs_dir: &Path) -> anyhow::Result<AdoProxyAnalysisResult> {
    match tokio::fs::metadata(logs_dir).await {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_dir(),
                "ado-proxy logs path is not a directory: {}",
                logs_dir.display()
            );
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(AdoProxyAnalysisResult::default());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to stat {}", logs_dir.display()));
        }
    }

    let mut result = AdoProxyAnalysisResult::default();
    let lifecycle = analyze_lifecycle(logs_dir, &mut result.warnings).await?;
    let mut analysis = AdoProxyAnalysis {
        lifecycle,
        ..AdoProxyAnalysis::default()
    };

    let decision_evidence =
        analyze_decisions(logs_dir, &mut analysis, &mut result.warnings).await?;
    if analysis.lifecycle.is_some() || decision_evidence {
        result.analysis = Some(analysis);
    }
    Ok(result)
}

async fn analyze_lifecycle(
    logs_dir: &Path,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Option<AdoProxyLifecycle>> {
    let mut lifecycle = AdoProxyLifecycle::default();
    let mut saw_evidence = false;

    let state_path = logs_dir.join(CONTAINER_STATE_FILE);
    if let Some(contents) = read_optional_file(&state_path).await? {
        let trimmed = contents.trim();
        if !trimmed.is_empty() {
            saw_evidence = true;
            parse_container_state(trimmed, &mut lifecycle, warnings);
        }
    }

    let log_path = logs_dir.join(CONTAINER_LOG_FILE);
    if let Some(file) = open_optional_file(&log_path).await? {
        let mut lines = BufReader::new(file).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .with_context(|| format!("Failed to read {}", log_path.display()))?
        {
            let Some(message) = line.strip_prefix("[ado-proxy] ") else {
                continue;
            };
            if message.starts_with("listening on ") {
                lifecycle.listening = true;
                saw_evidence = true;
                continue;
            }
            if is_lifecycle_failure(message) {
                saw_evidence = true;
                if lifecycle.diagnostics.len() < MAX_LIFECYCLE_DIAGNOSTICS
                    && let Some(message) = normalize_text(message)
                {
                    lifecycle.diagnostics.push(message);
                }
            }
        }
    }

    if !saw_evidence {
        return Ok(None);
    }

    lifecycle.healthy_before_teardown = lifecycle.listening
        && lifecycle.state_before_teardown.as_deref() == Some("running")
        && lifecycle.docker_error.is_none()
        && lifecycle.diagnostics.is_empty();
    Ok(Some(lifecycle))
}

fn parse_container_state(
    line: &str,
    lifecycle: &mut AdoProxyLifecycle,
    warnings: &mut Vec<String>,
) {
    if line == "state=missing before teardown" {
        lifecycle.state_before_teardown = Some("missing".to_string());
        return;
    }
    let Some(rest) = line.strip_prefix("state=") else {
        warnings.push(format!(
            "{CONTAINER_STATE_FILE} did not match the compiler-owned lifecycle format"
        ));
        return;
    };
    let Some((state, rest)) = rest.split_once(" exit=") else {
        warnings.push(format!(
            "{CONTAINER_STATE_FILE} did not match the compiler-owned lifecycle format"
        ));
        return;
    };
    let Some((exit_code, docker_error)) = rest.split_once(" error=") else {
        warnings.push(format!(
            "{CONTAINER_STATE_FILE} did not match the compiler-owned lifecycle format"
        ));
        return;
    };

    lifecycle.state_before_teardown = normalize_text(state);
    match exit_code.parse::<i64>() {
        Ok(exit_code) => lifecycle.exit_code_before_teardown = Some(exit_code),
        Err(_) => warnings.push(format!(
            "{CONTAINER_STATE_FILE} contained a non-integer exit code"
        )),
    }
    lifecycle.docker_error = normalize_text(docker_error);
}

fn is_lifecycle_failure(message: &str) -> bool {
    [
        "configuration error:",
        "cannot establish the interception identity:",
        "decision log disabled:",
        "decision log write failed:",
    ]
    .iter()
    .any(|prefix| message.starts_with(prefix))
}

async fn analyze_decisions(
    logs_dir: &Path,
    analysis: &mut AdoProxyAnalysis,
    warnings: &mut Vec<String>,
) -> anyhow::Result<bool> {
    let path = logs_dir.join(DECISION_LOG_FILE);
    let Some(file) = open_optional_file(&path).await? else {
        return Ok(false);
    };

    let mut lines = BufReader::new(file).lines();
    let mut header_line = None;
    while let Some(line) = lines
        .next_line()
        .await
        .with_context(|| format!("Failed to read {}", path.display()))?
    {
        if !line.trim().is_empty() {
            header_line = Some(line);
            break;
        }
    }

    let Some(header_line) = header_line else {
        warnings.push(format!("{DECISION_LOG_FILE} contained no schema header"));
        return Ok(false);
    };
    let header: DecisionLogHeader = match serde_json::from_str(header_line.trim()) {
        Ok(header) => header,
        Err(_) => {
            warnings.push(format!(
                "{DECISION_LOG_FILE} contained an invalid schema header"
            ));
            return Ok(false);
        }
    };
    if header.schema != DECISION_LOG_SCHEMA {
        warnings.push(format!(
            "{DECISION_LOG_FILE} uses unsupported schema '{}'; expected {DECISION_LOG_SCHEMA}",
            normalize_text(&header.schema).unwrap_or_else(|| "(empty)".to_string())
        ));
        return Ok(false);
    }

    analysis.schema_version = Some(header.schema);
    let mut accumulator = DecisionAccumulator::default();
    let mut malformed = 0_u64;
    while let Some(line) = lines
        .next_line()
        .await
        .with_context(|| format!("Failed to read {}", path.display()))?
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<DecisionRecord>(trimmed) {
            Ok(record) => accumulator.record(record),
            Err(_) => malformed += 1,
        }
    }

    accumulator.apply(analysis);
    analysis.malformed_record_count = malformed;
    if malformed > 0 {
        warnings.push(format!(
            "{DECISION_LOG_FILE} contained {malformed} malformed decision record(s)"
        ));
    }
    Ok(true)
}

async fn open_optional_file(path: &Path) -> anyhow::Result<Option<tokio::fs::File>> {
    match tokio::fs::File::open(path).await {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Failed to open {}", path.display())),
    }
}

async fn read_optional_file(path: &Path) -> anyhow::Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Failed to read {}", path.display())),
    }
}

fn normalize_text(value: &str) -> Option<String> {
    let normalized: String = value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_DETAIL_CHARS)
        .collect();
    non_empty(normalized.trim().to_string())
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn write(dir: &Path, name: &str, contents: &str) {
        tokio::fs::create_dir_all(dir).await.unwrap();
        tokio::fs::write(dir.join(name), contents).await.unwrap();
    }

    fn header() -> &'static str {
        "{\"schema\":\"ado-aw/ado-proxy-decisions/v1\"}\n"
    }

    #[tokio::test]
    async fn missing_directory_returns_none() {
        let temp = TempDir::new().unwrap();
        let result = analyze_ado_proxy_logs(&temp.path().join("missing"))
            .await
            .unwrap();
        assert!(result.analysis.is_none());
        assert!(result.warnings.is_empty());
    }

    #[tokio::test]
    async fn empty_directory_returns_none() {
        let temp = TempDir::new().unwrap();
        let result = analyze_ado_proxy_logs(temp.path()).await.unwrap();
        assert!(result.analysis.is_none());
    }

    #[tokio::test]
    async fn aggregates_valid_decisions_deterministically() {
        let temp = TempDir::new().unwrap();
        write(
            temp.path(),
            DECISION_LOG_FILE,
            &format!(
                "{}{}{}{}",
                header(),
                "{\"ts\":\"2026-01-01T00:00:00Z\",\"request_id\":\"1\",\"host\":\"dev.azure.com\",\"method\":\"GET\",\"operation\":\"core.project.get\",\"decision\":\"allow\",\"upstream_status_class\":\"2xx\",\"latency_ms\":10,\"response_bytes\":100,\"stripped_credentials\":[\"Authorization\"]}\n",
                "{\"ts\":\"2026-01-01T00:00:01Z\",\"request_id\":\"2\",\"host\":\"dev.azure.com\",\"method\":\"POST\",\"decision\":\"deny\",\"reason\":\"method-not-read\",\"detail\":\"POST is not a read method\",\"stripped_credentials\":[]}\n",
                "{\"ts\":\"2026-01-01T00:00:02Z\",\"request_id\":\"3\",\"host\":\"dev.azure.com\",\"method\":\"GET\",\"operation\":\"core.project.get\",\"decision\":\"error\",\"reason\":\"upstream-failed\",\"detail\":\"network down\",\"latency_ms\":20,\"stripped_credentials\":[\"authorization\"]}\n"
            ),
        )
        .await;

        let result = analyze_ado_proxy_logs(temp.path()).await.unwrap();
        let analysis = result.analysis.unwrap();
        assert_eq!(analysis.total_requests, 3);
        assert_eq!(analysis.allow_count, 1);
        assert_eq!(analysis.deny_count, 1);
        assert_eq!(analysis.error_count, 1);
        assert_eq!(analysis.response_bytes, 100);
        assert_eq!(analysis.upstream_status_classes["2xx"], 1);
        assert_eq!(analysis.stripped_credentials["authorization"], 2);
        assert_eq!(
            analysis.latency,
            Some(AdoProxyLatencyStats {
                observed_count: 2,
                total_ms: 30,
                average_ms: 15.0,
                max_ms: 20,
            })
        );
        assert_eq!(analysis.operations.len(), 2);
        assert_eq!(
            analysis.operations[0].operation.as_deref(),
            Some("core.project.get")
        );
        assert_eq!(analysis.operations[0].request_count, 2);
        assert!(analysis.operations[1].operation.is_none());
        assert_eq!(
            analysis
                .reasons
                .iter()
                .map(|reason| (
                    reason.decision.as_str(),
                    reason.reason.as_str(),
                    reason.count
                ))
                .collect::<Vec<_>>(),
            vec![
                ("deny", "method-not-read", 1),
                ("error", "upstream-failed", 1)
            ]
        );
        assert_eq!(analysis.recent_problem_events.len(), 2);
        assert!(result.warnings.is_empty());
    }

    #[tokio::test]
    async fn retains_only_the_final_twenty_problem_events() {
        let temp = TempDir::new().unwrap();
        let mut contents = header().to_string();
        for index in 0..25 {
            contents.push_str(&format!(
                "{{\"ts\":\"2026-01-01T00:00:{index:02}Z\",\"request_id\":\"{index}\",\"host\":\"dev.azure.com\",\"method\":\"GET\",\"decision\":\"deny\",\"reason\":\"out-of-scope\",\"stripped_credentials\":[]}}\n"
            ));
        }
        write(temp.path(), DECISION_LOG_FILE, &contents).await;

        let analysis = analyze_ado_proxy_logs(temp.path())
            .await
            .unwrap()
            .analysis
            .unwrap();
        assert_eq!(analysis.recent_problem_events.len(), 20);
        assert_eq!(
            analysis.recent_problem_events[0].request_id.as_deref(),
            Some("5")
        );
        assert_eq!(
            analysis.recent_problem_events[19].request_id.as_deref(),
            Some("24")
        );
    }

    #[tokio::test]
    async fn malformed_records_are_counted_without_echoing_content() {
        let temp = TempDir::new().unwrap();
        write(
            temp.path(),
            DECISION_LOG_FILE,
            &format!(
                "{}{}\n{}",
                header(),
                "{\"secret\":\"must-not-appear\"}",
                "{\"ts\":\"2026-01-01T00:00:00Z\",\"request_id\":\"1\",\"host\":\"dev.azure.com\",\"method\":\"GET\",\"decision\":\"allow\",\"stripped_credentials\":[]}\n"
            ),
        )
        .await;

        let result = analyze_ado_proxy_logs(temp.path()).await.unwrap();
        let analysis = result.analysis.unwrap();
        assert_eq!(analysis.malformed_record_count, 1);
        assert_eq!(analysis.total_requests, 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(!result.warnings[0].contains("must-not-appear"));
        assert!(
            !serde_json::to_string(&analysis)
                .unwrap()
                .contains("must-not-appear")
        );
    }

    #[tokio::test]
    async fn unknown_schema_preserves_lifecycle() {
        let temp = TempDir::new().unwrap();
        write(
            temp.path(),
            CONTAINER_STATE_FILE,
            "state=running exit=0 error=\n",
        )
        .await;
        write(
            temp.path(),
            CONTAINER_LOG_FILE,
            "[ado-proxy] listening on 0.0.0.0:11080\n",
        )
        .await;
        write(
            temp.path(),
            DECISION_LOG_FILE,
            "{\"schema\":\"ado-aw/ado-proxy-decisions/v2\"}\n",
        )
        .await;

        let result = analyze_ado_proxy_logs(temp.path()).await.unwrap();
        let analysis = result.analysis.unwrap();
        assert!(analysis.schema_version.is_none());
        assert!(analysis.lifecycle.unwrap().healthy_before_teardown);
        assert_eq!(result.warnings.len(), 1);
    }

    #[tokio::test]
    async fn missing_schema_warns_without_analysis() {
        let temp = TempDir::new().unwrap();
        write(temp.path(), DECISION_LOG_FILE, "").await;
        let result = analyze_ado_proxy_logs(temp.path()).await.unwrap();
        assert!(result.analysis.is_none());
        assert_eq!(result.warnings.len(), 1);
    }

    #[tokio::test]
    async fn lifecycle_failures_are_bounded_and_unhealthy() {
        let temp = TempDir::new().unwrap();
        write(
            temp.path(),
            CONTAINER_STATE_FILE,
            "state=exited exit=1 error=container failed\u{1b}[31m\n",
        )
        .await;
        let long = "x".repeat(600);
        write(
            temp.path(),
            CONTAINER_LOG_FILE,
            &format!(
                "[ado-proxy] configuration error: {long}\n[ado-proxy] decision log write failed: disk full\n"
            ),
        )
        .await;

        let analysis = analyze_ado_proxy_logs(temp.path())
            .await
            .unwrap()
            .analysis
            .unwrap();
        let lifecycle = analysis.lifecycle.unwrap();
        assert_eq!(lifecycle.state_before_teardown.as_deref(), Some("exited"));
        assert_eq!(lifecycle.exit_code_before_teardown, Some(1));
        assert_eq!(
            lifecycle.docker_error.as_deref(),
            Some("container failed[31m")
        );
        assert!(!lifecycle.listening);
        assert!(!lifecycle.healthy_before_teardown);
        assert_eq!(lifecycle.diagnostics.len(), 2);
        assert_eq!(lifecycle.diagnostics[0].chars().count(), MAX_DETAIL_CHARS);
    }

    #[tokio::test]
    async fn invalid_exit_code_warns_without_aborting() {
        let temp = TempDir::new().unwrap();
        write(
            temp.path(),
            CONTAINER_STATE_FILE,
            "state=running exit=not-a-number error=\n",
        )
        .await;
        let result = analyze_ado_proxy_logs(temp.path()).await.unwrap();
        let lifecycle = result.analysis.unwrap().lifecycle.unwrap();
        assert_eq!(lifecycle.state_before_teardown.as_deref(), Some("running"));
        assert_eq!(lifecycle.exit_code_before_teardown, None);
        assert_eq!(result.warnings.len(), 1);
    }

    #[tokio::test]
    async fn missing_before_teardown_state_is_preserved() {
        let temp = TempDir::new().unwrap();
        write(
            temp.path(),
            CONTAINER_STATE_FILE,
            "state=missing before teardown\n",
        )
        .await;

        let lifecycle = analyze_ado_proxy_logs(temp.path())
            .await
            .unwrap()
            .analysis
            .unwrap()
            .lifecycle
            .unwrap();
        assert_eq!(lifecycle.state_before_teardown.as_deref(), Some("missing"));
        assert!(!lifecycle.healthy_before_teardown);
    }
}
