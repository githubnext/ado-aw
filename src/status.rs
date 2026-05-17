//! The `status` CLI command.
//!
//! Renders the per-pipeline status for every ADO build definition
//! that matches a local fixture: name, id, folder, queueStatus, the
//! latest-run summary, and a deep link. Read-only.
//!
//! `status` is intentionally a thin renderer over the same data path
//! as `list` — same `list_definitions` + `match_definitions` +
//! `get_latest_build` sequence, just a denser per-pipeline block
//! instead of a table. The `--json` shape is byte-for-byte identical
//! to `list --json` so scripts can use either.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ado::{
    PATH_SEGMENT, get_latest_build, list_definitions, match_definitions_in, resolve_ado_context,
    resolve_auth,
};
use crate::detect;
use crate::list::{ListRow, build_rows, render_json};

/// CLI options for [`run`].
pub struct StatusOptions<'a> {
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    pub path: Option<&'a Path>,
    pub json: bool,
}

/// Pure renderer: dense per-pipeline block.
pub fn render_blocks(ado_org_url: &str, ado_project: &str, rows: &[ListRow]) -> String {
    if rows.is_empty() {
        return "(no matched definitions)\n".to_string();
    }

    let mut out = String::new();
    for r in rows {
        out.push_str(&format!("● {}\n", r.name));
        out.push_str(&format!("  id:           {}\n", r.id));
        if let Some(folder) = &r.folder {
            out.push_str(&format!("  folder:       {}\n", folder));
        }
        out.push_str(&format!(
            "  queueStatus:  {}\n",
            r.queue_status.as_deref().unwrap_or("?")
        ));
        if let Some(yaml) = &r.yaml_filename {
            out.push_str(&format!("  source:       {}\n", yaml.trim_start_matches('/')));
        }
        match &r.last_run {
            Some(lr) => {
                let result = lr
                    .result
                    .clone()
                    .or_else(|| lr.status.clone())
                    .unwrap_or_else(|| "in progress".to_string());
                out.push_str(&format!("  last run:     build {} — {}", lr.id, result));
                if let Some(t) = &lr.finish_time {
                    out.push_str(&format!(" @ {}", t));
                }
                out.push('\n');
                let url = lr.url.clone().unwrap_or_else(|| {
                    format!(
                        "{}/{}/_build/results?buildId={}",
                        ado_org_url.trim_end_matches('/'),
                        percent_encoding::utf8_percent_encode(ado_project, PATH_SEGMENT),
                        lr.id,
                    )
                });
                out.push_str(&format!("  url:          {}\n", url));
            }
            None => {
                out.push_str("  last run:     never\n");
            }
        }
        out.push('\n');
    }
    out
}

/// Run the `status` command.
pub async fn run(opts: StatusOptions<'_>) -> Result<()> {
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
    let detected = detect::detect_pipelines(&repo_path).await.unwrap_or_else(|e| {
        // Distinguish "detection failed" from "no pipelines compiled
        // here": both produce zero matches downstream, but only the
        // former is something the operator should know about. Don't
        // bail outright — status is read-only and useful even with
        // partial inputs.
        eprintln!("warning: failed to scan local pipelines: {:#}", e);
        Vec::new()
    });
    let matched = match_definitions_in(&definitions, &detected);

    // Surface the "no matched fixtures" case explicitly. `status` is
    // read-only and intentionally non-fatal (unlike `disable` which
    // bails), but rendering "(no matched definitions)" without a
    // warning is indistinguishable from running in the wrong
    // directory. Mirror the existing "failed to scan" warning.
    if matched.is_empty() {
        eprintln!(
            "warning: no local fixtures matched any ADO definition under {}",
            repo_path.display()
        );
    }

    let target_ids: HashSet<u64> = matched.iter().map(|m| m.id).collect();
    let mut last_runs = std::collections::HashMap::new();
    for id in &target_ids {
        match get_latest_build(&client, &ado_ctx, &auth, *id).await {
            Ok(Some(v)) => {
                last_runs.insert(*id, v);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("  warning: failed to fetch latest build for {}: {:#}", id, e);
            }
        }
    }

    let rows = build_rows(&definitions, &matched, &last_runs, false);

    if opts.json {
        println!("{}", render_json(&rows)?);
    } else {
        print!("{}", render_blocks(&ado_ctx.org_url, &ado_ctx.project, &rows));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::LastRun;

    fn row_with_run(id: u64, name: &str, status: Option<&str>, last_run: Option<LastRun>) -> ListRow {
        ListRow {
            id,
            name: name.to_string(),
            folder: Some("\\smoke".to_string()),
            queue_status: status.map(str::to_string),
            yaml_filename: Some(format!("/tests/{}.lock.yml", name)),
            matched: true,
            last_run,
        }
    }

    #[test]
    fn empty_renders_placeholder() {
        let out = render_blocks("https://dev.azure.com/o", "p", &[]);
        assert_eq!(out, "(no matched definitions)\n");
    }

    #[test]
    fn block_shows_succeeded_run_with_url() {
        let row = row_with_run(
            123,
            "noop",
            Some("enabled"),
            Some(LastRun {
                id: 999,
                result: Some("succeeded".to_string()),
                status: Some("completed".to_string()),
                finish_time: Some("2026-05-17T08:00:00Z".to_string()),
                url: Some("https://dev.azure.com/.../999".to_string()),
                raw: serde_json::Value::Null,
            }),
        );
        let out = render_blocks("https://dev.azure.com/o", "p", &[row]);
        assert!(out.contains("● noop"));
        assert!(out.contains("id:           123"));
        assert!(out.contains("folder:       \\smoke"));
        assert!(out.contains("queueStatus:  enabled"));
        assert!(out.contains("source:       tests/noop.lock.yml"));
        assert!(out.contains("last run:     build 999 — succeeded"));
        assert!(out.contains("2026-05-17T08:00:00Z"));
        assert!(out.contains("https://dev.azure.com/.../999"));
    }

    #[test]
    fn block_synthesizes_url_when_missing() {
        let row = row_with_run(
            42,
            "x",
            Some("disabled"),
            Some(LastRun {
                id: 7,
                result: Some("failed".to_string()),
                status: Some("completed".to_string()),
                finish_time: None,
                url: None,
                raw: serde_json::Value::Null,
            }),
        );
        let out = render_blocks("https://dev.azure.com/myorg/", "myproject", &[row]);
        assert!(
            out.contains("https://dev.azure.com/myorg/myproject/_build/results?buildId=7"),
            "expected synthesized URL in:\n{out}"
        );
    }

    #[test]
    fn block_shows_never_when_no_last_run() {
        let row = row_with_run(1, "x", Some("enabled"), None);
        let out = render_blocks("o", "p", &[row]);
        assert!(out.contains("last run:     never"));
        assert!(!out.contains("url:"));
    }

    #[test]
    fn block_shows_in_progress_when_no_result_yet() {
        let row = row_with_run(
            1,
            "x",
            Some("enabled"),
            Some(LastRun {
                id: 10,
                result: None,
                status: Some("inProgress".to_string()),
                finish_time: None,
                url: None,
                raw: serde_json::Value::Null,
            }),
        );
        let out = render_blocks("o", "p", &[row]);
        assert!(out.contains("build 10 — inProgress"));
    }

    #[test]
    fn block_uses_question_mark_when_queue_status_missing() {
        let row = row_with_run(1, "x", None, None);
        let out = render_blocks("o", "p", &[row]);
        assert!(out.contains("queueStatus:  ?"));
    }
}
