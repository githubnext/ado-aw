use anyhow::Context;
use log::debug;
use std::path::{Path, PathBuf};

use crate::agent_stats::AgentStats;
use crate::audit::model::{AuditEngineConfig, AwInfo, MetricsData, PerformanceMetrics};

/// Combined OTel + aw_info analysis result.
#[derive(Debug, Clone, Default)]
pub struct OtelAnalysis {
    pub metrics: MetricsData,
    pub engine_config: Option<AuditEngineConfig>,
    pub performance: Option<PerformanceMetrics>,
    pub aw_info: Option<AwInfo>,
}

/// Read `staging/otel.jsonl` + `staging/aw_info.json` from an agent
/// outputs directory and produce metrics, engine config, performance
/// metrics, and aw_info for the audit report.
///
/// `agent_outputs_dir` should be the path to the extracted artifact
/// root (e.g. `<output>/build-<id>/agent_outputs_<BuildId>/`).
///
/// Both files are optional:
///   - OTel may be absent (non-Copilot engine, or older builds).
///   - aw_info may be absent (older builds; will become standard once the
///     `audit-pipeline-awinfo` change merges).
/// The function never errors on absence; it logs a `debug!` and leaves
/// the corresponding field empty / None.
pub async fn analyze_otel(agent_outputs_dir: &std::path::Path) -> anyhow::Result<OtelAnalysis> {
    let mut analysis = OtelAnalysis::default();

    if let Some(otel_path) = locate_agent_output_file(agent_outputs_dir, "otel.jsonl") {
        let stats = AgentStats::from_otel_file(&otel_path, "audit")
            .await
            .with_context(|| format!("Failed to analyze OTel file: {}", otel_path.display()))?;

        let total_tokens = stats.input_tokens + stats.output_tokens;
        analysis.metrics = MetricsData {
            token_usage: total_tokens,
            effective_tokens: total_tokens,
            estimated_cost: 0.0,
            turns: stats.turns,
            error_count: 0,
            warning_count: 0,
        };

        let tokens_per_minute = if stats.duration_seconds > 0.0 {
            Some(total_tokens as f64 / (stats.duration_seconds / 60.0))
        } else {
            None
        };

        if let Some(tokens_per_minute) = tokens_per_minute {
            analysis.performance = Some(PerformanceMetrics {
                tokens_per_minute: Some(tokens_per_minute),
                cost_efficiency: None,
                most_used_tool: None,
                network_requests: None,
            });
        }
    } else {
        debug!(
            "No otel.jsonl found under {} (checked staging/ and top-level)",
            agent_outputs_dir.display()
        );
    }

    if let Some(aw_info_path) = locate_agent_output_file(agent_outputs_dir, "aw_info.json") {
        let aw_info_contents = tokio::fs::read_to_string(&aw_info_path)
            .await
            .with_context(|| format!("Failed to read aw_info file: {}", aw_info_path.display()))?;
        let aw_info = serde_json::from_str::<AwInfo>(&aw_info_contents)
            .with_context(|| format!("Failed to parse aw_info file: {}", aw_info_path.display()))?;

        analysis.engine_config = Some(AuditEngineConfig {
            engine: aw_info.engine.clone().unwrap_or_default(),
            model: aw_info.model.clone(),
            version: aw_info.compiler_version.clone(),
            timeout_minutes: None,
        });
        analysis.aw_info = Some(aw_info);
    } else {
        debug!(
            "No aw_info.json found under {} (checked staging/ and top-level)",
            agent_outputs_dir.display()
        );
    }

    Ok(analysis)
}

