//! MCP gateway log analyzer for `ado-aw audit`.

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::audit::model::{
    MCPFailureReport, MCPServerHealth, MCPServerStats, MCPToolSummary, MCPToolUsageData,
};

#[derive(Debug, Default)]
struct ToolAccumulator {
    call_count: u64,
    error_count: u64,
    max_input_size: u64,
    max_output_size: u64,
}

#[derive(Debug, Default)]
struct AnalyzeAllResult {
    tool_usage: Option<MCPToolUsageData>,
    server_health: Option<MCPServerHealth>,
    failures: Vec<MCPFailureReport>,
}

/// Aggregate per-tool MCP usage from gateway logs.
pub async fn analyze_mcp_tool_usage(
    mcpg_logs_dir: &std::path::Path,
) -> anyhow::Result<Option<crate::audit::model::MCPToolUsageData>> {
    Ok(analyze_all(mcpg_logs_dir).await?.tool_usage)
}

/// Aggregate per-server MCP health from gateway logs.
pub async fn analyze_mcp_server_health(
    mcpg_logs_dir: &std::path::Path,
) -> anyhow::Result<Option<crate::audit::model::MCPServerHealth>> {
    Ok(analyze_all(mcpg_logs_dir).await?.server_health)
}

/// Extract MCP failure reports (tool_error events) from gateway logs.
pub async fn extract_mcp_failures(
    mcpg_logs_dir: &std::path::Path,
) -> anyhow::Result<Vec<crate::audit::model::MCPFailureReport>> {
    Ok(analyze_all(mcpg_logs_dir).await?.failures)
}

/// Accumulated state built from MCP gateway log events.
#[derive(Default)]
struct EventAccumulators {
    per_tool: BTreeMap<(String, String), ToolAccumulator>,
    observed_servers: BTreeSet<String>,
    server_error_counts: BTreeMap<String, u64>,
    failures: Vec<MCPFailureReport>,
    saw_recognizable_event: bool,
}

impl EventAccumulators {
    fn on_tool_call(&mut self, value: &Value) {
        self.saw_recognizable_event = true;
        let server =
            extract_string_field(value, &["server", "mcp_server", "provider"]).unwrap_or_default();
        let tool = extract_string_field(value, &["tool", "name"]);
        if !server.is_empty() {
            self.observed_servers.insert(server.clone());
        }
        if let Some(tool) = tool.filter(|t| !t.is_empty()) {
            let entry = self.per_tool.entry((server, tool)).or_default();
            update_tool_sizes(entry, value);
            entry.call_count += 1;
        }
    }

    fn on_tool_error(&mut self, value: Value) {
        self.saw_recognizable_event = true;
        let server =
            extract_string_field(&value, &["server", "mcp_server", "provider"]).unwrap_or_default();
        let tool = extract_string_field(&value, &["tool", "name"]);
        if !server.is_empty() {
            self.observed_servers.insert(server.clone());
        }
        if let Some(tool_name) = tool.clone().filter(|t| !t.is_empty()) {
            let entry = self.per_tool.entry((server, tool_name)).or_default();
            update_tool_sizes(entry, &value);
            entry.error_count += 1;
        }
        self.failures.push(MCPFailureReport {
            tool: tool.filter(|t| !t.is_empty()),
            context: None,
            reason: extract_stringish_field(&value, &["error"]),
            timestamp: extract_string_field(&value, &["ts", "time", "timestamp", "@timestamp"]),
            extra: value,
        });
    }

    fn on_server_error(&mut self, value: Value) {
        self.saw_recognizable_event = true;
        let server =
            extract_string_field(&value, &["server", "mcp_server", "provider"]).unwrap_or_default();
        if !server.is_empty() {
            self.observed_servers.insert(server.clone());
            *self.server_error_counts.entry(server).or_default() += 1;
        }
        self.failures.push(MCPFailureReport {
            tool: None,
            context: None,
            reason: extract_stringish_field(&value, &["error"]),
            timestamp: extract_string_field(&value, &["ts", "time", "timestamp", "@timestamp"]),
            extra: value,
        });
    }

