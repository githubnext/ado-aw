use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use log::{debug, warn};
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::audit::model::AuditData;

const CURRENT_ADO_AW_VERSION: &str = env!("CARGO_PKG_VERSION");

/// On-disk run summary written under `<output>/build-<id>/run-summary.json`.
/// CLI-version-keyed so a new ado-aw release transparently re-processes.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    pub ado_aw_version: String,
    pub build_id: u64,
    pub processed_at: DateTime<Utc>,
    pub audit_data: AuditData,
}

/// Filename for the cached run summary stored inside each audited run directory.
pub const RUN_SUMMARY_FILENAME: &str = "run-summary.json";

fn run_summary_path(run_dir: &Path) -> PathBuf {
    run_dir.join(RUN_SUMMARY_FILENAME)
}

fn temp_run_summary_path(run_dir: &Path) -> PathBuf {
    run_dir.join(format!(".{RUN_SUMMARY_FILENAME}.tmp"))
}

// `chrono` is present without its `serde` feature in Cargo.toml, so we keep the
// public `DateTime<Utc>` field and serialize it explicitly as RFC 3339.
impl serde::Serialize for RunSummary {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(serde::Serialize)]
        struct RunSummaryDisk<'a> {
            ado_aw_version: &'a str,
            build_id: u64,
            processed_at: String,
            audit_data: &'a AuditData,
        }

        RunSummaryDisk {
            ado_aw_version: &self.ado_aw_version,
            build_id: self.build_id,
            processed_at: self
                .processed_at
                .to_rfc3339_opts(SecondsFormat::AutoSi, true),
            audit_data: &self.audit_data,
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for RunSummary {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct RunSummaryDisk {
            ado_aw_version: String,
            build_id: u64,
            processed_at: String,
            audit_data: AuditData,
        }

        let disk = RunSummaryDisk::deserialize(deserializer)?;
        let processed_at = chrono::DateTime::parse_from_rfc3339(&disk.processed_at)
            .map_err(serde::de::Error::custom)?
            .with_timezone(&Utc);

        Ok(Self {
            ado_aw_version: disk.ado_aw_version,
            build_id: disk.build_id,
            processed_at,
            audit_data: disk.audit_data,
        })
    }
}

/// Save a run summary to `<run_dir>/run-summary.json`.
/// Creates parent dirs as needed. Atomic write via temp-file + rename.
pub async fn save_run_summary(run_dir: &Path, summary: &RunSummary) -> Result<()> {
    fs::create_dir_all(run_dir)
        .await
        .with_context(|| format!("create run summary cache directory {}", run_dir.display()))?;

    let summary_path = run_summary_path(run_dir);
    let temp_path = temp_run_summary_path(run_dir);
    let bytes = serde_json::to_vec_pretty(summary).context("serialize run summary cache")?;

    fs::write(&temp_path, bytes)
        .await
        .with_context(|| format!("write temporary run summary cache {}", temp_path.display()))?;

    if let Err(error) = fs::rename(&temp_path, &summary_path).await {
        let _ = fs::remove_file(&temp_path).await;
        return Err(anyhow::Error::new(error).context(format!(
            "rename temporary run summary cache {} to {}",
            temp_path.display(),
            summary_path.display()
        )));
    }

    debug!("Saved run summary cache to {}", summary_path.display());
    Ok(())
}

