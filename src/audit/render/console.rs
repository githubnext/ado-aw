//! Console renderer for `ado-aw audit`.

use crate::audit::model::{self, Severity};

/// Render `AuditData` as a Markdown-flavored text report suitable for
/// terminal output. Pure function; no I/O.
///
/// Section ordering mirrors gh-aw's audit report:
///   Overview → Metrics → Key Findings → Recommendations →
///   Safe Output Summary → Rejected Safe Outputs →
///   MCP Server Health → Firewall Analysis → Policy Analysis →
///   Detection Analysis → Jobs → Downloaded Files →
///   Missing Tools → Missing Data → Noops → MCP Failures →
///   Errors → Warnings → Tool Usage → MCP Tool Usage →
///   Created Items.
///
/// Sections that are empty/None are omitted entirely (no empty headers).
pub fn render_console(audit: &crate::audit::model::AuditData) -> String {
    let mut sections = vec![
        render_overview_section(&audit.overview, audit.engine_config.as_ref()),
        render_metrics_section(&audit.metrics, audit.performance_metrics.as_ref()),
    ];

    if let Some(section) = render_key_findings_section(&audit.key_findings) {
        sections.push(section);
    }
    if let Some(section) = render_recommendations_section(&audit.recommendations) {
        sections.push(section);
    }
    if let Some(section) = render_safe_output_summary_section(audit.safe_output_summary.as_ref()) {
        sections.push(section);
    }
    if let Some(section) =
        render_rejected_safe_outputs_section(audit.rejected_safe_outputs.as_ref())
    {
        sections.push(section);
    }
    if let Some(section) = render_mcp_server_health_section(audit.mcp_server_health.as_ref()) {
        sections.push(section);
    }
    if let Some(section) = render_firewall_analysis_section(audit.firewall_analysis.as_ref()) {
        sections.push(section);
    }
    if let Some(section) = render_policy_analysis_section(audit.policy_analysis.as_ref()) {
        sections.push(section);
    }
    if let Some(section) = render_detection_analysis_section(audit.detection_analysis.as_ref()) {
        sections.push(section);
    }
    if let Some(section) = render_jobs_section(&audit.jobs) {
        sections.push(section);
    }
    if let Some(section) = render_downloaded_files_section(&audit.downloaded_files) {
        sections.push(section);
    }
    if let Some(section) = render_missing_tools_section(&audit.missing_tools) {
        sections.push(section);
    }
    if let Some(section) = render_missing_data_section(&audit.missing_data) {
        sections.push(section);
    }
    if let Some(section) = render_noops_section(&audit.noops) {
        sections.push(section);
    }
    if let Some(section) = render_mcp_failures_section(&audit.mcp_failures) {
        sections.push(section);
    }
    if let Some(section) = render_errors_section(&audit.errors) {
        sections.push(section);
    }
    if let Some(section) = render_warnings_section(&audit.warnings) {
        sections.push(section);
    }
    if let Some(section) = render_tool_usage_section(&audit.tool_usage) {
        sections.push(section);
    }
    if let Some(section) = render_mcp_tool_usage_section(audit.mcp_tool_usage.as_ref()) {
        sections.push(section);
    }
    if let Some(section) = render_created_items_section(&audit.created_items) {
        sections.push(section);
    }

    let mut out = sections.join("\n\n");
    out.push('\n');
    out
}

fn render_overview_section(
    overview: &model::OverviewData,
    engine_config: Option<&model::AuditEngineConfig>,
) -> String {
    let aw_info = overview.aw_info.as_ref();
    let mut rows = Vec::new();

    if overview.build_id > 0 {
        rows.push(("build_id".to_string(), format_number(overview.build_id)));
    }
    push_non_empty_row(&mut rows, "pipeline", &overview.pipeline_name);
    push_non_empty_row(&mut rows, "status", &overview.status);
    push_opt_row(&mut rows, "result", overview.result.as_deref());
    push_opt_row(&mut rows, "branch", overview.source_branch.as_deref());
    push_opt_row(&mut rows, "commit", overview.source_version.as_deref());
    if let Some(duration) = overview.duration.as_deref() {
        rows.push(("duration".to_string(), normalize_duration(duration)));
    }
    push_opt_row(&mut rows, "url", overview.url.as_deref());
    push_opt_row(&mut rows, "created_at", overview.created_at.as_deref());
    push_opt_row(&mut rows, "started_at", overview.started_at.as_deref());
    push_opt_row(&mut rows, "finished_at", overview.finished_at.as_deref());
    push_opt_row(&mut rows, "logs_path", overview.logs_path.as_deref());

    let engine = aw_info
        .and_then(|info| info.engine.as_deref())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            engine_config
                .filter(|config| !config.engine.is_empty())
                .map(|config| config.engine.clone())
        });
    let model = aw_info
        .and_then(|info| info.model.as_deref())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| engine_config.and_then(|config| config.model.clone()));

    push_opt_owned_row(&mut rows, "engine", engine);
    push_opt_owned_row(&mut rows, "model", model);
    push_opt_owned_row(
        &mut rows,
        "agent",
        aw_info.and_then(|info| info.agent_name.clone()),
    );
    push_opt_owned_row(
        &mut rows,
        "source",
        aw_info.and_then(|info| info.source.clone()),
    );
    push_opt_owned_row(
        &mut rows,
        "target",
        aw_info.and_then(|info| info.target.clone()),
    );
    push_opt_owned_row(
        &mut rows,
        "compiler_version",
        aw_info.and_then(|info| info.compiler_version.clone()),
    );
    push_opt_owned_row(
        &mut rows,
        "engine_version",
        engine_config.and_then(|config| config.version.clone()),
    );
    if let Some(timeout_minutes) = engine_config.and_then(|config| config.timeout_minutes) {
        rows.push((
            "timeout_minutes".to_string(),
            format_number(timeout_minutes),
        ));
    }

    render_kv_section("Overview", rows, true)
}