fn locate_agent_output_file(agent_outputs_dir: &Path, file_name: &str) -> Option<PathBuf> {
    [
        agent_outputs_dir.join("staging").join(file_name),
        agent_outputs_dir.join(file_name),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const AW_INFO_JSON: &str = r#"{"schema":"ado-aw/aw_info/1","engine":"copilot","model":"claude-sonnet-4.5","agent_name":"test","target":"standalone","source":"agents/test.md","compiler_version":"0.30.0"}"#;
    const COPILOT_OTEL_FIXTURE: &str = include_str!("../../../tests/fixtures/copilot-otel.jsonl");

    async fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(path, contents).await.unwrap();
    }

    #[tokio::test]
    async fn analyze_otel_returns_defaults_when_files_absent() {
        let temp_dir = TempDir::new().unwrap();

        let analysis = analyze_otel(temp_dir.path()).await.unwrap();

        assert_eq!(analysis.metrics, MetricsData::default());
        assert!(analysis.engine_config.is_none());
        assert!(analysis.performance.is_none());
        assert!(analysis.aw_info.is_none());
    }

    #[tokio::test]
    async fn analyze_otel_reads_aw_info_only() {
        let temp_dir = TempDir::new().unwrap();
        let aw_info_path = temp_dir.path().join("staging").join("aw_info.json");
        write_file(&aw_info_path, AW_INFO_JSON).await;

        let analysis = analyze_otel(temp_dir.path()).await.unwrap();

        assert_eq!(analysis.metrics, MetricsData::default());
        assert!(analysis.performance.is_none());

        let aw_info = analysis.aw_info.expect("expected aw_info");
        assert_eq!(aw_info.engine.as_deref(), Some("copilot"));
        assert_eq!(aw_info.model.as_deref(), Some("claude-sonnet-4.5"));
        assert_eq!(aw_info.compiler_version.as_deref(), Some("0.30.0"));

        let engine_config = analysis.engine_config.expect("expected engine config");
        assert_eq!(engine_config.engine, "copilot");
        assert_eq!(engine_config.model.as_deref(), Some("claude-sonnet-4.5"));
        assert_eq!(engine_config.version.as_deref(), Some("0.30.0"));
        assert_eq!(engine_config.timeout_minutes, None);
    }

    #[tokio::test]
    async fn analyze_otel_reads_otel_only() {
        let temp_dir = TempDir::new().unwrap();
        let otel_path = temp_dir.path().join("staging").join("otel.jsonl");
        write_file(&otel_path, COPILOT_OTEL_FIXTURE).await;

        let analysis = analyze_otel(temp_dir.path()).await.unwrap();

        assert!(analysis.metrics.token_usage > 0);
        assert_eq!(
            analysis.metrics.effective_tokens,
            analysis.metrics.token_usage
        );
        assert!(analysis.metrics.turns > 0);
        assert!(analysis.engine_config.is_none());
        assert!(analysis.aw_info.is_none());
    }

    #[tokio::test]
    async fn analyze_otel_reads_both_files() {
        let temp_dir = TempDir::new().unwrap();
        let staging_dir = temp_dir.path().join("staging");
        write_file(&staging_dir.join("otel.jsonl"), COPILOT_OTEL_FIXTURE).await;
        write_file(&staging_dir.join("aw_info.json"), AW_INFO_JSON).await;

        let analysis = analyze_otel(temp_dir.path()).await.unwrap();

        assert!(analysis.metrics.token_usage > 0);
        assert!(analysis.metrics.turns > 0);
        assert_eq!(
            analysis
                .engine_config
                .as_ref()
                .and_then(|config| config.model.as_deref()),
            Some("claude-sonnet-4.5")
        );
        assert_eq!(
            analysis
                .aw_info
                .as_ref()
                .and_then(|info| info.engine.as_deref()),
            Some("copilot")
        );
        assert!(
            analysis
                .performance
                .as_ref()
                .and_then(|performance| performance.tokens_per_minute)
                .is_some_and(|value| value > 0.0)
        );
    }

    #[tokio::test]
    async fn analyze_otel_falls_back_to_top_level_files() {
        let temp_dir = TempDir::new().unwrap();
        write_file(&temp_dir.path().join("otel.jsonl"), COPILOT_OTEL_FIXTURE).await;
        write_file(&temp_dir.path().join("aw_info.json"), AW_INFO_JSON).await;

        let analysis = analyze_otel(temp_dir.path()).await.unwrap();

        assert!(analysis.metrics.token_usage > 0);
        assert_eq!(
            analysis
                .aw_info
                .as_ref()
                .and_then(|info| info.engine.as_deref()),
            Some("copilot")
        );
        assert_eq!(
            analysis
                .engine_config
                .as_ref()
                .map(|config| config.engine.as_str()),
            Some("copilot")
        );
    }
}
