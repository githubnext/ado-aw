//! Add PR comment safe output tool

use log::{debug, info};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::path::Path;

use super::pr_common::{
    PullRequestReference, describe_pr_reference, repository_api_base, resolve_configured_pr_target,
    validate_reference,
};
use super::pr_inline::{PrCommentSide, PrInlineComment};
use super::pr_mutations::UpdatePrContext;
use super::{ToolResult, authenticate_ado_request};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config, sanitize_markdown};
use crate::secure::{CommitSha, Identifier, RelativeSafePath};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};

/// Parameters for adding a comment thread on a pull request
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddPrCommentParams {
    /// The pull request ID to comment on
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,

    /// Comment text in markdown format. Ensure adequate content > 10 characters.
    pub content: String,

    /// Repository alias: "self" for pipeline repo, or an alias from the checkout list.
    /// Defaults to "self" if omitted.
    #[serde(default)]
    pub repository: Option<String>,

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
    /// Side of the PR diff; right by default.
    #[serde(default)]
    pub side: PrCommentSide,
    /// Exact reviewed source commit. Required for inline comments.
    #[serde(default)]
    pub expected_head_sha: Option<CommitSha>,

    /// Thread status: "active" (default), "fixed", "wont-fix", "closed", or "by-design".
    /// CamelCase forms ("Active", "WontFix", etc.) are also accepted for backwards compatibility.
    #[serde(default = "default_status")]
    pub status: String,
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
        if let Some(reference) = &self.pull_request_id {
            validate_reference(reference)?;
        }
        ensure!(
            self.content.len() >= 10,
            "content must be at least 10 characters"
        );
        super::pr_comments::validate_body(&self.content)?;
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
            RelativeSafePath::parse(fp)?;
            ensure!(
                self.expected_head_sha.is_some(),
                "expected_head_sha is required for inline comments"
            );
            PrInlineComment {
                file_path: RelativeSafePath::parse(fp)?,
                side: self.side,
                line: u32::try_from(self.line.unwrap_or(1)).context("line must be positive")?,
                start_line: self
                    .start_line
                    .map(u32::try_from)
                    .transpose()
                    .context("start_line must be positive")?,
                content: self.content.clone(),
            }
            .validate()?;
        } else {
            ensure!(self.side == PrCommentSide::Right, "side requires file_path");
        }
        if let Some(repository) = &self.repository {
            validate_repository_selector(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "add-pull-request-comment",
    write = true,
    params = AddPrCommentParams,
    /// Result of adding a comment thread on a pull request
    #[serde(deny_unknown_fields)]
    pub struct AddPrCommentResult {
        #[serde(default)]
        pull_request_id: Option<PullRequestReference>,
        content: String,
        repository: Option<String>,
        file_path: Option<String>,
        start_line: Option<i32>,
        line: Option<i32>,
        #[serde(default)]
        side: PrCommentSide,
        #[serde(default)]
        expected_head_sha: Option<CommitSha>,
        status: String,
    }
}

impl SanitizeContent for AddPrCommentResult {
    fn sanitize_content_fields(&mut self) {
        self.content = sanitize_markdown(&self.content);
        self.repository = self.repository.as_deref().map(sanitize_config);
        // Strip control characters from remaining structural fields for defense-in-depth
        self.status = self.status.chars().filter(|c| !c.is_control()).collect();
        self.file_path = self
            .file_path
            .as_ref()
            .map(|fp| fp.chars().filter(|c| !c.is_control()).collect());
    }
}

/// Configuration for the add-pull-request-comment tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   add-pull-request-comment:
///     comment-prefix: "[Agent Review] "
///     allowed-repositories:
///       - self
///       - other-repo
///     allowed-statuses:
///       - Active
///       - Closed
/// ```
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddPrCommentConfig {
    #[serde(default, rename = "supersede-older-comments")]
    #[sanitize_config(skip)]
    pub supersede_older_comments: bool,
    #[serde(
        default = "super::pr_comments::default_comment_key",
        rename = "comment-key"
    )]
    #[sanitize_config(skip)]
    pub comment_key: Identifier,
    #[serde(default = "default_max_superseded", rename = "max-superseded-comments")]
    #[sanitize_config(skip)]
    pub max_superseded_comments: usize,
    #[serde(default)]
    #[sanitize_config(skip)]
    pub target: super::update_pull_request::UpdatePullRequestTarget,
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    #[serde(default, rename = "allow-temporary-ids")]
    #[sanitize_config(skip)]
    pub allow_temporary_ids: bool,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

