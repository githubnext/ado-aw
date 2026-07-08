//! Upload build attachment safe output tool.
//!
//! Lets an agent propose attaching a workspace file to the **current** Azure
//! DevOps build as a build attachment.
//!
//! ## Which API this uses
//!
//! A "build attachment" is created via the **DistributedTask timeline
//! attachment** API — the same mechanism the agent's
//! `##vso[task.addattachment type=…;name=…]<path>` logging command uses under
//! the hood:
//!
//! ```text
//! PUT {org}/{projectId}/_apis/distributedtask/hubs/build
//!     /plans/{planId}/timelines/{timelineId}/records/{recordId}
//!     /attachments/{type}/{name}?api-version=7.1
//! ```
//!
//! The resulting object *is* a build attachment: it is stored once by
//! `{type}`/`{name}` and is read back through the Build ▸ Attachments
//! **Get/List** API (and rendered by ADO extensions that register for a given
//! attachment `type`). We call the REST endpoint directly (rather than printing
//! the `##vso` command) so the executor gets a response body — it can surface
//! `attachment_url` and report a deterministic success/failure instead of the
//! fire-and-forget `##vso` behaviour.
//!
//! **Current-run only.** `planId` / `timelineId` / `recordId` exist only for the
//! job that is executing, so a build attachment can only be added to the
//! **current** run. There is no ADO API to attach to an arbitrary other build.
//! The `build_id` param is therefore accepted only when it is omitted or equals
//! the current run's `BUILD_BUILDID`; any other value is rejected. (The old
//! `/_apis/build/builds/{id}/attachments/…` PUT route this tool used never
//! existed — the Build ▸ Attachments API is read-only.)
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
//!   `name-prefix`, `attachment-type`), and PUTs the bytes to the timeline
//!   attachment endpoint for the current job's record.

use ado_aw_derive::SanitizeConfig;
use log::{debug, info, warn};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::SanitizeContent;
use crate::secure::{ArtifactName, StrictRelativePath};
use crate::tool_result;
use crate::validate::is_valid_artifact_name;
use anyhow::{Context, ensure};

/// Parameters for attaching a workspace file to an ADO build.
#[derive(Deserialize, JsonSchema)]
pub struct UploadBuildAttachmentParams {
    /// Build ID to attach the file to. **Omit to target the current pipeline
    /// run** (recommended) — the executor resolves it from `BUILD_BUILDID`.
    /// Build attachments can only be added to the current run, so if you set
    /// this it MUST equal the current build id; any other (still positive)
    /// value is rejected at execution time.
    pub build_id: Option<i64>,

    /// The artifact name to attach the file under. Used as the `{name}`
    /// segment of the attachment path. ADO requires a non-empty
    /// name made of alphanumerics, `-`, `_`, or `.`. Must be 1-100 characters
    /// and must not start with `.`. Validated at deserialization time via
    /// [`ArtifactName`].
    pub artifact_name: ArtifactName,

    /// Path to the file in the workspace to attach. Must be a relative path
    /// with no directory traversal, no absolute prefix, and no `.git`
    /// segments. Validated at deserialization time via [`StrictRelativePath`].
    pub file_path: StrictRelativePath,
}

impl Validate for UploadBuildAttachmentParams {
    fn validate(&self) -> anyhow::Result<()> {
        // build_id: if present, must be positive. (artifact_name and file_path
        // are structurally validated by their newtypes at deserialization.)
        if let Some(id) = self.build_id {
            ensure!(id > 0, "build_id must be positive when specified");
        }
        Ok(())
    }
}

/// Internal params struct mirroring `UploadBuildAttachmentResult` fields for the
/// `tool_result!` macro's `TryFrom` plumbing. The actual MCP parameters come
/// from `UploadBuildAttachmentParams`; this struct only exists so the macro can
/// wire up `Validate`/`TryFrom` while the real construction happens in MCP via
/// `UploadBuildAttachmentResult::new()` after the file is staged.
#[derive(Deserialize, JsonSchema)]
struct UploadBuildAttachmentResultFields {
    build_id: Option<i64>,
    artifact_name: String,
    file_path: String,
    staged_file: String,
    file_size: u64,
    staged_sha256: String,
}

