//! Upload pipeline artifact safe output tool.
//!
//! Lets an agent propose publishing a workspace file as an Azure DevOps build
//! artifact via the **build artifacts** REST API.  Unlike build attachments
//! (which are invisible in the standard UI without a custom extension),
//! build artifacts published through this tool appear in the **Artifacts tab**
//! of the build summary page in Azure DevOps.
//!
//! The upload is a three-step REST flow:
//!
//! 1. **Create container** — `POST /_apis/resources/containers` returns a
//!    `containerId`.
//! 2. **Upload file** — `PUT /_apis/resources/containers/{containerId}` sends
//!    the file bytes with `itemPath` and `scope` query params.
//! 3. **Associate artifact** — `POST /_apis/build/builds/{buildId}/artifacts`
//!    links the container to the build as a named artifact.
//!
//! The flow mirrors `upload-build-attachment`:
//!
//! * **Stage 1 (MCP, in the agent sandbox):** the MCP server validates the
//!   agent-supplied params, resolves `file_path` against the agent's workspace,
//!   rejects symlink escapes / directories, and **copies the file** into the
//!   safe-outputs `output_directory` under a generated unique name.
//! * **Stage 3 (executor, outside the sandbox):** reads the staged file from
//!   `ctx.working_directory.join(staged_file)`, applies operator-supplied
//!   limits (`max-file-size`, `allowed-extensions`, `allowed-artifact-names`,
//!   `allowed-build-ids`, `name-prefix`), resolves the target build ID, and
//!   executes the three-step upload flow.

use ado_aw_derive::SanitizeConfig;
use log::{debug, info, warn};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::sanitize::SanitizeContent;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::tool_result;
use crate::validate::{is_safe_path_segment, is_valid_artifact_name};
use anyhow::{Context, ensure};

/// Parameters for publishing a workspace file as an ADO pipeline artifact.
#[derive(Deserialize, JsonSchema)]
pub struct UploadPipelineArtifactParams {
    /// The build ID to publish the artifact to.  **Omit to target the current
    /// pipeline run** — the executor resolves the build ID from the
    /// `BUILD_BUILDID` environment variable automatically.  When provided,
    /// must be a positive integer.
    pub build_id: Option<i64>,

    /// The artifact name shown in the Artifacts tab.  ADO requires a non-empty
    /// name made of alphanumerics, `-`, `_`, or `.`. Must be 1-100 characters
    /// and must not start with `.`.
    pub artifact_name: String,

    /// Path to the file in the workspace to publish. Must be a relative path
    /// with no directory traversal, no absolute prefix, and no `.git`
    /// segments.
    pub file_path: String,
}

impl Validate for UploadPipelineArtifactParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(id) = self.build_id {
            ensure!(id > 0, "build_id must be positive when specified");
        }

        ensure!(
            self.artifact_name.len() <= 100,
            "artifact_name must be at most 100 characters"
        );
        ensure!(
            !self.artifact_name.starts_with('.'),
            "artifact_name must not start with '.'"
        );
        ensure!(
            is_valid_artifact_name(&self.artifact_name),
            "artifact_name must be non-empty and contain only alphanumeric characters, '-', '_' or '.'"
        );

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

/// Internal params struct for the `tool_result!` macro's `TryFrom` plumbing.
#[derive(Deserialize, JsonSchema)]
struct UploadPipelineArtifactResultFields {
    build_id: Option<i64>,
    artifact_name: String,
    file_path: String,
    staged_file: String,
    file_size: u64,
    staged_sha256: String,
}

impl Validate for UploadPipelineArtifactResultFields {}

tool_result! {
    name = "upload-pipeline-artifact",
    write = true,
    params = UploadPipelineArtifactResultFields,
    default_max = 3,
    /// Result of publishing a workspace file as an ADO pipeline artifact.
    pub struct UploadPipelineArtifactResult {
        /// Build ID the artifact should be published to.  `None` means "current
        /// build" — resolved at execution time from `BUILD_BUILDID`.
        build_id: Option<i64>,
        /// Artifact name as proposed by the agent (pre-prefix).
        artifact_name: String,
        /// Original file path proposed by the agent.
        file_path: String,
        /// Filename of the staged copy inside the safe-outputs directory.
        staged_file: String,
        /// Size in bytes of the staged file at copy time.
        file_size: u64,
        /// SHA-256 hex digest of the staged file recorded at copy time.
        staged_sha256: String,
    }
}

impl SanitizeContent for UploadPipelineArtifactResult {
    fn sanitize_content_fields(&mut self) {
        // All textual fields are strictly validated to safe charsets.
    }
}

