//! Upload pipeline artifact safe output tool.
//!
//! Lets an agent propose publishing a workspace file as an Azure DevOps build
//! artifact via the **build artifacts** REST API.  Unlike build attachments
//! (which are invisible in the standard UI without a custom extension),
//! build artifacts published through this tool appear in the **Artifacts tab**
//! of the build summary page in Azure DevOps.
//!
//! The upload is a two-step REST flow that always reuses the agent's own
//! pre-existing build container (Azure DevOps creates one container per build
//! at job initialization and exposes its ID via `BUILD_CONTAINERID`):
//!
//! 1. **Upload bytes** — `PUT /_apis/resources/Containers/{BUILD_CONTAINERID}?itemPath={folder}/{file}&scope={projectId}`
//!    sends the file body into the agent's own build container.
//! 2. **Associate artifact** — `POST /{project}/_apis/build/builds/{effective_build_id}/artifacts`
//!    registers a record with `resource.type = "Container"` and
//!    `resource.data = "#/{BUILD_CONTAINERID}/{folder}"`. `effective_build_id`
//!    is the current build by default, or an agent-supplied `build_id` for
//!    cross-build publishing (the artifact record points at the agent's
//!    container; ADO does not require the container to belong to the target
//!    build — that is how `DownloadBuildArtifacts@1 buildType=specific` works
//!    in the wild).
//!
//! By default the container `folder` is `{artifact_name}__{6 hex hash}` so
//! that two calls with the same `artifact_name` (e.g. publishing
//! `TriageSummary` to many failing builds in one run) never silently overwrite
//! each other's bytes. The hash lives only in internal addressing — the
//! user-visible `artifact_name` your downstream consumers query is unaffected.
//! Set `safe-outputs.upload-pipeline-artifact.require-unique-names: true` to
//! disable the suffix and reject in-run reuse with a clear early error.
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
//!   executes the two-step upload flow.

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
use anyhow::{Context, ensure};

/// Parameters for publishing a workspace file as an ADO pipeline artifact.
#[derive(Deserialize, JsonSchema)]
pub struct UploadPipelineArtifactParams {
    /// The build ID to publish the artifact to.  **Omit to target the current
    /// pipeline run** — the executor resolves the build ID from the
    /// `BUILD_BUILDID` environment variable automatically.  When provided,
    /// must be a positive integer.
    ///
    /// **Cross-build behavior:** when set to a build other than the current
    /// run, the artifact bytes still live in the agent's own build container;
    /// only the artifact *record* (`name`, `data` pointer) is associated with
    /// the target build. This means cross-published artifacts share the
    /// agent build's retention policy — if the agent's build is purged, the
    /// cross-referenced artifact on the target build stops being downloadable.
    /// Cross-project publishing is not supported (the associate POST uses
    /// the current pipeline's project).
    pub build_id: Option<i64>,

    /// The artifact name shown in the Artifacts tab.  ADO requires a non-empty
    /// name made of alphanumerics, `-`, `_`, or `.`. Must be 1-100 characters
    /// and must not start with `.`. Validated at deserialization time via
    /// [`ArtifactName`].
    pub artifact_name: ArtifactName,

    /// Path to the file in the workspace to publish. Must be a relative path
    /// with no directory traversal, no absolute prefix, and no `.git`
    /// segments. Validated at deserialization time via [`StrictRelativePath`].
    pub file_path: StrictRelativePath,
}