impl Validate for UploadBuildAttachmentResultFields {}

tool_result! {
    name = "upload-build-attachment",
    write = true,
    params = UploadBuildAttachmentResultFields,
    default_max = 3,
    /// Result of attaching a workspace file to an ADO build.
    pub struct UploadBuildAttachmentResult {
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
        /// SHA-256 hex digest of the staged file recorded at copy time.
        /// Stage 3 re-hashes the file and rejects mismatches — catches
        /// same-size file swaps between stages.
        staged_sha256: String,
    }
}

impl SanitizeContent for UploadBuildAttachmentResult {
    fn sanitize_content_fields(&mut self) {
        // All textual fields are strictly validated to safe charsets; no
        // additional textual sanitization is required.
    }
}

impl UploadBuildAttachmentResult {
    /// Construct a result after the agent's file has been staged into the
    /// safe-outputs directory.
    pub fn new(
        build_id: Option<i64>,
        artifact_name: String,
        file_path: String,
        staged_file: String,
        file_size: u64,
        staged_sha256: String,
    ) -> Self {
        Self {
            name: <Self as crate::safe_outputs::ToolResult>::NAME.to_string(),
            build_id,
            artifact_name,
            file_path,
            staged_file,
            file_size,
            staged_sha256,
        }
    }
}

/// Default maximum file size for upload-build-attachment (50 MB).
/// Also used by the MCP handler as the Stage 1 staging cap.
pub const DEFAULT_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;
const DEFAULT_ATTACHMENT_TYPE: &str = "agent-artifact";

/// Configuration for the upload-build-attachment tool (specified in front
/// matter).
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   upload-build-attachment:
///     max-file-size: 52428800
///     allowed-extensions:
///       - .png
///       - .pdf
///       - .log
///     allowed-artifact-names:
///       - agent-*
///     name-prefix: "agent-"
///     attachment-type: "agent-artifact"
///     max: 5
/// ```
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct UploadBuildAttachmentConfig {
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

impl Default for UploadBuildAttachmentConfig {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            allowed_extensions: Vec::new(),
            allowed_artifact_names: Vec::new(),
            name_prefix: None,
            attachment_type: None,
        }
    }
}

#[async_trait::async_trait]
impl Executor for UploadBuildAttachmentResult {
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
        // Resolve the current run's build ID. A build attachment can only ever
        // be added to the *current* job's timeline record (see module docs), so
        // `build_id`, when the agent supplies it, must match the current run.
        let current_build_id: Option<i64> = match ctx.build_id {
            Some(current) => {
                Some(i64::try_from(current).context("BUILD_BUILDID value overflows i64")?)
            }
            None => None,
        };
        let effective_build_id: i64 = match (self.build_id, current_build_id) {
            // Agent supplied a build_id that differs from the current run — not
            // possible for a build attachment; fail with a clear message.
            (Some(requested), Some(current)) if requested != current => {
                return Ok(ExecutionResult::failure(format!(
                    "build_id {requested} does not match the current build ({current}). Build \
                     attachments can only be added to the current run — omit build_id (or set it \
                     to {current}) to attach to this build."
                )));
            }
            (Some(requested), Some(_current)) => requested,
            // Agent supplied a build_id but the current build is unknown; we
            // cannot prove it targets the current run, so refuse.
            (Some(requested), None) => {
                return Ok(ExecutionResult::failure(format!(
                    "build_id {requested} was specified but the current build id (BUILD_BUILDID) \
                     is not set, so it cannot be confirmed to target the current run. Build \
                     attachments can only be added to the current run — omit build_id."
                )));
            }
            (None, Some(current)) => current,
            (None, None) => {
                return Ok(ExecutionResult::failure(
                    "Cannot attach a build attachment: BUILD_BUILDID is not set, so the current \
                     run cannot be determined."
                        .to_string(),
                ));
            }
        };

