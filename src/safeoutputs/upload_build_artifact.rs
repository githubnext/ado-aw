//! Upload build artifact safe output tool.
//!
//! Lets an agent propose attaching a workspace file to an Azure DevOps build
//! via the **build attachments** REST API
//! (`PUT /_apis/build/builds/{buildId}/attachments/{type}/{name}`).
//!
//! When `build_id` is omitted the executor targets the **current** pipeline run
//! by resolving `BUILD_BUILDID` from the environment — so the same tool covers
//! both "attach to the run I'm part of" and "attach to a different build".
//!
//! The flow mirrors `create-pull-request`:
//!
//! * **Stage 1 (MCP, in the agent sandbox):** the MCP server validates the
//!   agent-supplied params, resolves `file_path` against the agent's workspace,
//!   rejects symlink escapes / directories, and **copies the file** into the
//!   safe-outputs `output_directory` under a generated unique name. The
//!   sandbox workspace is no longer accessible at execution time, so the file
//!   has to be staged where Stage 3 can find it.
//! * **Stage 3 (executor, outside the sandbox):** reads the staged file from
//!   `ctx.working_directory.join(staged_file)`, applies operator-supplied
//!   limits (`max-file-size`, `allowed-extensions`, `allowed-artifact-names`,
//!   `allowed-build-ids`, `name-prefix`, `attachment-type`), resolves the
//!   target build ID, and PUTs the bytes to the ADO build attachments endpoint.

use ado_aw_derive::SanitizeConfig;
use log::{debug, info, warn};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::sanitize::SanitizeContent;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::tool_result;
use crate::validate::{is_safe_path_segment, is_valid_version};
use anyhow::{Context, ensure};

/// Parameters for attaching a workspace file to an ADO build.
#[derive(Deserialize, JsonSchema)]
pub struct UploadBuildArtifactParams {
    /// The build ID to attach the file to.  **Omit to target the current
    /// pipeline run** — the executor resolves the build ID from the
    /// `BUILD_BUILDID` environment variable automatically.  When provided,
    /// must be a positive integer.
    pub build_id: Option<i64>,

    /// The artifact name to attach the file under. Used as the `{name}`
    /// segment of the ADO build attachments URL. ADO requires a non-empty
    /// name made of alphanumerics, `-`, `_`, or `.`. Must be 1-100 characters
    /// and must not start with `.`.
    pub artifact_name: String,

    /// Path to the file in the workspace to attach. Must be a relative path
    /// with no directory traversal, no absolute prefix, and no `.git`
    /// segments.
    pub file_path: String,
}

