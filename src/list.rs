//! The `list` CLI command.
//!
//! Renders the current state of every ADO build definition that
//! matches a local fixture (or *all* definitions with `--all`).
//! Phase 1 of the pipeline-lifecycle CLI family — see `docs/cli.md`.
//!
//! Output:
//!
//! - Default: human-readable text table.
//! - `--json`: JSON array, stable shape suitable for programmatic
//!   consumption.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ado::{
    DefinitionSummary, MatchedDefinition, get_latest_build, list_definitions, match_definitions_in,
    resolve_ado_context, resolve_auth,
};
use crate::detect;

/// One row of the rendered output.
///
/// `last_run` is intentionally untyped here — ADO build records have
/// many optional fields and the JSON renderer passes them through
/// verbatim, while the text renderer only inspects `result`. Pure
/// data so we can snapshot-test both renderers.
#[derive(Debug, Clone, PartialEq)]
pub struct ListRow {
    pub id: u64,
    pub name: String,
    pub folder: Option<String>,
    pub queue_status: Option<String>,
    pub yaml_filename: Option<String>,
    pub matched: bool,
    pub last_run: Option<LastRun>,
}

/// Latest-build summary, projected to the fields the text renderer
/// uses. JSON output passes the full `serde_json::Value` through so
/// callers don't lose access to fields we don't currently surface.
#[derive(Debug, Clone, PartialEq)]
pub struct LastRun {
    pub id: u64,
    pub result: Option<String>,
    pub status: Option<String>,
    pub finish_time: Option<String>,
    pub url: Option<String>,
    /// Raw value for JSON pass-through.
    pub raw: serde_json::Value,
}