impl UploadPipelineArtifactResult {
    /// Construct a result after the agent's file has been staged.
    pub fn new(
        build_id: Option<i64>,
        artifact_name: String,
        file_path: String,
        staged_file: String,
        file_size: u64,
        staged_sha256: String,
    ) -> Self {
        Self {
            name: <Self as crate::safeoutputs::ToolResult>::NAME.to_string(),
            build_id,
            artifact_name,
            file_path,
            staged_file,
            file_size,
            staged_sha256,
        }
    }
}

/// Default maximum file size (50 MB).
pub const PIPELINE_ARTIFACT_DEFAULT_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;

/// Configuration for the upload-pipeline-artifact tool (specified in front
/// matter).
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct UploadPipelineArtifactConfig {
    /// Maximum file size in bytes (default: 50 MB).
    #[serde(default = "default_pipeline_max_file_size", rename = "max-file-size")]
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

    /// Restrict which build IDs the agent may publish to. Empty means any
    /// build ID accessible to the executor's token is allowed. This check
    /// is skipped when `build_id` is omitted (targeting the current build).
    #[serde(default, rename = "allowed-build-ids")]
    pub allowed_build_ids: Vec<i64>,

    /// Prefix prepended to the agent-supplied artifact name before publishing.
    #[serde(default, rename = "name-prefix")]
    pub name_prefix: Option<String>,
}

fn default_pipeline_max_file_size() -> u64 {
    PIPELINE_ARTIFACT_DEFAULT_MAX_FILE_SIZE
}

impl Default for UploadPipelineArtifactConfig {
    fn default() -> Self {
        Self {
            max_file_size: PIPELINE_ARTIFACT_DEFAULT_MAX_FILE_SIZE,
            allowed_extensions: Vec::new(),
            allowed_artifact_names: Vec::new(),
            allowed_build_ids: Vec::new(),
            name_prefix: None,
        }
    }
}

