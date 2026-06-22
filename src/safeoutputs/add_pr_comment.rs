//! Add PR comment safe output tool

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::PATH_SEGMENT;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};

/// Parameters for adding a comment thread on a pull request
#[derive(Deserialize, JsonSchema)]
pub struct AddPrCommentParams {
    /// The pull request ID to comment on
    pub pull_request_id: i32,

    /// Comment text in markdown format. Ensure adequate content > 10 characters.
    pub content: String,

    /// Repository alias: "self" for pipeline repo, or an alias from the checkout list.
    /// Defaults to "self" if omitted.
    #[serde(default = "default_repository")]
    pub repository: String,

    /// File path for an inline comment. When set, the comment is anchored to this file.
    #[serde(default)]
    pub file_path: Option<String>,

    /// Starting line number for a multi-line inline comment. Requires `file_path` and `line`.
    /// When set, the comment spans from `start_line` to `line`. Must be strictly less than
    /// `line` (use `line` alone for single-line comments — do not pass `start_line == line`).
    #[serde(default)]
    pub start_line: Option<i32>,

    /// Line number for an inline comment. Requires `file_path` to be set.
    #[serde(default)]
    pub line: Option<i32>,

    /// Thread status: "active" (default), "fixed", "wont-fix", "closed", or "by-design".
    /// CamelCase forms ("Active", "WontFix", etc.) are also accepted for backwards compatibility.
    #[serde(default = "default_status")]
    pub status: String,
}

fn default_repository() -> String {
    "self".to_string()
}

fn default_status() -> String {
    "active".to_string()
}

fn validate_repository_selector(repository: &str) -> anyhow::Result<()> {
    reject_pipeline_injection(repository, "repository")?;
    if !repository.is_empty() {
        crate::validate::validate_relative_safe_path(repository, "repository")?;
    }
    Ok(())
}

impl Validate for AddPrCommentParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.pull_request_id > 0, "pull_request_id must be positive");
        ensure!(
            self.content.len() >= 10,
            "content must be at least 10 characters"
        );
        ensure!(
            status_to_int(&self.status).is_some(),
            "status must be one of: {}",
            VALID_STATUSES.join(", ")
        );
        if self.line.is_some() {
            ensure!(
                self.file_path.is_some(),
                "line requires file_path to be set"
            );
        }
        if self.start_line.is_some() {
            ensure!(self.line.is_some(), "start_line requires line to be set");
            if let (Some(start), Some(end)) = (self.start_line, self.line) {
                ensure!(
                    start < end,
                    "start_line ({}) must be less than line ({})",
                    start,
                    end
                );
            }
        }
        if let Some(fp) = &self.file_path {
            validate_file_path(fp)?;
        }
        validate_repository_selector(&self.repository)?;
        Ok(())
    }
}

tool_result! {
    name = "add-pr-comment",
    write = true,
    params = AddPrCommentParams,
    /// Result of adding a comment thread on a pull request
    pub struct AddPrCommentResult {
        pull_request_id: i32,
        content: String,
        repository: String,
        file_path: Option<String>,
        start_line: Option<i32>,
        line: Option<i32>,
        status: String,
    }
}

impl SanitizeContent for AddPrCommentResult {
    fn sanitize_content_fields(&mut self) {
        self.content = sanitize_text(&self.content);
        self.repository = sanitize_config(&self.repository);
        // Strip control characters from remaining structural fields for defense-in-depth
        self.status = self.status.chars().filter(|c| !c.is_control()).collect();
        self.file_path = self
            .file_path
            .as_ref()
            .map(|fp| fp.chars().filter(|c| !c.is_control()).collect());
    }
}

/// Configuration for the add-pr-comment tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   add-pr-comment:
///     comment-prefix: "[Agent Review] "
///     allowed-repositories:
///       - self
///       - other-repo
///     allowed-statuses:
///       - Active
///       - Closed
/// ```
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct AddPrCommentConfig {
    /// Prefix prepended to all comments (e.g., "[Agent Review] ")
    #[serde(default, rename = "comment-prefix")]
    pub comment_prefix: Option<String>,

    /// Restrict which repositories the agent can comment on.
    /// If empty, all repositories in the checkout list (plus "self") are allowed.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,

    /// Restrict which thread statuses can be set.
    /// If empty, all valid statuses are allowed.
    #[serde(default, rename = "allowed-statuses")]
    pub allowed_statuses: Vec<String>,
    /// Whether to include agent execution stats in the output (default: true).
    #[serde(
        default = "crate::agent_stats::default_include_stats",
        rename = "include-stats"
    )]
    pub include_stats: bool,
}

