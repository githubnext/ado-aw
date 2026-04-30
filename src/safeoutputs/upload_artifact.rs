//! Upload pipeline artifact safe output tool.
//!
//! Lets an agent propose publishing a workspace file as an Azure DevOps build
//! artifact. Stage 3 validates the path, size, extension, and configured
//! allow-lists, then emits an `##vso[artifact.upload]` logging command on
//! stdout — the surrounding pipeline agent picks this up and attaches the
//! file to the run as a build artifact.

use ado_aw_derive::SanitizeConfig;
use log::{debug, info};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::sanitize::SanitizeContent;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::tool_result;
use anyhow::{Context, ensure};

/// Parameters for uploading a workspace file as a pipeline artifact.
#[derive(Deserialize, JsonSchema)]
pub struct UploadArtifactParams {
    /// The artifact name to publish under. ADO requires a non-empty name made
    /// of alphanumerics, `-`, `_`, or `.`. Must be 1-100 characters and must
    /// not start with `.`.
    pub artifact_name: String,

    /// Path to the file in the workspace to upload. Must be a relative path
    /// with no directory traversal, no absolute prefix, and no `.git` segments.
    pub file_path: String,
}

impl Validate for UploadArtifactParams {
    fn validate(&self) -> anyhow::Result<()> {
        // artifact_name: charset/length/structural rules.
        ensure!(!self.artifact_name.is_empty(), "artifact_name must not be empty");
        ensure!(
            self.artifact_name.len() <= 100,
            "artifact_name must be at most 100 characters"
        );
        ensure!(
            !self.artifact_name.starts_with('.'),
            "artifact_name must not start with '.'"
        );
        ensure!(
            self.artifact_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.'),
            "artifact_name must contain only alphanumeric characters, '-', '_' or '.'"
        );

        // file_path: same path-safety rules as upload-attachment.
        ensure!(!self.file_path.is_empty(), "file_path must not be empty");
        ensure!(
            !self.file_path.split(['/', '\\']).any(|component| component == ".."),
            "file_path must not contain '..' path components"
        );
        ensure!(
            !self.file_path.starts_with('/') && !self.file_path.starts_with('\\'),
            "file_path must not be an absolute path"
        );
        ensure!(
            !self.file_path.contains(':'),
            "file_path must not contain ':'"
        );
        ensure!(
            !self.file_path.contains('\0'),
            "file_path must not contain null bytes"
        );
        ensure!(
            !self
                .file_path
                .split(['/', '\\'])
                .any(|component| component == ".git"),
            "file_path must not contain '.git' components"
        );
        Ok(())
    }
}

tool_result! {
    name = "upload-artifact",
    write = true,
    params = UploadArtifactParams,
    /// Result of publishing a workspace file as a pipeline artifact.
    pub struct UploadArtifactResult {
        artifact_name: String,
        file_path: String,
    }
}

impl SanitizeContent for UploadArtifactResult {
    fn sanitize_content_fields(&mut self) {
        // Both fields are strictly validated to a safe charset above; no
        // additional textual sanitization is required.
    }
}

const DEFAULT_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB

/// Configuration for the upload-artifact tool (specified in front matter).
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   upload-artifact:
///     max-file-size: 52428800
///     allowed-extensions:
///       - .png
///       - .pdf
///       - .log
///     allowed-artifact-names:
///       - agent-*
///     name-prefix: "agent-"
///     max: 5
/// ```
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct UploadArtifactConfig {
    /// Maximum file size in bytes (default: 50 MB).
    #[serde(default = "default_max_file_size", rename = "max-file-size")]
    pub max_file_size: u64,

    /// Allowed file extensions (e.g., `[".png", ".pdf"]`). Empty means all
    /// extensions are allowed.
    #[serde(default, rename = "allowed-extensions")]
    pub allowed_extensions: Vec<String>,

    /// Restrict which artifact names may be published. Empty means any name
    /// (subject to charset rules) is allowed. Entries ending with `*` match by
    /// prefix; otherwise the comparison is exact.
    #[serde(default, rename = "allowed-artifact-names")]
    pub allowed_artifact_names: Vec<String>,

    /// Prefix prepended to the agent-supplied artifact name before publishing.
    #[serde(default, rename = "name-prefix")]
    pub name_prefix: Option<String>,
}

fn default_max_file_size() -> u64 {
    DEFAULT_MAX_FILE_SIZE
}

impl Default for UploadArtifactConfig {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            allowed_extensions: Vec::new(),
            allowed_artifact_names: Vec::new(),
            name_prefix: None,
        }
    }
}