impl Validate for UploadBuildArtifactParams {
    fn validate(&self) -> anyhow::Result<()> {
        // build_id: if present, must be positive.
        if let Some(id) = self.build_id {
            ensure!(id > 0, "build_id must be positive when specified");
        }

        // artifact_name: ADO requires non-empty, ≤100 chars, charset
        // [A-Za-z0-9._-], and (per our hardening) no leading `.`.
        // `is_valid_version` is reused here — its charset rule happens to
        // match ADO's artifact-name requirements exactly.
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

/// Internal params struct mirroring `UploadBuildArtifactResult` fields for the
/// `tool_result!` macro's `TryFrom` plumbing. The actual MCP parameters come
/// from `UploadBuildArtifactParams`; this struct only exists so the macro can
/// wire up `Validate`/`TryFrom` while the real construction happens in MCP via
/// `UploadBuildArtifactResult::new()` after the file is staged.
#[derive(Deserialize, JsonSchema)]
struct UploadBuildArtifactResultFields {
    build_id: Option<i64>,
    artifact_name: String,
    file_path: String,
    staged_file: String,
    file_size: u64,
}

impl Validate for UploadBuildArtifactResultFields {}

tool_result! {
    name = "upload-build-artifact",
    write = true,
    params = UploadBuildArtifactResultFields,
    default_max = 3,
    /// Result of attaching a workspace file to an ADO build.
    pub struct UploadBuildArtifactResult {
        /// Build ID the file should be attached to.  `None` means "current
        /// build" — resolved at execution time from `BUILD_BUILDID`.
        build_id: Option<i64>,
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

impl SanitizeContent for UploadBuildArtifactResult {
    fn sanitize_content_fields(&mut self) {
        // All textual fields are strictly validated to safe charsets; no
        // additional textual sanitization is required.
    }
}

impl UploadBuildArtifactResult {
    /// Construct a result after the agent's file has been staged into the
    /// safe-outputs directory.
    pub fn new(
        build_id: Option<i64>,
        artifact_name: String,
        file_path: String,
        staged_file: String,
        file_size: u64,
    ) -> Self {
        Self {
            name: <Self as crate::safeoutputs::ToolResult>::NAME.to_string(),
            build_id,
            artifact_name,
            file_path,
            staged_file,
            file_size,
        }
    }
}

const DEFAULT_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
const DEFAULT_ATTACHMENT_TYPE: &str = "agent-artifact";

/// Configuration for the upload-build-artifact tool (specified in front
/// matter).
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   upload-build-artifact:
///     max-file-size: 52428800
///     allowed-extensions:
///       - .png
///       - .pdf
///       - .log
///     allowed-artifact-names:
///       - agent-*
///     allowed-build-ids:
///       - 12345
///       - 67890
///     name-prefix: "agent-"
///     attachment-type: "agent-artifact"
///     max: 5
/// ```
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct UploadBuildArtifactConfig {
    /// Maximum file size in bytes (default: 50 MB).
    #[serde(default = "default_max_file_size", rename = "max-file-size")]
    pub max_file_size: u64,

    /// Allowed file extensions (e.g., `[".png", ".pdf"]`). Empty means all
    /// extensions are allowed.
    #[serde(default, rename = "allowed-extensions")]
    pub allowed_extensions: Vec<String>,

    /// Restrict which artifact names may be published. Empty means any name
    /// (subject to charset rules) is allowed. Entries ending with `*` match
    /// by prefix; otherwise the comparison is exact.
    #[serde(default, rename = "allowed-artifact-names")]
    pub allowed_artifact_names: Vec<String>,

    /// Restrict which build IDs the agent may attach to. Empty means any
    /// build ID accessible to the executor's token is allowed. This check
    /// is skipped when `build_id` is omitted (targeting the current build).
    #[serde(default, rename = "allowed-build-ids")]
    pub allowed_build_ids: Vec<i64>,

    /// Prefix prepended to the agent-supplied artifact name before publishing.
    #[serde(default, rename = "name-prefix")]
    pub name_prefix: Option<String>,

    /// Value to use for the `{type}` segment of the build attachments URL.
    /// Defaults to `agent-artifact`. Must satisfy the same charset rules as
    /// an artifact name.
    #[serde(default, rename = "attachment-type")]
    pub attachment_type: Option<String>,
}

fn default_max_file_size() -> u64 {
    DEFAULT_MAX_FILE_SIZE
}

impl Default for UploadBuildArtifactConfig {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            allowed_extensions: Vec::new(),
            allowed_artifact_names: Vec::new(),
            allowed_build_ids: Vec::new(),
            name_prefix: None,
            attachment_type: None,
        }
    }
}

#[async_trait::async_trait]
impl Executor for UploadBuildArtifactResult {
    fn dry_run_summary(&self) -> String {
        match self.build_id {
            Some(id) => format!(
                "attach '{}' as artifact '{}' to build #{}",
                self.file_path, self.artifact_name, id
            ),
            None => format!(
                "attach '{}' as artifact '{}' to current build",
                self.file_path, self.artifact_name
            ),
        }
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        // Resolve the effective build ID: explicit value from the agent, or
        // fall back to the current pipeline's BUILD_BUILDID.
        let effective_build_id: i64 = match self.build_id {
            Some(id) => id,
            None => {
                let current = ctx.build_id.context(
                    "build_id was not specified and BUILD_BUILDID is not set — \
                     cannot determine which build to attach the artifact to",
                )?;
                i64::try_from(current).context("BUILD_BUILDID value overflows i64")?
            }
        };

        info!(
            "Attaching '{}' as artifact '{}' to build #{}{}",
            self.file_path,
            self.artifact_name,
            effective_build_id,
            if self.build_id.is_none() { " (current build)" } else { "" }
        );
        debug!(
            "upload-build-artifact: build_id={}, artifact_name='{}', file_path='{}'",
            effective_build_id, self.artifact_name, self.file_path
        );

        let config: UploadBuildArtifactConfig = ctx.get_tool_config("upload-build-artifact");
        debug!("Max file size: {} bytes", config.max_file_size);
        debug!("Allowed extensions: {:?}", config.allowed_extensions);
        debug!("Allowed artifact names: {:?}", config.allowed_artifact_names);
        debug!("Allowed build IDs: {:?}", config.allowed_build_ids);

        // Check build-id allow-list (if configured).  When the agent omitted
        // build_id (targeting the current build), skip this check — the
        // current build is implicitly trusted because the operator chose to
        // run this pipeline.
        if self.build_id.is_some()
            && !config.allowed_build_ids.is_empty()
            && !config.allowed_build_ids.contains(&effective_build_id)
        {
            return Ok(ExecutionResult::failure(format!(
                "Build ID {} is not in the allowed-build-ids list",
                effective_build_id
            )));
        }

        // Apply name-prefix and re-validate the resulting name's charset (the
        // prefix itself is operator-controlled and sanitized at config load,
        // but we still defensively check the joined string).
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
        // Uses Path::extension() for a precise match rather than suffix
        // matching on the full path — this prevents "log" from matching
        // filenames like "catalog" when the operator omits the leading dot.
        if !config.allowed_extensions.is_empty() {
            let file_ext = std::path::Path::new(&self.file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let has_valid_ext = config.allowed_extensions.iter().any(|ext| {
                ext.trim_start_matches('.').eq_ignore_ascii_case(file_ext)
            });
            if !has_valid_ext {
                return Ok(ExecutionResult::failure(format!(
                    "File '{}' has an extension not in the allowed list: {:?}",
                    self.file_path, config.allowed_extensions
                )));
            }
        }

        // Resolve the attachment type. Operator config wins; otherwise use the
        // default. Re-validate the charset defensively even though
        // `SanitizeConfig` strips control characters, because the type is
        // interpolated into a URL path segment. `is_valid_version` is reused
        // here — its [A-Za-z0-9._-] charset matches the attachment-type
        // requirements.
        let attachment_type = config
            .attachment_type
            .as_deref()
            .unwrap_or(DEFAULT_ATTACHMENT_TYPE);
        if attachment_type.is_empty()
            || attachment_type.len() > 100
            || !is_valid_version(attachment_type)
        {
            return Ok(ExecutionResult::failure(format!(
                "attachment-type '{}' is not a valid value (must be non-empty, ≤100 chars, alphanumeric/'-'/'_'/'.')",
                attachment_type
            )));
        }
        debug!("Attachment type: {}", attachment_type);

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
                "Staged path '{}' is a directory; upload-build-artifact only supports single files",
                self.staged_file
            )));
        }
        let file_size = metadata.len();
        debug!("File size: {} bytes", file_size);

