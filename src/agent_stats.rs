//! Agent statistics extracted from Copilot CLI OpenTelemetry output.
//!
//! The Copilot CLI can write OTel spans and metrics to a JSONL file via
//! `COPILOT_OTEL_FILE_EXPORTER_PATH`. This module parses that file to
//! extract agent execution statistics (token usage, duration, model,
//! tool calls, turns) for inclusion in safe output write actions.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Agent execution statistics parsed from OTel JSONL.
#[derive(Debug, Clone)]
pub struct AgentStats {
    /// Agent name from front matter.
    pub agent_name: String,
    /// AI model used (e.g., "claude-sonnet-4.5").
    pub model: Option<String>,
    /// Total input tokens across all LLM calls.
    pub input_tokens: u64,
    /// Total output tokens across all LLM calls.
    pub output_tokens: u64,
    /// Wall-clock duration in seconds.
    pub duration_seconds: f64,
    /// Number of tool invocations.
    pub tool_calls: u64,
    /// Number of LLM round-trips (turns).
    pub turns: u64,
}

/// OTel JSONL filename written by Copilot CLI.
pub const OTEL_FILENAME: &str = "otel.jsonl";

/// Copilot CLI internal tool names excluded from the tool call count.
/// These are administrative spans, not user-visible tool invocations.
/// Names must include the "execute_tool " prefix as emitted in the OTel span name.
const INTERNAL_TOOL_NAMES: &[&str] = &[
    "execute_tool report_intent",
];

impl AgentStats {
    /// Parse agent stats from an OTel JSONL file.
    ///
    /// Uses [`crate::ndjson::read_ndjson_file`] for file I/O, then
    /// extracts stats from the parsed entries.
    pub async fn from_otel_file(path: &Path, agent_name: &str) -> Result<Self> {
        let entries = crate::ndjson::read_ndjson_file(path)
            .await
            .with_context(|| format!("Failed to read OTel file: {}", path.display()))?;
        Self::from_otel_entries(&entries, agent_name)
    }