impl Default for AddPrCommentConfig {
    fn default() -> Self {
        Self {
            comment_prefix: None,
            allowed_repositories: Vec::new(),
            allowed_statuses: Vec::new(),
            include_stats: true,
        }
    }
}

/// Map a thread status string to the ADO API integer value.
/// Accepts both kebab-case (preferred) and CamelCase for backwards compatibility.
fn status_to_int(status: &str) -> Option<i32> {
    match status {
        "active" | "Active" => Some(1),
        "fixed" | "Fixed" => Some(2),
        "wont-fix" | "WontFix" => Some(3),
        "closed" | "Closed" => Some(4),
        "by-design" | "ByDesign" => Some(5),
        _ => None,
    }
}

/// All valid thread status strings (kebab-case canonical form)
const VALID_STATUSES: &[&str] = &["active", "fixed", "wont-fix", "closed", "by-design"];

/// Validate a file path for inline comments: no `..` path traversal, not absolute
fn validate_file_path(path: &str) -> anyhow::Result<()> {
    ensure!(!path.is_empty(), "file_path must not be empty");
    ensure!(
        !path.split(['/', '\\']).any(|component| component == ".."),
        "file_path must not contain a '..' path component"
    );
    ensure!(
        !path.starts_with('/') && !path.starts_with('\\'),
        "file_path must not be absolute"
    );
    Ok(())
}

fn repository_checkout_dir(repository: &str, ctx: &ExecutionContext) -> anyhow::Result<PathBuf> {
    if crate::safeoutputs::input_refers_to_self(repository, ctx) {
        return Ok(ctx.source_directory.clone());
    }

    if let Some(alias) =
        crate::safeoutputs::lookup_allowed_repository_alias(repository, &ctx.allowed_repositories)
    {
        return Ok(ctx.source_directory.join(alias));
    }

    anyhow::bail!(
        "Repository alias '{}' not found in allowed repositories",
        repository
    )
}

fn build_inline_thread_context(
    workspace_root: &Path,
    repo_root: &Path,
    file_path: &str,
    start_line: i32,
    end_line: i32,
) -> anyhow::Result<serde_json::Value> {
    ensure!(start_line > 0, "start_line must be positive");
    ensure!(end_line > 0, "end_line must be positive");
    ensure!(
        start_line <= end_line,
        "start_line ({start_line}) must be less than or equal to line ({end_line})"
    );

    let resolved_path = repo_root.join(file_path);
    let canonical = resolved_path.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize inline comment file '{}' — file may not exist",
            file_path
        )
    })?;
    let canonical_root = repo_root
        .canonicalize()
        .context("Failed to canonicalize repository checkout root")?;
    ensure!(
        canonical.starts_with(&canonical_root),
        "Inline comment file '{}' resolves outside the repository checkout",
        file_path
    );
    let canonical_workspace = workspace_root
        .canonicalize()
        .context("Failed to canonicalize build workspace root")?;
    ensure!(
        canonical.starts_with(&canonical_workspace),
        "Inline comment file '{}' resolves outside the build workspace",
        file_path
    );

    let contents = std::fs::read_to_string(&canonical)
        .with_context(|| format!("Failed to read inline comment file '{}'", file_path))?;
    let target_line = contents
        .lines()
        .nth((end_line - 1) as usize)
        .with_context(|| format!("Inline comment line {} is out of range", end_line))?;
    // Azure DevOps threadContext offsets are 1-based, so the end offset must point
    // one UTF-16 code unit past the final character to span the whole target line.
    let end_offset = target_line.encode_utf16().count() as i32 + 1;

    Ok(serde_json::json!({
        "filePath": format!("/{}", file_path),
        "rightFileStart": { "line": start_line, "offset": 1 },
        "rightFileEnd": { "line": end_line, "offset": end_offset }
    }))
}

