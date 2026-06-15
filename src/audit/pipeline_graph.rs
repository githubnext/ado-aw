//! Pipeline-IR graph correlation for `ado-aw audit`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::audit::model::{AuditData, AwInfo, ErrorInfo, PipelineGraphSection};
use crate::compile::ir::summary::{JobSummary, PipelineSummary};

/// Populate `audit.pipeline_graph` and per-job upstream/downstream IR edges.
///
/// The source markdown is resolved from the runtime `aw_info.json` metadata
/// emitted by the Agent job. Missing local sources are common when auditing an
/// arbitrary build, so absence is recorded as a warning rather than an error.
pub async fn populate_pipeline_graph(audit: &mut AuditData, run_dir: &Path) -> Result<()> {
    let source = match read_source_from_aw_info(run_dir).await {
        Some(Ok(value)) if !value.trim().is_empty() => Some(value),
        Some(Err(err)) => {
            // Previously `transpose()?` propagated this as a hard
            // error and aborted the audit. A corrupt aw_info.json
            // from a bad run is a realistic scenario; downgrade to
            // the same warn-and-continue path documented for
            // resolve_source_path failures below.
            record_warning(
                audit,
                "audit::pipeline_graph",
                format!("failed to read aw_info.json: {err:#}; skipping IR graph correlation"),
            );
            return Ok(());
        }
        _ => audit
            .overview
            .aw_info
            .as_ref()
            .and_then(|info| info.source.clone()),
    };
    let Some(source) = source else {
        record_warning(
            audit,
            "audit::pipeline_graph",
            "could not locate aw_info.json source metadata; skipping IR graph correlation",
        );
        return Ok(());
    };

    let source_path = match resolve_source_path(&source).await {
        Ok(path) => path,
        Err(err) => {
            record_warning(
                audit,
                "audit::pipeline_graph",
                format!("could not resolve source path: {err:#}; skipping IR graph correlation"),
            );
            return Ok(());
        }
    };
    if tokio::fs::metadata(&source_path).await.is_err() {
        record_warning(
            audit,
            "audit::pipeline_graph",
            format!(
                "source markdown '{}' is not available locally; skipping IR graph correlation",
                source_path.display()
            ),
        );
        return Ok(());
    }

    let resolved_source_path = tokio::fs::canonicalize(&source_path)
        .await
        .unwrap_or_else(|_| source_path.clone());
    let (_fm, pipeline) = crate::compile::build_pipeline_ir(&resolved_source_path)
        .await
        .with_context(|| format!("build IR for {}", resolved_source_path.display()))?;
    let summary = PipelineSummary::from_pipeline(&pipeline)
        .with_context(|| format!("summarize IR for {}", resolved_source_path.display()))?;

    populate_job_edges(audit, &summary);
    audit.pipeline_graph = Some(PipelineGraphSection {
        source_path: resolved_source_path.display().to_string(),
        summary,
    });
    Ok(())
}

fn populate_job_edges(audit: &mut AuditData, summary: &PipelineSummary) {
    for job in &mut audit.jobs {
        let Some(ir_job) = find_matching_job_summary(summary, &job.name) else {
            continue;
        };
        let job_id = ir_job.id.as_str();
        job.upstream_jobs = summary
            .graph
            .job_edges
            .iter()
            .filter(|edge| edge.consumer == job_id)
            .map(|edge| edge.producer.clone())
            .collect();
        job.downstream_jobs = summary
            .graph
            .job_edges
            .iter()
            .filter(|edge| edge.producer == job_id)
            .map(|edge| edge.consumer.clone())
            .collect();
    }
}

fn find_matching_job_summary<'a>(
    summary: &'a PipelineSummary,
    timeline_name: &str,
) -> Option<&'a JobSummary> {
    summary
        .all_jobs()
        .find(|job| timeline_name_matches_job(timeline_name, &job.id, job.stage.as_deref()))
}

pub(crate) fn timeline_name_matches_job(
    timeline_name: &str,
    job_id: &str,
    stage: Option<&str>,
) -> bool {
    let timeline_name = timeline_name.trim();
    if timeline_name == job_id {
        return true;
    }
    if let Some(stage) = stage
        && timeline_name == format!("{stage}.{job_id}")
    {
        return true;
    }
    // Fallback for unusual pipelines where the caller did not supply
    // the stage but the timeline still emits a `Stage.Job` name. We
    // only accept a *single-level* prefix — strings with two or more
    // dots like `Stage1.SubStage.Agent` are rejected even when the
    // trailing component matches, because the old
    // `rsplit('.').next()` form could attach IR edges to the wrong
    // runtime job in unusual pipeline shapes.
    matches!(
        timeline_name.rsplit_once('.'),
        Some((prefix, suffix))
            if suffix == job_id && !prefix.contains('.')
    )
}

