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
use crate::validate::{
    contains_newline, contains_pipeline_command, is_safe_path_segment, is_valid_version,
};
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
        // artifact_name: ADO requires non-empty, ≤100 chars, charset
        // [A-Za-z0-9._-], and (per our hardening) no leading `.`.
        // `is_valid_version` enforces the non-empty + charset rules.
        ensure!(
            self.artifact_name.len() <= 100,
            "artifact_name must be at most 100 characters"
        );
        ensure!(
            !self.artifact_name.starts_with('.'),
            "artifact_name must not start with '.'"
        );
        ensure!(
            is_valid_version(&self.artifact_name),
            "artifact_name must be non-empty and contain only alphanumeric characters, '-', '_' or '.'"
        );

        // file_path: must be relative, with no traversal, no absolute prefix,
        // no `.git`/hidden segments, no null bytes, no drive-letter colons.
        // Splitting on both separators and checking each segment with
        // `is_safe_path_segment` covers: empty (catches absolute paths),
        // `..`, embedded `/` or `\`, and leading `.` (which catches `.git`
        // and other dotfiles).
        ensure!(!self.file_path.is_empty(), "file_path must not be empty");
        ensure!(
            !self.file_path.contains('\0'),
            "file_path must not contain null bytes"
        );
        ensure!(
            !self.file_path.contains(':'),
            "file_path must not contain ':'"
        );
        for component in self.file_path.split(['/', '\\']) {
            ensure!(
                is_safe_path_segment(component),
                "file_path component '{}' is not a safe path segment (no empty, '..', or leading '.' allowed)",
                component
            );
        }
        Ok(())
    }
}

/// Internal params struct mirroring `UploadArtifactResult` fields for the
/// `tool_result!` macro's `TryFrom` plumbing. The actual MCP parameters come
/// from `UploadArtifactParams`; this struct only exists so the macro can wire
/// up `Validate`/`TryFrom` while the real construction happens in MCP via
/// `UploadArtifactResult::new()` after the file is staged into the safe
/// outputs directory.
#[derive(Deserialize, JsonSchema)]
struct UploadArtifactResultFields {
    artifact_name: String,
    file_path: String,
    staged_file: String,
    file_size: u64,
}

impl Validate for UploadArtifactResultFields {}

tool_result! {
    name = "upload-artifact",
    write = true,
    params = UploadArtifactResultFields,
    /// Result of publishing a workspace file as a pipeline artifact.
    pub struct UploadArtifactResult {
        /// Artifact name as proposed by the agent (pre-prefix).
        artifact_name: String,
        /// Original file path proposed by the agent (used for display and the
        /// extension-allowlist check).
        file_path: String,
        /// Filename of the staged copy inside the safe-outputs directory.
        /// Stage 1 (MCP) copies the agent's file into the safe-outputs dir
        /// under this name; Stage 3 reads it back from
        /// `ctx.working_directory.join(staged_file)` because the agent's
        /// sandbox workspace is no longer accessible by then.
        staged_file: String,
        /// Size in bytes of the staged file at copy time.
        file_size: u64,
    }
}

impl SanitizeContent for UploadArtifactResult {
    fn sanitize_content_fields(&mut self) {
        // All textual fields are strictly validated to safe charsets; no
        // additional textual sanitization is required.
    }
}