    /// Extract stats from pre-parsed OTel JSONL entries.
    ///
    /// Looks for:
    /// - The last `invoke_agent` span for aggregated tokens, model, turns, duration
    /// - `execute_tool` spans for tool call count (excluding internal tools)
    pub fn from_otel_entries(entries: &[Value], agent_name: &str) -> Result<Self> {
        let mut stats = AgentStats {
            agent_name: agent_name.to_string(),
            model: None,
            input_tokens: 0,
            output_tokens: 0,
            duration_seconds: 0.0,
            tool_calls: 0,
            turns: 0,
        };

        // Find the last invoke_agent span (contains aggregated totals)
        let last_agent_span = entries
            .iter()
            .filter(|e| {
                e.get("type").and_then(|t| t.as_str()) == Some("span")
                    && e.get("name").and_then(|n| n.as_str()) == Some("invoke_agent")
            })
            .last();

        if let Some(span) = last_agent_span {
            let attrs = span.get("attributes").cloned().unwrap_or(Value::Null);

            // Model
            stats.model = attrs
                .get("gen_ai.request.model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Tokens (aggregated across all chat spans)
            stats.input_tokens = attrs
                .get("gen_ai.usage.input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            stats.output_tokens = attrs
                .get("gen_ai.usage.output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // Turns
            stats.turns = attrs
                .get("github.copilot.turn_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // Duration from startTime/endTime ([seconds, nanoseconds] arrays)
            stats.duration_seconds = compute_duration(span);
        }

        // Count execute_tool spans, excluding internal Copilot CLI tools
        stats.tool_calls = entries
            .iter()
            .filter(|e| {
                e.get("type").and_then(|t| t.as_str()) == Some("span")
                    && e.get("name")
                        .and_then(|n| n.as_str())
                        .is_some_and(|n| {
                            n.starts_with("execute_tool")
                                && !INTERNAL_TOOL_NAMES.contains(&n)
                        })
            })
            .count() as u64;

        Ok(stats)
    }

    /// Render as a compact markdown stats line.
    ///
    /// Uses middle-dot separators for a lightweight single-line format
    /// that works across all ADO markdown surfaces.
    pub fn to_markdown(&self) -> String {
        let duration = format_duration(self.duration_seconds);
        let model = sanitize_for_markdown(
            self.model.as_deref().unwrap_or("unknown"),
        );
        let name = sanitize_for_markdown(&self.agent_name);

        format!(
            "\n---\n\
             \u{1F916} {name} \u{00B7} {model} \u{00B7} \
             {input} in / {output} out \u{00B7} \
             {tools} tool calls \u{00B7} {duration}\n",
            name = name,
            model = model,
            input = format_number(self.input_tokens),
            output = format_number(self.output_tokens),
            tools = self.tool_calls,
            duration = duration,
        )
    }
}

/// Default value for `include_stats` serde fields (true).
///
/// Used by safe output config structs via `#[serde(default = "...")]`.
pub(crate) fn default_include_stats() -> bool {
    true
}

/// Sanitize a string for safe embedding in a single-line markdown format.
///
/// Strips control characters (including newlines — the stats line is
/// single-line), neutralizes ADO pipeline commands (`##vso[`), and
/// escapes pipe characters that break markdown tables.
fn sanitize_for_markdown(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .replace("##vso[", "[vso-filtered][")
        .replace("##[", "[filtered][")
        .replace('|', "\\|")
}

/// Append agent stats markdown to a body string if stats are available
/// and stats are not opted out.
///
/// Used by safe output executors after they read their typed config
/// (which contains the `include_stats` field).
pub fn append_stats_to_body(
    body: &str,
    ctx: &crate::safeoutputs::ExecutionContext,
    include_stats: bool,
) -> String {
    if !include_stats {
        return body.to_string();
    }

    match &ctx.agent_stats {
        Some(stats) => format!("{}{}", body, stats.to_markdown()),
        None => body.to_string(),
    }
}

/// Compute the wall-clock duration of a span in seconds.
///
/// Times are `[seconds, nanoseconds]` arrays.
fn compute_duration(span: &Value) -> f64 {
    let start = parse_otel_time(span.get("startTime"));
    let end = parse_otel_time(span.get("endTime"));
    match (start, end) {
        (Some(s), Some(e)) => (e - s).max(0.0),
        _ => 0.0,
    }
}

/// Parse an OTel `[seconds, nanoseconds]` time array into seconds.
fn parse_otel_time(value: Option<&Value>) -> Option<f64> {
    let arr = value?.as_array()?;
    let secs = arr.first()?.as_f64()?;
    let nanos = arr.get(1)?.as_f64().unwrap_or(0.0);
    Some(secs + nanos / 1_000_000_000.0)
}

/// Format seconds as human-readable duration (e.g., "4m 32s").
fn format_duration(seconds: f64) -> String {
    let total_secs = seconds.round() as u64;
    if total_secs < 60 {
        format!("{}s", total_secs)
    } else if total_secs < 3600 {
        format!("{}m {}s", total_secs / 60, total_secs % 60)
    } else {
        format!(
            "{}h {}m {}s",
            total_secs / 3600,
            (total_secs % 3600) / 60,
            total_secs % 60
        )
    }
}

/// Format a number with comma separators (e.g., 45230 → "45,230").
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(0.0), "0s");
        assert_eq!(format_duration(45.0), "45s");
        assert_eq!(format_duration(59.4), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(60.0), "1m 0s");
        assert_eq!(format_duration(272.0), "4m 32s");
        assert_eq!(format_duration(3599.0), "59m 59s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3600.0), "1h 0m 0s");
        assert_eq!(format_duration(7384.0), "2h 3m 4s");
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(45230), "45,230");
        assert_eq!(format_number(1234567), "1,234,567");
    }

    #[test]
    fn test_from_otel_entries_empty() {
        let stats = AgentStats::from_otel_entries(&[], "test-agent").unwrap();
        assert_eq!(stats.agent_name, "test-agent");
        assert_eq!(stats.input_tokens, 0);
        assert_eq!(stats.output_tokens, 0);
        assert_eq!(stats.tool_calls, 0);
        assert_eq!(stats.turns, 0);
        assert!(stats.model.is_none());
    }

    #[test]
    fn test_from_otel_entries_real_fixture() {
        let content = include_str!("../tests/fixtures/copilot-otel.jsonl");
        let entries = crate::ndjson::parse_ndjson(content).unwrap();
        let stats = AgentStats::from_otel_entries(&entries, "test-agent").unwrap();

        assert_eq!(stats.model.as_deref(), Some("claude-sonnet-4.5"));
        assert_eq!(stats.input_tokens, 32949);
        assert_eq!(stats.output_tokens, 236);
        assert_eq!(stats.input_tokens + stats.output_tokens, 33185);
        assert_eq!(stats.turns, 2);
        // execute_tool spans: bash only (report_intent is filtered as internal)
        assert_eq!(stats.tool_calls, 1);
        // Duration should be ~8 seconds (from the last invoke_agent span)
        assert!(stats.duration_seconds > 7.0 && stats.duration_seconds < 10.0);
    }

    #[test]
    fn test_to_markdown_contains_key_elements() {
        let stats = AgentStats {
            agent_name: "Daily Code Review".to_string(),
            model: Some("claude-opus-4.5".to_string()),
            input_tokens: 45230,
            output_tokens: 12450,
            duration_seconds: 272.0,
            tool_calls: 23,
            turns: 8,
        };
        let md = stats.to_markdown();
        assert!(md.contains("Daily Code Review"));
        assert!(md.contains("claude-opus-4.5"));
        assert!(md.contains("45,230 in"));
        assert!(md.contains("12,450 out"));
        assert!(md.contains("23 tool calls"));
        assert!(md.contains("4m 32s"));
        assert!(md.contains("\u{00B7}"), "should use middle-dot separators");
        assert!(!md.contains("turns"), "turns should not be in output");
    }

    #[test]
    fn test_parse_otel_time() {
        // [1776287701, 726000000] = epoch seconds + nanoseconds
        let val = serde_json::json!([1776287701, 726000000]);
        let t = parse_otel_time(Some(&val)).unwrap();
        assert!((t - 1776287701.726).abs() < 0.001);
    }

    #[test]
    fn test_compute_duration_from_span() {
        let span = serde_json::json!({
            "startTime": [1776287701, 726000000],
            "endTime": [1776287710, 8631000]
        });
        let d = compute_duration(&span);
        assert!((d - 8.282631).abs() < 0.01);
    }

    #[test]
    fn test_sanitize_for_markdown_strips_vso_commands() {
        assert_eq!(
            sanitize_for_markdown("normal text"),
            "normal text"
        );
        assert_eq!(
            sanitize_for_markdown("##vso[task.setvariable]evil"),
            "[vso-filtered][task.setvariable]evil"
        );
        assert_eq!(
            sanitize_for_markdown("model|name"),
            "model\\|name"
        );
    }

    #[test]
    fn test_sanitize_for_markdown_strips_shorthand_pipeline_command() {
        assert_eq!(
            sanitize_for_markdown("##[error]Something bad"),
            "[filtered][error]Something bad"
        );
    }

    #[test]
    fn test_internal_tools_excluded_from_count() {
        let entries = vec![
            serde_json::json!({"type": "span", "name": "execute_tool report_intent"}),
            serde_json::json!({"type": "span", "name": "execute_tool bash"}),
            serde_json::json!({"type": "span", "name": "execute_tool grep"}),
            // "permission" span has no "execute_tool" prefix so is already excluded
            serde_json::json!({"type": "span", "name": "permission"}),
        ];
        let stats = AgentStats::from_otel_entries(&entries, "test").unwrap();
        assert_eq!(stats.tool_calls, 2); // bash + grep only
    }

    #[test]
    fn test_append_stats_to_body_opt_out() {
        let mut ctx = crate::safeoutputs::ExecutionContext::default();
        ctx.agent_stats = Some(AgentStats {
            agent_name: "test".to_string(),
            model: Some("model".to_string()),
            input_tokens: 100,
            output_tokens: 50,
            duration_seconds: 10.0,
            tool_calls: 1,
            turns: 1,
        });
        assert_eq!(append_stats_to_body("body", &ctx, false), "body");
    }

    #[test]
    fn test_append_stats_to_body_no_stats() {
        let ctx = crate::safeoutputs::ExecutionContext::default(); // agent_stats: None
        assert_eq!(append_stats_to_body("body", &ctx, true), "body");
    }

    #[test]
    fn test_append_stats_to_body_with_stats() {
        let mut ctx = crate::safeoutputs::ExecutionContext::default();
        ctx.agent_stats = Some(AgentStats {
            agent_name: "test".to_string(),
            model: Some("model".to_string()),
            input_tokens: 100,
            output_tokens: 50,
            duration_seconds: 10.0,
            tool_calls: 1,
            turns: 1,
        });
        let result = append_stats_to_body("body", &ctx, true);
        assert!(result.starts_with("body"));
        assert!(result.contains("test"));
        assert!(result.contains("model"));
    }
}