async fn read_source_from_aw_info(run_dir: &Path) -> Option<Result<String>> {
    let agent_outputs = find_artifact_dir(run_dir, "agent_outputs").await?;
    for path in [
        agent_outputs.join("staging").join("aw_info.json"),
        agent_outputs.join("aw_info.json"),
    ] {
        if tokio::fs::metadata(&path).await.is_err() {
            continue;
        }
        let contents = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => contents,
            Err(error) => return Some(Err(error).context(format!("read {}", path.display()))),
        };
        let aw_info = match serde_json::from_str::<AwInfo>(&contents) {
            Ok(aw_info) => aw_info,
            Err(error) => return Some(Err(error).context(format!("parse {}", path.display()))),
        };
        return Some(Ok(aw_info.source.unwrap_or_default()));
    }
    None
}

/// Resolve the `source` string taken from a downloaded `aw_info.json`
/// into an on-disk path.
///
/// Delegates the whole security contract to
/// [`crate::compile::source_path_guard::validate_workflow_source_path`],
/// which both this entry point and the mcp-author server share.
/// See that module-level doc for the full list of mitigations.
async fn resolve_source_path(source: &str) -> Result<PathBuf> {
    let validated = crate::compile::source_path_guard::validate_workflow_source_path(source)
        .await
        .with_context(|| "validate aw_info.json source string from audited build artifact")?;
    Ok(validated.path)
}

async fn find_artifact_dir(run_dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(run_dir).await.ok()?;
    let mut hits: Vec<(String, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false)
            && let Some(name) = entry.file_name().to_str()
            && (name == prefix || name.starts_with(&format!("{prefix}_")))
        {
            hits.push((name.to_string(), entry.path()));
        }
    }
    hits.sort_by(|(a, _), (b, _)| crate::audit::cmp_numeric_suffix(a, b));
    hits.pop().map(|(_, path)| path)
}

