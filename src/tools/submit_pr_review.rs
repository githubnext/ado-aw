//! Submit PR review safe output tool

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PATH_SEGMENT, resolve_repo_name};
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use crate::tool_result;
use crate::tools::{ExecutionContext, ExecutionResult, Executor, Validate};
use anyhow::{Context, ensure};

/// Valid event values for submit-pr-review
const VALID_EVENTS: &[&str] = &[
    "approve",
    "approve-with-suggestions",
    "request-changes",
    "comment",
];

/// Map a review event string to its ADO vote numeric value
fn event_to_vote(event: &str) -> Option<i32> {
    match event {
        "approve" => Some(10),
        "approve-with-suggestions" => Some(5),
        "request-changes" => Some(-5),
        "comment" => Some(0),
        _ => None,
    }
}

fn default_repository() -> String {
    "self".to_string()
}

/// Parameters for submitting a pull request review
#[derive(Deserialize, JsonSchema)]
pub struct SubmitPrReviewParams {
    /// The pull request ID to review (must be positive)
    pub pull_request_id: i32,

    /// Review decision: "approve", "approve-with-suggestions", "request-changes", or "comment"
    pub event: String,

    /// Review rationale in markdown. Required for "request-changes", optional otherwise.
    /// Must be at least 10 characters when provided.
    #[serde(default)]
    pub body: Option<String>,

    /// Repository alias: "self" for pipeline repo, or an alias from the checkout list.
    /// Defaults to "self" if omitted.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for SubmitPrReviewParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.pull_request_id > 0,
            "pull_request_id must be a positive integer"
        );
        ensure!(
            VALID_EVENTS.contains(&self.event.as_str()),
            "event must be one of: {}",
            VALID_EVENTS.join(", ")
        );
        if self.event == "request-changes" {
            ensure!(
                self.body.is_some(),
                "body is required when event is 'request-changes'"
            );
        }
        if let Some(ref body) = self.body {
            ensure!(
                body.len() >= 10,
                "body must be at least 10 characters"
            );
        }
        Ok(())
    }
}

tool_result! {
    name = "submit-pr-review",
    params = SubmitPrReviewParams,
    /// Result of submitting a pull request review
    pub struct SubmitPrReviewResult {
        pull_request_id: i32,
        event: String,
        body: Option<String>,
        repository: Option<String>,
    }
}

impl Sanitize for SubmitPrReviewResult {
    fn sanitize_fields(&mut self) {
        self.event = sanitize_text(&self.event);
        self.body = self.body.as_deref().map(sanitize_text);
        self.repository = self.repository.as_deref().map(sanitize_text);
    }
}

/// Configuration for the submit-pr-review tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   submit-pr-review:
///     allowed-events:
///       - approve
///       - comment
///     allowed-repositories:
///       - self
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitPrReviewConfig {
    /// Which events are permitted. REQUIRED — empty list rejects all.
    #[serde(default, rename = "allowed-events")]
    pub allowed_events: Vec<String>,

    /// Which repositories the agent may target. Empty list means all allowed repos.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
}

impl Default for SubmitPrReviewConfig {
    fn default() -> Self {
        Self {
            allowed_events: Vec::new(),
            allowed_repositories: Vec::new(),
        }
    }
}

