use anyhow::Result;
use log::{debug, warn};
use serde_json::Value;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::audit::model::{DetectionAnalysis, DetectionThreats};

/// Read the detection verdict from `analyzed_outputs_<BuildId>/threat-analysis.json`.
///
/// Returns `Ok(None)` when the file is absent (detection didn't run, or
/// failed before writing the verdict).
///
/// Returns `Ok(Some(DetectionAnalysis { ..., safe_to_process: <conjunction> }))`
/// where `safe_to_process = !(prompt_injection || secret_leak || malicious_patch)`.
/// The `verdict_path` field is set to the relative path of the verdict
/// file from `download_root` so the audit renderer can link to it.
pub async fn analyze_detection(download_root: &Path) -> Result<Option<DetectionAnalysis>> {
    let Some(verdict_path) = find_verdict_path(download_root).await else {
        return Ok(None);
    };

    let verdict_bytes = match tokio::fs::read(&verdict_path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            warn!(
                "Failed to read detection verdict {}: {err}",
                verdict_path.display()
            );
            return Ok(None);
        }
    };

    let verdict_json: Value = match serde_json::from_slice(&verdict_bytes) {
        Ok(value) => value,
        Err(err) => {
            warn!(
                "Failed to parse detection verdict {} as JSON: {err}",
                verdict_path.display()
            );
            return Ok(None);
        }
    };

    let prompt_injection = extract_bool(&verdict_json, "prompt_injection");
    let secret_leak = extract_bool(&verdict_json, "secret_leak");
    let malicious_patch = extract_bool(&verdict_json, "malicious_patch");
    let reasons = extract_reasons(&verdict_json, &verdict_path);
    let safe_to_process = !(prompt_injection || secret_leak || malicious_patch);
    let verdict_path = verdict_path
        .strip_prefix(download_root)
        .ok()
        .map(|path| path.to_string_lossy().into_owned());

    Ok(Some(DetectionAnalysis {
        threats: DetectionThreats {
            prompt_injection,
            secret_leak,
            malicious_patch,
        },
        reasons,
        safe_to_process,
        verdict_path,
    }))
}

async fn find_verdict_path(download_root: &Path) -> Option<PathBuf> {
    let mut entries = match tokio::fs::read_dir(download_root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return None,
        Err(err) => {
            warn!(
                "Failed to read detection download root {}: {err}",
                download_root.display()
            );
            return None;
        }
    };

    let mut latest_dir: Option<(String, PathBuf)> = None;

    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(err) => {
                warn!(
                    "Failed to enumerate detection download root {}: {err}",
                    download_root.display()
                );
                return None;
            }
        };

        let file_type = match entry.file_type().await {
            Ok(file_type) => file_type,
            Err(err) => {
                debug!(
                    "Skipping detection artifact entry {} after file-type error: {err}",
                    entry.path().display()
                );
                continue;
            }
        };

        if !file_type.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("analyzed_outputs_") {
            continue;
        }

        let path = entry.path();
        match &latest_dir {
            Some((current_name, _))
                if crate::audit::cmp_numeric_suffix(&name, current_name)
                    != std::cmp::Ordering::Greater => {}
            _ => latest_dir = Some((name, path)),
        }
    }

    latest_dir.map(|(_, dir)| dir.join("threat-analysis.json"))
}