fn record_warning(audit: &mut AuditData, source: &str, message: impl Into<String>) {
    audit.warnings.push(ErrorInfo {
        source: source.to_string(),
        message: message.into(),
        timestamp: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::model::JobData;

    #[tokio::test]
    async fn populate_pipeline_graph_correlates_jobs_from_aw_info_source() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let run_dir = temp_dir.path().join("build-42");
        let staging_dir = run_dir.join("agent_outputs_42").join("staging");
        tokio::fs::create_dir_all(&staging_dir)
            .await
            .expect("create staging");

        let source_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("safe-outputs")
            .join("create-pull-request.md");
        let aw_info = serde_json::json!({
            "source": source_path.display().to_string(),
            "target": "standalone"
        });
        tokio::fs::write(staging_dir.join("aw_info.json"), aw_info.to_string())
            .await
            .expect("write aw_info");

        let mut audit = AuditData {
            jobs: vec![
                JobData {
                    name: "Agent".to_string(),
                    status: "completed".to_string(),
                    result: Some("succeeded".to_string()),
                    ..Default::default()
                },
                JobData {
                    name: "Detection".to_string(),
                    status: "completed".to_string(),
                    result: Some("succeeded".to_string()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        populate_pipeline_graph(&mut audit, &run_dir)
            .await
            .expect("populate graph");

        assert!(audit.pipeline_graph.is_some());
        let agent = audit
            .jobs
            .iter()
            .find(|job| job.name == "Agent")
            .expect("agent job");
        assert!(agent.downstream_jobs.iter().any(|job| job == "Detection"));
        let detection = audit
            .jobs
            .iter()
            .find(|job| job.name == "Detection")
            .expect("detection job");
        assert!(detection.upstream_jobs.iter().any(|job| job == "Agent"));
    }

    #[tokio::test]
    async fn resolve_source_path_rejects_non_markdown_absolute_paths() {
        // The exfiltration vector flagged by the PR reviewer: a malicious
        // aw_info.json carries an absolute path to a non-`.md` file. The
        // resolver must refuse before any file open happens.
        assert!(
            resolve_source_path("/home/user/.ssh/id_rsa").await.is_err(),
            "expected resolver to reject non-markdown absolute path"
        );
    }

    #[tokio::test]
    async fn resolve_source_path_rejects_parent_traversal() {
        assert!(
            resolve_source_path("../../../etc/passwd.md").await.is_err(),
            "expected resolver to reject parent-dir components"
        );
    }

    #[tokio::test]
    async fn resolve_source_path_rejects_tilde_prefix() {
        assert!(
            resolve_source_path("~/secret.md").await.is_err(),
            "expected resolver to reject tilde-prefixed path"
        );
    }

    #[tokio::test]
    async fn resolve_source_path_accepts_markdown_absolute_paths() {
        // Legitimate compiled-elsewhere workflows: absolute `.md` paths must still work.
        let path = if cfg!(windows) {
            r"C:\workflows\foo.md"
        } else {
            "/repo/workflows/foo.md"
        };
        assert!(
            resolve_source_path(path).await.is_ok(),
            "expected absolute `.md` paths to be accepted"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_source_path_rejects_md_symlink_to_non_md_target() {
        // Symlink-bypass regression: `foo.md` → `/etc/passwd` lexically
        // satisfies the `.md` extension check but resolves to a
        // non-markdown file. The post-canonicalize re-check must
        // reject it.
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let target = temp_dir.path().join("not_markdown.bin");
        tokio::fs::write(&target, b"binary")
            .await
            .expect("write target");
        let link = temp_dir.path().join("evil.md");
        tokio::fs::symlink(&target, &link)
            .await
            .expect("create symlink");

        let err = resolve_source_path(link.to_str().unwrap())
            .await
            .expect_err("symlink to non-md target must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("symlink resolves to non-`.md` target"),
            "expected symlink-target rejection message, got: {msg}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_source_path_accepts_md_symlink_to_md_target() {
        // Legitimate `current.md` → `v1.md` style symlinks must still
        // be accepted — the post-canonicalize re-check only rejects
        // when the resolved target lacks the `.md` extension.
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let target = temp_dir.path().join("v1.md");
        tokio::fs::write(&target, b"# pipeline")
            .await
            .expect("write target");
        let link = temp_dir.path().join("current.md");
        tokio::fs::symlink(&target, &link)
            .await
            .expect("create symlink");

        let resolved = resolve_source_path(link.to_str().unwrap())
            .await
            .expect("md symlink to md target must be accepted");
        assert_eq!(resolved, link);
    }

    #[tokio::test]
    async fn populate_pipeline_graph_records_warning_on_malicious_source() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let run_dir = temp_dir.path().join("build-99");
        let staging_dir = run_dir.join("agent_outputs_99").join("staging");
        tokio::fs::create_dir_all(&staging_dir)
            .await
            .expect("create staging");

        let aw_info = serde_json::json!({
            "source": "/home/user/.ssh/id_rsa",
            "target": "standalone"
        });
        tokio::fs::write(staging_dir.join("aw_info.json"), aw_info.to_string())
            .await
            .expect("write aw_info");

        let mut audit = AuditData::default();
        populate_pipeline_graph(&mut audit, &run_dir)
            .await
            .expect("populate graph should not error on malicious source");

        assert!(
            audit.pipeline_graph.is_none(),
            "malicious source must not populate pipeline_graph"
        );
        assert!(
            audit
                .warnings
                .iter()
                .any(|w| w.source == "audit::pipeline_graph"
                    && w.message.contains("could not resolve source path")),
            "expected a warning recording the rejection, got {:?}",
            audit.warnings
        );
    }

    #[tokio::test]
    async fn populate_pipeline_graph_records_warning_on_corrupt_aw_info_json() {
        // Regression: previously `read_source_from_aw_info`'s
        // Some(Err(_)) was propagated via `transpose()?` and aborted
        // the entire audit. A corrupt aw_info.json from a bad run is
        // a realistic scenario; it must degrade to a warning.
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let run_dir = temp_dir.path().join("build-77");
        let staging_dir = run_dir.join("agent_outputs_77").join("staging");
        tokio::fs::create_dir_all(&staging_dir)
            .await
            .expect("create staging");
        tokio::fs::write(staging_dir.join("aw_info.json"), b"{not valid json")
            .await
            .expect("write malformed aw_info");

        let mut audit = AuditData::default();
        populate_pipeline_graph(&mut audit, &run_dir)
            .await
            .expect("populate graph must not bail on corrupt aw_info.json");

        assert!(
            audit.pipeline_graph.is_none(),
            "corrupt aw_info.json must not populate pipeline_graph"
        );
        assert!(
            audit
                .warnings
                .iter()
                .any(|w| w.source == "audit::pipeline_graph"
                    && w.message.contains("failed to read aw_info.json")),
            "expected a warning recording the read failure, got {:?}",
            audit.warnings
        );
    }
}