#[async_trait::async_trait]
impl Executor for UploadArtifactResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "publish '{}' as artifact '{}'",
            self.file_path, self.artifact_name
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Publishing '{}' as pipeline artifact '{}'",
            self.file_path, self.artifact_name
        );
        debug!(
            "upload-artifact: artifact_name='{}', file_path='{}'",
            self.artifact_name, self.file_path
        );

        let config: UploadArtifactConfig = ctx.get_tool_config("upload-artifact");
        debug!("Max file size: {} bytes", config.max_file_size);
        debug!("Allowed extensions: {:?}", config.allowed_extensions);
        debug!("Allowed artifact names: {:?}", config.allowed_artifact_names);

        // Apply name-prefix and re-validate the resulting name's charset
        // (the prefix itself is operator-controlled and sanitized at config
        // load, but we still defensively check the joined string).
        let final_name = match &config.name_prefix {
            Some(prefix) => format!("{}{}", prefix, self.artifact_name),
            None => self.artifact_name.clone(),
        };
        if final_name.is_empty()
            || final_name.starts_with('.')
            || final_name.len() > 100
            || !final_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        {
            return Ok(ExecutionResult::failure(format!(
                "Resolved artifact name '{}' is not a valid Azure DevOps artifact name",
                final_name
            )));
        }
        debug!("Final artifact name (after prefix): {}", final_name);

        // Check artifact-name allow-list (if configured).
        if !config.allowed_artifact_names.is_empty() {
            let allowed = config.allowed_artifact_names.iter().any(|pattern| {
                if let Some(prefix) = pattern.strip_suffix('*') {
                    final_name.starts_with(prefix)
                } else {
                    *pattern == final_name
                }
            });
            if !allowed {
                return Ok(ExecutionResult::failure(format!(
                    "Artifact name '{}' is not in the allowed list",
                    final_name
                )));
            }
        }

        // Validate file extension against allowed-extensions (if configured).
        if !config.allowed_extensions.is_empty() {
            let has_valid_ext = config.allowed_extensions.iter().any(|ext| {
                self.file_path
                    .to_lowercase()
                    .ends_with(&ext.to_lowercase())
            });
            if !has_valid_ext {
                return Ok(ExecutionResult::failure(format!(
                    "File '{}' has an extension not in the allowed list: {:?}",
                    self.file_path, config.allowed_extensions
                )));
            }
        }

        // Resolve file path relative to source_directory, then canonicalize
        // and verify it stays inside the workspace (symlink-escape guard).
        let resolved_path = ctx.source_directory.join(&self.file_path);
        debug!("Resolved file path: {}", resolved_path.display());

        let canonical = resolved_path.canonicalize().context(
            "Failed to canonicalize file path — file may not exist or contains broken symlinks",
        )?;
        let canonical_base = ctx
            .source_directory
            .canonicalize()
            .context("Failed to canonicalize source directory")?;
        if !canonical.starts_with(&canonical_base) {
            return Ok(ExecutionResult::failure(format!(
                "File path '{}' resolves outside the workspace (symlink escape)",
                self.file_path
            )));
        }

        // Reject directories — the ##vso[artifact.upload] command takes a
        // single file path. Directory uploads can be added later by emitting
        // multiple commands or switching to ##vso[artifact.upload]+containerfolder.
        let metadata = std::fs::metadata(&canonical).context("Failed to read file metadata")?;
        if metadata.is_dir() {
            return Ok(ExecutionResult::failure(format!(
                "File path '{}' is a directory; the upload-artifact tool only supports single files",
                self.file_path
            )));
        }
        let file_size = metadata.len();
        debug!("File size: {} bytes", file_size);
        if file_size > config.max_file_size {
            return Ok(ExecutionResult::failure(format!(
                "File size ({} bytes) exceeds maximum allowed size ({} bytes)",
                file_size, config.max_file_size
            )));
        }

        // Scan text files for ##vso[ command-injection sequences. Binary
        // files (where from_utf8 fails) skip this check; the build agent
        // doesn't parse logging commands out of binary file contents.
        let file_bytes = std::fs::read(&canonical).context("Failed to read file contents")?;
        if let Ok(text) = std::str::from_utf8(&file_bytes) {
            if text.contains("##vso[") {
                return Ok(ExecutionResult::failure(format!(
                    "File '{}' contains '##vso[' command injection sequence",
                    self.file_path
                )));
            }
        }

        // Convert to an absolute path string for the logging command. ADO
        // logging commands are UTF-8; reject non-UTF-8 paths explicitly
        // rather than letting `to_string_lossy()` silently replace bytes.
        let abs_path = match canonical.to_str() {
            Some(s) => s.to_string(),
            None => {
                return Ok(ExecutionResult::failure(format!(
                    "Resolved file path for '{}' is not valid UTF-8 and cannot be used in a pipeline logging command",
                    self.file_path
                )));
            }
        };

        // Final guard: the absolute path must not contain `]`, newlines or
        // carriage returns, all of which would break out of the logging
        // command. Canonicalization on Linux/macOS guarantees no `\r`/`\n`
        // and `]` is structurally illegal in the parser, but we check
        // explicitly so the failure is clear instead of being a silent
        // mis-parse on the agent side.
        if abs_path.contains(['\n', '\r', ']']) {
            return Ok(ExecutionResult::failure(format!(
                "Resolved file path '{}' contains characters that cannot be used in a pipeline logging command",
                abs_path
            )));
        }

        if ctx.dry_run {
            return Ok(ExecutionResult::success(format!(
                "[dry-run] would publish '{}' ({} bytes) as artifact '{}'",
                self.file_path, file_size, final_name
            )));
        }

        // Emit the ADO logging command. The pipeline agent parses stdout
        // and asynchronously uploads the file as a build artifact attached
        // to the current run.
        println!(
            "##vso[artifact.upload artifactname={}]{}",
            final_name, abs_path
        );

        info!(
            "Queued artifact upload: '{}' -> artifact '{}'",
            self.file_path, final_name
        );

        Ok(ExecutionResult::success_with_data(
            format!(
                "Queued '{}' for upload as pipeline artifact '{}'",
                self.file_path, final_name
            ),
            serde_json::json!({
                "artifact_name": final_name,
                "file_path": self.file_path,
                "size_bytes": file_size,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(UploadArtifactResult::NAME, "upload-artifact");
    }

    #[test]
    fn test_requires_write() {
        assert!(UploadArtifactResult::REQUIRES_WRITE);
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"artifact_name": "agent-report", "file_path": "out/report.pdf"}"#;
        let params: UploadArtifactParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.artifact_name, "agent-report");
        assert_eq!(params.file_path, "out/report.pdf");
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = UploadArtifactParams {
            artifact_name: "agent-report".to_string(),
            file_path: "out/report.pdf".to_string(),
        };
        let result: UploadArtifactResult = params.try_into().unwrap();
        assert_eq!(result.name, "upload-artifact");
        assert_eq!(result.artifact_name, "agent-report");
        assert_eq!(result.file_path, "out/report.pdf");
    }

    #[test]
    fn test_validation_rejects_empty_artifact_name() {
        let params = UploadArtifactParams {
            artifact_name: String::new(),
            file_path: "out/report.pdf".to_string(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_starting_with_dot() {
        let params = UploadArtifactParams {
            artifact_name: ".hidden".to_string(),
            file_path: "out/report.pdf".to_string(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_space() {
        let params = UploadArtifactParams {
            artifact_name: "my artifact".to_string(),
            file_path: "out/report.pdf".to_string(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_slash() {
        let params = UploadArtifactParams {
            artifact_name: "my/artifact".to_string(),
            file_path: "out/report.pdf".to_string(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_err());
    }

    #[test]
    fn test_validation_accepts_dotted_artifact_name() {
        let params = UploadArtifactParams {
            artifact_name: "agent.report.v2".to_string(),
            file_path: "out/report.pdf".to_string(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_ok());
    }

    #[test]
    fn test_validation_rejects_empty_file_path() {
        let params = UploadArtifactParams {
            artifact_name: "agent-report".to_string(),
            file_path: String::new(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_err());
    }

    #[test]
    fn test_validation_rejects_path_traversal() {
        let params = UploadArtifactParams {
            artifact_name: "agent-report".to_string(),
            file_path: "../etc/passwd".to_string(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_err());
    }

    #[test]
    fn test_validation_rejects_absolute_path() {
        let params = UploadArtifactParams {
            artifact_name: "agent-report".to_string(),
            file_path: "/etc/passwd".to_string(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_err());
    }

    #[test]
    fn test_validation_rejects_backslash_traversal() {
        let params = UploadArtifactParams {
            artifact_name: "agent-report".to_string(),
            file_path: "src\\..\\secret.txt".to_string(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_err());
    }

    #[test]
    fn test_validation_rejects_dotgit_component() {
        let params = UploadArtifactParams {
            artifact_name: "agent-report".to_string(),
            file_path: ".git/config".to_string(),
        };
        assert!(<UploadArtifactParams as TryInto<UploadArtifactResult>>::try_into(params).is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = UploadArtifactParams {
            artifact_name: "agent-report".to_string(),
            file_path: "out/report.pdf".to_string(),
        };
        let result: UploadArtifactResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r#""name":"upload-artifact""#));
        assert!(json.contains(r#""artifact_name":"agent-report""#));
        assert!(json.contains(r#""file_path":"out/report.pdf""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = UploadArtifactConfig::default();
        assert_eq!(config.max_file_size, 50 * 1024 * 1024);
        assert!(config.allowed_extensions.is_empty());
        assert!(config.allowed_artifact_names.is_empty());
        assert!(config.name_prefix.is_none());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
max-file-size: 1048576
allowed-extensions:
  - .png
  - .pdf
allowed-artifact-names:
  - agent-*
  - report
name-prefix: "agent-"
"#;
        let config: UploadArtifactConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.max_file_size, 1_048_576);
        assert_eq!(config.allowed_extensions, vec![".png", ".pdf"]);
        assert_eq!(config.allowed_artifact_names, vec!["agent-*", "report"]);
        assert_eq!(config.name_prefix, Some("agent-".to_string()));
    }
}