#[async_trait::async_trait]
impl Executor for UploadPipelineArtifactResult {
    fn dry_run_summary(&self) -> String {
        match self.build_id {
            Some(id) => format!(
                "publish '{}' as pipeline artifact '{}' on build #{}",
                self.file_path, self.artifact_name, id
            ),
            None => format!(
                "publish '{}' as pipeline artifact '{}' on current build",
                self.file_path, self.artifact_name
            ),
        }
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        let effective_build_id: i64 = match self.build_id {
            Some(id) => id,
            None => {
                let current = ctx.build_id.context(
                    "build_id was not specified and BUILD_BUILDID is not set — \
                     cannot determine which build to publish the artifact to",
                )?;
                i64::try_from(current).context("BUILD_BUILDID value overflows i64")?
            }
        };

        info!(
            "Publishing '{}' as pipeline artifact '{}' on build #{}{}",
            self.file_path,
            self.artifact_name,
            effective_build_id,
            if self.build_id.is_none() { " (current build)" } else { "" }
        );

        let config: UploadPipelineArtifactConfig = ctx.get_tool_config("upload-pipeline-artifact");

        // ── Build-ID allow-list ──────────────────────────────────────────
        if self.build_id.is_some()
            && !config.allowed_build_ids.is_empty()
            && !config.allowed_build_ids.contains(&effective_build_id)
        {
            return Ok(ExecutionResult::failure(format!(
                "Build ID {} is not in the allowed-build-ids list",
                effective_build_id
            )));
        }

        // ── Name-prefix ─────────────────────────────────────────────────
        if let Some(prefix) = &config.name_prefix {
            if prefix.len() > 50 {
                return Ok(ExecutionResult::failure(format!(
                    "name-prefix '{}...' is too long ({} chars, max 50)",
                    prefix.chars().take(20).collect::<String>(),
                    prefix.len()
                )));
            }
        }
        let final_name = match &config.name_prefix {
            Some(prefix) => format!("{}{}", prefix, self.artifact_name),
            None => self.artifact_name.clone(),
        };
        if final_name.starts_with('.') || final_name.len() > 100 || !is_valid_artifact_name(&final_name) {
            return Ok(ExecutionResult::failure(format!(
                "Resolved artifact name '{}' is not a valid Azure DevOps artifact name",
                final_name
            )));
        }

        // ── Artifact-name allow-list ─────────────────────────────────────
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

        // ── Extension allow-list ─────────────────────────────────────────
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

        // ── Staged file resolution ───────────────────────────────────────
        let staged_path = ctx.working_directory.join(&self.staged_file);
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
        let metadata = std::fs::metadata(&canonical).context("Failed to read file metadata")?;
        if metadata.is_dir() {
            return Ok(ExecutionResult::failure(format!(
                "Staged path '{}' is a directory; upload-pipeline-artifact only supports single files",
                self.staged_file
            )));
        }
        let file_size = metadata.len();

        // ── Integrity checks ─────────────────────────────────────────────
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
                "[dry-run] would publish '{}' ({} bytes) as pipeline artifact '{}' on build #{}{}",
                self.file_path,
                file_size,
                final_name,
                effective_build_id,
                if self.build_id.is_none() { " (current build)" } else { "" }
            )));
        }

        let file_bytes = tokio::fs::read(&canonical).await.context("Failed to read file contents")?;

        let live_hash = crate::hash::sha256_hex(&file_bytes);
        if live_hash != self.staged_sha256 {
            return Ok(ExecutionResult::failure(format!(
                "Staged file SHA-256 mismatch: expected {} (recorded at Stage 1), got {} — \
                 the file may have been tampered with between stages",
                self.staged_sha256, live_hash
            )));
        }

        // ── Resolve ADO context ──────────────────────────────────────────
        let org_url = ctx
            .ado_org_url
            .as_ref()
            .context("AZURE_DEVOPS_ORG_URL not set")?;
        let project = ctx
            .ado_project
            .as_ref()
            .context("SYSTEM_TEAMPROJECT not set")?;
        let project_id = ctx
            .ado_project_id
            .as_ref()
            .context("SYSTEM_TEAMPROJECTID not set — required for pipeline artifact upload")?;
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;

        let client = reqwest::Client::new();

        // Derive a filename from the original file path for use as the
        // itemPath inside the container (e.g. "report.pdf" from "out/report.pdf").
        let filename = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.staged_file);

        // ── Step 1: Create container ─────────────────────────────────────
        // The `scopeIdentifier` query parameter (project GUID) is required for
        // the POST to route correctly in ADO.  Omitting it causes a 405 because
        // the unscoped `_apis/resources/containers` collection does not support
        // POST.  The body only needs the container name; the project scope must
        // be in the query string.
        let container_url = format!(
            "{}/_apis/resources/containers?scopeIdentifier={}&api-version=7.1-preview.4",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project_id, PATH_SEGMENT),
        );
        debug!("Creating container for artifact '{}': {}", final_name, container_url);

        let container_body = serde_json::json!({
            "name": final_name,
        });
        let container_resp = client
            .post(&container_url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&container_body)
            .send()
            .await
            .context("Failed to create artifact container")?;

        if !container_resp.status().is_success() {
            let status = container_resp.status();
            let error_body = container_resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Ok(ExecutionResult::failure(format!(
                "Failed to create artifact container (HTTP {}): {}",
                status, error_body
            )));
        }
        let container_json: serde_json::Value = container_resp
            .json()
            .await
            .context("Failed to parse container creation response")?;
        let container_id = container_json
            .get("containerId")
            .and_then(|v| v.as_u64())
            .context("Container creation response missing 'containerId'")?;
        debug!("Container created: id={}", container_id);

        // ── Step 2: Upload file to container ─────────────────────────────
        // Use `scopeIdentifier` (not `scope`) to match the ADO containers API.
        let upload_url = format!(
            "{}/_apis/resources/containers/{}?itemPath={}/{}&scopeIdentifier={}&api-version=7.1-preview.4",
            org_url.trim_end_matches('/'),
            container_id,
            utf8_percent_encode(&final_name, PATH_SEGMENT),
            utf8_percent_encode(filename, PATH_SEGMENT),
            utf8_percent_encode(project_id, PATH_SEGMENT),
        );
        debug!("Uploading {} bytes to container: {}", file_size, upload_url);

        let upload_resp = client
            .put(&upload_url)
            .header("Content-Type", "application/octet-stream")
            .basic_auth("", Some(token))
            .body(file_bytes)
            .send()
            .await
            .context("Failed to upload file to artifact container")?;

        if !upload_resp.status().is_success() {
            let status = upload_resp.status();
            let error_body = upload_resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Ok(ExecutionResult::failure(format!(
                "Failed to upload file to container (HTTP {}): {}",
                status, error_body
            )));
        }
        debug!("File uploaded to container {}", container_id);

        // ── Step 3: Associate artifact with build ────────────────────────
        let artifact_url = format!(
            "{}/{}/_apis/build/builds/{}/artifacts?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            effective_build_id,
        );
        debug!("Associating artifact '{}' with build #{}: {}", final_name, effective_build_id, artifact_url);

        let artifact_body = serde_json::json!({
            "name": final_name,
            "resource": {
                "data": format!("#/{}/{}", container_id, final_name),
                "type": "container",
            },
        });
        let artifact_resp = client
            .post(&artifact_url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&artifact_body)
            .send()
            .await
            .context("Failed to associate artifact with build")?;

        if artifact_resp.status().is_success() {
            let resp_body: serde_json::Value = artifact_resp.json().await.unwrap_or_else(|e| {
                warn!(
                    "Pipeline artifact created for build #{} but the response JSON could not be parsed: {} — proceeding without download URL",
                    effective_build_id, e
                );
                serde_json::Value::Null
            });
            let download_url = resp_body
                .get("resource")
                .and_then(|r| r.get("downloadUrl"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            info!(
                "Published '{}' as pipeline artifact '{}' on build #{}",
                self.file_path, final_name, effective_build_id
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Published '{}' as pipeline artifact '{}' on build #{}",
                    self.file_path, final_name, effective_build_id
                ),
                serde_json::json!({
                    "build_id": effective_build_id,
                    "artifact_name": final_name,
                    "file_path": self.file_path,
                    "size_bytes": file_size,
                    "download_url": download_url,
                    "project": project,
                }),
            ))
        } else {
            let status = artifact_resp.status();
            let error_body = artifact_resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Ok(ExecutionResult::failure(format!(
                "Failed to associate artifact with build #{} (HTTP {}): {}",
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
        assert_eq!(UploadPipelineArtifactResult::NAME, "upload-pipeline-artifact");
    }

    fn make_params(
        build_id: Option<i64>,
        artifact_name: &str,
        file_path: &str,
    ) -> UploadPipelineArtifactParams {
        UploadPipelineArtifactParams {
            build_id,
            artifact_name: artifact_name.to_string(),
            file_path: file_path.to_string(),
        }
    }

    const DUMMY_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn test_params_validate_accepts_valid() {
        assert!(make_params(Some(1), "agent-report", "out/report.pdf")
            .validate()
            .is_ok());
        assert!(make_params(None, "agent-report", "out/report.pdf")
            .validate()
            .is_ok());
    }

    #[test]
    fn test_validation_rejects_zero_build_id() {
        assert!(make_params(Some(0), "report", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_negative_build_id() {
        assert!(make_params(Some(-1), "report", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_empty_artifact_name() {
        assert!(make_params(None, "", "out/report.pdf").validate().is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_spaces() {
        assert!(make_params(None, "my report", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_leading_dot_artifact_name() {
        assert!(make_params(None, ".hidden", "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_long_artifact_name() {
        let long_name = "a".repeat(101);
        assert!(make_params(None, &long_name, "out/report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_empty_file_path() {
        assert!(make_params(None, "report", "").validate().is_err());
    }

    #[test]
    fn test_validation_rejects_traversal_in_file_path() {
        assert!(make_params(None, "report", "../etc/passwd")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_null_bytes_in_file_path() {
        assert!(make_params(None, "report", "out/report\0.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_validation_rejects_colon_in_file_path() {
        assert!(make_params(None, "report", "C:\\out\\report.pdf")
            .validate()
            .is_err());
    }

    #[test]
    fn test_dry_run_summary() {
        let result = UploadPipelineArtifactResult::new(
            None,
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "staged-abc123.pdf".to_string(),
            1024,
            DUMMY_HASH.to_string(),
        );
        assert!(result.dry_run_summary().contains("agent-report"));
        assert!(result.dry_run_summary().contains("current build"));

        let result_with_id = UploadPipelineArtifactResult::new(
            Some(42),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "staged-abc123.pdf".to_string(),
            1024,
            DUMMY_HASH.to_string(),
        );
        assert!(result_with_id.dry_run_summary().contains("build #42"));
    }

    #[test]
    fn test_config_defaults() {
        let config = UploadPipelineArtifactConfig::default();
        assert_eq!(config.max_file_size, PIPELINE_ARTIFACT_DEFAULT_MAX_FILE_SIZE);
        assert!(config.allowed_extensions.is_empty());
        assert!(config.allowed_artifact_names.is_empty());
        assert!(config.allowed_build_ids.is_empty());
        assert!(config.name_prefix.is_none());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
max-file-size: 10485760
allowed-extensions:
  - .pdf
  - .log
allowed-artifact-names:
  - agent-*
name-prefix: "ci-"
"#;
        let config: UploadPipelineArtifactConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.max_file_size, 10485760);
        assert_eq!(config.allowed_extensions, vec![".pdf", ".log"]);
        assert_eq!(config.allowed_artifact_names, vec!["agent-*"]);
        assert_eq!(config.name_prefix.as_deref(), Some("ci-"));
    }

    #[tokio::test]
    async fn test_dry_run_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged-file.pdf");
        let content = b"test content";
        std::fs::write(&staged, content).unwrap();
        let hash = crate::hash::sha256_hex(content);

        let result = UploadPipelineArtifactResult::new(
            None,
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "staged-file.pdf".to_string(),
            content.len() as u64,
            hash,
        );

        let ctx = ExecutionContext {
            working_directory: dir.path().to_path_buf(),
            build_id: Some(123),
            dry_run: true,
            ..Default::default()
        };

        let exec_result = result.execute_impl(&ctx).await.unwrap();
        assert!(exec_result.success);
        assert!(exec_result.message.contains("[dry-run]"));
        assert!(exec_result.message.contains("agent-report"));
    }

    #[tokio::test]
    async fn test_rejects_file_size_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged-file.pdf");
        std::fs::write(&staged, b"test content").unwrap();

        let result = UploadPipelineArtifactResult::new(
            None,
            "report".to_string(),
            "out/report.pdf".to_string(),
            "staged-file.pdf".to_string(),
            9999, // wrong size
            DUMMY_HASH.to_string(),
        );

        let ctx = ExecutionContext {
            working_directory: dir.path().to_path_buf(),
            build_id: Some(123),
            dry_run: false,
            ..Default::default()
        };

        let exec_result = result.execute_impl(&ctx).await.unwrap();
        assert!(!exec_result.success);
        assert!(exec_result.message.contains("differs from size"));
    }

    #[tokio::test]
    async fn test_rejects_exceeding_max_file_size() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged-file.pdf");
        let content = b"x";
        std::fs::write(&staged, content).unwrap();

        let result = UploadPipelineArtifactResult::new(
            None,
            "report".to_string(),
            "out/report.pdf".to_string(),
            "staged-file.pdf".to_string(),
            1,
            crate::hash::sha256_hex(content),
        );

        let mut ctx = ExecutionContext {
            working_directory: dir.path().to_path_buf(),
            build_id: Some(123),
            dry_run: false,
            ..Default::default()
        };
        // Set max-file-size to 0 to trigger rejection
        ctx.tool_configs.insert(
            "upload-pipeline-artifact".to_string(),
            serde_json::json!({"max-file-size": 0}),
        );

        let exec_result = result.execute_impl(&ctx).await.unwrap();
        assert!(!exec_result.success);
        assert!(exec_result.message.contains("exceeds maximum"));
    }

    #[tokio::test]
    async fn test_rejects_disallowed_build_id() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged-file.pdf");
        std::fs::write(&staged, b"test").unwrap();

        let result = UploadPipelineArtifactResult::new(
            Some(999),
            "report".to_string(),
            "out/report.pdf".to_string(),
            "staged-file.pdf".to_string(),
            4,
            DUMMY_HASH.to_string(),
        );

        let mut ctx = ExecutionContext {
            working_directory: dir.path().to_path_buf(),
            dry_run: false,
            ..Default::default()
        };
        ctx.tool_configs.insert(
            "upload-pipeline-artifact".to_string(),
            serde_json::json!({"allowed-build-ids": [123, 456]}),
        );

        let exec_result = result.execute_impl(&ctx).await.unwrap();
        assert!(!exec_result.success);
        assert!(exec_result.message.contains("not in the allowed-build-ids"));
    }

    #[tokio::test]
    async fn test_rejects_disallowed_extension() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged-file.exe");
        let content = b"test";
        std::fs::write(&staged, content).unwrap();

        let result = UploadPipelineArtifactResult::new(
            None,
            "report".to_string(),
            "out/report.exe".to_string(),
            "staged-file.exe".to_string(),
            content.len() as u64,
            crate::hash::sha256_hex(content),
        );

        let mut ctx = ExecutionContext {
            working_directory: dir.path().to_path_buf(),
            build_id: Some(123),
            dry_run: false,
            ..Default::default()
        };
        ctx.tool_configs.insert(
            "upload-pipeline-artifact".to_string(),
            serde_json::json!({"allowed-extensions": [".pdf", ".log"]}),
        );

        let exec_result = result.execute_impl(&ctx).await.unwrap();
        assert!(!exec_result.success);
        assert!(exec_result.message.contains("not in the allowed list"));
    }
}