    fn on_server_lifecycle(&mut self, value: &Value) {
        self.saw_recognizable_event = true;
        if let Some(server) = extract_string_field(value, &["server", "mcp_server", "provider"]) {
            self.observed_servers.insert(server);
        }
    }

    fn process_event(&mut self, event_kind: &str, value: Value) {
        match event_kind {
            "tool_call" => self.on_tool_call(&value),
            "tool_error" => self.on_tool_error(value),
            "server_error" => self.on_server_error(value),
            "server_start" | "server_stop" => self.on_server_lifecycle(&value),
            _ => {}
        }
    }
}

async fn analyze_all(mcpg_logs_dir: &Path) -> Result<AnalyzeAllResult> {
    if !ensure_mcpg_logs_dir_exists(mcpg_logs_dir).await? {
        return Ok(AnalyzeAllResult::default());
    }
    let file_paths = read_log_file_paths(mcpg_logs_dir).await?;
    let mut acc = EventAccumulators::default();
    process_log_files(file_paths, &mut acc).await?;
    if !acc.saw_recognizable_event {
        return Ok(AnalyzeAllResult::default());
    }
    Ok(AnalyzeAllResult {
        tool_usage: Some(MCPToolUsageData {
            tools: build_tool_summaries(&acc.per_tool),
        }),
        server_health: Some(MCPServerHealth {
            servers: build_server_health_list(
                acc.observed_servers,
                &acc.per_tool,
                acc.server_error_counts,
            ),
        }),
        failures: acc.failures,
    })
}

/// Returns `true` if the directory exists and is a directory, `false` if not found.
/// Errors on other OS errors or if the path is not a directory.
async fn ensure_mcpg_logs_dir_exists(dir: &Path) -> Result<bool> {
    match tokio::fs::metadata(dir).await {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_dir(),
                "MCPG logs path is not a directory: {}",
                dir.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("Failed to stat {}", dir.display())),
    }
}

/// Reads every log file in `file_paths` and dispatches each event line to `acc`.
async fn process_log_files(file_paths: Vec<PathBuf>, acc: &mut EventAccumulators) -> Result<()> {
    for path in file_paths {
        let contents = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read MCP gateway log {}", path.display()))?;
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(trimmed) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let Some(event_kind) = extract_string_field(&value, &["event", "kind", "type"])
                .map(|kind| kind.to_ascii_lowercase())
            else {
                continue;
            };
            acc.process_event(&event_kind, value);
        }
    }
    Ok(())
}

fn build_tool_summaries(
    per_tool: &BTreeMap<(String, String), ToolAccumulator>,
) -> Vec<MCPToolSummary> {
    let mut tools: Vec<MCPToolSummary> = per_tool
        .iter()
        .map(|((server, tool), stats)| MCPToolSummary {
            name: format_tool_name(server, tool),
            call_count: stats.call_count,
            error_count: stats.error_count,
            max_input_size: stats.max_input_size,
            max_output_size: stats.max_output_size,
        })
        .collect();
    tools.sort_by(|left, right| {
        right
            .call_count
            .cmp(&left.call_count)
            .then_with(|| left.name.cmp(&right.name))
    });
    tools
}

fn build_server_health_list(
    observed_servers: BTreeSet<String>,
    per_tool: &BTreeMap<(String, String), ToolAccumulator>,
    server_error_counts: BTreeMap<String, u64>,
) -> Vec<MCPServerStats> {
    let mut server_rollups = BTreeMap::<String, MCPServerStats>::new();
    for server in observed_servers {
        server_rollups.insert(
            server.clone(),
            MCPServerStats {
                name: server,
                ..MCPServerStats::default()
            },
        );
    }
    for ((server, _tool), stats) in per_tool {
        if server.is_empty() {
            continue;
        }
        let entry = server_rollups
            .entry(server.clone())
            .or_insert_with(|| MCPServerStats {
                name: server.clone(),
                ..MCPServerStats::default()
            });
        entry.total_calls += stats.call_count;
        entry.error_count += stats.error_count;
    }
    for (server, error_count) in server_error_counts {
        let entry = server_rollups
            .entry(server.clone())
            .or_insert_with(|| MCPServerStats {
                name: server,
                ..MCPServerStats::default()
            });
        entry.error_count += error_count;
    }
    let mut servers: Vec<MCPServerStats> = server_rollups
        .into_values()
        .map(|mut stats| {
            stats.error_rate = if stats.total_calls == 0 {
                0.0
            } else {
                stats.error_count as f64 / stats.total_calls as f64
            };
            stats.unreliable = stats.error_rate > 0.10 && stats.total_calls >= 5;
            stats
        })
        .collect();
    servers.sort_by(|left, right| {
        right
            .total_calls
            .cmp(&left.total_calls)
            .then_with(|| left.name.cmp(&right.name))
    });
    servers
}

