//! Upload attachment safe output tool

use log::{debug, info, warn};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use anyhow::{Context, ensure};

/// Parameters for uploading an attachment to a work item
#[derive(Deserialize, JsonSchema)]
pub struct UploadAttachmentParams {
    /// The work item ID to attach the file to
    pub work_item_id: i64,

    /// Path to the file in the workspace to upload. Must be a relative path with no directory traversal.
    pub file_path: String,

    /// Optional description of the attachment. Must be at least 3 characters if provided.
    pub comment: Option<String>,
}

impl Validate for UploadAttachmentParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.work_item_id > 0, "work_item_id must be positive");
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
        if let Some(comment) = &self.comment {
            ensure!(
                comment.len() >= 3,
                "comment must be at least 3 characters"
            );
        }
        Ok(())
    }
}

tool_result! {
    name = "upload-attachment",
    write = true,
    params = UploadAttachmentParams,
    /// Result of uploading an attachment to a work item
    pub struct UploadAttachmentResult {
        work_item_id: i64,
        file_path: String,
        comment: Option<String>,
    }
}

impl Sanitize for UploadAttachmentResult {
    fn sanitize_fields(&mut self) {
        if let Some(comment) = &self.comment {
            self.comment = Some(sanitize_text(comment));
        }
    }
}

const DEFAULT_MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5 MB

/// Configuration for the upload-attachment tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   upload-attachment:
///     max-file-size: 5242880
///     allowed-extensions:
///       - .png
///       - .pdf
///       - .log
///     comment-prefix: "[Agent] "
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadAttachmentConfig {
    /// Maximum file size in bytes (default: 5 MB)
    #[serde(default = "default_max_file_size", rename = "max-file-size")]
    pub max_file_size: u64,

    /// Allowed file extensions (e.g., [".png", ".pdf"]). Empty means all extensions allowed.
    #[serde(default, rename = "allowed-extensions")]
    pub allowed_extensions: Vec<String>,

    /// Prefix to prepend to the comment
    #[serde(default, rename = "comment-prefix")]
    pub comment_prefix: Option<String>,
}

fn default_max_file_size() -> u64 {
    DEFAULT_MAX_FILE_SIZE
}

impl Default for UploadAttachmentConfig {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            allowed_extensions: Vec::new(),
            comment_prefix: None,
        }
    }
}

