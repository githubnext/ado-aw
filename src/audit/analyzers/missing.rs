//! Missing-tool, missing-data, and noop audit analyzers backed by
//! `safe_outputs.ndjson` proposal artifacts.
//!
//! This module intentionally does not export an `extract_mcp_failures` function.
//! If agents ever emit `name == "mcp_failure"` safe outputs, they follow the
//! same record shape handled here, but public MCP failure extraction is owned by
//! `crate::audit::analyzers::mcp`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;
use tokio::fs;

use crate::audit::model::{MissingDataReport, MissingToolReport, NoopReport};

const AGENT_OUTPUTS_PREFIX: &str = "agent_outputs_";
const SAFE_OUTPUTS_FILE: &str = "safe_outputs.ndjson";

pub async fn extract_missing_tools(download_root: &Path) -> Result<Vec<MissingToolReport>> {
    let values = read_safe_outputs_ndjson(download_root).await;
    Ok(values
        .into_iter()
        .filter(|value| matches_signal(value, "missing_tool"))
        .map(|value| MissingToolReport {
            tool: string_field(&value, "tool"),
            context: string_field(&value, "context"),
            reason: reason_field(&value),
            timestamp: string_field(&value, "timestamp"),
            extra: value,
        })
        .collect())
}

pub async fn extract_missing_data(download_root: &Path) -> Result<Vec<MissingDataReport>> {
    let values = read_safe_outputs_ndjson(download_root).await;
    Ok(values
        .into_iter()
        .filter(|value| matches_signal(value, "missing_data"))
        .map(|value| MissingDataReport {
            tool: string_field(&value, "tool"),
            context: string_field(&value, "context"),
            reason: reason_field(&value),
            timestamp: string_field(&value, "timestamp"),
            extra: value,
        })
        .collect())
}

pub async fn extract_noops(download_root: &Path) -> Result<Vec<NoopReport>> {
    let values = read_safe_outputs_ndjson(download_root).await;
    Ok(values
        .into_iter()
        .filter(|value| matches_signal(value, "noop"))
        .map(|value| NoopReport {
            tool: None,
            context: string_field(&value, "context"),
            reason: reason_field(&value),
            timestamp: string_field(&value, "timestamp"),
            extra: value,
        })
        .collect())
}

async fn read_safe_outputs_ndjson(download_root: &Path) -> Vec<Value> {
    let mut dirs = VecDeque::from([download_root.to_path_buf()]);
    let mut safe_output_paths = Vec::new();

    while let Some(dir) = dirs.pop_front() {
        if is_agent_outputs_dir(&dir) {
            if let Some(path) = preferred_safe_outputs_path(&dir).await {
                safe_output_paths.push(path);
            }
            continue;
        }

        let mut entries = match fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        loop {
            let Some(entry) = (match entries.next_entry().await {
                Ok(entry) => entry,
                Err(_) => None,
            }) else {
                break;
            };

            let file_type = match entry.file_type().await {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };

            if file_type.is_dir() {
                dirs.push_back(entry.path());
            }
        }
    }

    safe_output_paths.sort();
    safe_output_paths.dedup();

    let mut values = Vec::new();
    for path in safe_output_paths {
        let Ok(contents) = fs::read_to_string(&path).await else {
            continue;
        };

        values.extend(contents.lines().filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                serde_json::from_str(trimmed).ok()
            }
        }));
    }

    values
}

async fn preferred_safe_outputs_path(agent_outputs_dir: &Path) -> Option<PathBuf> {
    let staging = agent_outputs_dir.join("staging").join(SAFE_OUTPUTS_FILE);
    if fs::metadata(&staging).await.is_ok() {
        return Some(staging);
    }

    let fallback = agent_outputs_dir.join(SAFE_OUTPUTS_FILE);
    if fs::metadata(&fallback).await.is_ok() {
        return Some(fallback);
    }

    None
}

fn is_agent_outputs_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(AGENT_OUTPUTS_PREFIX))
}

