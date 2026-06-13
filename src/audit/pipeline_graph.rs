//! Pipeline-IR graph correlation for `ado-aw audit`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::audit::model::{AuditData, AwInfo, ErrorInfo, PipelineGraphSection};
use crate::compile::ir::summary::{JobSummary, PipelineBodySummary, PipelineSummary};

/// Populate `audit.pipeline_graph` and per-job upstream/downstream IR edges.
///
/// The source markdown is resolved from the runtime `aw_info.json` metadata
/// emitted by the Agent job. Missing local sources are common when auditing an
/// arbitrary build, so absence is recorded as a warning rather than an error.
pub async fn populate_pipeline_graph(audit: &mut AuditData, run_dir: &Path) -> Result<()> {
    let source = match read_source_from_aw_info(run_dir).await.transpose()? {
        Some(source) if !source.trim().is_empty() => Some(source),
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

    let source_path = resolve_source_path(&source).await?;
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
        graph: summary,
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
    all_jobs(summary)
        .into_iter()
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
    timeline_name
        .rsplit('.')
        .next()
        .is_some_and(|suffix| suffix == job_id)
}

pub(crate) fn all_jobs(summary: &PipelineSummary) -> Vec<&JobSummary> {
    match &summary.body {
        PipelineBodySummary::Jobs { jobs } => jobs.iter().collect(),
        PipelineBodySummary::Stages { stages } => {
            stages.iter().flat_map(|stage| stage.jobs.iter()).collect()
        }
    }
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

async fn resolve_source_path(source: &str) -> Result<PathBuf> {
    let normalized = normalize_source_path(source);
    let path = PathBuf::from(normalized);
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = tokio::fs::canonicalize(".")
        .await
        .context("Could not resolve current directory")?;
    Ok(cwd.join(path))
}

fn normalize_source_path(source: &str) -> String {
    let trimmed = source.trim();
    if std::path::MAIN_SEPARATOR == '/' {
        trimmed.replace('\\', "/")
    } else {
        trimmed.replace('/', "\\")
    }
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
}