impl LastRun {
    /// Project an ADO `build` JSON value into a [`LastRun`]. Returns
    /// `None` when the JSON has no usable `id` field.
    pub fn from_json(value: serde_json::Value) -> Option<Self> {
        let id = value.get("id").and_then(|v| v.as_u64())?;
        Some(Self {
            id,
            result: value
                .get("result")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            status: value
                .get("status")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            finish_time: value
                .get("finishTime")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            url: value
                .get("_links")
                .and_then(|l| l.get("web"))
                .and_then(|w| w.get("href"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or_else(|| {
                    value
                        .get("url")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                }),
            raw: value,
        })
    }
}

/// CLI options for [`run`].
pub struct ListOptions<'a> {
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub path: Option<&'a Path>,
    pub all: bool,
    pub json: bool,
}

/// Pure function: assemble [`ListRow`]s from raw inputs.
///
/// - `definitions`: project-wide listing from `list_definitions`.
/// - `matched`: subset that maps to a local fixture (yaml-path or
///   pipeline-name).
/// - `last_runs`: latest-build JSON keyed by definition id. Missing
///   entries become `last_run: None`.
/// - `include_unmatched`: when `false`, definitions that aren't in
///   `matched` are filtered out (default for `list`).
///
/// Row ordering: matched rows first (sorted by name), then unmatched
/// rows (also sorted by name) when included.
pub fn build_rows(
    definitions: &[DefinitionSummary],
    matched: &[MatchedDefinition],
    last_runs: &std::collections::HashMap<u64, serde_json::Value>,
    include_unmatched: bool,
) -> Vec<ListRow> {
    let matched_ids: HashSet<u64> = matched.iter().map(|m| m.id).collect();
    let yaml_by_id: std::collections::HashMap<u64, String> = matched
        .iter()
        .filter(|m| !m.yaml_path.is_empty())
        .map(|m| (m.id, m.yaml_path.clone()))
        .collect();

    let mut rows: Vec<ListRow> = definitions
        .iter()
        .filter(|d| include_unmatched || matched_ids.contains(&d.id))
        .map(|d| {
            let yaml_filename = yaml_by_id
                .get(&d.id)
                .cloned()
                .or_else(|| d.process.as_ref().and_then(|p| p.yaml_filename.clone()));
            let last_run = last_runs.get(&d.id).cloned().and_then(LastRun::from_json);
            ListRow {
                id: d.id,
                name: d.name.clone(),
                folder: d.path.clone(),
                queue_status: d.queue_status.clone(),
                yaml_filename,
                matched: matched_ids.contains(&d.id),
                last_run,
            }
        })
        .collect();

    rows.sort_by(|a, b| {
        b.matched
            .cmp(&a.matched)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

/// Pure function: render a list of rows as a text table.
pub fn render_text(rows: &[ListRow]) -> String {
    if rows.is_empty() {
        return "(no definitions)\n".to_string();
    }
    let headers = ["NAME", "ID", "FOLDER", "STATUS", "LAST RUN", "SOURCE"];
    let mut widths = headers.map(|h| h.chars().count());
    let str_rows: Vec<[String; 6]> = rows
        .iter()
        .map(|r| {
            [
                r.name.clone(),
                r.id.to_string(),
                r.folder.clone().unwrap_or_default(),
                r.queue_status.clone().unwrap_or_else(|| "?".to_string()),
                r.last_run
                    .as_ref()
                    .map(|lr| {
                        lr.result
                            .clone()
                            .or_else(|| lr.status.clone())
                            .unwrap_or_else(|| "in progress".to_string())
                    })
                    .unwrap_or_else(|| "never".to_string()),
                r.yaml_filename
                    .clone()
                    .map(|y| y.trim_start_matches('/').to_string())
                    .unwrap_or_default(),
            ]
        })
        .collect();
    for cells in &str_rows {
        for (i, cell) in cells.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    write_row(&mut out, &headers.map(str::to_string), &widths);
    for cells in &str_rows {
        write_row(&mut out, cells, &widths);
    }
    out
}

fn write_row(out: &mut String, cells: &[String; 6], widths: &[usize; 6]) {
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        let pad = widths[i].saturating_sub(cell.chars().count());
        out.push_str(cell);
        if i < cells.len() - 1 {
            for _ in 0..pad {
                out.push(' ');
            }
        }
    }
    out.push('\n');
}

/// Pure function: render the rows as JSON.
pub fn render_json(rows: &[ListRow]) -> Result<String> {
    let array: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            // Keep this as a raw pass-through for scripting stability.
            // Text output trims the leading slash for readability; JSON
            // intentionally retains ADO/local-matcher path shape.
            serde_json::json!({
                "name": r.name,
                "id": r.id,
                "folder": r.folder,
                "queueStatus": r.queue_status,
                "yamlFilename": r.yaml_filename,
                "matched": r.matched,
                "lastRun": r.last_run.as_ref().map(|lr| &lr.raw),
            })
        })
        .collect();
    serde_json::to_string_pretty(&array).context("Failed to serialize list rows as JSON")
}

/// Run the `list` command.
pub async fn run(opts: ListOptions<'_>) -> Result<()> {
    let repo_path: PathBuf = match opts.path {
        Some(p) => tokio::fs::canonicalize(p)
            .await
            .with_context(|| format!("Could not resolve path: {}", p.display()))?,
        None => tokio::fs::canonicalize(".")
            .await
            .context("Could not resolve current directory")?,
    };

    let auth = resolve_auth(opts.pat).await?;
    let ado_ctx = resolve_ado_context(&repo_path, opts.org, opts.project).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let definitions = list_definitions(&client, &ado_ctx, &auth).await?;
    let detected = detect::detect_pipelines(&repo_path)
        .await
        .unwrap_or_else(|e| {
            // Distinguish "detection failed" from "no pipelines compiled
            // here": both produce zero matches downstream, but only the
            // former is something the operator should know about. Don't
            // bail outright — list is read-only and useful even with
            // partial inputs (`--all` doesn't need fixtures at all).
            eprintln!("warning: failed to scan local pipelines: {:#}", e);
            Vec::new()
        });
    let matched = match_definitions_in(&definitions, &detected);

    // Decide which IDs need a last-build fetch.
    let target_ids: HashSet<u64> = if opts.all {
        definitions.iter().map(|d| d.id).collect()
    } else {
        matched.iter().map(|m| m.id).collect()
    };

    // Sequential fetch (small N; bounded fanout via JoinSet is a
    // straightforward future improvement once we have a project with
    // 50+ matched pipelines).
    let mut last_runs = std::collections::HashMap::new();
    for id in &target_ids {
        match get_latest_build(&client, &ado_ctx, &auth, *id).await {
            Ok(Some(v)) => {
                last_runs.insert(*id, v);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!(
                    "  warning: failed to fetch latest build for {}: {:#}",
                    id, e
                );
            }
        }
    }

    let rows = build_rows(&definitions, &matched, &last_runs, opts.all);

    if opts.json {
        println!("{}", render_json(&rows)?);
    } else {
        print!("{}", render_text(&rows));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ado::{MatchMethod, ProcessInfo};
    use std::collections::HashMap;

    fn def(
        id: u64,
        name: &str,
        folder: Option<&str>,
        yaml: Option<&str>,
        status: Option<&str>,
    ) -> DefinitionSummary {
        DefinitionSummary {
            id,
            name: name.to_string(),
            process: yaml.map(|y| ProcessInfo {
                yaml_filename: Some(y.to_string()),
            }),
            queue_status: status.map(str::to_string),
            path: folder.map(str::to_string),
            repository: None,
            revision: None,
        }
    }

    fn matched(id: u64, name: &str, yaml: &str) -> MatchedDefinition {
        MatchedDefinition {
            id,
            name: name.to_string(),
            match_method: MatchMethod::YamlPath,
            yaml_path: yaml.to_string(),
            queue_status: None,
        }
    }

    // ============ LastRun::from_json ============

    #[test]
    fn last_run_extracts_fields() {
        let v = serde_json::json!({
            "id": 1234,
            "result": "succeeded",
            "status": "completed",
            "finishTime": "2026-05-17T08:00:00Z",
            "_links": { "web": { "href": "https://dev.azure.com/.../1234" } }
        });
        let lr = LastRun::from_json(v).unwrap();
        assert_eq!(lr.id, 1234);
        assert_eq!(lr.result.as_deref(), Some("succeeded"));
        assert_eq!(lr.status.as_deref(), Some("completed"));
        assert_eq!(lr.finish_time.as_deref(), Some("2026-05-17T08:00:00Z"));
        assert_eq!(lr.url.as_deref(), Some("https://dev.azure.com/.../1234"));
    }

    #[test]
    fn last_run_falls_back_to_top_level_url() {
        let v = serde_json::json!({
            "id": 7,
            "url": "https://dev.azure.com/.../7"
        });
        let lr = LastRun::from_json(v).unwrap();
        assert_eq!(lr.url.as_deref(), Some("https://dev.azure.com/.../7"));
    }

    #[test]
    fn last_run_returns_none_when_id_missing() {
        let v = serde_json::json!({ "result": "succeeded" });
        assert!(LastRun::from_json(v).is_none());
    }

    // ============ build_rows ============

    #[test]
    fn build_rows_default_filters_unmatched() {
        let defs = vec![
            def(
                1,
                "matched",
                Some("\\smoke"),
                Some("/a.yml"),
                Some("enabled"),
            ),
            def(
                2,
                "unmatched",
                Some("\\other"),
                Some("/b.yml"),
                Some("enabled"),
            ),
        ];
        let m = vec![matched(1, "matched", "/a.yml")];
        let rows = build_rows(&defs, &m, &HashMap::new(), false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 1);
        assert!(rows[0].matched);
    }

    #[test]
    fn build_rows_all_flag_includes_unmatched() {
        let defs = vec![
            def(
                1,
                "matched",
                Some("\\smoke"),
                Some("/a.yml"),
                Some("enabled"),
            ),
            def(
                2,
                "unmatched",
                Some("\\other"),
                Some("/b.yml"),
                Some("disabled"),
            ),
        ];
        let m = vec![matched(1, "matched", "/a.yml")];
        let rows = build_rows(&defs, &m, &HashMap::new(), true);
        assert_eq!(rows.len(), 2);
        // Matched rows sort first.
        assert!(rows[0].matched);
        assert!(!rows[1].matched);
    }

    #[test]
    fn build_rows_sorts_within_group_by_name_case_insensitive() {
        let defs = vec![
            def(1, "Zebra", None, None, None),
            def(2, "alpha", None, None, None),
            def(3, "Beta", None, None, None),
        ];
        let m = vec![
            matched(1, "Zebra", "/z.yml"),
            matched(2, "alpha", "/a.yml"),
            matched(3, "Beta", "/b.yml"),
        ];
        let rows = build_rows(&defs, &m, &HashMap::new(), false);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "Beta", "Zebra"]
        );
    }

    #[test]
    fn build_rows_attaches_last_run() {
        let defs = vec![def(1, "x", None, Some("/x.yml"), Some("enabled"))];
        let m = vec![matched(1, "x", "/x.yml")];
        let mut runs = HashMap::new();
        runs.insert(1u64, serde_json::json!({ "id": 99, "result": "succeeded" }));
        let rows = build_rows(&defs, &m, &runs, false);
        assert_eq!(rows[0].last_run.as_ref().unwrap().id, 99);
        assert_eq!(
            rows[0].last_run.as_ref().unwrap().result.as_deref(),
            Some("succeeded")
        );
    }

    // ============ render_text ============

    #[test]
    fn render_text_includes_headers_and_data() {
        let rows = vec![ListRow {
            id: 123,
            name: "Daily noop".to_string(),
            folder: Some("\\smoke".to_string()),
            queue_status: Some("enabled".to_string()),
            yaml_filename: Some("/tests/noop.lock.yml".to_string()),
            matched: true,
            last_run: Some(LastRun {
                id: 999,
                result: Some("succeeded".to_string()),
                status: None,
                finish_time: None,
                url: None,
                raw: serde_json::Value::Null,
            }),
        }];
        let out = render_text(&rows);
        assert!(out.contains("NAME"));
        assert!(out.contains("ID"));
        assert!(out.contains("FOLDER"));
        assert!(out.contains("STATUS"));
        assert!(out.contains("Daily noop"));
        assert!(out.contains("123"));
        assert!(out.contains("\\smoke"));
        assert!(out.contains("enabled"));
        assert!(out.contains("succeeded"));
        // Yaml source rendered without the leading slash.
        assert!(out.contains("tests/noop.lock.yml"));
    }

    #[test]
    fn render_text_uses_never_when_no_last_run() {
        let rows = vec![ListRow {
            id: 1,
            name: "x".to_string(),
            folder: None,
            queue_status: Some("enabled".to_string()),
            yaml_filename: None,
            matched: true,
            last_run: None,
        }];
        let out = render_text(&rows);
        assert!(out.contains("never"));
    }

    #[test]
    fn render_text_empty_returns_placeholder() {
        assert_eq!(render_text(&[]), "(no definitions)\n");
    }

    // ============ render_json ============

    #[test]
    fn render_json_emits_expected_shape() {
        let raw = serde_json::json!({
            "id": 999,
            "result": "succeeded",
            "status": "completed",
            "finishTime": "2026-05-17T08:00:00Z",
            "requestedFor": { "displayName": "A User" },
            "triggerInfo": { "ci.sourceSha": "abc123" },
        });
        let rows = vec![ListRow {
            id: 123,
            name: "Daily noop".to_string(),
            folder: Some("\\smoke".to_string()),
            queue_status: Some("enabled".to_string()),
            yaml_filename: Some("/tests/noop.lock.yml".to_string()),
            matched: true,
            last_run: Some(LastRun {
                id: 999,
                result: Some("succeeded".to_string()),
                status: Some("completed".to_string()),
                finish_time: Some("2026-05-17T08:00:00Z".to_string()),
                url: Some("https://dev.azure.com/.../999".to_string()),
                raw: raw.clone(),
            }),
        }];
        let out = render_json(&rows).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[0]["name"], "Daily noop");
        assert_eq!(parsed[0]["id"], 123);
        assert_eq!(parsed[0]["folder"], "\\smoke");
        assert_eq!(parsed[0]["queueStatus"], "enabled");
        assert_eq!(parsed[0]["yamlFilename"], "/tests/noop.lock.yml");
        assert_eq!(parsed[0]["matched"], true);
        assert_eq!(parsed[0]["lastRun"], raw);
    }

    #[test]
    fn render_json_lastrun_is_null_when_missing() {
        let rows = vec![ListRow {
            id: 1,
            name: "x".to_string(),
            folder: None,
            queue_status: None,
            yaml_filename: None,
            matched: true,
            last_run: None,
        }];
        let out = render_json(&rows).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed[0]["lastRun"].is_null());
    }
}