#[async_trait::async_trait]
impl Executor for AddPrCommentResult {
    fn dry_run_summary(&self) -> String {
        format!("add comment to PR #{}", self.pull_request_id)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Adding comment to PR #{}: {} chars",
            self.pull_request_id,
            self.content.len()
        );
        debug!(
            "add-pr-comment: pr_id={}, content length={}",
            self.pull_request_id,
            self.content.len()
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

        let config: AddPrCommentConfig = ctx.get_tool_config("add-pr-comment");
        debug!("Config: {:?}", config);

        // Validate repository against allowed-repositories config
        if !config.allowed_repositories.is_empty()
            && !config.allowed_repositories.contains(&self.repository)
        {
            return Ok(ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed-repositories list",
                self.repository
            )));
        }

        // Validate status against allowed-statuses config (case-insensitive)
        if !config.allowed_statuses.is_empty()
            && !config
                .allowed_statuses
                .iter()
                .any(|s| s.eq_ignore_ascii_case(&self.status))
        {
            return Ok(ExecutionResult::failure(format!(
                "Status '{}' is not in the allowed-statuses list",
                self.status
            )));
        }

        // Validate status is a known value
        let status_int = match status_to_int(&self.status) {
            Some(v) => v,
            None => {
                return Ok(ExecutionResult::failure(format!(
                    "Invalid status '{}'. Valid statuses: {}",
                    self.status,
                    VALID_STATUSES.join(", ")
                )));
            }
        };

        // Validate file_path if present
        if let Some(ref fp) = self.file_path
            && let Err(e) = validate_file_path(fp)
        {
            return Ok(ExecutionResult::failure(format!(
                "Invalid file_path: {}",
                e
            )));
        }

        // Determine the repository name for the API call
        let repo_name = if self.repository == "self" || self.repository.is_empty() {
            ctx.repository_name
                .as_ref()
                .context("BUILD_REPOSITORY_NAME not set and repository is 'self'")?
                .clone()
        } else {
            match crate::safeoutputs::lookup_allowed_repository(
                &self.repository,
                &ctx.allowed_repositories,
            ) {
                Some(name) => name.clone(),
                None => {
                    return Ok(ExecutionResult::failure(format!(
                        "Repository alias '{}' not found in allowed repositories",
                        self.repository
                    )));
                }
            }
        };

        // Build comment content with optional prefix
        let comment_body = match &config.comment_prefix {
            Some(prefix) => format!("{}{}", prefix, self.content),
            None => self.content.clone(),
        };
        let comment_body =
            crate::agent_stats::append_stats_to_body(&comment_body, ctx, config.include_stats);

        // Build the API URL
        let url = format!(
            "{}/{}/_apis/git/repositories/{}/pullRequests/{}/threads?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            utf8_percent_encode(&repo_name, PATH_SEGMENT),
            self.pull_request_id,
        );
        debug!("API URL: {}", url);

        // Build the request body
        let comment_obj = serde_json::json!({
            "parentCommentId": 0,
            "content": comment_body,
            "commentType": 1
        });

        let mut thread_body = serde_json::json!({
            "comments": [comment_obj],
            "status": status_int
        });

        // Add thread context for inline comments
        if let Some(ref fp) = self.file_path {
            let end_line = self.line.unwrap_or(1);
            let start_line = self.start_line.unwrap_or(end_line);
            let repo_root = match repository_checkout_dir(&self.repository, ctx).and_then(|path| {
                crate::validate::ensure_path_within_base(
                    &path,
                    &ctx.source_directory,
                    "Repository checkout root",
                )
            }) {
                Ok(path) => path,
                Err(err) => {
                    return Ok(ExecutionResult::failure(format!(
                        "Failed to resolve repository checkout for '{}': {}",
                        self.repository, err
                    )));
                }
            };
            match build_inline_thread_context(
                &ctx.source_directory,
                &repo_root,
                fp,
                start_line,
                end_line,
            ) {
                Ok(thread_context) => thread_body["threadContext"] = thread_context,
                Err(err) => {
                    return Ok(ExecutionResult::failure(format!(
                        "Failed to anchor inline comment for '{}': {}",
                        fp, err
                    )));
                }
            }
        }

        let client = reqwest::Client::new();

        info!("Sending comment thread to PR #{}", self.pull_request_id);
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&thread_body)
            .send()
            .await
            .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            let thread_id = body.get("id").and_then(|v| v.as_i64()).unwrap_or(0);

            info!(
                "Comment thread added to PR #{}: thread #{}",
                self.pull_request_id, thread_id
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Added comment thread #{} to PR #{}",
                    thread_id, self.pull_request_id
                ),
                serde_json::json!({
                    "thread_id": thread_id,
                    "pull_request_id": self.pull_request_id,
                    "repository": repo_name,
                    "project": project,
                    "status": self.status,
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            Ok(ExecutionResult::failure(format!(
                "Failed to add comment to PR #{} (HTTP {}): {}",
                self.pull_request_id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;
    use tempfile::tempdir;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(AddPrCommentResult::NAME, "add-pr-comment");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"pull_request_id": 42, "content": "This is a review comment on the PR."}"#;
        let params: AddPrCommentParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pull_request_id, 42);
        assert!(params.content.contains("review comment"));
        assert_eq!(params.repository, "self");
        assert!(params.file_path.is_none());
        assert!(params.line.is_none());
        assert_eq!(params.status, "active");
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = AddPrCommentParams {
            pull_request_id: 42,
            content: "This is a test comment with enough characters.".to_string(),
            repository: "self".to_string(),
            file_path: None,
            start_line: None,
            line: None,
            status: "active".to_string(),
        };
        let result: AddPrCommentResult = params.try_into().unwrap();
        assert_eq!(result.name, "add-pr-comment");
        assert_eq!(result.pull_request_id, 42);
        assert!(result.content.contains("test comment"));
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = AddPrCommentParams {
            pull_request_id: 0,
            content: "This is a valid comment body text.".to_string(),
            repository: "self".to_string(),
            file_path: None,
            start_line: None,
            line: None,
            status: "active".to_string(),
        };
        let result: Result<AddPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_short_content() {
        let params = AddPrCommentParams {
            pull_request_id: 42,
            content: "Too short".to_string(),
            repository: "self".to_string(),
            file_path: None,
            start_line: None,
            line: None,
            status: "active".to_string(),
        };
        let result: Result<AddPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_repository_pipeline_command() {
        let params = AddPrCommentParams {
            pull_request_id: 42,
            content: "This is a valid comment body text.".to_string(),
            repository: "##vso[task.setvariable variable=x]y".to_string(),
            file_path: None,
            start_line: None,
            line: None,
            status: "active".to_string(),
        };
        let result: Result<AddPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_repository_traversal_selector() {
        let params = AddPrCommentParams {
            pull_request_id: 42,
            content: "This is a valid comment body text.".to_string(),
            repository: "../sibling-repo".to_string(),
            file_path: None,
            start_line: None,
            line: None,
            status: "active".to_string(),
        };
        let result: Result<AddPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_accepts_project_scoped_repository_selector() {
        let params = AddPrCommentParams {
            pull_request_id: 42,
            content: "This is a valid comment body text.".to_string(),
            repository: "4x4/sdk-FtdiDeviceControl".to_string(),
            file_path: None,
            start_line: None,
            line: None,
            status: "active".to_string(),
        };
        let result: Result<AddPrCommentResult, _> = params.try_into();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validation_rejects_line_without_file_path() {
        let params = AddPrCommentParams {
            pull_request_id: 42,
            content: "This is a valid comment body text.".to_string(),
            repository: "self".to_string(),
            file_path: None,
            start_line: None,
            line: Some(10),
            status: "active".to_string(),
        };
        let result: Result<AddPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = AddPrCommentParams {
            pull_request_id: 42,
            content: "A comment body that is definitely longer than ten characters.".to_string(),
            repository: "self".to_string(),
            file_path: Some("src/main.rs".to_string()),
            start_line: None,
            line: Some(10),
            status: "active".to_string(),
        };
        let result: AddPrCommentResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"add-pr-comment""#));
        assert!(json.contains(r#""pull_request_id":42"#));
    }

    #[test]
    fn test_config_defaults() {
        let config = AddPrCommentConfig::default();
        assert!(config.comment_prefix.is_none());
        assert!(config.allowed_repositories.is_empty());
        assert!(config.allowed_statuses.is_empty());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
comment-prefix: "[Agent Review] "
allowed-repositories:
  - self
  - other-repo
allowed-statuses:
  - Active
  - Closed
"#;
        let config: AddPrCommentConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.comment_prefix, Some("[Agent Review] ".to_string()));
        assert_eq!(config.allowed_repositories, vec!["self", "other-repo"]);
        assert_eq!(config.allowed_statuses, vec!["Active", "Closed"]);
    }

    #[test]
    fn test_status_to_int_mapping() {
        // Kebab-case (canonical)
        assert_eq!(status_to_int("active"), Some(1));
        assert_eq!(status_to_int("fixed"), Some(2));
        assert_eq!(status_to_int("wont-fix"), Some(3));
        assert_eq!(status_to_int("closed"), Some(4));
        assert_eq!(status_to_int("by-design"), Some(5));
        // CamelCase (backwards compat)
        assert_eq!(status_to_int("Active"), Some(1));
        assert_eq!(status_to_int("WontFix"), Some(3));
        assert_eq!(status_to_int("ByDesign"), Some(5));
        // Invalid
        assert_eq!(status_to_int("Invalid"), None);
    }

    #[test]
    fn test_validate_file_path_rejects_traversal() {
        assert!(validate_file_path("../etc/passwd").is_err());
        assert!(validate_file_path("src/../secret").is_err());
    }

    #[test]
    fn test_validate_file_path_rejects_absolute() {
        assert!(validate_file_path("/etc/passwd").is_err());
        assert!(validate_file_path("\\windows\\system32").is_err());
    }

    #[test]
    fn test_validate_file_path_accepts_valid() {
        assert!(validate_file_path("src/main.rs").is_ok());
        assert!(validate_file_path("path/to/file.txt").is_ok());
        // ".." within a component name is not a traversal — must be accepted
        assert!(validate_file_path("releases..notes/v1.md").is_ok());
        assert!(validate_file_path("v2..beta/file.txt").is_ok());
    }

    #[test]
    fn test_validation_rejects_invalid_status() {
        let params = AddPrCommentParams {
            pull_request_id: 42,
            content: "This is a valid comment body text.".to_string(),
            repository: "self".to_string(),
            file_path: None,
            start_line: None,
            line: None,
            status: "unknown".to_string(),
        };
        let result: Result<AddPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("status must be one of"));
    }

    #[test]
    fn test_validation_accepts_valid_statuses() {
        for s in &[
            "active",
            "fixed",
            "wont-fix",
            "closed",
            "by-design",
            "Active",
            "WontFix",
        ] {
            let params = AddPrCommentParams {
                pull_request_id: 42,
                content: "This is a valid comment body text.".to_string(),
                repository: "self".to_string(),
                file_path: None,
                start_line: None,
                line: None,
                status: s.to_string(),
            };
            let result: Result<AddPrCommentResult, _> = params.try_into();
            assert!(result.is_ok(), "Expected status '{}' to be valid", s);
        }
    }

    #[test]
    fn test_allowed_statuses_case_insensitive_match() {
        // Config has "Active" but agent sends "active" (canonical lowercase) — should be allowed
        let config = AddPrCommentConfig {
            comment_prefix: None,
            allowed_repositories: Vec::new(),
            allowed_statuses: vec!["Active".to_string(), "Closed".to_string()],
            include_stats: true,
        };
        // Test the exact comparison logic extracted from execute_impl
        let status = "active";
        let matched = config
            .allowed_statuses
            .iter()
            .any(|s| s.eq_ignore_ascii_case(status));
        assert!(
            matched,
            "lowercase 'active' should match config value 'Active'"
        );
    }

    #[test]
    fn test_sanitize_content_neutralizes_repository_pipeline_command() {
        let params = AddPrCommentParams {
            pull_request_id: 42,
            content: "This is a valid comment body text.".to_string(),
            repository: "##vso[task.setvariable variable=x]y".to_string(),
            file_path: None,
            start_line: None,
            line: None,
            status: "active".to_string(),
        };
        let mut result = AddPrCommentResult {
            name: "add-pr-comment".to_string(),
            pull_request_id: params.pull_request_id,
            content: params.content,
            repository: params.repository,
            file_path: params.file_path,
            start_line: params.start_line,
            line: params.line,
            status: params.status,
        };
        result.sanitize_content_fields();
        assert!(
            result.repository.contains("`##vso[`"),
            "repository pipeline command should be neutralized with backticks: {}",
            result.repository
        );
    }

    #[test]
    fn test_build_inline_thread_context_uses_utf16_end_offset() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("suggestion.rs"), "prefix\nab😀\n").unwrap();

        let thread_context =
            build_inline_thread_context(dir.path(), dir.path(), "suggestion.rs", 2, 2).unwrap();

        assert_eq!(thread_context["rightFileStart"]["line"], 2);
        assert_eq!(thread_context["rightFileStart"]["offset"], 1);
        assert_eq!(thread_context["rightFileEnd"]["line"], 2);
        assert_eq!(thread_context["rightFileEnd"]["offset"], 5);
    }

    #[test]
    fn test_build_inline_thread_context_uses_last_line_for_multiline_span() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("suggestion.rs"),
            "first line\nab😀\nthird\n",
        )
        .unwrap();

        let thread_context =
            build_inline_thread_context(dir.path(), dir.path(), "suggestion.rs", 1, 2).unwrap();

        assert_eq!(thread_context["rightFileStart"]["line"], 1);
        assert_eq!(thread_context["rightFileEnd"]["line"], 2);
        assert_eq!(thread_context["rightFileEnd"]["offset"], 5);
    }

    #[test]
    fn test_build_inline_thread_context_rejects_repo_root_outside_workspace() {
        let workspace = tempdir().unwrap();
        let outside_repo = tempdir().unwrap();
        std::fs::write(outside_repo.path().join("suggestion.rs"), "line 1\n").unwrap();

        let err = build_inline_thread_context(
            workspace.path(),
            outside_repo.path(),
            "suggestion.rs",
            1,
            1,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("outside the build workspace"), "got: {err}");
    }

    #[test]
    fn test_repository_checkout_dir_resolves_full_repository_name_to_alias_path() {
        let workspace = tempdir().unwrap();
        let alias_dir = workspace.path().join("repo-sdk-ftdidevicecontrol");
        std::fs::create_dir(&alias_dir).unwrap();

        let mut allowed_repositories = std::collections::HashMap::new();
        allowed_repositories.insert(
            "repo-sdk-ftdidevicecontrol".to_string(),
            "4x4/sdk-FtdiDeviceControl".to_string(),
        );

        let ctx = ExecutionContext {
            source_directory: workspace.path().to_path_buf(),
            allowed_repositories,
            repository_name: Some("4x4/current-repo".to_string()),
            ..Default::default()
        };

        let resolved = repository_checkout_dir("4x4/sdk-ftdidevicecontrol", &ctx).unwrap();

        assert_eq!(resolved, alias_dir);
    }

    #[test]
    fn test_repository_checkout_dir_resolves_alias_key_to_alias_path() {
        let workspace = tempdir().unwrap();
        let alias_dir = workspace.path().join("repo-sdk-ftdidevicecontrol");
        std::fs::create_dir(&alias_dir).unwrap();

        let mut allowed_repositories = std::collections::HashMap::new();
        allowed_repositories.insert(
            "repo-sdk-ftdidevicecontrol".to_string(),
            "4x4/sdk-FtdiDeviceControl".to_string(),
        );

        let ctx = ExecutionContext {
            source_directory: workspace.path().to_path_buf(),
            allowed_repositories,
            repository_name: Some("4x4/current-repo".to_string()),
            ..Default::default()
        };

        let resolved = repository_checkout_dir("repo-sdk-ftdidevicecontrol", &ctx).unwrap();

        assert_eq!(resolved, alias_dir);
    }

    #[test]
    fn test_repository_checkout_dir_treats_empty_repository_as_self() {
        let workspace = tempdir().unwrap();
        let ctx = ExecutionContext {
            source_directory: workspace.path().to_path_buf(),
            repository_name: Some("4x4/current-repo".to_string()),
            ..Default::default()
        };

        let resolved = repository_checkout_dir("", &ctx).unwrap();

        assert_eq!(resolved, workspace.path());
    }
}