        // Integrity check: compare the live file size against the size
        // recorded in Stage 1. A mismatch means the staged file was modified
        // between stages — fail hard rather than uploading tampered content.
        if file_size != self.file_size {
            return Ok(ExecutionResult::failure(format!(
                "Staged file size ({} bytes) differs from size recorded at Stage 1 ({} bytes) — \
                 the file may have been modified between stages",
                file_size, self.file_size
            )));
        }

        if file_size > config.max_file_size {
            return Ok(ExecutionResult::failure(format!(
                "File size ({} bytes) exceeds maximum allowed size ({} bytes)",
                file_size, config.max_file_size
            )));
        }

        if ctx.dry_run {
            return Ok(ExecutionResult::success(format!(
                "[dry-run] would attach '{}' ({} bytes) as artifact '{}' to build #{}{}",
                self.file_path,
                file_size,
                final_name,
                effective_build_id,
                if self.build_id.is_none() { " (current build)" } else { "" }
            )));
        }

        // Read the file bytes for upload (after the dry-run guard to avoid
        // reading up to 50 MB into memory only to discard it).  Uses async I/O
        // to avoid blocking the tokio runtime for large files.
        let file_bytes = tokio::fs::read(&canonical).await.context("Failed to read file contents")?;

        // Resolve the ADO API context (org URL, project, token).
        let org_url = ctx
            .ado_org_url
            .as_ref()
            .context("AZURE_DEVOPS_ORG_URL not set")?;
        let project = ctx
            .ado_project
            .as_ref()
            .context("SYSTEM_TEAMPROJECT not set")?;
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;
        debug!("ADO org: {}, project: {}", org_url, project);