fn render_metrics_section(
    metrics: &model::MetricsData,
    performance_metrics: Option<&model::PerformanceMetrics>,
) -> String {
    let mut rows = vec![
        (
            "token_usage".to_string(),
            format_number(metrics.token_usage),
        ),
        (
            "effective_tokens".to_string(),
            format_number(metrics.effective_tokens),
        ),
        (
            "estimated_cost".to_string(),
            format_cost(metrics.estimated_cost),
        ),
        ("turns".to_string(), format_number(metrics.turns)),
        ("errors".to_string(), format_number(metrics.error_count)),
        ("warnings".to_string(), format_number(metrics.warning_count)),
    ];

    if let Some(performance_metrics) = performance_metrics {
        if let Some(tokens_per_minute) = performance_metrics.tokens_per_minute {
            rows.push((
                "tokens_per_minute".to_string(),
                format_float(tokens_per_minute),
            ));
        }
        push_opt_row(
            &mut rows,
            "cost_efficiency",
            performance_metrics.cost_efficiency.as_deref(),
        );
        push_opt_row(
            &mut rows,
            "most_used_tool",
            performance_metrics.most_used_tool.as_deref(),
        );
        if let Some(network_requests) = performance_metrics.network_requests {
            rows.push((
                "network_requests".to_string(),
                format_number(network_requests),
            ));
        }
    }

    render_kv_section("Metrics", rows, true)
}

fn render_key_findings_section(findings: &[model::Finding]) -> Option<String> {
    if findings.is_empty() {
        return None;
    }

    let lines = findings.iter().map(format_finding).collect();
    Some(render_lines_section("Key Findings", lines, false))
}

fn render_recommendations_section(recommendations: &[model::Recommendation]) -> Option<String> {
    if recommendations.is_empty() {
        return None;
    }

    let lines = recommendations.iter().map(format_recommendation).collect();
    Some(render_lines_section("Recommendations", lines, false))
}

fn render_safe_output_summary_section(
    summary: Option<&model::SafeOutputSummary>,
) -> Option<String> {
    let summary = summary?;
    let rows = vec![
        (
            "proposed".to_string(),
            format_number(summary.proposed_count),
        ),
        (
            "executed".to_string(),
            format_number(summary.executed_count),
        ),
        (
            "rejected_by_execution".to_string(),
            format_number(summary.rejected_by_execution_count),
        ),
        (
            "not_processed".to_string(),
            format_number(summary.not_processed_count),
        ),
    ];
    Some(render_kv_section("Safe Output Summary", rows, false))
}

fn render_rejected_safe_outputs_section(
    rollup: Option<&model::RejectedSafeOutputsRollup>,
) -> Option<String> {
    let rollup = rollup?;
    let by_reason = positive_count_lines(&rollup.by_reason);
    let by_threat = positive_count_lines(&rollup.by_threat);

    if rollup.total_rejected == 0 && by_reason.is_empty() && by_threat.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    if !by_reason.is_empty() {
        lines.push("By reason:".to_string());
        lines.extend(by_reason);
    }
    if !by_threat.is_empty() {
        lines.push("By threat:".to_string());
        lines.extend(by_threat);
    }

    Some(render_lines_section(
        format!(
            "Rejected Safe Outputs ({} total)",
            format_number(rollup.total_rejected)
        ),
        lines,
        false,
    ))
}

fn render_mcp_server_health_section(health: Option<&model::MCPServerHealth>) -> Option<String> {
    let health = health?;
    if health.servers.is_empty() {
        return None;
    }

    let lines = health
        .servers
        .iter()
        .map(|server| {
            let mut line = format!(
                "- {}  {} calls, {} errors ({})",
                fallback_text(&server.name, "(unnamed server)"),
                format_number(server.total_calls),
                format_number(server.error_count),
                format_percentage(server.error_rate),
            );
            if server.unreliable {
                line.push_str(" [unreliable]");
            }
            line
        })
        .collect();

    Some(render_lines_section("MCP Server Health", lines, false))
}

fn render_firewall_analysis_section(analysis: Option<&model::FirewallAnalysis>) -> Option<String> {
    let analysis = analysis?;
    if analysis.domains.is_empty()
        && analysis.total_requests == 0
        && analysis.allowed_count == 0
        && analysis.denied_count == 0
    {
        return None;
    }

    let width = analysis
        .domains
        .iter()
        .map(|domain| domain.domain.chars().count())
        .max()
        .unwrap_or(0);
    let lines = analysis
        .domains
        .iter()
        .map(|domain| {
            let name = fallback_text(&domain.domain, "(unknown domain)");
            format!(
                "- {:<width$} [{}]  {} requests",
                name,
                fallback_text(&domain.status, "unknown"),
                format_number(domain.request_count),
                width = width,
            )
        })
        .collect();

    Some(render_lines_section(
        format!(
            "Firewall Analysis (total: {} requests, allowed: {}, denied: {})",
            format_number(analysis.total_requests),
            format_number(analysis.allowed_count),
            format_number(analysis.denied_count),
        ),
        lines,
        false,
    ))
}