        info!(
            "Attaching '{}' as artifact '{}' to the current build #{}",
            self.file_path, self.artifact_name, effective_build_id,
        );
        debug!(
            "upload-build-attachment: build_id={}, artifact_name='{}', file_path='{}'",
            effective_build_id, self.artifact_name, self.file_path
        );

        let config: UploadBuildAttachmentConfig = ctx.get_tool_config("upload-build-attachment");
        debug!("Max file size: {} bytes", config.max_file_size);
        debug!("Allowed extensions: {:?}", config.allowed_extensions);
        debug!(
            "Allowed artifact names: {:?}",
            config.allowed_artifact_names
        );

        // Validate name-prefix length before applying. A long prefix would
        // be caught later by the final_name.len() > 100 check, but rejecting
        // early gives operators a clearer error message.
        if let Some(prefix) = &config.name_prefix
            && prefix.len() > 50
        {
            return Ok(ExecutionResult::failure(format!(
                "name-prefix '{}...' is too long ({} chars, max 50)",
                prefix.chars().take(20).collect::<String>(),
                prefix.len()
            )));
        }

        // Apply name-prefix and re-validate the resulting name's charset (the
        // prefix itself is operator-controlled and sanitized at config load,
        // but we still defensively check the joined string).
        let final_name = match &config.name_prefix {
            Some(prefix) => format!("{}{}", prefix, self.artifact_name),
            None => self.artifact_name.clone(),
        };
        if final_name.starts_with('.')
            || final_name.len() > 100
            || !is_valid_artifact_name(&final_name)
        {
            return Ok(ExecutionResult::failure(format!(
                "Resolved artifact name '{}' is not a valid Azure DevOps artifact name",
                final_name
            )));
        }
        debug!("Final artifact name (after prefix): {}", final_name);