/// Load a run summary from `<run_dir>/run-summary.json`.
///
/// Returns:
///   Ok(Some(summary)) — file present, parsed, AND `ado_aw_version` matches
///                       the current CLI version (`env!("CARGO_PKG_VERSION")`).
///   Ok(None)          — file absent, OR `ado_aw_version` mismatch, OR JSON parse error.
///                       In the mismatch/parse-error cases, logs a `warn!` so the
///                       operator sees that the cache was skipped.
///   Err(...)          — only for I/O errors other than NotFound.
pub async fn load_run_summary(run_dir: &Path) -> Result<Option<RunSummary>> {
    let summary_path = run_summary_path(run_dir);
    let bytes = match fs::read(&summary_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            debug!("No run summary cache found at {}", summary_path.display());
            return Ok(None);
        }
        Err(error) => {
            return Err(anyhow::Error::new(error)
                .context(format!("read run summary cache {}", summary_path.display())));
        }
    };

    let summary = match serde_json::from_slice::<RunSummary>(&bytes) {
        Ok(summary) => summary,
        Err(error) => {
            warn!(
                "Ignoring run summary cache at {} because it could not be parsed: {}",
                summary_path.display(),
                error
            );
            return Ok(None);
        }
    };

    if summary.ado_aw_version != CURRENT_ADO_AW_VERSION {
        warn!(
            "Ignoring run summary cache at {} because it was written by ado-aw {} instead of {}",
            summary_path.display(),
            summary.ado_aw_version,
            CURRENT_ADO_AW_VERSION
        );
        return Ok(None);
    }

    debug!("Loaded run summary cache from {}", summary_path.display());
    Ok(Some(summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::model::{ErrorInfo, Finding, MetricsData, OverviewData, Severity};
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn sample_summary(version: &str) -> RunSummary {
        RunSummary {
            ado_aw_version: version.to_string(),
            build_id: 12345,
            processed_at: Utc
                .with_ymd_and_hms(2026, 5, 21, 12, 0, 0)
                .single()
                .unwrap(),
            audit_data: AuditData {
                overview: OverviewData {
                    build_id: 12345,
                    pipeline_name: "agentic-audit".to_string(),
                    status: "completed".to_string(),
                    result: Some("succeeded".to_string()),
                    source_branch: Some("refs/heads/main".to_string()),
                    url: Some(
                        "https://dev.azure.com/example/project/_build/results?buildId=12345"
                            .to_string(),
                    ),
                    ..Default::default()
                },
                metrics: MetricsData {
                    token_usage: 2048,
                    effective_tokens: 1536,
                    estimated_cost: 1.25,
                    turns: 8,
                    warning_count: 1,
                    ..Default::default()
                },
                key_findings: vec![Finding {
                    category: "tooling".to_string(),
                    severity: Severity::High,
                    title: "Missing validation artifact".to_string(),
                    description: "The run completed without publishing a validation artifact."
                        .to_string(),
                    impact: Some("Audit confidence is reduced for this run.".to_string()),
                }],
                warnings: vec![ErrorInfo {
                    source: "artifact-download".to_string(),
                    message: "safe outputs artifact missing; rendered partial report".to_string(),
                    timestamp: Some("2026-05-21T12:01:00Z".to_string()),
                }],
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn round_trip_run_summary() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let run_dir = temp_dir.path().join("build-12345");
        let summary = sample_summary(CURRENT_ADO_AW_VERSION);

        save_run_summary(&run_dir, &summary)
            .await
            .expect("save run summary");

        let loaded = load_run_summary(&run_dir)
            .await
            .expect("load run summary")
            .expect("run summary should exist");

        assert_eq!(loaded, summary);
    }

    #[tokio::test]
    async fn version_mismatch_returns_none() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let run_dir = temp_dir.path().join("build-12345");

        save_run_summary(&run_dir, &sample_summary("999.0.0"))
            .await
            .expect("save mismatched run summary");

        assert!(
            load_run_summary(&run_dir)
                .await
                .expect("load run summary")
                .is_none()
        );
    }

    #[tokio::test]
    async fn missing_file_returns_none() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let run_dir = temp_dir.path().join("build-12345");

        assert!(
            load_run_summary(&run_dir)
                .await
                .expect("load missing run summary")
                .is_none()
        );
    }

    #[tokio::test]
    async fn corrupt_json_returns_none() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let run_dir = temp_dir.path().join("build-12345");
        let summary_path = run_summary_path(&run_dir);

        fs::create_dir_all(&run_dir)
            .await
            .expect("create run summary dir");
        fs::write(&summary_path, b"{ definitely not json }")
            .await
            .expect("write corrupt run summary");

        assert!(
            load_run_summary(&run_dir)
                .await
                .expect("load corrupt run summary")
                .is_none()
        );
    }

    #[tokio::test]
    async fn save_cleans_up_temporary_file() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let run_dir = temp_dir.path().join("build-12345");
        let temp_path = temp_run_summary_path(&run_dir);

        save_run_summary(&run_dir, &sample_summary(CURRENT_ADO_AW_VERSION))
            .await
            .expect("save run summary");

        assert!(fs::metadata(&temp_path).await.is_err());
    }
}
