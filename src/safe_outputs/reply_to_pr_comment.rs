//! Reply to PR review comment safe output tool

use ado_aw_derive::SanitizeConfig;
use log::{debug, info};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::pr_common::{PullRequestReference, describe_pr_reference, repository_api_base, resolve_configured_pr_target, validate_reference};
use super::{ToolResult, authenticate_ado_request};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_markdown, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use anyhow::{Context, ensure};

/// Parameters for replying to an existing review comment thread on a pull request
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplyToPrCommentParams {
    /// The pull request ID containing the thread
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,

    /// The thread ID to reply to
    pub thread_id: i32,

    /// Reply text in markdown format. Ensure adequate content > 10 characters.
    pub content: String,

    /// Repository alias: "self" for pipeline repo, or an alias from the checkout list.
    /// Defaults to "self" if omitted.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for ReplyToPrCommentParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(reference) = &self.pull_request_id {
            validate_reference(reference)?;
        }
        ensure!(self.thread_id > 0, "thread_id must be positive");
        ensure!(
            self.content.len() >= 10,
            "content must be at least 10 characters"
        );
        super::pr_comments::validate_body(&self.content)?;
        if let Some(repository) = &self.repository {
            reject_pipeline_injection(repository, "repository")?;
        }
        Ok(())
    }
}

tool_result! {
    name = "reply-to-pull-request-comment",
    write = true,
    params = ReplyToPrCommentParams,
    /// Result of replying to a review comment thread on a pull request
    #[serde(deny_unknown_fields)]
    pub struct ReplyToPrCommentResult {
        #[serde(default)]
        pull_request_id: Option<PullRequestReference>,
        thread_id: i32,
        content: String,
        repository: Option<String>,
    }
}