        // Check artifact-name allow-list (if configured).
        if !config.allowed_artifact_names.is_empty() {
            let allowed = config
                .allowed_artifact_names
                .iter()
                .any(|pattern| super::name_matches_pattern(&final_name, pattern));
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
            let has_valid_ext = config
                .allowed_extensions
                .iter()
                .any(|ext| ext.trim_start_matches('.').eq_ignore_ascii_case(file_ext));
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
        // interpolated into a URL path segment.
        let attachment_type = config
            .attachment_type
            .as_deref()
            .unwrap_or(DEFAULT_ATTACHMENT_TYPE);
        if attachment_type.is_empty()
            || attachment_type.starts_with('.')
            || attachment_type.len() > 100
            || !is_valid_artifact_name(attachment_type)
        {
            return Ok(ExecutionResult::failure(format!(
                "attachment-type '{}' is not a valid value (must be non-empty, ≤100 chars, no leading '.', alphanumeric/'-'/'_'/'.')",
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
                "Staged path '{}' is a directory; upload-build-attachment only supports single files",
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
                "[dry-run] would attach '{}' ({} bytes) as artifact '{}' to the current build #{}",
                self.file_path, file_size, final_name, effective_build_id,
            )));
        }

        // Read the file bytes for upload (after the dry-run guard to avoid
        // reading up to 50 MB into memory only to discard it).  Uses async I/O
        // to avoid blocking the tokio runtime for large files.
        let file_bytes = tokio::fs::read(&canonical)
            .await
            .context("Failed to read file contents")?;

        // SHA-256 integrity check: verify the staged file hasn't been swapped
        // between stages.  This catches same-size replacements that the size
        // check alone would miss.
        let live_hash = crate::hash::sha256_hex(&file_bytes);
        if live_hash != self.staged_sha256 {
            return Ok(ExecutionResult::failure(format!(
                "Staged file SHA-256 mismatch: expected {} (recorded at Stage 1), got {} — \
                 the file may have been tampered with between stages",
                self.staged_sha256, live_hash
            )));
        }

        // Resolve the ADO API context (collection URL, token) and the current
        // job's timeline coordinates. A build attachment is a DistributedTask
        // **timeline attachment** on the running job's record (the same object
        // `##vso[task.addattachment]` creates), so we need the plan / timeline /
        // record IDs of the current run — these come from the auto-injected
        // SYSTEM_* predefined variables and only exist for the current job.
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
        // The DistributedTask hub route's `{scopeIdentifier}` is the **project
        // GUID** (SYSTEM_TEAMPROJECTID), not the project name — the name routes
        // but is rejected with HTTP 400.
        let project_id = ctx.ado_project_id.as_ref().context(
            "SYSTEM_TEAMPROJECTID is not set — required as the scope identifier for the build \
             attachment (timeline attachment) API",
        )?;
        let plan_id = ctx.plan_id.as_ref().context(
            "SYSTEM_PLANID is not set — required to attach to the current build (build attachments \
             are written to the current job's timeline record)",
        )?;
        let timeline_id = ctx.timeline_id.as_ref().context(
            "SYSTEM_TIMELINEID is not set — required to attach to the current build (build \
             attachments are written to the current job's timeline record)",
        )?;
        let record_id = ctx.job_id.as_ref().context(
            "SYSTEM_JOBID is not set — required to attach to the current build (build attachments \
             are written to the current job's timeline record)",
        )?;
        debug!("ADO org: {}, project: {} ({})", org_url, project, project_id);

        // Build the DistributedTask timeline-attachment URL. This is the write
        // side of a build attachment — the object is read back via the Build ▸
        // Attachments Get/List API by `{type}`/`{name}`. The `build` hub covers
        // build/YAML pipelines. The route's `{scopeIdentifier}` is the project
        // **GUID**; released api-version is 7.1.
        // PUT {org}/{projectId}/_apis/distributedtask/hubs/build/plans/{planId}
        //     /timelines/{timelineId}/records/{recordId}
        //     /attachments/{type}/{name}?api-version=7.1
        let url = format!(
            "{}/{}/_apis/distributedtask/hubs/build/plans/{}/timelines/{}/records/{}/attachments/{}/{}?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project_id, PATH_SEGMENT),
            utf8_percent_encode(plan_id, PATH_SEGMENT),
            utf8_percent_encode(timeline_id, PATH_SEGMENT),
            utf8_percent_encode(record_id, PATH_SEGMENT),
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
            // The timeline-attachment response carries the attachment URL under
            // `_links.self.href` (there is no top-level `url` field); fall back
            // to a top-level `url` defensively for forward compatibility.
            let attachment_url = resp_body
                .get("_links")
                .and_then(|l| l.get("self"))
                .and_then(|s| s.get("href"))
                .and_then(|v| v.as_str())
                .or_else(|| resp_body.get("url").and_then(|v| v.as_str()))
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

    fn make_params(
        build_id: Option<i64>,
        artifact_name: &str,
        file_path: &str,
    ) -> UploadBuildAttachmentParams {
        UploadBuildAttachmentParams {
            build_id,
            artifact_name: artifact_name
                .try_into()
                .expect("test artifact_name must be valid"),
            file_path: file_path.try_into().expect("test file_path must be valid"),
        }
    }

    /// Attempt to deserialize params from raw field values. Returns the serde
    /// error when a newtype field (`artifact_name` / `file_path`) rejects its
    /// value at parse time — used by the rejection tests that previously relied
    /// on `validate()`. Note the trade-off: invalid path/name input now surfaces
    /// as a serde deserialization error (wrapping the newtype validator's
    /// message) instead of an explicit `validate()` error; the underlying
    /// `ArtifactName::parse` / `StrictRelativePath::parse` messages still
    /// describe the specific failure.
    fn try_params(
        build_id: Option<i64>,
        artifact_name: &str,
        file_path: &str,
    ) -> Result<UploadBuildAttachmentParams, serde_json::Error> {
        let value = serde_json::json!({
            "build_id": build_id,
            "artifact_name": artifact_name,
            "file_path": file_path,
        });
        serde_json::from_value(value)
    }

    /// Dummy SHA-256 hash for tests that use dry_run=true (hash check is
    /// skipped on the dry-run path).
    const DUMMY_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn test_params_deserializes_with_build_id() {
        let json =
            r#"{"build_id": 1234, "artifact_name": "agent-report", "file_path": "out/report.pdf"}"#;
        let params: UploadBuildAttachmentParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.build_id, Some(1234));
        assert_eq!(params.artifact_name.as_str(), "agent-report");
        assert_eq!(params.file_path.as_str(), "out/report.pdf");
    }

    #[test]
    fn test_params_deserializes_without_build_id() {
        let json = r#"{"artifact_name": "agent-report", "file_path": "out/report.pdf"}"#;
        let params: UploadBuildAttachmentParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.build_id, None);
        assert_eq!(params.artifact_name.as_str(), "agent-report");
        assert_eq!(params.file_path.as_str(), "out/report.pdf");
    }