fn matches_signal(value: &Value, signal: &str) -> bool {
    let Some(name) = value.get("name").and_then(Value::as_str) else {
        return false;
    };

    name == signal || name == signal.replace('_', "-")
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn reason_field(value: &Value) -> Option<String> {
    string_field(value, "reason").or_else(|| string_field(value, "description"))
}

#[cfg(test)]
mod tests {
    use std::fs as stdfs;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn no_safe_outputs_ndjson_returns_empty_reports() {
        let temp_dir = TempDir::new().unwrap();

        assert!(
            extract_missing_tools(temp_dir.path())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            extract_missing_data(temp_dir.path())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(extract_noops(temp_dir.path()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mixed_ndjson_file_filters_each_signal_and_ignores_unrelated_entries() {
        let temp_dir = TempDir::new().unwrap();
        write_safe_outputs(
            &temp_dir,
            true,
            &[
                r#"{"name":"missing_tool","tool":"bash","context":"ctx-1","reason":"missing bash","timestamp":"2026-05-21T12:01:00Z"}"#,
                r#"{"name":"missing-tool","tool":"python","context":"ctx-2","reason":"missing python","timestamp":"2026-05-21T12:02:00Z"}"#,
                r#"{"name":"missing_data","tool":"create_work_item","context":"ctx-3","reason":"missing title","timestamp":"2026-05-21T12:03:00Z"}"#,
                r#"{"name":"noop","tool":"ignored","context":"ctx-4","reason":"nothing to do","timestamp":"2026-05-21T12:04:00Z"}"#,
                r#"{"name":"noop","context":"ctx-5","reason":"already complete","timestamp":"2026-05-21T12:05:00Z"}"#,
                r#"{"name":"noop","context":"ctx-6","description":"skipped","timestamp":"2026-05-21T12:06:00Z"}"#,
                r#"{"name":"create_pull_request"}"#,
                r#"{"name":"add_pr_comment"}"#,
                r#"{"name":"report_incomplete"}"#,
                r#"{"name":"mcp_failure"}"#,
                r#"{"name":"other"}"#,
            ],
        );

        let missing_tools = extract_missing_tools(temp_dir.path()).await.unwrap();
        let missing_data = extract_missing_data(temp_dir.path()).await.unwrap();
        let noops = extract_noops(temp_dir.path()).await.unwrap();

        assert_eq!(missing_tools.len(), 2);
        assert_eq!(missing_data.len(), 1);
        assert_eq!(noops.len(), 3);
        assert_eq!(missing_tools[0].tool.as_deref(), Some("bash"));
        assert_eq!(missing_tools[1].tool.as_deref(), Some("python"));
        assert!(noops.iter().all(|report| report.tool.is_none()));
    }

    #[tokio::test]
    async fn description_field_falls_back_to_reason() {
        let temp_dir = TempDir::new().unwrap();
        write_safe_outputs(
            &temp_dir,
            false,
            &[r#"{"name":"missing_data","description":"need schema details"}"#],
        );

        let reports = extract_missing_data(temp_dir.path()).await.unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].reason.as_deref(), Some("need schema details"));
    }

    #[tokio::test]
    async fn malformed_lines_are_skipped_silently() {
        let temp_dir = TempDir::new().unwrap();
        write_safe_outputs(
            &temp_dir,
            true,
            &[
                r#"{"name":"noop","context":"first"}"#,
                r#"{"name":"noop","context": }"#,
                r#"{"name":"noop","context":"second"}"#,
            ],
        );

        let reports = extract_noops(temp_dir.path()).await.unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].context.as_deref(), Some("first"));
        assert_eq!(reports[1].context.as_deref(), Some("second"));
    }

    #[tokio::test]
    async fn extra_payload_is_preserved_on_report() {
        let temp_dir = TempDir::new().unwrap();
        let original = json!({
            "name": "missing_tool",
            "tool": "azure-devops",
            "context": "work-item-sync",
            "reason": "Tool not configured",
            "timestamp": "2026-05-21T12:03:00Z",
            "nested": {
                "required": true,
                "attempts": [1, 2, 3]
            }
        });
        write_safe_outputs(&temp_dir, true, &[&original.to_string()]);

        let reports = extract_missing_tools(temp_dir.path()).await.unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].extra, original);
    }

    fn write_safe_outputs(temp_dir: &TempDir, use_staging_path: bool, lines: &[&str]) {
        let base_dir = temp_dir.path().join("agent_outputs_42");
        let file_path = if use_staging_path {
            base_dir.join("staging").join(SAFE_OUTPUTS_FILE)
        } else {
            base_dir.join(SAFE_OUTPUTS_FILE)
        };

        stdfs::create_dir_all(file_path.parent().unwrap()).unwrap();
        stdfs::write(file_path, lines.join("\n")).unwrap();
    }
}