async fn read_log_file_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("Failed to read {}", dir.display()))?;
    let mut paths = Vec::new();

    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("Failed to iterate {}", dir.display()))?
    {
        let file_type = entry
            .file_type()
            .await
            .with_context(|| format!("Failed to inspect {}", entry.path().display()))?;
        if !file_type.is_file() {
            continue;
        }

        let path = entry.path();
        if is_mcp_log_file(&path) {
            paths.push(path);
        }
    }

    paths.sort();
    Ok(paths)
}

fn is_mcp_log_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            extension.eq_ignore_ascii_case("jsonl") || extension.eq_ignore_ascii_case("log")
        })
        .unwrap_or(false)
}

fn format_tool_name(server: &str, tool: &str) -> String {
    if server.is_empty() {
        tool.to_string()
    } else {
        format!("{server}.{tool}")
    }
}

fn update_tool_sizes(stats: &mut ToolAccumulator, value: &Value) {
    stats.max_input_size = stats
        .max_input_size
        .max(extract_u64_field(value, &["input_size"]).unwrap_or(0));
    stats.max_output_size = stats
        .max_output_size
        .max(extract_u64_field(value, &["output_size"]).unwrap_or(0));
}

fn extract_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|candidate| !candidate.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_stringish_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(value_to_string)
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        other => serde_json::to_string(other)
            .ok()
            .filter(|value| !value.is_empty()),
    }
}