    #[test]
    fn test_params_validate_accepts_valid_with_build_id() {
        assert!(
            make_params(Some(1), "agent-report", "out/report.pdf")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn test_params_validate_accepts_valid_without_build_id() {
        assert!(
            make_params(None, "agent-report", "out/report.pdf")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn test_validation_rejects_zero_build_id() {
        let err = make_params(Some(0), "agent-report", "out/report.pdf")
            .validate()
            .unwrap_err();
        assert!(
            err.to_string().contains("build_id must be positive"),
            "expected 'build_id must be positive' error, got: {}",
            err
        );
    }

    #[test]
    fn test_validation_rejects_negative_build_id() {
        let err = make_params(Some(-1), "agent-report", "out/report.pdf")
            .validate()
            .unwrap_err();
        assert!(
            err.to_string().contains("build_id must be positive"),
            "expected 'build_id must be positive' error, got: {}",
            err
        );
    }

    #[test]
    fn test_validation_rejects_empty_artifact_name() {
        let err = try_params(Some(1), "", "out/report.pdf")
            .err()
            .expect("expected artifact_name validation error");
        assert!(
            err.to_string().contains("artifact_name"),
            "expected 'artifact_name' in error, got: {}",
            err
        );
    }

    #[test]
    fn test_validation_rejects_artifact_name_starting_with_dot() {
        let err = try_params(Some(1), ".hidden", "out/report.pdf")
            .err()
            .expect("expected artifact_name dot-prefix validation error");
        assert!(
            err.to_string().contains("must not start with"),
            "expected 'must not start with' in error, got: {}",
            err
        );
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_space() {
        assert!(try_params(Some(1), "my artifact", "out/report.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_slash() {
        assert!(try_params(Some(1), "my/artifact", "out/report.pdf").is_err());
    }

    #[test]
    fn test_validation_accepts_dotted_artifact_name() {
        assert!(
            make_params(Some(1), "agent.report.v2", "out/report.pdf")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn test_validation_rejects_empty_file_path() {
        assert!(try_params(Some(1), "agent-report", "").is_err());
    }

    #[test]
    fn test_validation_rejects_path_traversal() {
        assert!(try_params(Some(1), "agent-report", "../etc/passwd").is_err());
    }

    #[test]
    fn test_validation_rejects_absolute_path() {
        assert!(try_params(Some(1), "agent-report", "/etc/passwd").is_err());
    }

    #[test]
    fn test_validation_rejects_backslash_traversal() {
        assert!(try_params(Some(1), "agent-report", "src\\..\\secret.txt").is_err());
    }

    #[test]
    fn test_validation_rejects_dotgit_component() {
        assert!(try_params(Some(1), "agent-report", ".git/config").is_err());
    }

    #[test]
    fn test_validation_rejects_newline_in_file_path() {
        assert!(try_params(Some(1), "agent-report", "out\n/report.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_carriage_return_in_file_path() {
        assert!(try_params(Some(1), "agent-report", "out\r/report.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_pipeline_command_sequences_in_file_path() {
        assert!(
            try_params(
                Some(1),
                "agent-report",
                "##vso[task.setvariable variable=EXPLOIT]value.txt"
            )
            .is_err()
        );
        assert!(try_params(Some(1), "agent-report", "##[error]value.txt").is_err());
    }

    #[test]
    fn test_result_serializes_correctly_with_build_id() {
        let result = UploadBuildAttachmentResult::new(
            Some(1234),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "upload-build-attachment-agent-report-1234.bin".to_string(),
            42,
            DUMMY_HASH.to_string(),
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r#""name":"upload-build-attachment""#));
        assert!(json.contains(r#""build_id":1234"#));
        assert!(json.contains(r#""artifact_name":"agent-report""#));
        assert!(json.contains(r#""file_path":"out/report.pdf""#));
        assert!(json.contains(r#""staged_file":"upload-build-attachment-agent-report-1234.bin""#));
    }

    #[test]
    fn test_result_serializes_correctly_without_build_id() {
        let result = UploadBuildAttachmentResult::new(
            None,
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "upload-build-attachment-agent-report-1234.bin".to_string(),
            42,
            DUMMY_HASH.to_string(),
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r#""name":"upload-build-attachment""#));
        assert!(json.contains(r#""build_id":null"#));
        assert!(json.contains(r#""artifact_name":"agent-report""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = UploadBuildAttachmentConfig::default();
        assert_eq!(config.max_file_size, 50 * 1024 * 1024);
        assert!(config.allowed_extensions.is_empty());
        assert!(config.allowed_artifact_names.is_empty());
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
name-prefix: "agent-"
attachment-type: "agent-artifact"
"#;
        let config: UploadBuildAttachmentConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.max_file_size, 1_048_576);
        assert_eq!(config.allowed_extensions, vec![".png", ".pdf"]);
        assert_eq!(config.allowed_artifact_names, vec!["agent-*", "report"]);
        assert_eq!(config.name_prefix, Some("agent-".to_string()));
        assert_eq!(config.attachment_type, Some("agent-artifact".to_string()));
    }

    /// A stray `allowed-build-ids` key (e.g. from a pre-migration lock the
    /// codemod hasn't rewritten) must be tolerated and ignored, not error.
    #[test]
    fn test_config_ignores_stray_allowed_build_ids() {
        let yaml = "allowed-build-ids:\n  - 100\n  - 200\n";
        let config: UploadBuildAttachmentConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.max_file_size, DEFAULT_MAX_FILE_SIZE);
    }

    fn make_ctx(working_directory: std::path::PathBuf, dry_run: bool) -> ExecutionContext {
        ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            ado_project_id: None,
            access_token: None,
            github_token: None,
            source_directory: working_directory.clone(),
            working_directory,
            tool_configs: std::collections::HashMap::new(),
            debug_enabled_tools: std::collections::HashSet::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: std::collections::HashMap::new(),
            agent_stats: None,
            dry_run,
            build_id: Some(1234),
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
            build_container_id: None,
            plan_id: None,
            timeline_id: None,
            job_id: None,
            uploaded_pipeline_artifact_keys: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
            agent_last_author: None,
        }
    }

    #[tokio::test]
    async fn test_executor_reads_staged_file_with_matching_build_id() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-agent-report-deadbeef.pdf";
        std::fs::write(dir.path().join(staged), b"%PDF-1.4 hello").unwrap();

        // Explicit build_id equal to the current run (make_ctx sets 1234) — OK.
        let result = UploadBuildAttachmentResult::new(
            Some(1234),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            14,
            DUMMY_HASH.to_string(),
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
        let staged = "upload-build-attachment-agent-report-feedf00d.pdf";
        std::fs::write(dir.path().join(staged), b"%PDF-1.4 hello").unwrap();

        let result = UploadBuildAttachmentResult::new(
            None,
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            14,
            DUMMY_HASH.to_string(),
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.build_id = Some(5678);
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(outcome.success, "expected success, got: {:?}", outcome);
        assert!(outcome.message.contains("[dry-run]"));
        assert!(outcome.message.contains("#5678"));
        assert!(outcome.message.contains("current build"));
    }

    #[tokio::test]
    async fn test_executor_rejects_build_id_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-agent-report-cafef00d.pdf";
        std::fs::write(dir.path().join(staged), b"hello").unwrap();

        // Explicit build_id 999 while the current run is 1234 — impossible for
        // a build attachment, so it must fail with a clear message.
        let result = UploadBuildAttachmentResult::new(
            Some(999),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            5,
            DUMMY_HASH.to_string(),
        );
        let ctx = make_ctx(dir.path().to_path_buf(), true);
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(!outcome.success);
        assert!(
            outcome.message.contains("does not match the current build"),
            "expected current-build mismatch rejection, got: {}",
            outcome.message
        );
    }

    #[tokio::test]
    async fn test_executor_fails_when_build_id_omitted_and_not_in_env() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-agent-report-cafef00d.pdf";
        std::fs::write(dir.path().join(staged), b"hello").unwrap();

        let result = UploadBuildAttachmentResult::new(
            None,
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            5,
            DUMMY_HASH.to_string(),
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.build_id = None; // no BUILD_BUILDID
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(!outcome.success);
        assert!(
            outcome.message.contains("BUILD_BUILDID"),
            "expected BUILD_BUILDID failure, got: {}",
            outcome.message
        );
    }

    #[tokio::test]
    async fn test_executor_fails_when_plan_id_missing() {
        // SHA-256 of b"hello" so the non-dry-run integrity check passes and we
        // reach the timeline-coordinate resolution.
        const HELLO_SHA: &str =
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-agent-report-0badf00d.txt";
        std::fs::write(dir.path().join(staged), b"hello").unwrap();

        let result = UploadBuildAttachmentResult::new(
            None,
            "agent-report".to_string(),
            "out/report.txt".to_string(),
            staged.to_string(),
            5,
            HELLO_SHA.to_string(),
        );
        // Non-dry-run with org/token set but SYSTEM_PLANID absent — must fail
        // before any HTTP call, naming the missing variable.
        let mut ctx = make_ctx(dir.path().to_path_buf(), false);
        ctx.ado_org_url = Some("https://dev.azure.com/org".to_string());
        ctx.ado_project = Some("Proj".to_string());
        ctx.ado_project_id = Some("00000000-0000-0000-0000-0000000000aa".to_string());
        ctx.access_token = Some("token".to_string());
        ctx.timeline_id = Some("00000000-0000-0000-0000-000000000001".to_string());
        ctx.job_id = Some("00000000-0000-0000-0000-000000000002".to_string());
        // plan_id intentionally left None.
        let err = result.execute_impl(&ctx).await.unwrap_err();
        assert!(
            err.to_string().contains("SYSTEM_PLANID"),
            "expected SYSTEM_PLANID error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_executor_rejects_missing_staged_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = UploadBuildAttachmentResult::new(
            Some(1234),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            "does-not-exist.pdf".to_string(),
            0,
            DUMMY_HASH.to_string(),
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
    async fn test_executor_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-agent-big-deadbeef.bin";
        std::fs::write(dir.path().join(staged), vec![0u8; 1024]).unwrap();

        let result = UploadBuildAttachmentResult::new(
            None,
            "agent-big".to_string(),
            "out/big.bin".to_string(),
            staged.to_string(),
            1024,
            DUMMY_HASH.to_string(),
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.tool_configs.insert(
            "upload-build-attachment".to_string(),
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
    async fn test_executor_rejects_disallowed_extension() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-agent-report-aabb1122.exe";
        std::fs::write(dir.path().join(staged), b"MZ hello").unwrap();

        let result = UploadBuildAttachmentResult::new(
            None,
            "agent-report".to_string(),
            "out/report.exe".to_string(),
            staged.to_string(),
            8,
            DUMMY_HASH.to_string(),
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.tool_configs.insert(
            "upload-build-attachment".to_string(),
            serde_json::json!({ "allowed-extensions": [".pdf", ".png"] }),
        );
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(!outcome.success);
        assert!(
            outcome
                .message
                .contains("extension not in the allowed list"),
            "expected extension rejection, got: {}",
            outcome.message
        );
    }

    #[tokio::test]
    async fn test_executor_rejects_disallowed_artifact_name() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-evil-report-ccdd3344.pdf";
        std::fs::write(dir.path().join(staged), b"hello").unwrap();

        let result = UploadBuildAttachmentResult::new(
            None,
            "evil-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            5,
            DUMMY_HASH.to_string(),
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.tool_configs.insert(
            "upload-build-attachment".to_string(),
            serde_json::json!({ "allowed-artifact-names": ["agent-*", "safe-report"] }),
        );
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(!outcome.success);
        assert!(
            outcome.message.contains("not in the allowed list"),
            "expected artifact-name rejection, got: {}",
            outcome.message
        );
    }

    #[tokio::test]
    async fn test_executor_applies_name_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-report-eeff5566.pdf";
        std::fs::write(dir.path().join(staged), b"hello").unwrap();

        let result = UploadBuildAttachmentResult::new(
            None,
            "report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            5,
            DUMMY_HASH.to_string(),
        );
        let mut ctx = make_ctx(dir.path().to_path_buf(), true);
        ctx.tool_configs.insert(
            "upload-build-attachment".to_string(),
            serde_json::json!({ "name-prefix": "agent-" }),
        );
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(outcome.success, "expected success, got: {:?}", outcome);
        // The dry-run message should contain the prefixed name.
        assert!(
            outcome.message.contains("agent-report"),
            "expected prefixed name 'agent-report' in message, got: {}",
            outcome.message
        );
    }

    #[tokio::test]
    async fn test_executor_rejects_tampered_staged_file() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-agent-report-tampered.pdf";
        // Write 100 bytes but record 50 in the result — simulates tampering.
        std::fs::write(dir.path().join(staged), vec![0u8; 100]).unwrap();

        let result = UploadBuildAttachmentResult::new(
            None,
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            50, // mismatched size
            DUMMY_HASH.to_string(),
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

    #[tokio::test]
    async fn test_executor_rejects_sha256_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let staged = "upload-build-attachment-agent-report-sha-mismatch.pdf";
        let content = b"real file content";
        std::fs::write(dir.path().join(staged), content).unwrap();

        // Record the hash of different content — same size but wrong hash.
        let wrong_hash = crate::hash::sha256_hex(b"wrong file content");

        let result = UploadBuildAttachmentResult::new(
            Some(1),
            "agent-report".to_string(),
            "out/report.pdf".to_string(),
            staged.to_string(),
            content.len() as u64,
            wrong_hash,
        );
        // dry_run = false so the hash check executes (it's after the
        // dry-run guard). The failure fires before any HTTP call.
        let mut ctx = make_ctx(dir.path().to_path_buf(), false);
        ctx.build_id = Some(1);
        ctx.ado_org_url = Some("https://dev.azure.com/test".to_string());
        ctx.ado_project = Some("TestProject".to_string());
        ctx.access_token = Some("fake-token".to_string());
        let outcome = result.execute_impl(&ctx).await.unwrap();
        assert!(!outcome.success);
        assert!(
            outcome.message.contains("SHA-256 mismatch"),
            "expected SHA-256 mismatch failure, got: {}",
            outcome.message
        );
    }

    #[test]
    fn test_dry_run_summary_with_build_id() {
        let result = UploadBuildAttachmentResult::new(
            Some(42),
            "report".to_string(),
            "out/report.pdf".to_string(),
            "staged.pdf".to_string(),
            100,
            DUMMY_HASH.to_string(),
        );
        let summary = result.dry_run_summary();
        assert!(summary.contains("#42"));
        assert!(summary.contains("report"));
    }

    #[test]
    fn test_dry_run_summary_without_build_id() {
        let result = UploadBuildAttachmentResult::new(
            None,
            "report".to_string(),
            "out/report.pdf".to_string(),
            "staged.pdf".to_string(),
            100,
            DUMMY_HASH.to_string(),
        );
        let summary = result.dry_run_summary();
        assert!(summary.contains("current build"));
        assert!(summary.contains("report"));
    }
}