#[async_trait::async_trait]
impl Executor for UploadAttachmentResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Uploading attachment '{}' to work item #{}",
            self.file_path, self.work_item_id
        );

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

        let config: UploadAttachmentConfig = ctx.get_tool_config("upload-attachment");
        debug!("Max file size: {} bytes", config.max_file_size);
        debug!("Allowed extensions: {:?}", config.allowed_extensions);

        // Validate file extension against allowed-extensions (if configured)
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

        // Resolve file path relative to source_directory
        let resolved_path = ctx.source_directory.join(&self.file_path);
        debug!("Resolved file path: {}", resolved_path.display());

        // Canonicalize to resolve symlinks, then verify the path stays within source_directory.
        let canonical = resolved_path
            .canonicalize()
            .context("Failed to canonicalize file path — file may not exist or contains broken symlinks")?;
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

        // Check file size
        let metadata = std::fs::metadata(&canonical)
            .context("Failed to read file metadata")?;
        let file_size = metadata.len();
        debug!("File size: {} bytes", file_size);
        if file_size > config.max_file_size {
            return Ok(ExecutionResult::failure(format!(
                "File size ({} bytes) exceeds maximum allowed size ({} bytes)",
                file_size, config.max_file_size
            )));
        }

        // Read file bytes
        let file_bytes =
            std::fs::read(&canonical).context("Failed to read file contents")?;

        // Check if file is text (valid UTF-8) — if text, scan for ##vso[ command injection.
        // Binary files (where from_utf8 fails) skip this check intentionally: ADO's attachment
        // viewer won't execute ##vso[ sequences from binary content. Note that a binary file
        // with a valid UTF-8 preamble but malformed tail will also skip the scan, but this is
        // acceptable because the injection risk from binary attachments is negligible.
        if let Ok(text) = std::str::from_utf8(&file_bytes) {
            if text.contains("##vso[") {
                return Ok(ExecutionResult::failure(format!(
                    "File '{}' contains '##vso[' command injection sequence",
                    self.file_path
                )));
            }
        }

        // Extract filename for upload
        let filename = std::path::Path::new(&self.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment");
        debug!("Upload filename: {}", filename);

        // Apply comment-prefix to comment if configured
        let effective_comment = match (&self.comment, &config.comment_prefix) {
            (Some(c), Some(prefix)) => format!("{}{}", prefix, c),
            (Some(c), None) => c.clone(),
            (None, _) => "Uploaded by agent".to_string(),
        };
        debug!("Effective comment: {}", effective_comment);

        let client = reqwest::Client::new();

        // Step 1: Upload file
        // POST {org_url}/{project}/_apis/wit/attachments?fileName={filename}&api-version=7.1
        let upload_url = format!(
            "{}/{}/_apis/wit/attachments?fileName={}&api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            utf8_percent_encode(filename, PATH_SEGMENT),
        );
        debug!("Upload URL: {}", upload_url);

        info!("Uploading file '{}' ({} bytes)", filename, file_size);
        let upload_response = client
            .post(&upload_url)
            .header("Content-Type", "application/octet-stream")
            .basic_auth("", Some(token))
            .body(file_bytes)
            .send()
            .await
            .context("Failed to upload attachment to Azure DevOps")?;

        if !upload_response.status().is_success() {
            let status = upload_response.status();
            let error_body = upload_response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Ok(ExecutionResult::failure(format!(
                "Failed to upload attachment (HTTP {}): {}",
                status, error_body
            )));
        }

        let upload_body: serde_json::Value = upload_response
            .json()
            .await
            .context("Failed to parse upload response JSON")?;

        let attachment_url = upload_body
            .get("url")
            .and_then(|v| v.as_str())
            .context("Upload response missing 'url' field")?
            .to_string();
        debug!("Attachment URL: {}", attachment_url);

        // Step 2: Link attachment to work item
        // PATCH {org_url}/{project}/_apis/wit/workitems/{work_item_id}?api-version=7.1
        let link_url = format!(
            "{}/{}/_apis/wit/workitems/{}?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            self.work_item_id,
        );
        debug!("Link URL: {}", link_url);

        let patch_doc = serde_json::json!([{
            "op": "add",
            "path": "/relations/-",
            "value": {
                "rel": "AttachedFile",
                "url": attachment_url,
                "attributes": {
                    "comment": effective_comment,
                }
            }
        }]);

        info!(
            "Linking attachment to work item #{}",
            self.work_item_id
        );
        let link_response = client
            .patch(&link_url)
            .header("Content-Type", "application/json-patch+json")
            .basic_auth("", Some(token))
            .json(&patch_doc)
            .send()
            .await
            .context("Failed to link attachment to work item")?;

        if link_response.status().is_success() {
            info!(
                "Attachment '{}' linked to work item #{}",
                filename, self.work_item_id
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Uploaded '{}' and linked to work item #{}",
                    filename, self.work_item_id
                ),
                serde_json::json!({
                    "work_item_id": self.work_item_id,
                    "file_path": self.file_path,
                    "attachment_url": attachment_url,
                    "project": project,
                }),
            ))
        } else {
            let status = link_response.status();
            let error_body = link_response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            warn!(
                "Attachment uploaded but linking failed — the attachment at {} is orphaned",
                attachment_url
            );

            Ok(ExecutionResult::failure(format!(
                "Attachment uploaded but failed to link to work item #{} (HTTP {}): {}",
                self.work_item_id, status, error_body
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
        assert_eq!(UploadAttachmentResult::NAME, "upload-attachment");
    }

    #[test]
    fn test_params_deserializes() {
        let json =
            r#"{"work_item_id": 42, "file_path": "output/report.pdf", "comment": "Weekly report"}"#;
        let params: UploadAttachmentParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.work_item_id, 42);
        assert_eq!(params.file_path, "output/report.pdf");
        assert_eq!(params.comment, Some("Weekly report".to_string()));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "output/report.pdf".to_string(),
            comment: Some("Weekly report".to_string()),
        };
        let result: UploadAttachmentResult = params.try_into().unwrap();
        assert_eq!(result.name, "upload-attachment");
        assert_eq!(result.work_item_id, 42);
        assert_eq!(result.file_path, "output/report.pdf");
        assert_eq!(result.comment, Some("Weekly report".to_string()));
    }

    #[test]
    fn test_validation_rejects_zero_work_item_id() {
        let params = UploadAttachmentParams {
            work_item_id: 0,
            file_path: "output/report.pdf".to_string(),
            comment: None,
        };
        let result: Result<UploadAttachmentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_empty_file_path() {
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "".to_string(),
            comment: None,
        };
        let result: Result<UploadAttachmentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_path_traversal() {
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "../etc/passwd".to_string(),
            comment: None,
        };
        let result: Result<UploadAttachmentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_embedded_traversal() {
        // "src/../secret" has ".." as a standalone component
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "src/../secret".to_string(),
            comment: None,
        };
        let result: Result<UploadAttachmentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_backslash_traversal() {
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "src\\..\\secret.txt".to_string(),
            comment: None,
        };
        let result: Result<UploadAttachmentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_backslash_absolute_path() {
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "\\etc\\passwd".to_string(),
            comment: None,
        };
        let result: Result<UploadAttachmentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_accepts_filename_with_dots_in_name() {
        // "report..v2.pdf" has ".." inside a filename, not as a standalone component
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "report..v2.pdf".to_string(),
            comment: None,
        };
        let result: Result<UploadAttachmentResult, _> = params.try_into();
        assert!(result.is_ok(), "report..v2.pdf should be a valid filename");
    }

    #[test]
    fn test_validation_accepts_directory_with_dots_in_name() {
        // "v2..3/notes.md" — ".." inside a directory name, not a standalone component
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "v2..3/notes.md".to_string(),
            comment: None,
        };
        let result: Result<UploadAttachmentResult, _> = params.try_into();
        assert!(result.is_ok(), "v2..3/notes.md should be valid");
    }

    #[test]
    fn test_validation_rejects_absolute_path() {
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "/etc/passwd".to_string(),
            comment: None,
        };
        let result: Result<UploadAttachmentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = UploadAttachmentParams {
            work_item_id: 42,
            file_path: "output/report.pdf".to_string(),
            comment: Some("Test attachment".to_string()),
        };
        let result: UploadAttachmentResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"upload-attachment""#));
        assert!(json.contains(r#""work_item_id":42"#));
        assert!(json.contains(r#""file_path":"output/report.pdf""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = UploadAttachmentConfig::default();
        assert_eq!(config.max_file_size, 5 * 1024 * 1024);
        assert!(config.allowed_extensions.is_empty());
        assert!(config.comment_prefix.is_none());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
max-file-size: 1048576
allowed-extensions:
  - .png
  - .pdf
  - .log
comment-prefix: "[Agent] "
"#;
        let config: UploadAttachmentConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.max_file_size, 1_048_576);
        assert_eq!(
            config.allowed_extensions,
            vec![".png", ".pdf", ".log"]
        );
        assert_eq!(config.comment_prefix, Some("[Agent] ".to_string()));
    }
}