fn render_policy_analysis_section(analysis: Option<&model::PolicyAnalysis>) -> Option<String> {
    let analysis = analysis?;
    if analysis.policies.is_empty() && analysis.allow_count == 0 && analysis.deny_count == 0 {
        return None;
    }

    let width = analysis
        .policies
        .iter()
        .map(|policy| policy.pattern.chars().count())
        .max()
        .unwrap_or(0);
    let lines = analysis
        .policies
        .iter()
        .map(|policy| {
            let pattern = fallback_text(&policy.pattern, "(unnamed rule)");
            format!(
                "- {:<width$} [{}]  {} hits",
                pattern,
                fallback_text(&policy.verdict, "unknown"),
                format_number(policy.hit_count),
                width = width,
            )
        })
        .collect();

    Some(render_lines_section(
        format!(
            "Policy Analysis (allow: {}, deny: {})",
            format_number(analysis.allow_count),
            format_number(analysis.deny_count),
        ),
        lines,
        false,
    ))
}

fn render_detection_analysis_section(
    analysis: Option<&model::DetectionAnalysis>,
) -> Option<String> {
    let analysis = analysis?;
    let mut rows = vec![(
        "safe_to_process".to_string(),
        analysis.safe_to_process.to_string(),
    )];

    rows.push((
        "threats".to_string(),
        format_detection_threats(&analysis.threats),
    ));
    for reason in &analysis.reasons {
        if !reason.trim().is_empty() {
            rows.push(("reason".to_string(), reason.clone()));
        }
    }
    push_opt_row(&mut rows, "verdict_path", analysis.verdict_path.as_deref());

    Some(render_kv_section("Detection Analysis", rows, false))
}

fn render_jobs_section(jobs: &[model::JobData]) -> Option<String> {
    if jobs.is_empty() {
        return None;
    }

    let width = jobs
        .iter()
        .map(|job| job.name.chars().count())
        .max()
        .unwrap_or(0);
    let lines = jobs
        .iter()
        .map(|job| {
            let mut state = fallback_text(&job.status, "unknown").to_string();
            if let Some(result) = job.result.as_deref().filter(|value| !value.is_empty()) {
                state.push('/');
                state.push_str(result);
            }

            let mut line = format!(
                "- {:<width$} [{}]",
                fallback_text(&job.name, "(unnamed job)"),
                state,
                width = width,
            );
            if let Some(duration) = job.duration.as_deref() {
                line.push_str("  ");
                line.push_str(&normalize_duration(duration));
            }
            line
        })
        .collect();

    Some(render_lines_section("Jobs", lines, false))
}

fn render_downloaded_files_section(files: &[model::FileInfo]) -> Option<String> {
    if files.is_empty() {
        return None;
    }

    let lines = files
        .iter()
        .map(|file| {
            let mut line = format!(
                "- {}  {}",
                fallback_text(&file.path, "(unknown path)"),
                format_bytes(file.size_bytes),
            );
            if let Some(sha256) = file.sha256.as_deref().filter(|value| !value.is_empty()) {
                line.push_str("  sha256: ");
                line.push_str(sha256);
            }
            line
        })
        .collect();

    Some(render_lines_section("Downloaded Files", lines, false))
}

fn render_missing_tools_section(reports: &[model::MissingToolReport]) -> Option<String> {
    if reports.is_empty() {
        return None;
    }

    let lines = reports
        .iter()
        .map(|report| {
            format_named_report(
                report.tool.as_deref(),
                report.context.as_deref(),
                report.reason.as_deref(),
                report.timestamp.as_deref(),
                "(unknown tool)",
            )
        })
        .collect();

    Some(render_lines_section("Missing Tools", lines, false))
}

fn render_missing_data_section(reports: &[model::MissingDataReport]) -> Option<String> {
    if reports.is_empty() {
        return None;
    }

    let lines = reports
        .iter()
        .map(|report| {
            format_named_report(
                report.tool.as_deref(),
                report.context.as_deref(),
                report.reason.as_deref(),
                report.timestamp.as_deref(),
                "(unknown tool)",
            )
        })
        .collect();

    Some(render_lines_section("Missing Data", lines, false))
}

fn render_noops_section(reports: &[model::NoopReport]) -> Option<String> {
    if reports.is_empty() {
        return None;
    }

    let lines = reports
        .iter()
        .map(|report| {
            format_named_report(
                report.tool.as_deref(),
                report.context.as_deref(),
                report.reason.as_deref(),
                report.timestamp.as_deref(),
                "(unknown tool)",
            )
        })
        .collect();

    Some(render_lines_section("Noops", lines, false))
}