impl UploadArtifactResult {
    /// Construct a result after the agent's file has been staged into the
    /// safe-outputs directory.
    pub fn new(
        artifact_name: String,
        file_path: String,
        staged_file: String,
        file_size: u64,
    ) -> Self {
        Self {
            name: <Self as crate::safeoutputs::ToolResult>::NAME.to_string(),
            artifact_name,
            file_path,
            staged_file,
            file_size,
        }
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
        if final_name.starts_with('.') || final_name.len() > 100 || !is_valid_version(&final_name)
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

        // Resolve the staged file inside the safe-outputs working directory.
        // Stage 1 (MCP) copied the agent's file there under `self.staged_file`;
        // the sandbox workspace where the original lived is no longer
        // accessible. Canonicalize and verify it stays inside
        // `working_directory` so a malicious staged_file value can't escape
        // (defense in depth — MCP generates the name itself).
        let staged_path = ctx.working_directory.join(&self.staged_file);
        debug!("Staged file path: {}", staged_path.display());

        let canonical = staged_path.canonicalize().context(
            "Failed to canonicalize staged file path — file may be missing or contains broken symlinks",
        )?;
        let canonical_base = ctx
            .working_directory
            .canonicalize()
            .context("Failed to canonicalize working directory")?;
        if !canonical.starts_with(&canonical_base) {
            return Ok(ExecutionResult::failure(format!(
                "Staged file '{}' resolves outside the safe-outputs directory",
                self.staged_file
            )));
        }

        // Reject directories defensively — the staged entry must always be a
        // single file (Stage 1 only copies single files).
        let metadata = std::fs::metadata(&canonical).context("Failed to read file metadata")?;
        if metadata.is_dir() {
            return Ok(ExecutionResult::failure(format!(
                "Staged path '{}' is a directory; the upload-artifact tool only supports single files",
                self.staged_file
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

        // Scan text files for ADO pipeline-command sequences (`##vso[`,
        // `##[`). Binary files (where from_utf8 fails) skip this check;
        // the build agent doesn't parse logging commands out of binary
        // file contents.
        let file_bytes = std::fs::read(&canonical).context("Failed to read file contents")?;
        if let Ok(text) = std::str::from_utf8(&file_bytes)
            && contains_pipeline_command(text)
        {
            return Ok(ExecutionResult::failure(format!(
                "File '{}' contains an ADO pipeline command sequence ('##vso[' or '##[')",
                self.file_path
            )));
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
        if contains_newline(&abs_path) || abs_path.contains(']') {
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

    fn make_params(artifact_name: &str, file_path: &str) -> UploadArtifactParams {
        UploadArtifactParams {
            artifact_name: artifact_name.to_string(),
            file_path: file_path.to_string(),
        }
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"artifact_name": "agent-report", "file_path": "out/report.pdf"}"#;
        let params: UploadArtifactParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.artifact_name, "agent-report");
        assert_eq!(params.file_path, "out/report.pdf");
    }

    #[test]
    fn test_params_validate_accepts_valid() {
        assert!(make_params("agent-report", "out/report.pdf").validate().is_ok());
    }

    #[test]
    fn test_validation_rejects_empty_artifact_name() {
        assert!(make_params("", "out/report.pdf").validate().is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_starting_with_dot() {
        assert!(make_params(".hidden", "out/report.pdf").validate().is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_space() {
        assert!(make_params("my artifact", "out/report.pdf").validate().is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_slash() {
        assert!(make_params("my/artifact", "out/report.pdf").validate().is_err());
    }

    #[test]
    fn test_validation_accepts_dotted_artifact_name() {
        assert!(make_params("agent.report.v2", "out/report.pdf").validate().is_ok());
    }

    #[test]
    fn test_validation_rejects_empty_file_path() {
        assert!(make_params("agent-report", "").validate().is_err());
    }

    #[test]
    fn test_validation_rejects_path_traversal() {
        assert!(make_params("agent-report", "../etc/passwd").validate().is_err());
    }

    #[test]
    fn test_validation_rejects_absolute_path() {
        assert!(make_params("agent-report", "/etc/passwd").validate().is_err());
    }

    #[test]
    fn test_validation_rejects_backslash_traversal() {
        assert!(make_params("agent-report", "src\\..\\secret.txt").validate().is_err());
    }

    #[test]
    fn test_validation_rejects_dotgit_component() {
        assert!(make_params("agent-report", ".git/config").validate().is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let result = UploadArtifactResult::new(
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "upload-artifact-agent-report-1234.bin".to_string(),
            42,
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r#""name":"upload-artifact""#));
        assert!(json.contains(r#""artifact_name":"agent-report""#));
        assert!(json.contains(r#""file_path":"out/report.pdf""#));
        assert!(json.contains(r#""staged_file":"upload-artifact-agent-report-1234.bin""#));
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

    fn make_ctx(working_directory: std::path::PathBuf) -> ExecutionContext {
        ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            source_directory: working_directory.clone(),
            working_directory,
            tool_configs: std::collections::HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: std::collections::HashMap::new(),
            agent_stats: None,
            dry_run: true,
        }
    }

    #[tokio::test]
    async fn test_executor_reads_staged_file_from_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-artifact-agent-report-deadbeef.pdf";
        std::fs::write(dir.path().join(staged), b"%PDF-1.4 hello").unwrap();

        let result = UploadArtifactResult::new(
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            14,
        );
        let ctx = make_ctx(dir.path().to_path_buf());
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(outcome.success, "expected success, got: {:?}", outcome);
        assert!(outcome.message.contains("[dry-run]"));
        assert!(outcome.message.contains("agent-report"));
    }

    #[tokio::test]
    async fn test_executor_rejects_missing_staged_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = UploadArtifactResult::new(
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "does-not-exist.pdf".to_string(),
            0,
        );
        let ctx = make_ctx(dir.path().to_path_buf());
        let err = result.execute_impl(&ctx).await.unwrap_err();
        assert!(
            err.to_string().contains("canonicalize"),
            "expected canonicalize error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_executor_rejects_pipeline_command_in_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-artifact-agent-report-cafef00d.log";
        std::fs::write(dir.path().join(staged), b"hello\n##vso[task.setvariable]x").unwrap();

        let result = UploadArtifactResult::new(
            "agent-report".to_string(),
            "out/report.log".to_string(),
            staged.to_string(),
            32,
        );
        let ctx = make_ctx(dir.path().to_path_buf());
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(!outcome.success);
        assert!(
            outcome.message.contains("##vso["),
            "expected ##vso[ rejection, got: {}",
            outcome.message
        );
    }
}