#[async_trait::async_trait]
impl Executor for SubmitPrReviewResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Submitting review on PR #{} — event: {}",
            self.pull_request_id, self.event
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

        let config: SubmitPrReviewConfig = ctx.get_tool_config("submit-pr-review");
        debug!("Config: {:?}", config);

        // Validate event against allowed-events — REQUIRED.
        // An empty allowed-events list means the operator hasn't opted in, so reject.
        if config.allowed_events.is_empty() {
            return Ok(ExecutionResult::failure(
                "submit-pr-review requires 'allowed-events' to be configured in \
                 safe-outputs.submit-pr-review. This prevents agents from casting \
                 unrestricted review votes. Example:\n  safe-outputs:\n    submit-pr-review:\n      \
                 allowed-events:\n        - comment\n        - approve-with-suggestions"
                    .to_string(),
            ));
        }
        if !config.allowed_events.contains(&self.event) {
            return Ok(ExecutionResult::failure(format!(
                "Event '{}' is not in the allowed-events list: [{}]",
                self.event,
                config.allowed_events.join(", ")
            )));
        }

        // Validate repository against allowed-repositories config
        let repo_alias = self.repository.as_deref().unwrap_or("self");
        if !config.allowed_repositories.is_empty()
            && !config.allowed_repositories.contains(&repo_alias.to_string())
        {
            return Ok(ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed-repositories list: [{}]",
                repo_alias,
                config.allowed_repositories.join(", ")
            )));
        }

        // Resolve repo name
        let repo_name = match resolve_repo_name(self.repository.as_deref(), ctx) {
            Ok(name) => name,
            Err(failure) => return Ok(failure),
        };
        debug!("Resolved repository: {}", repo_name);

        // Map event to vote value
        let vote_value = event_to_vote(&self.event).context(format!(
            "Invalid event: '{}'. Must be one of: {}",
            self.event,
            VALID_EVENTS.join(", ")
        ))?;

        let client = reqwest::Client::new();
        let encoded_project = utf8_percent_encode(project, PATH_SEGMENT).to_string();
        let encoded_repo = utf8_percent_encode(&repo_name, PATH_SEGMENT).to_string();
        let base_url = format!(
            "{}/{}/_apis/git/repositories",
            org_url.trim_end_matches('/'),
            encoded_project,
        );

        // Resolve the current user identity via connection data.
        // Use the org URL — supports vanity domains and national clouds.
        let connection_url = format!(
            "{}/_apis/connectiondata",
            org_url.trim_end_matches('/')
        );
        debug!("Connection data URL: {}", connection_url);

        let conn_response = client
            .get(&connection_url)
            .basic_auth("", Some(token))
            .send()
            .await
            .context("Failed to fetch connection data")?;

        if !conn_response.status().is_success() {
            let status = conn_response.status();
            let error_body = conn_response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Ok(ExecutionResult::failure(format!(
                "Failed to fetch connection data (HTTP {}): {}",
                status, error_body
            )));
        }

        let conn_body: serde_json::Value = conn_response
            .json()
            .await
            .context("Failed to parse connection data response")?;

        let user_id = conn_body
            .get("authenticatedUser")
            .and_then(|au| au.get("id"))
            .and_then(|id| id.as_str())
            .context("Connection data response missing authenticatedUser.id")?;
        debug!("Authenticated user ID: {}", user_id);

        // PUT vote to reviewers endpoint
        let encoded_user_id = utf8_percent_encode(user_id, PATH_SEGMENT).to_string();
        let vote_url = format!(
            "{}/{}/pullRequests/{}/reviewers/{}?api-version=7.1",
            base_url, encoded_repo, self.pull_request_id, encoded_user_id
        );
        let vote_body = serde_json::json!({
            "vote": vote_value
        });

        info!(
            "Voting '{}' ({}) on PR #{}",
            self.event, vote_value, self.pull_request_id
        );
        let response = client
            .put(&vote_url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&vote_body)
            .send()
            .await
            .context("Failed to submit vote")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Ok(ExecutionResult::failure(format!(
                "Failed to submit vote on PR #{} (HTTP {}): {}",
                self.pull_request_id, status, error_body
            )));
        }

        info!(
            "Vote '{}' submitted on PR #{}",
            self.event, self.pull_request_id
        );

        // If body is provided, also POST a comment thread with the review rationale
        if let Some(ref body) = self.body {
            let thread_url = format!(
                "{}/{}/pullRequests/{}/threads?api-version=7.1",
                base_url, encoded_repo, self.pull_request_id
            );
            let thread_body = serde_json::json!({
                "comments": [{
                    "parentCommentId": 0,
                    "content": body,
                    "commentType": 1
                }],
                "status": 1
            });

            info!(
                "Posting review comment on PR #{} ({} chars)",
                self.pull_request_id,
                body.len()
            );
            let thread_response = client
                .post(&thread_url)
                .header("Content-Type", "application/json")
                .basic_auth("", Some(token))
                .json(&thread_body)
                .send()
                .await
                .context("Failed to post review comment thread")?;

            if !thread_response.status().is_success() {
                let status = thread_response.status();
                let error_body = thread_response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string());
                return Ok(ExecutionResult::failure(format!(
                    "Vote submitted but failed to post review comment on PR #{} (HTTP {}): {}",
                    self.pull_request_id, status, error_body
                )));
            }

            let thread_resp: serde_json::Value = thread_response
                .json()
                .await
                .context("Failed to parse comment thread response")?;

            let thread_id = thread_resp
                .get("id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            info!(
                "Review comment thread #{} posted on PR #{}",
                thread_id, self.pull_request_id
            );

            return Ok(ExecutionResult::success_with_data(
                format!(
                    "Review '{}' submitted on PR #{} with comment thread #{}",
                    self.event, self.pull_request_id, thread_id
                ),
                serde_json::json!({
                    "pull_request_id": self.pull_request_id,
                    "event": self.event,
                    "vote_value": vote_value,
                    "thread_id": thread_id,
                    "repository": repo_name,
                }),
            ));
        }

        Ok(ExecutionResult::success_with_data(
            format!(
                "Review '{}' submitted on PR #{}",
                self.event, self.pull_request_id
            ),
            serde_json::json!({
                "pull_request_id": self.pull_request_id,
                "event": self.event,
                "vote_value": vote_value,
                "repository": repo_name,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(SubmitPrReviewResult::NAME, "submit-pr-review");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"pull_request_id": 42, "event": "approve"}"#;
        let params: SubmitPrReviewParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pull_request_id, 42);
        assert_eq!(params.event, "approve");
        assert!(params.body.is_none());
        assert!(params.repository.is_none());
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = SubmitPrReviewParams {
            pull_request_id: 42,
            event: "approve".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let result: SubmitPrReviewResult = params.try_into().unwrap();
        assert_eq!(result.name, "submit-pr-review");
        assert_eq!(result.pull_request_id, 42);
        assert_eq!(result.event, "approve");
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = SubmitPrReviewParams {
            pull_request_id: 0,
            event: "approve".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let result: Result<SubmitPrReviewResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_invalid_event() {
        let params = SubmitPrReviewParams {
            pull_request_id: 1,
            event: "merge".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let result: Result<SubmitPrReviewResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_request_changes_without_body() {
        let params = SubmitPrReviewParams {
            pull_request_id: 1,
            event: "request-changes".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let result: Result<SubmitPrReviewResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_accepts_approve_without_body() {
        let params = SubmitPrReviewParams {
            pull_request_id: 1,
            event: "approve".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let result: Result<SubmitPrReviewResult, _> = params.try_into();
        assert!(result.is_ok());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = SubmitPrReviewParams {
            pull_request_id: 99,
            event: "request-changes".to_string(),
            body: Some("This needs significant rework before merging.".to_string()),
            repository: Some("self".to_string()),
        };
        let result: SubmitPrReviewResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"submit-pr-review""#));
        assert!(json.contains(r#""pull_request_id":99"#));
        assert!(json.contains(r#""event":"request-changes""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = SubmitPrReviewConfig::default();
        assert!(config.allowed_events.is_empty());
        assert!(config.allowed_repositories.is_empty());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
allowed-events:
  - approve
  - comment
allowed-repositories:
  - self
  - other-repo
"#;
        let config: SubmitPrReviewConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.allowed_events, vec!["approve", "comment"]);
        assert_eq!(config.allowed_repositories, vec!["self", "other-repo"]);
    }
}
