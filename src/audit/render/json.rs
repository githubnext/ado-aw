use anyhow::Context;

/// Render `AuditData` as pretty-printed JSON to a writer.
///
/// Used when the CLI is invoked with `--json`. The JSON shape is the
/// public contract documented in `docs/audit.md` — top-level field
/// names are stable; nested fields may be extended but never removed
/// without deprecation.
pub fn render_json<W: std::io::Write>(
    audit: &crate::audit::model::AuditData,
    writer: &mut W,
) -> anyhow::Result<()> {
    serde_json::to_writer_pretty(&mut *writer, audit)
        .context("failed to serialize audit report as JSON")?;
    std::io::Write::write_all(writer, b"\n")
        .context("failed to write trailing newline for JSON audit report")?;
    Ok(())
}

/// Convenience: render to a `String`.
#[cfg(test)]
pub fn render_json_to_string(audit: &crate::audit::model::AuditData) -> anyhow::Result<String> {
    let mut json =
        serde_json::to_string_pretty(audit).context("failed to serialize audit report as JSON")?;
    json.push('\n');
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::{render_json, render_json_to_string};
    use crate::audit::model::*;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    fn populated_audit_data() -> AuditData {
        let mut by_reason = BTreeMap::new();
        by_reason.insert(String::from("aggregate_gate"), 1);

        let mut by_threat = BTreeMap::new();
        by_threat.insert(String::from("prompt_injection"), 1);
        by_threat.insert(String::from("secret_leak"), 0);
        by_threat.insert(String::from("malicious_patch"), 0);

        AuditData {
            overview: OverviewData {
                build_id: 42,
                pipeline_name: String::from("agentic-pipeline"),
                status: String::from("completed"),
                result: Some(String::from("succeeded")),
                created_at: Some(String::from("2026-05-21T12:00:00Z")),
                started_at: Some(String::from("2026-05-21T12:01:00Z")),
                finished_at: Some(String::from("2026-05-21T12:06:00Z")),
                duration: Some(String::from("5m")),
                source_branch: Some(String::from("refs/heads/main")),
                source_version: Some(String::from("abcdef123456")),
                url: Some(String::from(
                    "https://dev.azure.com/example/project/_build/results?buildId=42",
                )),
                logs_path: Some(String::from("logs\\build-42")),
                aw_info: Some(AwInfo {
                    engine: Some(String::from("copilot")),
                    model: Some(String::from("gpt-5.4")),
                    agent_name: Some(String::from("agentic-auditor")),
                    source: Some(String::from("agents/security-scan.md")),
                    target: Some(String::from("standalone")),
                    compiler_version: Some(String::from("0.30.2")),
                }),
            },
            task_domain: Some(TaskDomainInfo {
                summary: String::from("Security review workflow"),
                data: json!({"domain": "security"}),
            }),
            behavior_fingerprint: Some(BehaviorFingerprint {
                summary: String::from("High tool usage with safe outputs"),
                data: json!({"pattern": "tool-heavy"}),
            }),
            agentic_assessments: vec![AgenticAssessment {
                summary: String::from("Agent produced actionable changes"),
                data: json!({"score": 0.92}),
            }],
            metrics: MetricsData {
                token_usage: 1200,
                effective_tokens: 1000,
                estimated_cost: 1.23,
                turns: 12,
                error_count: 1,
                warning_count: 2,
            },
            key_findings: vec![Finding {
                category: String::from("security"),
                severity: Severity::High,
                title: String::from("Detection gate tripped"),
                description: String::from("Threat detection blocked the safe-output batch."),
                impact: Some(String::from("No proposed changes were executed.")),
            }],
            recommendations: vec![Recommendation {
                priority: String::from("high"),
                action: String::from("Review the detection-stage verdict"),
                reason: String::from("The aggregate gate prevented execution."),
                example: Some(String::from(
                    "Inspect analyzed_outputs_42\\threat-analysis.json",
                )),
            }],
            performance_metrics: Some(PerformanceMetrics {
                tokens_per_minute: Some(240.0),
                cost_efficiency: Some(String::from("moderate")),
                most_used_tool: Some(String::from("edit")),
                network_requests: Some(18),
            }),
            engine_config: Some(AuditEngineConfig {
                engine: String::from("copilot"),
                model: Some(String::from("gpt-5.4")),
                version: Some(String::from("2026.05")),
                timeout_minutes: Some(30),
            }),
            safe_output_summary: Some(SafeOutputSummary {
                proposed_count: 2,
                executed_count: 1,
                rejected_by_execution_count: 0,
                not_processed_count: 1,
            }),
            safe_output_execution: Some(SafeOutputExecution {
                items: vec![SafeOutputExecutionItem {
                    context: Some(String::from("pr-1")),
                    tool: String::from("create_pull_request"),
                    status: SafeOutputStatus::NotProcessedDueToAggregateGate,
                    proposal: json!({"title": "Fix pipeline", "repository": "repo"}),
                    error: Some(String::from("Batch blocked by detection gate")),
                    result: Some(json!({"status": "blocked"})),
                    rejection_reason: Some(String::from("prompt_injection")),
                    applies_to_whole_batch: true,
                }],
            }),
            rejected_safe_outputs: Some(RejectedSafeOutputsRollup {
                total_rejected: 1,
                by_reason,
                by_threat,
            }),
            detection_analysis: Some(DetectionAnalysis {
                threats: DetectionThreats {
                    prompt_injection: true,
                    secret_leak: false,
                    malicious_patch: false,
                },
                reasons: vec![String::from("Suspicious instruction in fetched content")],
                safe_to_process: false,
                verdict_path: Some(String::from("analyzed_outputs_42\\threat-analysis.json")),
            }),
            mcp_server_health: Some(MCPServerHealth {
                servers: vec![MCPServerStats {
                    name: String::from("github-mcp"),
                    total_calls: 8,
                    error_count: 1,
                    error_rate: 0.125,
                    unreliable: true,
                }],
            }),
            jobs: vec![JobData {
                name: String::from("Agent"),
                status: String::from("completed"),
                result: Some(String::from("succeeded")),
                duration: Some(String::from("4m")),
                started_at: Some(String::from("2026-05-21T12:01:00Z")),
                finished_at: Some(String::from("2026-05-21T12:05:00Z")),
            }],
            downloaded_files: vec![FileInfo {
                path: String::from("logs\\build-42\\agent_outputs_42\\otel.jsonl"),
                size_bytes: 2048,
                sha256: Some(String::from("abc123")),
            }],
            missing_tools: vec![MissingToolReport {
                tool: Some(String::from("azure-devops")),
                context: Some(String::from("work-item-sync")),
                reason: Some(String::from("Tool not configured")),
                timestamp: Some(String::from("2026-05-21T12:03:00Z")),
                extra: json!({"required": true}),
            }],
            missing_data: vec![MissingDataReport {
                tool: Some(String::from("create_work_item")),
                context: Some(String::from("wi-1")),
                reason: Some(String::from("missing title")),
                timestamp: Some(String::from("2026-05-21T12:03:10Z")),
                extra: json!({"field": "title"}),
            }],
            noops: vec![NoopReport {
                tool: Some(String::from("noop")),
                context: Some(String::from("noop-1")),
                reason: Some(String::from("Nothing to do")),
                timestamp: Some(String::from("2026-05-21T12:03:20Z")),
                extra: json!({"kind": "noop"}),
            }],
            mcp_failures: vec![MCPFailureReport {
                tool: Some(String::from("github.search_code")),
                context: Some(String::from("call-17")),
                reason: Some(String::from("HTTP 502")),
                timestamp: Some(String::from("2026-05-21T12:03:30Z")),
                extra: json!({"retryable": true}),
            }],
            firewall_analysis: Some(FirewallAnalysis {
                domains: vec![DomainStat {
                    domain: String::from("api.github.com"),
                    status: String::from("allowed"),
                    request_count: 7,
                    first_seen: Some(String::from("2026-05-21T12:01:10Z")),
                    last_seen: Some(String::from("2026-05-21T12:04:55Z")),
                }],
                total_requests: 10,
                allowed_count: 9,
                denied_count: 1,
            }),
            policy_analysis: Some(PolicyAnalysis {
                policies: vec![PolicyRule {
                    pattern: String::from("https://api.github.com/**"),
                    verdict: String::from("allow"),
                    hit_count: 7,
                }],
                allow_count: 1,
                deny_count: 1,
            }),
            errors: vec![ErrorInfo {
                source: String::from("audit::detection"),
                message: String::from("Threat detection blocked execution"),
                timestamp: Some(String::from("2026-05-21T12:05:00Z")),
            }],
            warnings: vec![ErrorInfo {
                source: String::from("audit::firewall"),
                message: String::from("One request was denied"),
                timestamp: Some(String::from("2026-05-21T12:04:00Z")),
            }],
            tool_usage: vec![ToolUsageInfo {
                name: String::from("edit"),
                call_count: 5,
                total_duration_ms: Some(1500),
            }],
            mcp_tool_usage: Some(MCPToolUsageData {
                tools: vec![MCPToolSummary {
                    name: String::from("github.search_code"),
                    call_count: 3,
                    error_count: 1,
                    max_input_size: 512,
                    max_output_size: 4096,
                }],
            }),
            created_items: vec![CreatedItemReport {
                kind: String::from("pull_request"),
                url: Some(String::from(
                    "https://dev.azure.com/example/project/_git/repo/pullrequest/123",
                )),
                id: Some(String::from("123")),
                title: Some(String::from("Fix pipeline")),
            }],
        }
    }

    #[test]
    fn default_audit_data_round_trips_through_json() {
        let original = AuditData::default();
        let mut rendered = Vec::new();

        render_json(&original, &mut rendered).expect("render default audit data as JSON");

        let round_tripped: AuditData =
            serde_json::from_slice(&rendered).expect("deserialize default audit data");
        assert_eq!(round_tripped, original);
    }

    #[test]
    fn populated_audit_data_round_trips_through_json() {
        let original = populated_audit_data();
        let rendered = render_json_to_string(&original).expect("render populated audit data");

        let round_tripped: AuditData =
            serde_json::from_str(&rendered).expect("deserialize populated audit data");
        assert_eq!(round_tripped, original);
    }

    #[test]
    fn render_json_to_string_appends_trailing_newline() {
        let rendered =
            render_json_to_string(&AuditData::default()).expect("render default audit data");

        assert!(rendered.ends_with('\n'));
    }

    #[test]
    fn default_audit_data_emits_required_top_level_keys() {
        let rendered =
            render_json_to_string(&AuditData::default()).expect("render default audit data");
        let value: Value = serde_json::from_str(&rendered).expect("parse rendered JSON");
        let mut keys: Vec<_> = value
            .as_object()
            .expect("top-level JSON object")
            .keys()
            .cloned()
            .collect();

        keys.sort();
        assert_eq!(keys, vec!["downloaded_files", "metrics", "overview"]);
    }
}