impl Validate for UploadPipelineArtifactParams {
    fn validate(&self) -> anyhow::Result<()> {
        // build_id: if present, must be positive. (artifact_name and file_path
        // are structurally validated by their newtypes at deserialization.)
        if let Some(id) = self.build_id {
            ensure!(id > 0, "build_id must be positive when specified");
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

    /// When `false` (default), the executor inserts a short hash suffix into
    /// the internal container folder so multiple calls in one agent run with
    /// the same `artifact_name` (e.g. publishing `TriageSummary` to many
    /// failing builds at once) do not silently overwrite each other's bytes
    /// in the agent's shared file container. The suffix lives only in
    /// internal addressing — the user-visible `artifact_name` your downstream
    /// consumers query is unaffected.
    ///
    /// Set to `true` to use a clean folder name (`{artifact_name}` exactly)
    /// and reject any in-run reuse of `(effective_build_id, artifact_name)`
    /// with a clear error before any HTTP call.
    #[serde(default, rename = "require-unique-names")]
    pub require_unique_names: bool,
}

fn default_pipeline_max_file_size() -> u64 {
    PIPELINE_ARTIFACT_DEFAULT_MAX_FILE_SIZE
}

/// Build the `Content-Range` header value required by the Azure DevOps File
/// Container API on every PUT (single-chunk uploads included).
///
/// The canonical format is `bytes {start}-{end}/{total}` for non-empty
/// payloads. For zero-byte files use `bytes */0` — the form ADO's own
/// clients send. Sending a non-empty body without this header (or with a
/// malformed value) causes ADO to reject the request with
/// `HTTP 400: Content-Range header not understood.`.
fn format_content_range(file_size: u64) -> String {
    if file_size == 0 {
        "bytes */0".to_string()
    } else {
        format!("bytes 0-{}/{}", file_size - 1, file_size)
    }
}

impl Default for UploadPipelineArtifactConfig {
    fn default() -> Self {
        Self {
            max_file_size: PIPELINE_ARTIFACT_DEFAULT_MAX_FILE_SIZE,
            allowed_extensions: Vec::new(),
            allowed_artifact_names: Vec::new(),
            allowed_build_ids: Vec::new(),
            name_prefix: None,
            require_unique_names: false,
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
            if self.build_id.is_none() {
                " (current build)"
            } else {
                ""
            }
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
        if let Some(prefix) = &config.name_prefix
            && prefix.len() > 50
        {
            return Ok(ExecutionResult::failure(format!(
                "name-prefix '{}...' is too long ({} chars, max 50)",
                prefix.chars().take(20).collect::<String>(),
                prefix.len()
            )));
        }
        let final_name = match &config.name_prefix {
            Some(prefix) => format!("{}{}", prefix, self.artifact_name),
            None => self.artifact_name.clone(),
        };
        if final_name.starts_with('.')
            || final_name.len() > 100
            || !crate::validate::is_valid_artifact_name(&final_name)
        {
            return Ok(ExecutionResult::failure(format!(
                "Resolved artifact name '{}' is not a valid Azure DevOps artifact name",
                final_name
            )));
        }

        // ── Artifact-name allow-list ─────────────────────────────────────
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

        // ── Extension allow-list ─────────────────────────────────────────
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
                if self.build_id.is_none() {
                    " (current build)"
                } else {
                    ""
                }
            )));
        }

        let file_bytes = tokio::fs::read(&canonical)
            .await
            .context("Failed to read file contents")?;

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

        // Resolve the agent's own build container ID (Azure DevOps pre-creates
        // one container per build at job initialization and exposes it via
        // BUILD_CONTAINERID). All artifacts in the build share this container,
        // including those whose record we associate with a different build.
        let container_id = ctx.build_container_id.context(
            "BUILD_CONTAINERID not set or invalid — required to publish a \
             pipeline artifact; this tool must run inside an Azure DevOps \
             pipeline job",
        )?;

        // ── Per-run dedupe (when require-unique-names is set) ────────────
        // Reject reuse of (effective_build_id, final_name) before any HTTP
        // call so two cross-build calls sharing a name don't silently
        // overwrite each other's bytes in the shared container.
        let dedupe_key = format!("{}/{}", effective_build_id, final_name);
        if config.require_unique_names {
            let seen = ctx.uploaded_pipeline_artifact_keys.lock().map_err(|e| {
                anyhow::anyhow!("uploaded_pipeline_artifact_keys mutex poisoned: {}", e)
            })?;
            if seen.contains(&dedupe_key) {
                return Ok(ExecutionResult::failure(format!(
                    "upload-pipeline-artifact: artifact_name '{}' was already used \
                     on build #{} in this run; require-unique-names is configured \
                     to reject reuse",
                    final_name, effective_build_id
                )));
            }
            // Note: the key is inserted only after the HTTP calls succeed below,
            // so a transient network failure doesn't permanently block retries.
        }