impl Default for AddPrCommentConfig {
    fn default() -> Self {
        Self {
            supersede_older_comments: false,
            comment_key: super::pr_comments::default_comment_key(),
            max_superseded_comments: default_max_superseded(),
            target: Default::default(),
            target_repo: None,
            required_labels: Vec::new(),
            required_title_prefix: None,
            allow_temporary_ids: false,
            comment_prefix: None,
            allowed_repositories: Vec::new(),
            allowed_statuses: Vec::new(),
            include_stats: true,
            max: None,
        }
    }
}

fn default_max_superseded() -> usize {
    20
}

pub(crate) fn validate_add_pr_comment_config(config: &AddPrCommentConfig) -> anyhow::Result<()> {
    ensure!(
        config.max_superseded_comments > 0 && config.max_superseded_comments <= 100,
        "max-superseded-comments must be between 1 and 100"
    );
    ensure!(
        config.comment_key.len() <= 100,
        "comment-key must fit 100 bytes"
    );
    Ok(())
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

#[cfg(test)]
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

impl AddPrCommentResult {
    /// Validates the request against the tool's config-driven policy
    /// (allowed-repositories, allowed-statuses, known status value, and
    /// file_path shape). Returns the resolved ADO status integer on success,
    /// or a human-readable failure message on the first violated rule.
    fn validate_against_config(&self, config: &AddPrCommentConfig) -> Result<i32, String> {
        if !config.allowed_statuses.is_empty()
            && !config
                .allowed_statuses
                .iter()
                .any(|s| s.eq_ignore_ascii_case(&self.status))
        {
            return Err(format!(
                "Status '{}' is not in the allowed-statuses list",
                self.status
            ));
        }

        let status_int = status_to_int(&self.status).ok_or_else(|| {
            format!(
                "Invalid status '{}'. Valid statuses: {}",
                self.status,
                VALID_STATUSES.join(", ")
            )
        })?;

        if let Some(ref fp) = self.file_path {
            validate_file_path(fp).map_err(|e| format!("Invalid file_path: {e}"))?;
        }

        Ok(status_int)
    }

    /// Builds the JSON body for the ADO "create thread" API call, attaching
    /// `threadContext` for inline (file-anchored) comments.
    fn build_thread_body(
        &self,
        ctx: &ExecutionContext,
        config: &AddPrCommentConfig,
        status_int: i32,
    ) -> Result<serde_json::Value, String> {
        let comment_body = match &config.comment_prefix {
            Some(prefix) => format!("{}{}", prefix, self.content),
            None => self.content.clone(),
        };
        let comment_body =
            crate::agent_stats::append_stats_to_body(&comment_body, ctx, config.include_stats);
        super::pr_comments::validate_body(&comment_body).map_err(|error| error.to_string())?;

        let comment_obj = serde_json::json!({
            "parentCommentId": 0,
            "content": &comment_body,
            "commentType": 1
        });

        let mut thread_body = serde_json::json!({
            "comments": [comment_obj],
            "status": status_int
        });
        let owner = super::pr_comments::owner(ctx, "comment", &config.comment_key)
            .map_err(|error| format!("Comment ownership: {error:#}"))?;
        super::pr_comments::stamp(&mut thread_body, owner.as_ref(), ctx, &comment_body)
            .map_err(|error| format!("Comment ownership: {error:#}"))?;

        Ok(thread_body)
    }
}

#[async_trait::async_trait]
impl Executor for AddPrCommentResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "add comment to {}",
            describe_pr_reference(self.pull_request_id.as_ref())
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if let Err(error) = (AddPrCommentParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
            content: self.content.clone(),
            file_path: self.file_path.clone(),
            line: self.line,
            start_line: self.start_line,
            side: self.side,
            expected_head_sha: self.expected_head_sha.clone(),
            status: self.status.clone(),
        })
        .validate()
        {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;

        let config: AddPrCommentConfig = ctx.get_tool_config("add-pull-request-comment")?;
        validate_add_pr_comment_config(&config)?;
        debug!("Config: {:?}", config);

        let status_int = match self.validate_against_config(&config) {
            Ok(v) => v,
            Err(msg) => return Ok(ExecutionResult::failure(msg)),
        };

        super::pr_common::validate_temporary_opt_in(
            self.pull_request_id.as_ref(),
            config.allow_temporary_ids,
        )?;
        let (pull_request_id, target) = match resolve_configured_pr_target(
            Self::NAME,
            self.pull_request_id.as_ref(),
            self.repository.as_deref(),
            ctx,
        )
        .await?
        {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        let repo_name = target.qualified_repository();
        let project = &target.project;

        let mut thread_body = match self.build_thread_body(ctx, &config, status_int) {
            Ok(body) => body,
            Err(msg) => return Ok(ExecutionResult::failure(msg)),
        };

        let url = format!(
            "{}/pullRequests/{}/threads?api-version=7.1",
            repository_api_base(&target),
            pull_request_id,
        );
        debug!("API URL: {}", url);

        let client = super::pr_comments::client()?;
        let operation = UpdatePrContext {
            client: &client,
            target: target.clone(),
            pr_id: pull_request_id,
            token,
            connection_type: ctx.write_connection_type,
        };
        if let Some(file_path) = &self.file_path {
            let head = self
                .expected_head_sha
                .as_ref()
                .context("Inline comments require expected_head_sha")?;
            let inline = PrInlineComment {
                file_path: RelativeSafePath::parse(file_path)?,
                side: self.side,
                line: u32::try_from(self.line.unwrap_or(1))?,
                start_line: self.start_line.map(u32::try_from).transpose()?,
                content: self.content.clone(),
            };
            let prepared = super::pr_inline::prepare(&operation, head, &[inline]).await?;
            let context = prepared
                .first()
                .context("Inline comment context was not prepared")?;
            thread_body["threadContext"] = context["threadContext"].clone();
            thread_body["pullRequestThreadContext"] = context["pullRequestThreadContext"].clone();
        }
        let supersession = if config.supersede_older_comments {
            let owner = super::pr_comments::owner(ctx, "comment", &config.comment_key)?
                .context("Supersession requires a complete trusted pipeline identity")?;
            let actor = super::pr_comments::actor(&operation).await?;
            let (candidates, skipped) = super::pr_comments::older_threads(
                &operation,
                &owner,
                &actor,
                ctx.build_id.context("Supersession requires a build ID")?,
                config.max_superseded_comments,
            )
            .await?;
            Some((owner, actor, candidates, skipped))
        } else {
            None
        };

        info!("Sending comment thread to PR #{}", pull_request_id);
        if let Some(head) = &self.expected_head_sha {
            super::pr_inline::verify_head(&operation, head).await?;
        }
        let response =
            authenticate_ado_request(client.post(&url), token, ctx.write_connection_type)
                .header("Content-Type", "application/json")
                .json(&thread_body)
                .send()
                .await
                .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            let thread_id = body
                .get("id")
                .and_then(|v| v.as_i64())
                .filter(|id| *id > 0)
                .context("Comment response missing a positive thread ID")?;

            info!(
                "Comment thread added to PR #{}: thread #{}",
                pull_request_id, thread_id
            );

            let mut data = serde_json::json!({
                "thread_id": thread_id,
                "pull_request_id": pull_request_id,
                "repository": repo_name,
                "project": project,
                "status": self.status,
            });
            if let Some((owner, actor, candidates, skipped)) = supersession {
                match super::pr_comments::supersede(
                    &operation,
                    &owner,
                    &actor,
                    &candidates,
                    i32::try_from(thread_id).context("Thread ID is outside the ADO range")?,
                    skipped,
                )
                .await
                {
                    Ok(details) => {
                        let failed = details["failures"]
                            .as_u64()
                            .context("Missing supersession outcome count")?
                            > 0;
                        data["supersession"] = details;
                        if failed {
                            return Ok(ExecutionResult::warning_with_data(
                                "New comment posted, but some older comments could not be superseded",
                                data,
                            ));
                        }
                    }
                    Err(error) => {
                        data["supersession_error"] = serde_json::json!(format!("{error:#}"));
                        return Ok(ExecutionResult::warning_with_data(
                            "New comment posted, but supersession failed",
                            data,
                        ));
                    }
                }
            }
            Ok(ExecutionResult::success_with_data(
                format!("Added comment thread #{thread_id} to PR #{pull_request_id}"),
                data,
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            Ok(ExecutionResult::failure(format!(
                "Failed to add comment to PR #{} (HTTP {}): {}",
                pull_request_id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;
    use tempfile::tempdir;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(AddPrCommentResult::NAME, "add-pull-request-comment");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"pull_request_id": 42, "content": "This is a review comment on the PR."}"#;
        let params: AddPrCommentParams = serde_json::from_str(json).unwrap();
        assert_eq!(
            params.pull_request_id,
            Some(PullRequestReference::Number(42))
        );
        assert!(params.content.contains("review comment"));
        assert_eq!(params.repository, None);
        assert!(params.file_path.is_none());
        assert!(params.line.is_none());
        assert_eq!(params.status, "active");
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = AddPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            content: "This is a test comment with enough characters.".to_string(),
            repository: Some("self".to_string()),
            file_path: None,
            start_line: None,
            line: None,
            side: PrCommentSide::Right,
            expected_head_sha: None,
            status: "active".to_string(),
        };
        let result: AddPrCommentResult = params.try_into().unwrap();
        assert_eq!(result.name, "add-pull-request-comment");
        assert_eq!(
            result.pull_request_id,
            Some(PullRequestReference::Number(42))
        );
        assert!(result.content.contains("test comment"));
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = AddPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(0)),
            content: "This is a valid comment body text.".to_string(),
            repository: Some("self".to_string()),
            file_path: None,
            start_line: None,
            line: None,
            side: PrCommentSide::Right,
            expected_head_sha: None,
            status: "active".to_string(),
        };
        let err: Result<AddPrCommentResult, _> = params.try_into();
        let err = err.unwrap_err().to_string();
        assert!(
            err.contains("pull_request_id must be a positive integer"),
            "got: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_short_content() {
        let params = AddPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            content: "Too short".to_string(),
            repository: Some("self".to_string()),
            file_path: None,
            start_line: None,
            line: None,
            side: PrCommentSide::Right,
            expected_head_sha: None,
            status: "active".to_string(),
        };
        let err: Result<AddPrCommentResult, _> = params.try_into();
        let err = err.unwrap_err().to_string();
        assert!(
            err.contains("content must be at least 10 characters"),
            "got: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_repository_pipeline_command() {
        let params = AddPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            content: "This is a valid comment body text.".to_string(),
            repository: Some("##vso[task.setvariable variable=x]y".to_string()),
            file_path: None,
            start_line: None,
            line: None,
            side: PrCommentSide::Right,
            expected_head_sha: None,
            status: "active".to_string(),
        };
        let err: Result<AddPrCommentResult, _> = params.try_into();
        let err = err.unwrap_err().to_string();
        assert!(err.contains("pipeline command"), "got: {err}");
    }

    #[test]
    fn test_validation_rejects_repository_traversal_selector() {
        let params = AddPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            content: "This is a valid comment body text.".to_string(),
            repository: Some("../sibling-repo".to_string()),
            file_path: None,
            start_line: None,
            line: None,
            side: PrCommentSide::Right,
            expected_head_sha: None,
            status: "active".to_string(),
        };
        let result: Result<AddPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_accepts_project_scoped_repository_selector() {
        let params = AddPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            content: "This is a valid comment body text.".to_string(),
            repository: Some("4x4/sdk-FtdiDeviceControl".to_string()),
            file_path: None,
            start_line: None,
            line: None,
            side: PrCommentSide::Right,
            expected_head_sha: None,
            status: "active".to_string(),
        };
        let result: Result<AddPrCommentResult, _> = params.try_into();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validation_rejects_line_without_file_path() {
        let params = AddPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            content: "This is a valid comment body text.".to_string(),
            repository: Some("self".to_string()),
            file_path: None,
            start_line: None,
            line: Some(10),
            side: PrCommentSide::Right,
            expected_head_sha: None,
            status: "active".to_string(),
        };
        let err: Result<AddPrCommentResult, _> = params.try_into();
        let err = err.unwrap_err().to_string();
        assert!(err.contains("line requires file_path"), "got: {err}");
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = AddPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            content: "A comment body that is definitely longer than ten characters.".to_string(),
            repository: Some("self".to_string()),
            file_path: Some("src/main.rs".to_string()),
            start_line: None,
            line: Some(10),
            side: PrCommentSide::Right,
            expected_head_sha: Some(CommitSha::parse("a".repeat(40)).unwrap()),
            status: "active".to_string(),
        };
        let result: AddPrCommentResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"add-pull-request-comment""#));
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
            pull_request_id: Some(PullRequestReference::Number(42)),
            content: "This is a valid comment body text.".to_string(),
            repository: Some("self".to_string()),
            file_path: None,
            start_line: None,
            line: None,
            side: PrCommentSide::Right,
            expected_head_sha: None,
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
                pull_request_id: Some(PullRequestReference::Number(42)),
                content: "This is a valid comment body text.".to_string(),
                repository: Some("self".to_string()),
                file_path: None,
                start_line: None,
                line: None,
                side: PrCommentSide::Right,
                expected_head_sha: None,
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
            max: None,
            ..Default::default()
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
            pull_request_id: Some(PullRequestReference::Number(42)),
            content: "This is a valid comment body text.".to_string(),
            repository: Some("##vso[task.setvariable variable=x]y".to_string()),
            file_path: None,
            start_line: None,
            line: None,
            side: PrCommentSide::Right,
            expected_head_sha: None,
            status: "active".to_string(),
        };
        let mut result = AddPrCommentResult {
            name: "add-pull-request-comment".to_string(),
            pull_request_id: params.pull_request_id,
            content: params.content,
            repository: params.repository,
            file_path: params.file_path,
            start_line: params.start_line,
            line: params.line,
            side: params.side,
            expected_head_sha: params.expected_head_sha,
            status: params.status,
        };
        result.sanitize_content_fields();
        assert!(
            result.repository.as_deref().unwrap().contains("`##vso[`"),
            "repository pipeline command should be neutralized with backticks: {:?}",
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

        let resolved =
            crate::safe_outputs::resolve_repository_checkout_dir("4x4/sdk-ftdidevicecontrol", &ctx)
                .unwrap();

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

        let resolved = crate::safe_outputs::resolve_repository_checkout_dir(
            "repo-sdk-ftdidevicecontrol",
            &ctx,
        )
        .unwrap();

        assert_eq!(resolved, alias_dir);
    }

    #[test]
    fn test_repository_checkout_dir_treats_empty_repository_as_self() {
        let workspace = tempdir().unwrap();
        let self_dir = workspace.path().join("current-repo");
        let ctx = ExecutionContext {
            source_directory: workspace.path().to_path_buf(),
            self_repository_directory: self_dir.clone(),
            repository_name: Some("4x4/current-repo".to_string()),
            ..Default::default()
        };

        let resolved = crate::safe_outputs::resolve_repository_checkout_dir("", &ctx).unwrap();

        assert_eq!(resolved, self_dir);
    }
}