fn render_mcp_failures_section(reports: &[model::MCPFailureReport]) -> Option<String> {
    if reports.is_empty() {
        return None;
    }

    let lines = reports
        .iter()
        .map(|report| {
            format_named_report(
                report.tool.as_deref(),
                report.context.as_deref(),
                report.reason.as_deref(),
                report.timestamp.as_deref(),
                "(unknown MCP tool)",
            )
        })
        .collect();

    Some(render_lines_section("MCP Failures", lines, false))
}

fn render_errors_section(errors: &[model::ErrorInfo]) -> Option<String> {
    if errors.is_empty() {
        return None;
    }

    let lines = errors
        .iter()
        .map(|error| {
            format_message_report(&error.source, &error.message, error.timestamp.as_deref())
        })
        .collect();

    Some(render_lines_section("Errors", lines, false))
}

fn render_warnings_section(warnings: &[model::ErrorInfo]) -> Option<String> {
    if warnings.is_empty() {
        return None;
    }

    let lines = warnings
        .iter()
        .map(|warning| {
            format_message_report(
                &warning.source,
                &warning.message,
                warning.timestamp.as_deref(),
            )
        })
        .collect();

    Some(render_lines_section("Warnings", lines, false))
}

fn render_tool_usage_section(tool_usage: &[model::ToolUsageInfo]) -> Option<String> {
    if tool_usage.is_empty() {
        return None;
    }

    let lines = tool_usage
        .iter()
        .map(|tool| {
            let mut line = format!(
                "- {}  {} calls",
                fallback_text(&tool.name, "(unnamed tool)"),
                format_number(tool.call_count),
            );
            if let Some(total_duration_ms) = tool.total_duration_ms {
                line.push_str("  ");
                line.push_str(&format_total_duration_ms(total_duration_ms));
            }
            line
        })
        .collect();

    Some(render_lines_section("Tool Usage", lines, false))
}

fn render_mcp_tool_usage_section(usage: Option<&model::MCPToolUsageData>) -> Option<String> {
    let usage = usage?;
    if usage.tools.is_empty() {
        return None;
    }

    let lines = usage
        .tools
        .iter()
        .map(|tool| {
            format!(
                "- {}  {} calls, {} errors, max input {}, max output {}",
                fallback_text(&tool.name, "(unnamed MCP tool)"),
                format_number(tool.call_count),
                format_number(tool.error_count),
                format_bytes(tool.max_input_size),
                format_bytes(tool.max_output_size),
            )
        })
        .collect();

    Some(render_lines_section("MCP Tool Usage", lines, false))
}

fn render_created_items_section(items: &[model::CreatedItemReport]) -> Option<String> {
    if items.is_empty() {
        return None;
    }

    let lines = items
        .iter()
        .map(|item| {
            let mut parts = vec![fallback_text(&item.kind, "(unknown kind)").to_string()];
            if let Some(id) = item.id.as_deref().filter(|value| !value.is_empty()) {
                parts.push(format_created_item_id(&item.kind, id));
            }
            if let Some(title) = item.title.as_deref().filter(|value| !value.is_empty()) {
                parts.push(format!("\"{}\"", title.replace('"', "\\\"")));
            }
            if let Some(url) = item.url.as_deref().filter(|value| !value.is_empty()) {
                parts.push(url.to_string());
            }
            format!("- {}", parts.join("  "))
        })
        .collect();

    Some(render_lines_section(
        format!("Created Items ({})", format_number(items.len() as u64)),
        lines,
        false,
    ))
}

fn render_kv_section(title: impl Into<String>, rows: Vec<(String, String)>, force: bool) -> String {
    let lines = format_kv_lines(&rows);
    render_lines_section(title, lines, force)
}

fn render_lines_section(title: impl Into<String>, lines: Vec<String>, _force: bool) -> String {
    let title = title.into();
    let mut out = format!("## {title}");
    if !lines.is_empty() {
        out.push('\n');
        out.push_str(&lines.join("\n"));
    }
    out
}

fn format_kv_lines(rows: &[(String, String)]) -> Vec<String> {
    let width = rows
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0);

    rows.iter()
        .map(|(key, value)| {
            let padding = " ".repeat(width.saturating_sub(key.chars().count()) + 1);
            format!("- {}:{}{}", key, padding, value)
        })
        .collect()
}

fn format_finding(finding: &model::Finding) -> String {
    let title = if finding.title.trim().is_empty() {
        fallback_text(&finding.description, "(untitled finding)")
    } else {
        finding.title.as_str()
    };

    let mut summary = format!("- [{}] ", format_severity(finding.severity));
    if !finding.category.trim().is_empty() {
        summary.push_str(&finding.category);
        summary.push_str(": ");
    }
    summary.push_str(title);

    if !finding.description.trim().is_empty() && finding.description != title {
        summary.push('\n');
        summary.push_str("    Description: ");
        summary.push_str(&finding.description);
    }
    if let Some(impact) = finding.impact.as_deref().filter(|value| !value.is_empty()) {
        summary.push('\n');
        summary.push_str("    Impact: ");
        summary.push_str(impact);
    }

    summary
}