fn extract_u64_field(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(|candidate| match candidate {
            Value::Number(number) => number.as_u64().or_else(|| {
                number
                    .as_i64()
                    .and_then(|value| (value >= 0).then_some(value as u64))
            }),
            Value::String(value) => value.trim().parse::<u64>().ok(),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn write_log_file(dir: &Path, name: &str, contents: &str) {
        tokio::fs::create_dir_all(dir).await.unwrap();
        tokio::fs::write(dir.join(name), contents).await.unwrap();
    }

    #[tokio::test]
    async fn absent_directory_returns_none_or_empty() {
        let temp_dir = TempDir::new().unwrap();
        let mcpg_dir = temp_dir.path().join("logs").join("mcpg");

        assert_eq!(analyze_mcp_tool_usage(&mcpg_dir).await.unwrap(), None);
        assert_eq!(analyze_mcp_server_health(&mcpg_dir).await.unwrap(), None);
        assert_eq!(extract_mcp_failures(&mcpg_dir).await.unwrap(), Vec::new());
    }

    #[tokio::test]
    async fn empty_directory_returns_none_or_empty() {
        let temp_dir = TempDir::new().unwrap();
        let mcpg_dir = temp_dir.path().join("logs").join("mcpg");
        tokio::fs::create_dir_all(&mcpg_dir).await.unwrap();

        assert_eq!(analyze_mcp_tool_usage(&mcpg_dir).await.unwrap(), None);
        assert_eq!(analyze_mcp_server_health(&mcpg_dir).await.unwrap(), None);
        assert_eq!(extract_mcp_failures(&mcpg_dir).await.unwrap(), Vec::new());
    }

    #[tokio::test]
    async fn aggregates_two_tool_calls_for_one_tool() {
        let temp_dir = TempDir::new().unwrap();
        let mcpg_dir = temp_dir.path().join("logs").join("mcpg");
        write_log_file(
            &mcpg_dir,
            "mcpg.jsonl",
            concat!(
                "{\"ts\":\"2026-01-01T00:00:00Z\",\"server\":\"github\",\"tool\":\"create_issue\",\"event\":\"tool_call\",\"input_size\":100,\"output_size\":200}\n",
                "{\"ts\":\"2026-01-01T00:00:01Z\",\"server\":\"github\",\"tool\":\"create_issue\",\"event\":\"tool_call\",\"input_size\":500,\"output_size\":1000}\n"
            ),
        )
        .await;

        assert_eq!(
            analyze_mcp_tool_usage(&mcpg_dir).await.unwrap(),
            Some(MCPToolUsageData {
                tools: vec![MCPToolSummary {
                    name: "github.create_issue".to_string(),
                    call_count: 2,
                    error_count: 0,
                    max_input_size: 500,
                    max_output_size: 1000,
                }],
            })
        );
    }

    #[tokio::test]
    async fn server_health_aggregates_tool_and_server_errors() {
        let temp_dir = TempDir::new().unwrap();
        let mcpg_dir = temp_dir.path().join("logs").join("mcpg");
        write_log_file(
            &mcpg_dir,
            "health.jsonl",
            concat!(
                "{\"ts\":\"2026-01-01T00:00:00Z\",\"server\":\"github\",\"tool\":\"create_issue\",\"event\":\"tool_call\"}\n",
                "{\"ts\":\"2026-01-01T00:00:01Z\",\"server\":\"github\",\"tool\":\"create_issue\",\"event\":\"tool_call\"}\n",
                "{\"ts\":\"2026-01-01T00:00:02Z\",\"server\":\"github\",\"tool\":\"create_issue\",\"event\":\"tool_error\",\"error\":\"rate-limit exceeded\"}\n",
                "{\"ts\":\"2026-01-01T00:00:03Z\",\"server\":\"github\",\"event\":\"server_error\",\"error\":\"gateway restart\"}\n"
            ),
        )
        .await;

        assert_eq!(
            analyze_mcp_server_health(&mcpg_dir).await.unwrap(),
            Some(MCPServerHealth {
                servers: vec![MCPServerStats {
                    name: "github".to_string(),
                    total_calls: 2,
                    error_count: 2,
                    error_rate: 1.0,
                    unreliable: false,
                }],
            })
        );

        assert_eq!(
            extract_mcp_failures(&mcpg_dir).await.unwrap(),
            vec![
                MCPFailureReport {
                    tool: Some("create_issue".to_string()),
                    context: None,
                    reason: Some("rate-limit exceeded".to_string()),
                    timestamp: Some("2026-01-01T00:00:02Z".to_string()),
                    extra: serde_json::json!({
                        "ts": "2026-01-01T00:00:02Z",
                        "server": "github",
                        "tool": "create_issue",
                        "event": "tool_error",
                        "error": "rate-limit exceeded"
                    }),
                },
                MCPFailureReport {
                    tool: None,
                    context: None,
                    reason: Some("gateway restart".to_string()),
                    timestamp: Some("2026-01-01T00:00:03Z".to_string()),
                    extra: serde_json::json!({
                        "ts": "2026-01-01T00:00:03Z",
                        "server": "github",
                        "event": "server_error",
                        "error": "gateway restart"
                    }),
                },
            ]
        );
    }

    #[tokio::test]
    async fn unreliable_flag_respects_rate_and_sample_size() {
        let temp_dir = TempDir::new().unwrap();
        let mcpg_dir = temp_dir.path().join("logs").join("mcpg");
        let mut contents = String::new();

        for index in 0..10 {
            contents.push_str(&format!(
                "{{\"ts\":\"2026-01-01T00:00:{index:02}Z\",\"server\":\"azdo\",\"tool\":\"queue_build\",\"event\":\"tool_call\"}}\n"
            ));
        }
        for index in 10..20 {
            contents.push_str(&format!(
                "{{\"ts\":\"2026-01-01T00:00:{index:02}Z\",\"server\":\"github\",\"tool\":\"create_issue\",\"event\":\"tool_call\"}}\n"
            ));
        }
        for index in 20..22 {
            contents.push_str(&format!(
                "{{\"ts\":\"2026-01-01T00:00:{index:02}Z\",\"server\":\"github\",\"tool\":\"create_issue\",\"event\":\"tool_error\",\"error\":\"boom\"}}\n"
            ));
        }
        for index in 22..25 {
            contents.push_str(&format!(
                "{{\"ts\":\"2026-01-01T00:00:{index:02}Z\",\"server\":\"local\",\"tool\":\"echo\",\"event\":\"tool_call\"}}\n"
            ));
        }
        for index in 25..27 {
            contents.push_str(&format!(
                "{{\"ts\":\"2026-01-01T00:00:{index:02}Z\",\"server\":\"local\",\"tool\":\"echo\",\"event\":\"tool_error\",\"error\":\"boom\"}}\n"
            ));
        }

        write_log_file(&mcpg_dir, "unreliable.jsonl", &contents).await;

        assert_eq!(
            analyze_mcp_server_health(&mcpg_dir).await.unwrap(),
            Some(MCPServerHealth {
                servers: vec![
                    MCPServerStats {
                        name: "azdo".to_string(),
                        total_calls: 10,
                        error_count: 0,
                        error_rate: 0.0,
                        unreliable: false,
                    },
                    MCPServerStats {
                        name: "github".to_string(),
                        total_calls: 10,
                        error_count: 2,
                        error_rate: 0.2,
                        unreliable: true,
                    },
                    MCPServerStats {
                        name: "local".to_string(),
                        total_calls: 3,
                        error_count: 2,
                        error_rate: 2.0 / 3.0,
                        unreliable: false,
                    },
                ],
            })
        );
    }

    #[tokio::test]
    async fn supports_field_name_fallbacks() {
        let temp_dir = TempDir::new().unwrap();
        let mcpg_dir = temp_dir.path().join("logs").join("mcpg");
        write_log_file(
            &mcpg_dir,
            "fallbacks.log",
            "{\"time\":\"2026-01-01T00:00:05Z\",\"mcp_server\":\"github\",\"name\":\"search_code\",\"kind\":\"tool_error\",\"error\":\"oops\"}\n",
        )
        .await;

        assert_eq!(
            analyze_mcp_tool_usage(&mcpg_dir).await.unwrap(),
            Some(MCPToolUsageData {
                tools: vec![MCPToolSummary {
                    name: "github.search_code".to_string(),
                    call_count: 0,
                    error_count: 1,
                    max_input_size: 0,
                    max_output_size: 0,
                }],
            })
        );
        assert_eq!(
            extract_mcp_failures(&mcpg_dir).await.unwrap(),
            vec![MCPFailureReport {
                tool: Some("search_code".to_string()),
                context: None,
                reason: Some("oops".to_string()),
                timestamp: Some("2026-01-01T00:00:05Z".to_string()),
                extra: serde_json::json!({
                    "time": "2026-01-01T00:00:05Z",
                    "mcp_server": "github",
                    "name": "search_code",
                    "kind": "tool_error",
                    "error": "oops"
                }),
            }]
        );
    }

    #[tokio::test]
    async fn skips_malformed_lines_silently() {
        let temp_dir = TempDir::new().unwrap();
        let mcpg_dir = temp_dir.path().join("logs").join("mcpg");
        write_log_file(
            &mcpg_dir,
            "malformed.jsonl",
            concat!(
                "not-json\n",
                "{\"ts\":\"2026-01-01T00:00:06Z\",\"server\":\"github\",\"tool\":\"create_issue\",\"event\":\"tool_call\",\"input_size\":64,\"output_size\":128}\n"
            ),
        )
        .await;

        assert_eq!(
            analyze_mcp_tool_usage(&mcpg_dir).await.unwrap(),
            Some(MCPToolUsageData {
                tools: vec![MCPToolSummary {
                    name: "github.create_issue".to_string(),
                    call_count: 1,
                    error_count: 0,
                    max_input_size: 64,
                    max_output_size: 128,
                }],
            })
        );
    }
}