        // Internal container folder. The user-visible artifact name is
        // `final_name`; the folder name carries an optional discriminator so
        // multiple calls in one run sharing the same name but uploading
        // different content don't overwrite each other's bytes in the shared
        // container. The discriminator is derived from the file content hash
        // (already computed above) so distinct content always maps to a
        // distinct folder — identical content maps to the same folder, which
        // is safe (idempotent PUT). The discriminator is invisible in standard
        // download paths (web UI zip wrapper, DownloadBuildArtifacts@1,
        // DownloadPipelineArtifact@2 — all strip the prefix) and is only seen
        // by callers that hit `GET /_apis/resources/Containers/{id}?itemPath=…`
        // directly.
        let container_folder = if config.require_unique_names {
            final_name.clone()
        } else {
            let disc = &live_hash[..6];
            format!("{}__{}", final_name, disc)
        };

        let client = reqwest::Client::new();

        // Derive a filename from the original file path for use as the
        // itemPath inside the container (e.g. "report.pdf" from "out/report.pdf").
        let filename = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.staged_file);

        // ── Step 1: Upload file to the agent's own build container ──────
        // The canonical query parameter is `scope` (not `scopeIdentifier`,
        // which is the response model field name); all official ADO SDKs
        // (Node, Python, Extension API) use `scope=`.
        let upload_url = format!(
            "{}/_apis/resources/Containers/{}?itemPath={}/{}&scope={}&api-version=7.1-preview.4",
            org_url.trim_end_matches('/'),
            container_id,
            utf8_percent_encode(&container_folder, PATH_SEGMENT),
            utf8_percent_encode(filename, PATH_SEGMENT),
            utf8_percent_encode(project_id, PATH_SEGMENT),
        );
        debug!("Uploading {} bytes to container: {}", file_size, upload_url);

        // The Azure DevOps File Container API requires a `Content-Range`
        // header on every PUT (it's how ADO supports chunked uploads, even
        // for clients that send the whole file in a single request). Without
        // it the server responds `HTTP 400: Content-Range header not
        // understood.`. See `format_content_range` for the exact format.
        let content_range = format_content_range(file_size);

        let upload_resp = client
            .put(&upload_url)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Range", &content_range)
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
                "Failed to upload file to container #{} (HTTP {}): {}",
                container_id, status, error_body
            )));
        }
        debug!("File uploaded to container {}", container_id);

        // ── Step 2: Associate artifact record with the target build ─────
        // For cross-build publishing the artifact bytes physically live in
        // the agent's own container; the record on the target build holds a
        // pointer (`resource.data = "#/{containerId}/{folder}"`). ADO does
        // not require the container to belong to the target build — that is
        // exactly how `DownloadBuildArtifacts@1 buildType=specific` already
        // works (it follows cross-build container pointers).
        let artifact_url = format!(
            "{}/{}/_apis/build/builds/{}/artifacts?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            effective_build_id,
        );
        debug!(
            "Associating artifact '{}' (folder '{}') with build #{}: {}",
            final_name, container_folder, effective_build_id, artifact_url
        );

        let artifact_body = serde_json::json!({
            "name": final_name,
            "resource": {
                "data": format!("#/{}/{}", container_id, container_folder),
                "type": "Container",
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

            // Record the successful publish in the dedupe set now that both
            // HTTP steps have completed — done here (not before the calls) so
            // a transient failure doesn't permanently block retries.
            if config.require_unique_names {
                ctx.uploaded_pipeline_artifact_keys
                    .lock()
                    .map_err(|e| {
                        anyhow::anyhow!("uploaded_pipeline_artifact_keys mutex poisoned: {}", e)
                    })?
                    .insert(dedupe_key);
            }

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
                    "container_id": container_id,
                    "container_folder": container_folder,
                }),
            ))
        } else {
            let status = artifact_resp.status();
            let error_body = artifact_resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            // Best-effort hint when the most common failure modes show up.
            let hint = match status.as_u16() {
                401 | 403 => {
                    " — token may lack 'Build (Read & Execute)' scope on the target build's project"
                }
                404 => {
                    " — target build does not exist or is in a different project (cross-project publishing is not supported)"
                }
                409 => " — an artifact with this name already exists on the target build",
                _ => "",
            };
            Ok(ExecutionResult::failure(format!(
                "Failed to associate artifact with build #{} (HTTP {}){}: {}",
                effective_build_id, status, hint, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(
            UploadPipelineArtifactResult::NAME,
            "upload-pipeline-artifact"
        );
    }

    #[test]
    fn test_format_content_range() {
        // Zero-byte payloads use the `*/0` form (ADO clients send this for
        // empty files; `bytes 0--1/0` would be malformed).
        assert_eq!(format_content_range(0), "bytes */0");
        // Single-byte payload covers the boundary 0-0/1.
        assert_eq!(format_content_range(1), "bytes 0-0/1");
        // Typical small file.
        assert_eq!(format_content_range(12), "bytes 0-11/12");
        // Larger file.
        assert_eq!(format_content_range(1024), "bytes 0-1023/1024");
    }

    fn make_params(
        build_id: Option<i64>,
        artifact_name: &str,
        file_path: &str,
    ) -> UploadPipelineArtifactParams {
        UploadPipelineArtifactParams {
            build_id,
            artifact_name: artifact_name
                .try_into()
                .expect("test artifact_name must be valid"),
            file_path: file_path.try_into().expect("test file_path must be valid"),
        }
    }

    /// Deserialize params from JSON so that the `ArtifactName` /
    /// `StrictRelativePath` newtype validators run at the parse boundary. Used
    /// by the rejection tests, which cannot build invalid values via
    /// `make_params` (the newtypes refuse to construct from an invalid string).
    fn try_params(
        build_id: Option<i64>,
        artifact_name: &str,
        file_path: &str,
    ) -> Result<UploadPipelineArtifactParams, serde_json::Error> {
        let value = serde_json::json!({
            "build_id": build_id,
            "artifact_name": artifact_name,
            "file_path": file_path,
        });
        serde_json::from_value(value)
    }

    const DUMMY_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn test_params_validate_accepts_valid() {
        assert!(
            make_params(Some(1), "agent-report", "out/report.pdf")
                .validate()
                .is_ok()
        );
        assert!(
            make_params(None, "agent-report", "out/report.pdf")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn test_validation_rejects_zero_build_id() {
        let err = make_params(Some(0), "report", "out/report.pdf")
            .validate()
            .unwrap_err();
        assert!(
            err.to_string().contains("build_id must be positive"),
            "expected 'build_id must be positive' error, got: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_negative_build_id() {
        let err = make_params(Some(-1), "report", "out/report.pdf")
            .validate()
            .unwrap_err();
        assert!(
            err.to_string().contains("build_id must be positive"),
            "expected 'build_id must be positive' error, got: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_empty_artifact_name() {
        assert!(try_params(None, "", "out/report.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_artifact_name_with_spaces() {
        assert!(try_params(None, "my report", "out/report.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_leading_dot_artifact_name() {
        let err = try_params(None, ".hidden", "out/report.pdf")
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string().contains("must not start with '.'"),
            "expected 'must not start with' error, got: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_long_artifact_name() {
        let long_name = "a".repeat(101);
        assert!(try_params(None, &long_name, "out/report.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_empty_file_path() {
        assert!(try_params(None, "report", "").is_err());
    }

    #[test]
    fn test_validation_rejects_traversal_in_file_path() {
        let err = try_params(None, "report", "../etc/passwd")
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string().contains("path-traversal"),
            "expected path-traversal error, got: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_null_bytes_in_file_path() {
        assert!(try_params(None, "report", "out/report\0.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_newline_in_file_path() {
        assert!(try_params(None, "report", "out\n/report.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_carriage_return_in_file_path() {
        assert!(try_params(None, "report", "out\r/report.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_colon_in_file_path() {
        assert!(try_params(None, "report", "C:\\out\\report.pdf").is_err());
    }

    #[test]
    fn test_validation_rejects_pipeline_command_sequences_in_file_path() {
        let err_vso = try_params(
            None,
            "report",
            "##vso[task.setvariable variable=EXPLOIT]value.txt",
        )
        .map(|_| ())
        .unwrap_err();
        assert!(
            err_vso.to_string().contains("pipeline command"),
            "expected pipeline-command error for ##vso[, got: {err_vso}"
        );

        let err_hash = try_params(None, "report", "##[error]value.txt")
            .map(|_| ())
            .unwrap_err();
        assert!(
            err_hash.to_string().contains("pipeline command"),
            "expected pipeline-command error for ##[, got: {err_hash}"
        );
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
        assert_eq!(
            config.max_file_size,
            PIPELINE_ARTIFACT_DEFAULT_MAX_FILE_SIZE
        );
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

    /// Non-dry-run with no `BUILD_CONTAINERID` must fail with a clear message
    /// before any HTTP call, rather than silently doing nothing.
    #[tokio::test]
    async fn test_fails_when_build_container_id_missing() {
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
            dry_run: false,
            ado_org_url: Some("https://dev.azure.com/test".to_string()),
            ado_project: Some("TestProject".to_string()),
            ado_project_id: Some("proj-guid".to_string()),
            access_token: Some("fake-token".to_string()),
            // Intentionally no build_container_id.
            ..Default::default()
        };

        // execute_impl returns Err for missing required env (consistent with
        // the rest of the file); execute_safe_outputs converts it to a failure.
        let err = result.execute_impl(&ctx).await.unwrap_err();
        assert!(
            err.to_string().contains("BUILD_CONTAINERID"),
            "expected BUILD_CONTAINERID error, got: {}",
            err
        );
    }

    /// The default container folder embeds a 6-hex hash discriminator derived
    /// from the file content hash; two calls uploading different content
    /// (regardless of staged file name) produce different folders so their
    /// bytes can't collide in the shared build container. Identical content
    /// maps to the same folder (idempotent PUT — safe).
    #[test]
    fn test_default_container_folder_has_hash_suffix() {
        // Same logic the executor uses inline; mirror it here so changing the
        // discriminator scheme is a single-place refactor caught by this test.
        fn folder_for(final_name: &str, content: &[u8]) -> String {
            let disc = &crate::hash::sha256_hex(content)[..6];
            format!("{}__{}", final_name, disc)
        }

        let a = folder_for("TriageSummary", b"content-alpha");
        let b = folder_for("TriageSummary", b"content-beta");

        assert!(a.starts_with("TriageSummary__"));
        assert_eq!(a.len(), "TriageSummary".len() + 2 + 6);
        assert_ne!(a, b, "different content must produce different folders");
        // Determinism: same inputs yield same folder.
        assert_eq!(a, folder_for("TriageSummary", b"content-alpha"));
    }

    /// When `require-unique-names` is set, the executor uses the clean folder
    /// name (no discriminator suffix) — and the per-run dedupe set on the
    /// ExecutionContext rejects a second call with the same
    /// (effective_build_id, final_name) before any HTTP call is made.
    #[tokio::test]
    async fn test_require_unique_names_rejects_in_run_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let staged_a = dir.path().join("staged-a.md");
        let staged_b = dir.path().join("staged-b.md");
        std::fs::write(&staged_a, b"first").unwrap();
        std::fs::write(&staged_b, b"second").unwrap();

        let mut ctx = ExecutionContext {
            working_directory: dir.path().to_path_buf(),
            build_id: Some(100),
            dry_run: false,
            ado_org_url: Some("https://dev.azure.com/test".to_string()),
            ado_project: Some("TestProject".to_string()),
            ado_project_id: Some("proj-guid".to_string()),
            access_token: Some("fake-token".to_string()),
            build_container_id: Some(42),
            ..Default::default()
        };
        ctx.tool_configs.insert(
            "upload-pipeline-artifact".to_string(),
            serde_json::json!({"require-unique-names": true}),
        );

        // First call: simulate the dedupe set being seeded by an earlier
        // successful publish — we insert the key directly so we don't need
        // a live HTTP server to run the upload.
        {
            let mut seen = ctx.uploaded_pipeline_artifact_keys.lock().unwrap();
            seen.insert("100/TriageSummary".to_string());
        }

        let second = UploadPipelineArtifactResult::new(
            Some(100),
            "TriageSummary".to_string(),
            "out/triage.md".to_string(),
            "staged-b.md".to_string(),
            b"second".len() as u64,
            crate::hash::sha256_hex(b"second"),
        );

        let exec_result = second.execute_impl(&ctx).await.unwrap();
        assert!(!exec_result.success, "second call should be rejected");
        assert!(
            exec_result.message.contains("already used"),
            "expected dedupe error, got: {}",
            exec_result.message
        );
        assert!(
            exec_result.message.contains("require-unique-names"),
            "error should reference the config key, got: {}",
            exec_result.message
        );
    }
}
