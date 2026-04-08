//! Reply to PR review comment safe output tool

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use crate::tool_result;
use crate::tools::{ExecutionContext, ExecutionResult, Executor, Validate};
use anyhow::{Context, ensure};

/// Parameters for replying to an existing review comment thread on a pull request
#[derive(Deserialize, JsonSchema)]
pub struct ReplyToPrCommentParams {
    /// The pull request ID containing the thread
    pub pull_request_id: i32,

    /// The thread ID to reply to
    pub thread_id: i32,

    /// Reply text in markdown format. Ensure adequate content > 10 characters.
    pub content: String,

    /// Repository alias: "self" for pipeline repo, or an alias from the checkout list.
    /// Defaults to "self" if omitted.
    #[serde(default = "default_repository")]
    pub repository: Option<String>,
}

fn default_repository() -> Option<String> {
    Some("self".to_string())
}

impl Validate for ReplyToPrCommentParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.pull_request_id > 0, "pull_request_id must be positive");
        ensure!(self.thread_id > 0, "thread_id must be positive");
        ensure!(
            self.content.len() >= 10,
            "content must be at least 10 characters"
        );
        Ok(())
    }
}

tool_result! {
    name = "reply-to-pr-review-comment",
    params = ReplyToPrCommentParams,
    /// Result of replying to a review comment thread on a pull request
    pub struct ReplyToPrCommentResult {
        pull_request_id: i32,
        thread_id: i32,
        content: String,
        repository: Option<String>,
    }
}

impl Sanitize for ReplyToPrCommentResult {
    fn sanitize_fields(&mut self) {
        self.content = sanitize_text(&self.content);
    }
}

/// Configuration for the reply-to-pr-review-comment tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   reply-to-pr-review-comment:
///     comment-prefix: "[Agent] "
///     allowed-repositories:
///       - self
///       - other-repo
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyToPrCommentConfig {
    /// Prefix prepended to all replies (e.g., "[Agent] ")
    #[serde(default, rename = "comment-prefix")]
    pub comment_prefix: Option<String>,

    /// Restrict which repositories the agent can reply on.
    /// If empty, all repositories in the checkout list (plus "self") are allowed.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
}

impl Default for ReplyToPrCommentConfig {
    fn default() -> Self {
        Self {
            comment_prefix: None,
            allowed_repositories: Vec::new(),
        }
    }
}

#[async_trait::async_trait]
impl Executor for ReplyToPrCommentResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Replying to PR #{} thread #{}: {} chars",
            self.pull_request_id,
            self.thread_id,
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

        let config: ReplyToPrCommentConfig = ctx.get_tool_config("reply-to-pr-review-comment");
        debug!("Config: {:?}", config);

        let repository = self
            .repository
            .as_deref()
            .unwrap_or("self");

        // Validate repository against allowed-repositories config
        if !config.allowed_repositories.is_empty()
            && !config.allowed_repositories.contains(&repository.to_string())
        {
            return Ok(ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed-repositories list",
                repository
            )));
        }

        // Determine the repository name for the API call
        let repo_name = if repository == "self" || repository.is_empty() {
            ctx.repository_name
                .as_ref()
                .context("BUILD_REPOSITORY_NAME not set and repository is 'self'")?
                .clone()
        } else {
            match ctx.allowed_repositories.get(repository) {
                Some(name) => name.clone(),
                None => {
                    return Ok(ExecutionResult::failure(format!(
                        "Repository alias '{}' not found in allowed repositories",
                        repository
                    )));
                }
            }
        };

        // Build comment content with optional prefix
        let comment_body = match &config.comment_prefix {
            Some(prefix) => format!("{}{}", prefix, self.content),
            None => self.content.clone(),
        };

        // Build the API URL for adding a comment to an existing thread
        let url = format!(
            "{}/{}/_apis/git/repositories/{}/pullRequests/{}/threads/{}/comments?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            utf8_percent_encode(&repo_name, PATH_SEGMENT),
            self.pull_request_id,
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
            self.pull_request_id, self.thread_id
        );
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            let comment_id = body.get("id").and_then(|v| v.as_i64()).unwrap_or(0);

            info!(
                "Reply added to PR #{} thread #{}: comment #{}",
                self.pull_request_id, self.thread_id, comment_id
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Added reply #{} to PR #{} thread #{}",
                    comment_id, self.pull_request_id, self.thread_id
                ),
                serde_json::json!({
                    "comment_id": comment_id,
                    "pull_request_id": self.pull_request_id,
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
                self.pull_request_id, self.thread_id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(ReplyToPrCommentResult::NAME, "reply-to-pr-review-comment");
    }

    #[test]
    fn test_params_deserializes() {
        let json =
            r#"{"pull_request_id": 42, "thread_id": 7, "content": "This is a reply to the review comment."}"#;
        let params: ReplyToPrCommentParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pull_request_id, 42);
        assert_eq!(params.thread_id, 7);
        assert!(params.content.contains("reply"));
        assert_eq!(params.repository, Some("self".to_string()));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = ReplyToPrCommentParams {
            pull_request_id: 42,
            thread_id: 7,
            content: "This is a test reply with enough characters.".to_string(),
            repository: Some("self".to_string()),
        };
        let result: ReplyToPrCommentResult = params.try_into().unwrap();
        assert_eq!(result.name, "reply-to-pr-review-comment");
        assert_eq!(result.pull_request_id, 42);
        assert_eq!(result.thread_id, 7);
        assert!(result.content.contains("test reply"));
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = ReplyToPrCommentParams {
            pull_request_id: 0,
            thread_id: 7,
            content: "This is a valid reply body text.".to_string(),
            repository: Some("self".to_string()),
        };
        let result: Result<ReplyToPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_zero_thread_id() {
        let params = ReplyToPrCommentParams {
            pull_request_id: 42,
            thread_id: 0,
            content: "This is a valid reply body text.".to_string(),
            repository: Some("self".to_string()),
        };
        let result: Result<ReplyToPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_short_content() {
        let params = ReplyToPrCommentParams {
            pull_request_id: 42,
            thread_id: 7,
            content: "Too short".to_string(),
            repository: Some("self".to_string()),
        };
        let result: Result<ReplyToPrCommentResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = ReplyToPrCommentParams {
            pull_request_id: 42,
            thread_id: 7,
            content: "A reply body that is definitely longer than ten characters.".to_string(),
            repository: Some("self".to_string()),
        };
        let result: ReplyToPrCommentResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"reply-to-pr-review-comment""#));
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
}