fn extract_bool(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn extract_reasons(v: &Value, verdict_path: &Path) -> Vec<String> {
    match v.get("reasons") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(reasons)) => reasons
            .iter()
            .map(|reason| match reason {
                Value::String(reason) => reason.clone(),
                other => {
                    debug!(
                        "Detection verdict {} contains non-string reason entry: {:?}",
                        verdict_path.display(),
                        other
                    );
                    String::new()
                }
            })
            .collect(),
        Some(other) => {
            debug!(
                "Detection verdict {} contains non-array reasons field: {:?}",
                verdict_path.display(),
                other
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::analyze_detection;
    use crate::audit::model::DetectionThreats;
    use tempfile::TempDir;

    fn expected_verdict_path(dir_name: &str) -> String {
        std::path::Path::new(dir_name)
            .join("threat-analysis.json")
            .to_string_lossy()
            .into_owned()
    }

    async fn create_analyzed_outputs_dir(temp_dir: &TempDir, dir_name: &str) {
        tokio::fs::create_dir_all(temp_dir.path().join(dir_name))
            .await
            .unwrap();
    }

    async fn write_verdict(temp_dir: &TempDir, dir_name: &str, contents: &str) {
        let dir = temp_dir.path().join(dir_name);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("threat-analysis.json"), contents)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn returns_none_when_download_root_is_absent() {
        let temp_dir = TempDir::new().unwrap();
        let missing_root = temp_dir.path().join("missing");

        let analysis = analyze_detection(&missing_root).await.unwrap();

        assert!(analysis.is_none());
    }

    #[tokio::test]
    async fn returns_none_when_verdict_file_is_missing() {
        let temp_dir = TempDir::new().unwrap();
        create_analyzed_outputs_dir(&temp_dir, "analyzed_outputs_42").await;

        let analysis = analyze_detection(temp_dir.path()).await.unwrap();

        assert!(analysis.is_none());
    }

    #[tokio::test]
    async fn parses_clean_verdict() {
        let temp_dir = TempDir::new().unwrap();
        write_verdict(
            &temp_dir,
            "analyzed_outputs_42",
            r#"{"prompt_injection":false,"secret_leak":false,"malicious_patch":false,"reasons":[]}"#,
        )
        .await;

        let analysis = analyze_detection(temp_dir.path()).await.unwrap().unwrap();

        assert_eq!(analysis.threats, DetectionThreats::default());
        assert!(analysis.reasons.is_empty());
        assert!(analysis.safe_to_process);
        assert_eq!(
            analysis.verdict_path,
            Some(expected_verdict_path("analyzed_outputs_42"))
        );
    }

    #[tokio::test]
    async fn marks_run_unsafe_when_any_threat_is_true() {
        let temp_dir = TempDir::new().unwrap();
        write_verdict(
            &temp_dir,
            "analyzed_outputs_42",
            r#"{"prompt_injection":true,"secret_leak":false,"malicious_patch":false,"reasons":["prompt injection detected"]}"#,
        )
        .await;

        let analysis = analyze_detection(temp_dir.path()).await.unwrap().unwrap();

        assert!(!analysis.safe_to_process);
        assert!(analysis.threats.prompt_injection);
        assert!(!analysis.threats.secret_leak);
        assert!(!analysis.threats.malicious_patch);
        assert_eq!(
            analysis.reasons,
            vec![String::from("prompt injection detected")]
        );
    }

    #[tokio::test]
    async fn accepts_string_booleans() {
        let temp_dir = TempDir::new().unwrap();
        write_verdict(
            &temp_dir,
            "analyzed_outputs_42",
            r#"{"prompt_injection":"true","secret_leak":"false","malicious_patch":"false","reasons":[]}"#,
        )
        .await;

        let analysis = analyze_detection(temp_dir.path()).await.unwrap().unwrap();

        assert!(analysis.threats.prompt_injection);
        assert!(!analysis.threats.secret_leak);
        assert!(!analysis.threats.malicious_patch);
        assert!(!analysis.safe_to_process);
    }

    #[tokio::test]
    async fn defaults_missing_reasons_to_empty() {
        let temp_dir = TempDir::new().unwrap();
        write_verdict(
            &temp_dir,
            "analyzed_outputs_42",
            r#"{"prompt_injection":false,"secret_leak":false,"malicious_patch":false}"#,
        )
        .await;

        let analysis = analyze_detection(temp_dir.path()).await.unwrap().unwrap();

        assert!(analysis.reasons.is_empty());
    }

    #[tokio::test]
    async fn returns_none_for_malformed_json() {
        let temp_dir = TempDir::new().unwrap();
        write_verdict(&temp_dir, "analyzed_outputs_42", "{not valid json").await;

        let analysis = analyze_detection(temp_dir.path()).await.unwrap();

        assert!(analysis.is_none());
    }

    #[tokio::test]
    async fn uses_highest_numbered_analyzed_outputs_directory() {
        let temp_dir = TempDir::new().unwrap();
        write_verdict(
            &temp_dir,
            "analyzed_outputs_41",
            r#"{"prompt_injection":false,"secret_leak":false,"malicious_patch":false,"reasons":[]}"#,
        )
        .await;
        write_verdict(
            &temp_dir,
            "analyzed_outputs_42",
            r#"{"prompt_injection":false,"secret_leak":false,"malicious_patch":true,"reasons":["malicious patch detected"]}"#,
        )
        .await;

        let analysis = analyze_detection(temp_dir.path()).await.unwrap().unwrap();

        assert!(!analysis.safe_to_process);
        assert!(analysis.threats.malicious_patch);
        assert_eq!(
            analysis.verdict_path,
            Some(expected_verdict_path("analyzed_outputs_42"))
        );
    }

    /// Regression: lexicographic sort would pick `analyzed_outputs_9`
    /// here. Numeric-suffix sort must pick `analyzed_outputs_10`.
    #[tokio::test]
    async fn picks_highest_numeric_suffix_not_lexicographic() {
        let temp_dir = TempDir::new().unwrap();
        write_verdict(
            &temp_dir,
            "analyzed_outputs_9",
            r#"{"prompt_injection":false,"secret_leak":false,"malicious_patch":false,"reasons":[]}"#,
        )
        .await;
        write_verdict(
            &temp_dir,
            "analyzed_outputs_10",
            r#"{"prompt_injection":true,"secret_leak":false,"malicious_patch":false,"reasons":["newer verdict"]}"#,
        )
        .await;

        let analysis = analyze_detection(temp_dir.path()).await.unwrap().unwrap();

        assert!(analysis.threats.prompt_injection);
        assert_eq!(
            analysis.verdict_path,
            Some(expected_verdict_path("analyzed_outputs_10"))
        );
    }
}