fn format_recommendation(recommendation: &model::Recommendation) -> String {
    let action = if recommendation.action.trim().is_empty() {
        fallback_text(&recommendation.reason, "(unspecified action)")
    } else {
        recommendation.action.as_str()
    };

    let mut summary = String::from("- ");
    if !recommendation.priority.trim().is_empty() {
        summary.push('[');
        summary.push_str(&recommendation.priority);
        summary.push_str("] ");
    }
    summary.push_str(action);

    if !recommendation.reason.trim().is_empty() {
        summary.push('\n');
        summary.push_str("    Reason: ");
        summary.push_str(&recommendation.reason);
    }
    if let Some(example) = recommendation
        .example
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        summary.push('\n');
        summary.push_str("    Example: ");
        summary.push_str(example);
    }

    summary
}

fn positive_count_lines(map: &std::collections::BTreeMap<String, u64>) -> Vec<String> {
    let entries: Vec<_> = map.iter().filter(|(_, count)| **count > 0).collect();
    let width = entries
        .iter()
        .map(|(key, _)| key.chars().count() + 2)
        .max()
        .unwrap_or(0);

    entries
        .into_iter()
        .map(|(key, count)| {
            let quoted = format!("\"{}\"", key);
            let padding = " ".repeat(width.saturating_sub(quoted.chars().count()) + 1);
            format!("- {}:{}{}", quoted, padding, format_number(*count))
        })
        .collect()
}

fn format_detection_threats(threats: &model::DetectionThreats) -> String {
    let mut detected = Vec::new();
    if threats.prompt_injection {
        detected.push("prompt_injection");
    }
    if threats.secret_leak {
        detected.push("secret_leak");
    }
    if threats.malicious_patch {
        detected.push("malicious_patch");
    }

    if detected.is_empty() {
        "none".to_string()
    } else {
        detected.join(", ")
    }
}

fn format_named_report(
    primary: Option<&str>,
    context: Option<&str>,
    reason: Option<&str>,
    timestamp: Option<&str>,
    fallback: &str,
) -> String {
    let mut line = format!(
        "- {}",
        primary
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback)
    );
    if let Some(context) = context.filter(|value| !value.is_empty()) {
        line.push_str(" [");
        line.push_str(context);
        line.push(']');
    }
    if let Some(reason) = reason.filter(|value| !value.is_empty()) {
        line.push_str(": ");
        line.push_str(reason);
    }
    if let Some(timestamp) = timestamp.filter(|value| !value.is_empty()) {
        line.push('\n');
        line.push_str("    Timestamp: ");
        line.push_str(timestamp);
    }
    line
}

fn format_message_report(source: &str, message: &str, timestamp: Option<&str>) -> String {
    let mut line = format!(
        "- {}: {}",
        fallback_text(source, "(unknown source)"),
        fallback_text(message, "(no message)"),
    );
    if let Some(timestamp) = timestamp.filter(|value| !value.is_empty()) {
        line.push('\n');
        line.push_str("    Timestamp: ");
        line.push_str(timestamp);
    }
    line
}

fn format_created_item_id(kind: &str, id: &str) -> String {
    match kind {
        "pull_request" | "issue" => format!("#{id}"),
        _ => id.to_string(),
    }
}

fn push_non_empty_row(rows: &mut Vec<(String, String)>, key: &str, value: &str) {
    if !value.trim().is_empty() {
        rows.push((key.to_string(), value.to_string()));
    }
}

fn push_opt_row(rows: &mut Vec<(String, String)>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        rows.push((key.to_string(), value.to_string()));
    }
}

fn push_opt_owned_row(rows: &mut Vec<(String, String)>, key: &str, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        rows.push((key.to_string(), value));
    }
}

fn format_severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "critical",
        Severity::High => "high",
        Severity::Medium => "medium",
        Severity::Low => "low",
        Severity::Info => "info",
    }
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }

    out.chars().rev().collect()
}

fn format_cost(value: f64) -> String {
    format!("${value:.2}")
}