impl SanitizeContent for ReplyToPrCommentResult {
    fn sanitize_content_fields(&mut self) {
        self.content = sanitize_markdown(&self.content);
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

/// Configuration for the reply-to-pull-request-comment tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   reply-to-pull-request-comment:
///     comment-prefix: "[Agent] "
///     allowed-repositories:
///       - self
///       - other-repo
/// ```
#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyToPrCommentConfig {
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
    /// Prefix prepended to all replies (e.g., `"[Agent] "`)
    #[serde(default, rename = "comment-prefix")]
    pub comment_prefix: Option<String>,

    /// Restrict which repositories the agent can reply on.
    /// If empty, all repositories in the checkout list (plus "self") are allowed.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

#[async_trait::async_trait]
impl Executor for ReplyToPrCommentResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "reply to thread #{} on {}",
            self.thread_id, describe_pr_reference(self.pull_request_id.as_ref())
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if let Err(error) = (ReplyToPrCommentParams {
            pull_request_id: self.pull_request_id.clone(),
            thread_id: self.thread_id,
            content: self.content.clone(),
            repository: self.repository.clone(),
        }).validate() {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;

        let config: ReplyToPrCommentConfig =
            ctx.get_tool_config("reply-to-pull-request-comment")?;
        debug!("Config: {:?}", config);

        super::pr_common::validate_temporary_opt_in(self.pull_request_id.as_ref(), config.allow_temporary_ids)?;
        let (pull_request_id, target) = match resolve_configured_pr_target(
            Self::NAME, self.pull_request_id.as_ref(), self.repository.as_deref(), ctx,
        ).await? {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        let repo_name = target.qualified_repository();
        let project = &target.project;

        // Build comment content with optional prefix
        let comment_body = match &config.comment_prefix {
            Some(prefix) => format!("{}{}", prefix, self.content),
            None => self.content.clone(),
        };
        super::pr_comments::validate_body(&comment_body)?;

        // Build the API URL for adding a comment to an existing thread
        let url = format!(
            "{}/pullRequests/{}/threads/{}/comments?api-version=7.1",
            repository_api_base(&target),
            pull_request_id,
            self.thread_id,
        );
        debug!("API URL: {}", url);

        // parentCommentId=1 targets the root comment in the thread. In ADO,
        // the first comment in a thread is always ID 1 (IDs are thread-scoped).
        let request_body = serde_json::json!({
            "parentCommentId": 1,
            "content": comment_body,
            "commentType": 1
        });

        let client = reqwest::Client::new();

        info!(
            "Sending reply to PR #{} thread #{}",
            pull_request_id, self.thread_id
        );
        let response = authenticate_ado_request(client.post(&url), token, ctx.write_connection_type)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            let comment_id = body.get("id").and_then(|v| v.as_i64()).filter(|id| *id > 0)
                .context("Reply response missing a positive comment ID")?;

            info!(
                "Reply added to PR #{} thread #{}: comment #{}",
                pull_request_id, self.thread_id, comment_id
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Added reply #{} to PR #{} thread #{}",
                    comment_id, pull_request_id, self.thread_id
                ),
                serde_json::json!({
                    "comment_id": comment_id,
                    "pull_request_id": pull_request_id,
                    "thread_id": self.thread_id,
                    "repository": repo_name,
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
                "Failed to reply to PR #{} thread #{} (HTTP {}): {}",
                pull_request_id, self.thread_id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"pull_request_id": 42, "thread_id": 7, "content": "This is a reply to the review comment."}"#;
        let params: ReplyToPrCommentParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pull_request_id, Some(PullRequestReference::Number(42)));
        assert_eq!(params.thread_id, 7);
        assert_eq!(params.content, "This is a reply to the review comment.");
        assert_eq!(params.repository, None);
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = ReplyToPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 7,
            content: "This is a test reply with enough characters.".to_string(),
            repository: Some("self".to_string()),
        };
        let result: ReplyToPrCommentResult = params.try_into().unwrap();
        assert_eq!(result.name, "reply-to-pull-request-comment");
        assert_eq!(result.pull_request_id, Some(PullRequestReference::Number(42)));
        assert_eq!(result.thread_id, 7);
        assert_eq!(
            result.content,
            "This is a test reply with enough characters."
        );
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = ReplyToPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(0)),
            thread_id: 7,
            content: "This is a valid reply body text.".to_string(),
            repository: Some("self".to_string()),
        };
        let result: Result<ReplyToPrCommentResult, _> = params.try_into();
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("pull_request_id must be a positive integer"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_zero_thread_id() {
        let params = ReplyToPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 0,
            content: "This is a valid reply body text.".to_string(),
            repository: Some("self".to_string()),
        };
        let result: Result<ReplyToPrCommentResult, _> = params.try_into();
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("thread_id must be positive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_short_content() {
        let params = ReplyToPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 7,
            content: "Too short".to_string(),
            repository: Some("self".to_string()),
        };
        let result: Result<ReplyToPrCommentResult, _> = params.try_into();
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("content must be at least 10 characters"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_repository_pipeline_command() {
        let params = ReplyToPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 7,
            content: "This is a valid reply body text.".to_string(),
            repository: Some("##vso[task.setvariable variable=x]y".to_string()),
        };
        let result: Result<ReplyToPrCommentResult, _> = params.try_into();
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("repository"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains("##vso["),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = ReplyToPrCommentParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 7,
            content: "A reply body that is definitely longer than ten characters.".to_string(),
            repository: Some("self".to_string()),
        };
        let result: ReplyToPrCommentResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"reply-to-pull-request-comment""#));
        assert!(json.contains(r#""pull_request_id":42"#));
        assert!(json.contains(r#""thread_id":7"#));
    }

    #[test]
    fn test_config_defaults() {
        let config = ReplyToPrCommentConfig::default();
        assert!(config.comment_prefix.is_none());
        assert!(config.allowed_repositories.is_empty());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
comment-prefix: "[Agent] "
allowed-repositories:
  - self
  - other-repo
"#;
        let config: ReplyToPrCommentConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.comment_prefix, Some("[Agent] ".to_string()));
        assert_eq!(config.allowed_repositories, vec!["self", "other-repo"]);
    }

    #[test]
    fn test_sanitize_content_neutralizes_repository_pipeline_command() {
        let mut result = ReplyToPrCommentResult {
            name: "reply-to-pull-request-comment".to_string(),
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 7,
            content: "This is a valid reply body text.".to_string(),
            repository: Some("##vso[task.setvariable variable=x]y".to_string()),
        };
        result.sanitize_content_fields();
        let repository = result.repository.as_deref().unwrap_or("");
        assert!(
            repository.contains("`##vso[`"),
            "repository pipeline command should be neutralized with backticks: {}",
            repository
        );
    }
}