        // Build the build-attachments URL.
        // PUT {org_url}/{project}/_apis/build/builds/{buildId}/attachments/{type}/{name}?api-version=7.1-preview.1
        let url = format!(
            "{}/{}/_apis/build/builds/{}/attachments/{}/{}?api-version=7.1-preview.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            effective_build_id,
            utf8_percent_encode(attachment_type, PATH_SEGMENT),
            utf8_percent_encode(&final_name, PATH_SEGMENT),
        );
        debug!("Attachment URL: {}", url);

        let client = reqwest::Client::new();
        info!(
            "Uploading {} bytes to build #{} as attachment '{}/{}'",
            file_size, effective_build_id, attachment_type, final_name
        );
        let response = client
            .put(&url)
            .header("Content-Type", "application/octet-stream")
            .basic_auth("", Some(token))
            .body(file_bytes)
            .send()
            .await
            .context("Failed to send attachment upload request to Azure DevOps")?;

        if response.status().is_success() {
            let resp_body: serde_json::Value = response.json().await.unwrap_or_else(|e| {
                warn!(
                    "Build attachment uploaded for build #{} but the response JSON could not be parsed: {} — proceeding without attachment URL",
                    effective_build_id, e
                );
                serde_json::Value::Null
            });
            let attachment_url = resp_body
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            info!(
                "Attached '{}' to build #{} as '{}'",
                self.file_path, effective_build_id, final_name
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Attached '{}' to build #{} as artifact '{}'",
                    self.file_path, effective_build_id, final_name
                ),
                serde_json::json!({
                    "build_id": effective_build_id,
                    "artifact_name": final_name,
                    "attachment_type": attachment_type,
                    "file_path": self.file_path,
                    "size_bytes": file_size,
                    "attachment_url": attachment_url,
                    "project": project,
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Ok(ExecutionResult::failure(format!(
                "Failed to attach artifact to build #{} (HTTP {}): {}",
                effective_build_id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(UploadBuildArtifactResult::NAME, "upload-build-artifact");
    }

    fn make_params(
        build_id: Option<i64>,
        artifact_name: &str,
        file_path: &str,
    ) -> UploadBuildArtifactParams {
        UploadBuildArtifactParams {
            build_id,
            artifact_name: artifact_name.to_string(),
            file_path: file_path.to_string(),
        }
    }

    #[test]
    fn test_params_deserializes_with_build_id() {
        let json =
            r#"{"build_id": 1234, "artifact_name": "agent-report", "file_path": "out/report.pdf"}"#;
        let params: UploadBuildArtifactParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.build_id, Some(1234));
        assert_eq!(params.artifact_name, "agent-report");
        assert_eq!(params.file_path, "out/report.pdf");
    }

    #[test]
    fn test_params_deserializes_without_build_id() {
        let json = r#"{"artifact_name": "agent-report", "file_path": "out/report.pdf"}"#;
        let params: UploadBuildArtifactParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.build_id, None);
        assert_eq!(params.artifact_name, "agent-report");
        assert_eq!(params.file_path, "out/report.pdf");
    }

    #[test]
    fn test_params_validate_accepts_valid_with_build_id() {
        assert!(make_params(Some(1), "agent-report", "out/report.pdf")
            .validate()
            .is_ok());
    }

    #[test]
    fn test_params_validate_accepts_valid_without_build_id() {
        assert!(make_params(None, "agent-report", "out/report.pdf")
            .validate()
            .is_ok());
    }

    #[test]
    fn test_validation_rejects_zero_build_id() {
        assert!(make_params(Some(0), "agent-report", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_negative_build_id() {
        assert!(make_params(Some(-1), "agent-report", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_empty_artifact_name() {
        assert!(make_params(Some(1), "", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_starting_with_dot() {
        assert!(make_params(Some(1), ".hidden", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_space() {
        assert!(make_params(Some(1), "my artifact", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_slash() {
        assert!(make_params(Some(1), "my/artifact", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_accepts_dotted_artifact_name() {
        assert!(make_params(Some(1), "agent.report.v2", "out/report.pdf")
            .validate()
            .is_ok());
    }

    #[test]
    fn test_validation_rejects_empty_file_path() {
        assert!(make_params(Some(1), "agent-report", "")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_path_traversal() {
        assert!(make_params(Some(1), "agent-report", "../etc/passwd")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_absolute_path() {
        assert!(make_params(Some(1), "agent-report", "/etc/passwd")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_backslash_traversal() {
        assert!(make_params(Some(1), "agent-report", "src\\..\\secret.txt")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_dotgit_component() {
        assert!(make_params(Some(1), "agent-report", ".git/config")
            .validate()
            .is_err());
    }

    #[test]
    fn test_result_serializes_correctly_with_build_id() {
        let result = UploadBuildArtifactResult::new(
            Some(1234),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "upload-build-artifact-agent-report-1234.bin".to_string(),
            42,
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r#""name":"upload-build-artifact""#));
        assert!(json.contains(r#""build_id":1234"#));
        assert!(json.contains(r#""artifact_name":"agent-report""#));
        assert!(json.contains(r#""file_path":"out/report.pdf""#));
        assert!(json.contains(r#""staged_file":"upload-build-artifact-agent-report-1234.bin""#));
    }

    #[test]
    fn test_result_serializes_correctly_without_build_id() {
        let result = UploadBuildArtifactResult::new(
            None,
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "upload-build-artifact-agent-report-1234.bin".to_string(),
            42,
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r#""name":"upload-build-artifact""#));
        assert!(json.contains(r#""build_id":null"#));
        assert!(json.contains(r#""artifact_name":"agent-report""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = UploadBuildArtifactConfig::default();
        assert_eq!(config.max_file_size, 50 * 1024 * 1024);
        assert!(config.allowed_extensions.is_empty());
        assert!(config.allowed_artifact_names.is_empty());
        assert!(config.allowed_build_ids.is_empty());
        assert!(config.name_prefix.is_none());
        assert!(config.attachment_type.is_none());
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
allowed-build-ids:
  - 100
  - 200
name-prefix: "agent-"
attachment-type: "agent-artifact"
"#;
        let config: UploadBuildArtifactConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.max_file_size, 1_048_576);
        assert_eq!(config.allowed_extensions, vec![".png", ".pdf"]);
        assert_eq!(config.allowed_artifact_names, vec!["agent-*", "report"]);
        assert_eq!(config.allowed_build_ids, vec![100, 200]);
        assert_eq!(config.name_prefix, Some("agent-".to_string()));
        assert_eq!(config.attachment_type, Some("agent-artifact".to_string()));
    }

    fn make_ctx(working_directory: std::path::PathBuf, dry_run: bool) -> ExecutionContext {
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
            dry_run,
            build_id: None,
            build_number: None,
            build_reason: None,
            definition_name: None,
            source_branch: None,
            source_branch_name: None,
            source_version: None,
            triggered_by_build_id: None,
            triggered_by_definition_name: None,
            triggered_by_build_number: None,
            triggered_by_project_id: None,
            pull_request_id: None,
            pull_request_source_branch: None,
            pull_request_target_branch: None,
        }
    }

    #[tokio::test]
    async fn test_executor_reads_staged_file_with_explicit_build_id() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-artifact-agent-report-deadbeef.pdf";
        std::fs::write(dir.path().join(staged), b"%PDF-1.4 hello").unwrap();

        let result = UploadBuildArtifactResult::new(
            Some(1234),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            14,
        );
        let ctx = make_ctx(dir.path().to_path_buf(), true);
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(outcome.success, "expected success, got: {:?}", outcome);
        assert!(outcome.message.contains("[dry-run]"));
        assert!(outcome.message.contains("agent-report"));
        assert!(outcome.message.contains("#1234"));
    }

    #[tokio::test]
    async fn test_executor_resolves_current_build_when_build_id_omitted() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-artifact-agent-report-feedf00d.pdf";
        std::fs::write(dir.path().join(staged), b"%PDF-1.4 hello").unwrap();

        let result = UploadBuildArtifactResult::new(
            None,
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            14,
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.build_id = Some(5678);
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(outcome.success, "expected success, got: {:?}", outcome);
        assert!(outcome.message.contains("[dry-run]"));
        assert!(outcome.message.contains("#5678"));
        assert!(outcome.message.contains("(current build)"));
    }

    #[tokio::test]
    async fn test_executor_fails_when_build_id_omitted_and_not_in_env() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-artifact-agent-report-cafef00d.pdf";
        std::fs::write(dir.path().join(staged), b"hello").unwrap();

        let result = UploadBuildArtifactResult::new(
            None,
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            5,
        );
        let ctx = make_ctx(dir.path().to_path_buf(), true);
        // ctx.build_id is None — no BUILD_BUILDID
        let err = result.execute_impl(&ctx).await.unwrap_err();
        assert!(
            err.to_string().contains("BUILD_BUILDID"),
            "expected BUILD_BUILDID error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_executor_rejects_missing_staged_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = UploadBuildArtifactResult::new(
            Some(1234),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "does-not-exist.pdf".to_string(),
            0,
        );
        let ctx = make_ctx(dir.path().to_path_buf(), true);
        let err = result.execute_impl(&ctx).await.unwrap_err();
        assert!(
            err.to_string().contains("canonicalize"),
            "expected canonicalize error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_executor_rejects_disallowed_build_id() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-artifact-agent-report-cafef00d.pdf";
        std::fs::write(dir.path().join(staged), b"hello").unwrap();

        let result = UploadBuildArtifactResult::new(
            Some(999),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            5,
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.tool_configs.insert(
            "upload-build-artifact".to_string(),
            serde_json::json!({ "allowed-build-ids": [100, 200] }),
        );
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(!outcome.success);
        assert!(
            outcome.message.contains("allowed-build-ids"),
            "expected allowed-build-ids rejection, got: {}",
            outcome.message
        );
    }

    #[tokio::test]
    async fn test_executor_skips_build_id_allowlist_for_current_build() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-artifact-agent-report-aabbccdd.pdf";
        std::fs::write(dir.path().join(staged), b"hello").unwrap();

        let result = UploadBuildArtifactResult::new(
            None, // targeting current build
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            5,
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.build_id = Some(999); // current build not in allowed list
        ctx.tool_configs.insert(
            "upload-build-artifact".to_string(),
            serde_json::json!({ "allowed-build-ids": [100, 200] }),
        );
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(
            outcome.success,
            "current build should bypass allowed-build-ids, got: {:?}",
            outcome
        );
    }

    #[tokio::test]
    async fn test_executor_accepts_allowed_build_id() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-artifact-agent-report-feedf00d.pdf";
        std::fs::write(dir.path().join(staged), b"hello").unwrap();

        let result = UploadBuildArtifactResult::new(
            Some(100),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            5,
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.tool_configs.insert(
            "upload-build-artifact".to_string(),
            serde_json::json!({ "allowed-build-ids": [100, 200] }),
        );
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(outcome.success, "expected success, got: {:?}", outcome);
    }

    #[tokio::test]
    async fn test_executor_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-artifact-agent-big-deadbeef.bin";
        std::fs::write(dir.path().join(staged), vec![0u8; 1024]).unwrap();

        let result = UploadBuildArtifactResult::new(
            Some(1),
            "agent-big".to_string(),
            "out/big.bin".to_string(),
            staged.to_string(),
            1024,
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.tool_configs.insert(
            "upload-build-artifact".to_string(),
            serde_json::json!({ "max-file-size": 100 }),
        );
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(!outcome.success);
        assert!(
            outcome.message.contains("exceeds maximum"),
            "expected size rejection, got: {}",
            outcome.message
        );
    }

    #[tokio::test]
    async fn test_executor_rejects_tampered_staged_file() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-artifact-agent-report-tampered.pdf";
        // Write 100 bytes but record 50 in the result — simulates tampering.
        std::fs::write(dir.path().join(staged), vec![0u8; 100]).unwrap();

        let result = UploadBuildArtifactResult::new(
            Some(1),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            50, // mismatched size
        );
        let ctx = make_ctx(dir.path().to_path_buf(), true);
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(!outcome.success);
        assert!(
            outcome.message.contains("differs from size recorded"),
            "expected integrity failure, got: {}",
            outcome.message
        );
    }

    #[test]
    fn test_dry_run_summary_with_build_id() {
        let result = UploadBuildArtifactResult::new(
            Some(42),
            "report".to_string(),
            "out/report.pdf".to_string(),
            "staged.pdf".to_string(),
            100,
        );
        let summary = result.dry_run_summary();
        assert!(summary.contains("#42"));
        assert!(summary.contains("report"));
    }

    #[test]
    fn test_dry_run_summary_without_build_id() {
        let result = UploadBuildArtifactResult::new(
            None,
            "report".to_string(),
            "out/report.pdf".to_string(),
            "staged.pdf".to_string(),
            100,
        );
        let summary = result.dry_run_summary();
        assert!(summary.contains("current build"));
        assert!(summary.contains("report"));
    }
}