fn format_float(value: f64) -> String {
    let formatted = format!("{value:.2}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn format_percentage(value: f64) -> String {
    format!("{:.2}%", value * 100.0)
}

fn format_bytes(value: u64) -> String {
    format!("{} B", format_number(value))
}

fn format_total_duration_ms(value: u64) -> String {
    if value % 1000 == 0 {
        format_duration_seconds(value / 1000)
    } else {
        format!("{} ms", format_number(value))
    }
}

fn normalize_duration(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut minutes = None;
    let mut seconds = None;
    for part in trimmed.split_whitespace() {
        if let Some(value) = part
            .strip_suffix('m')
            .and_then(|part| part.parse::<u64>().ok())
        {
            minutes = Some(value);
            continue;
        }
        if let Some(value) = part
            .strip_suffix('s')
            .and_then(|part| part.parse::<u64>().ok())
        {
            seconds = Some(value);
        }
    }

    match (minutes, seconds) {
        (Some(minutes), Some(seconds)) => format!("{minutes}m {seconds}s"),
        (Some(minutes), None) => format!("{minutes}m 0s"),
        (None, Some(seconds)) => format!("0m {seconds}s"),
        _ => trimmed.to_string(),
    }
}

fn format_duration_seconds(value: u64) -> String {
    let minutes = value / 60;
    let seconds = value % 60;
    format!("{minutes}m {seconds}s")
}

fn fallback_text<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::render_console;
    use crate::audit::model::{
        AgenticAssessment, AuditData, AuditEngineConfig, AwInfo, BehaviorFingerprint,
        CreatedItemReport, DetectionAnalysis, DetectionThreats, DomainStat, ErrorInfo, FileInfo,
        Finding, FirewallAnalysis, JobData, MCPFailureReport, MCPServerHealth, MCPServerStats,
        MCPToolSummary, MCPToolUsageData, MetricsData, MissingDataReport, MissingToolReport,
        NoopReport, PerformanceMetrics, PolicyAnalysis, PolicyRule, Recommendation,
        RejectedSafeOutputsRollup, SafeOutputExecution, SafeOutputExecutionItem, SafeOutputStatus,
        SafeOutputSummary, Severity, TaskDomainInfo, ToolUsageInfo,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn empty_audit_data_renders_only_overview_and_metrics() {
        let out = render_console(&AuditData::default());
        let headings: Vec<_> = out.lines().filter(|line| line.starts_with("## ")).collect();

        assert!(out.contains("## Overview"));
        assert!(out.contains("## Metrics"));
        assert_eq!(headings, vec!["## Overview", "## Metrics"]);
    }

    #[test]
    fn full_audit_data_renders_inline_snapshot() {
        let audit = populated_audit_data();
        let expected = r#"## Overview
- build_id:         12,345
- pipeline:         my-pipeline
- status:           completed
- result:           succeeded
- branch:           refs/heads/main
- commit:           abcdef123456
- duration:         3m 12s
- url:              https://dev.azure.com/example/project/_build/results?buildId=12345
- created_at:       2026-05-21T12:00:00Z
- started_at:       2026-05-21T12:01:00Z
- finished_at:      2026-05-21T12:04:12Z
- logs_path:        logs\build-12345
- engine:           copilot
- model:            claude-sonnet-4.5
- agent:            my-agent.md
- source:           agents/my-agent.md
- target:           standalone
- compiler_version: 0.30.2
- engine_version:   2026.05
- timeout_minutes:  30

## Metrics
- token_usage:       12,345
- effective_tokens:  12,345
- estimated_cost:    $0.00
- turns:             7
- errors:            1
- warnings:          1
- tokens_per_minute: 64.3
- cost_efficiency:   good
- most_used_tool:    edit
- network_requests:  42

## Key Findings
- [high] safe_outputs: Detection rejected 3 safe output(s)
    Description: The threat-analysis verdict had prompt_injection set.
    Impact: No items were created; the agent's work is not visible to downstream consumers.

## Recommendations
- [high] Review the detection-stage verdict
    Reason: The aggregate gate prevented execution.
    Example: Inspect analyzed_outputs_12345\threat-analysis.json

## Safe Output Summary
- proposed:              5
- executed:              3
- rejected_by_execution: 1
- not_processed:         1

## Rejected Safe Outputs (4 total)
By reason:
- "evil prompt detected": 2
- "permission denied":    1
- "skipped":              1
By threat:
- "prompt_injection": 2

## MCP Server Health
- github-mcp  8 calls, 1 errors (12.50%) [unreliable]

## Firewall Analysis (total: 42 requests, allowed: 40, denied: 2)
- api.github.com [allowed]  35 requests

## Policy Analysis (allow: 1, deny: 1)
- https://api.github.com/** [allow]  35 hits

## Detection Analysis
- safe_to_process: false
- threats:         prompt_injection
- reason:          Suspicious instruction in fetched content
- verdict_path:    analyzed_outputs_12345\threat-analysis.json

## Jobs
- Agent       [completed/succeeded]  2m 30s
- Detection   [completed/succeeded]  0m 30s
- SafeOutputs [completed/succeeded]  0m 12s

## Downloaded Files
- logs\build-12345\agent_outputs_12345\otel.jsonl  2,048 B  sha256: abc123

## Missing Tools
- azure-devops [work-item-sync]: Tool not configured
    Timestamp: 2026-05-21T12:03:00Z

## Missing Data
- create_work_item [wi-1]: missing title
    Timestamp: 2026-05-21T12:03:10Z

## Noops
- noop [noop-1]: Nothing to do
    Timestamp: 2026-05-21T12:03:20Z

## MCP Failures
- github.search_code [call-17]: HTTP 502
    Timestamp: 2026-05-21T12:03:30Z

## Errors
- audit::detection: Threat detection blocked execution
    Timestamp: 2026-05-21T12:05:00Z

## Warnings
- audit::firewall: One request was denied
    Timestamp: 2026-05-21T12:04:00Z

## Tool Usage
- edit  5 calls  0m 2s

## MCP Tool Usage
- github.search_code  3 calls, 1 errors, max input 512 B, max output 4,096 B

## Created Items (1)
- pull_request  #42  "Fix bug"  https://dev.azure.com/example/project/_git/repo/pullrequest/42
"#;

        assert_eq!(render_console(&audit), expected);
    }

    #[test]
    fn rejected_rollup_renders_totals_and_grouping() {
        let mut by_reason = BTreeMap::new();
        by_reason.insert("permission denied".to_string(), 1);

        let mut by_threat = BTreeMap::new();
        by_threat.insert("prompt_injection".to_string(), 2);

        let audit = AuditData {
            rejected_safe_outputs: Some(RejectedSafeOutputsRollup {
                total_rejected: 3,
                by_reason,
                by_threat,
            }),
            ..AuditData::default()
        };

        let out = render_console(&audit);
        assert!(out.contains("## Rejected Safe Outputs (3 total)"));
        assert!(out.contains("By reason:"));
        assert!(out.contains("By threat:"));
    }

    #[test]
    fn headings_follow_documented_order() {
        let out = render_console(&populated_audit_data());
        let headings = [
            "## Overview",
            "## Metrics",
            "## Key Findings",
            "## Recommendations",
            "## Safe Output Summary",
            "## Rejected Safe Outputs (4 total)",
            "## MCP Server Health",
            "## Firewall Analysis (total: 42 requests, allowed: 40, denied: 2)",
            "## Policy Analysis (allow: 1, deny: 1)",
            "## Detection Analysis",
            "## Jobs",
            "## Downloaded Files",
            "## Missing Tools",
            "## Missing Data",
            "## Noops",
            "## MCP Failures",
            "## Errors",
            "## Warnings",
            "## Tool Usage",
            "## MCP Tool Usage",
            "## Created Items (1)",
        ];

        let mut last_index = 0;
        for heading in headings {
            let index = out.find(heading).expect("expected heading to be present");
            assert!(index >= last_index, "heading {heading} was out of order");
            last_index = index;
        }
    }

    fn populated_audit_data() -> AuditData {
        let mut by_reason = BTreeMap::new();
        by_reason.insert("evil prompt detected".to_string(), 2);
        by_reason.insert("permission denied".to_string(), 1);
        by_reason.insert("skipped".to_string(), 1);

        let mut by_threat = BTreeMap::new();
        by_threat.insert("prompt_injection".to_string(), 2);
        by_threat.insert("secret_leak".to_string(), 0);

        AuditData {
            overview: crate::audit::model::OverviewData {
                build_id: 12_345,
                pipeline_name: "my-pipeline".to_string(),
                status: "completed".to_string(),
                result: Some("succeeded".to_string()),
                created_at: Some("2026-05-21T12:00:00Z".to_string()),
                started_at: Some("2026-05-21T12:01:00Z".to_string()),
                finished_at: Some("2026-05-21T12:04:12Z".to_string()),
                duration: Some("3m 12s".to_string()),
                source_branch: Some("refs/heads/main".to_string()),
                source_version: Some("abcdef123456".to_string()),
                url: Some(
                    "https://dev.azure.com/example/project/_build/results?buildId=12345"
                        .to_string(),
                ),
                logs_path: Some("logs\\build-12345".to_string()),
                aw_info: Some(AwInfo {
                    engine: Some("copilot".to_string()),
                    model: Some("claude-sonnet-4.5".to_string()),
                    agent_name: Some("my-agent.md".to_string()),
                    source: Some("agents/my-agent.md".to_string()),
                    target: Some("standalone".to_string()),
                    compiler_version: Some("0.30.2".to_string()),
                }),
            },
            task_domain: Some(TaskDomainInfo {
                summary: "security review workflow".to_string(),
                data: json!({"domain": "security"}),
            }),
            behavior_fingerprint: Some(BehaviorFingerprint {
                summary: "tool-heavy".to_string(),
                data: json!({"pattern": "tool-heavy"}),
            }),
            agentic_assessments: vec![AgenticAssessment {
                summary: "produced actionable changes".to_string(),
                data: json!({"score": 0.92}),
            }],
            metrics: MetricsData {
                token_usage: 12_345,
                effective_tokens: 12_345,
                estimated_cost: 0.0,
                turns: 7,
                error_count: 1,
                warning_count: 1,
            },
            key_findings: vec![Finding {
                category: "safe_outputs".to_string(),
                severity: Severity::High,
                title: "Detection rejected 3 safe output(s)".to_string(),
                description: "The threat-analysis verdict had prompt_injection set.".to_string(),
                impact: Some(
                    "No items were created; the agent's work is not visible to downstream consumers."
                        .to_string(),
                ),
            }],
            recommendations: vec![Recommendation {
                priority: "high".to_string(),
                action: "Review the detection-stage verdict".to_string(),
                reason: "The aggregate gate prevented execution.".to_string(),
                example: Some("Inspect analyzed_outputs_12345\\threat-analysis.json".to_string()),
            }],
            performance_metrics: Some(PerformanceMetrics {
                tokens_per_minute: Some(64.3),
                cost_efficiency: Some("good".to_string()),
                most_used_tool: Some("edit".to_string()),
                network_requests: Some(42),
            }),
            engine_config: Some(AuditEngineConfig {
                engine: "copilot".to_string(),
                model: Some("claude-sonnet-4.5".to_string()),
                version: Some("2026.05".to_string()),
                timeout_minutes: Some(30),
            }),
            safe_output_summary: Some(SafeOutputSummary {
                proposed_count: 5,
                executed_count: 3,
                rejected_by_execution_count: 1,
                not_processed_count: 1,
            }),
            safe_output_execution: Some(SafeOutputExecution {
                items: vec![SafeOutputExecutionItem {
                    context: Some("pr-1".to_string()),
                    tool: "create_pull_request".to_string(),
                    status: SafeOutputStatus::NotProcessedDueToAggregateGate,
                    proposal: json!({"title": "Fix bug"}),
                    error: Some("Blocked by detection gate".to_string()),
                    result: Some(json!({"status": "blocked"})),
                    rejection_reason: Some("prompt_injection".to_string()),
                    applies_to_whole_batch: true,
                }],
            }),
            rejected_safe_outputs: Some(RejectedSafeOutputsRollup {
                total_rejected: 4,
                by_reason,
                by_threat,
            }),
            detection_analysis: Some(DetectionAnalysis {
                threats: DetectionThreats {
                    prompt_injection: true,
                    secret_leak: false,
                    malicious_patch: false,
                },
                reasons: vec!["Suspicious instruction in fetched content".to_string()],
                safe_to_process: false,
                verdict_path: Some("analyzed_outputs_12345\\threat-analysis.json".to_string()),
            }),
            mcp_server_health: Some(MCPServerHealth {
                servers: vec![MCPServerStats {
                    name: "github-mcp".to_string(),
                    total_calls: 8,
                    error_count: 1,
                    error_rate: 0.125,
                    unreliable: true,
                }],
            }),
            jobs: vec![
                JobData {
                    name: "Agent".to_string(),
                    status: "completed".to_string(),
                    result: Some("succeeded".to_string()),
                    duration: Some("2m 30s".to_string()),
                    started_at: Some("2026-05-21T12:01:00Z".to_string()),
                    finished_at: Some("2026-05-21T12:03:30Z".to_string()),
                },
                JobData {
                    name: "Detection".to_string(),
                    status: "completed".to_string(),
                    result: Some("succeeded".to_string()),
                    duration: Some("30s".to_string()),
                    started_at: Some("2026-05-21T12:03:30Z".to_string()),
                    finished_at: Some("2026-05-21T12:04:00Z".to_string()),
                },
                JobData {
                    name: "SafeOutputs".to_string(),
                    status: "completed".to_string(),
                    result: Some("succeeded".to_string()),
                    duration: Some("12s".to_string()),
                    started_at: Some("2026-05-21T12:04:00Z".to_string()),
                    finished_at: Some("2026-05-21T12:04:12Z".to_string()),
                },
            ],
            downloaded_files: vec![FileInfo {
                path: "logs\\build-12345\\agent_outputs_12345\\otel.jsonl".to_string(),
                size_bytes: 2_048,
                sha256: Some("abc123".to_string()),
            }],
            missing_tools: vec![MissingToolReport {
                tool: Some("azure-devops".to_string()),
                context: Some("work-item-sync".to_string()),
                reason: Some("Tool not configured".to_string()),
                timestamp: Some("2026-05-21T12:03:00Z".to_string()),
                extra: json!({"required": true}),
            }],
            missing_data: vec![MissingDataReport {
                tool: Some("create_work_item".to_string()),
                context: Some("wi-1".to_string()),
                reason: Some("missing title".to_string()),
                timestamp: Some("2026-05-21T12:03:10Z".to_string()),
                extra: json!({"field": "title"}),
            }],
            noops: vec![NoopReport {
                tool: Some("noop".to_string()),
                context: Some("noop-1".to_string()),
                reason: Some("Nothing to do".to_string()),
                timestamp: Some("2026-05-21T12:03:20Z".to_string()),
                extra: json!({"kind": "noop"}),
            }],
            mcp_failures: vec![MCPFailureReport {
                tool: Some("github.search_code".to_string()),
                context: Some("call-17".to_string()),
                reason: Some("HTTP 502".to_string()),
                timestamp: Some("2026-05-21T12:03:30Z".to_string()),
                extra: json!({"retryable": true}),
            }],
            firewall_analysis: Some(FirewallAnalysis {
                domains: vec![DomainStat {
                    domain: "api.github.com".to_string(),
                    status: "allowed".to_string(),
                    request_count: 35,
                    first_seen: Some("2026-05-21T12:01:10Z".to_string()),
                    last_seen: Some("2026-05-21T12:04:10Z".to_string()),
                }],
                total_requests: 42,
                allowed_count: 40,
                denied_count: 2,
            }),
            policy_analysis: Some(PolicyAnalysis {
                policies: vec![PolicyRule {
                    pattern: "https://api.github.com/**".to_string(),
                    verdict: "allow".to_string(),
                    hit_count: 35,
                }],
                allow_count: 1,
                deny_count: 1,
            }),
            errors: vec![ErrorInfo {
                source: "audit::detection".to_string(),
                message: "Threat detection blocked execution".to_string(),
                timestamp: Some("2026-05-21T12:05:00Z".to_string()),
            }],
            warnings: vec![ErrorInfo {
                source: "audit::firewall".to_string(),
                message: "One request was denied".to_string(),
                timestamp: Some("2026-05-21T12:04:00Z".to_string()),
            }],
            tool_usage: vec![ToolUsageInfo {
                name: "edit".to_string(),
                call_count: 5,
                total_duration_ms: Some(2_000),
            }],
            mcp_tool_usage: Some(MCPToolUsageData {
                tools: vec![MCPToolSummary {
                    name: "github.search_code".to_string(),
                    call_count: 3,
                    error_count: 1,
                    max_input_size: 512,
                    max_output_size: 4_096,
                }],
            }),
            created_items: vec![CreatedItemReport {
                kind: "pull_request".to_string(),
                url: Some(
                    "https://dev.azure.com/example/project/_git/repo/pullrequest/42"
                        .to_string(),
                ),
                id: Some("42".to_string()),
                title: Some("Fix bug".to_string()),
            }],
        }
    }
}
