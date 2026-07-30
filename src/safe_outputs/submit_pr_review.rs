//! Submit PR review safe output tool

use ado_aw_derive::SanitizeConfig;
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PATH_SEGMENT, resolve_repo_name};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
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
        if let Some(repository) = &self.repository {
            reject_pipeline_injection(repository, "repository")?;
        }
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
            ensure!(body.len() >= 10, "body must be at least 10 characters");
        }
        Ok(())
    }
}

tool_result! {
    name = "submit-pr-review",
    write = true,
    params = SubmitPrReviewParams,
    /// Result of submitting a pull request review
    pub struct SubmitPrReviewResult {
        pull_request_id: i32,
        event: String,
        body: Option<String>,
        repository: Option<String>,
    }
}

impl SanitizeContent for SubmitPrReviewResult {
    fn sanitize_content_fields(&mut self) {
        self.event = sanitize_config(&self.event);
        self.body = self.body.as_deref().map(sanitize_text);
        self.repository = self.repository.as_deref().map(sanitize_config);
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
#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
pub struct SubmitPrReviewConfig {
    /// Which events are permitted. REQUIRED — empty list rejects all.
    #[serde(default, rename = "allowed-events")]
    pub allowed_events: Vec<String>,

    /// Which repositories the agent may target. Empty list means all allowed repos.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
}

/// Fetches the authenticated user's ID from the ADO connection data endpoint.
/// Returns `Ok(Err(ExecutionResult::failure(...)))` on HTTP errors, `Ok(Ok(user_id))` on success.
async fn fetch_authenticated_user_id(
    client: &reqwest::Client,
    org_url: &str,
    token: &str,
) -> anyhow::Result<Result<String, ExecutionResult>> {
    let connection_url = format!("{}/_apis/connectiondata", org_url.trim_end_matches('/'));
    debug!("Connection data URL: {}", connection_url);

    let response = client
        .get(&connection_url)
        .basic_auth("", Some(token))
        .send()
        .await
        .context("Failed to fetch connection data")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Ok(Err(ExecutionResult::failure(format!(
            "Failed to fetch connection data (HTTP {}): {}",
            status, error_body
        ))));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse connection data response")?;

    let user_id = body
        .get("authenticatedUser")
        .and_then(|au| au.get("id"))
        .and_then(|id| id.as_str())
        .context("Connection data response missing authenticatedUser.id")?
        .to_string();

    debug!("Authenticated user ID: {}", user_id);
    Ok(Ok(user_id))
}

/// Shared context for the vote-related helpers, reducing per-call argument count.
struct PrVoteCtx<'a> {
    client: &'a reqwest::Client,
    base_url: &'a str,
    encoded_repo: &'a str,
    pull_request_id: i32,
    event: &'a str,
    vote_value: i32,
    token: &'a str,
}

/// Self-approval guard: returns `Some(failure)` when a positive vote targets a PR the
/// authenticated user created; returns `None` when the vote is allowed to proceed.
async fn check_self_approval(
    ctx: &PrVoteCtx<'_>,
    user_id: &str,
) -> anyhow::Result<Option<ExecutionResult>> {
    let PrVoteCtx {
        client,
        base_url,
        encoded_repo,
        pull_request_id,
        event,
        vote_value,
        token,
    } = ctx;
    let (pull_request_id, vote_value) = (*pull_request_id, *vote_value);
    if vote_value <= 0 {
        return Ok(None);
    }

    let pr_url = format!(
        "{}/{}/pullRequests/{}?api-version=7.1",
        base_url, encoded_repo, pull_request_id
    );
    let pr_response = client
        .get(&pr_url)
        .basic_auth("", Some(token))
        .send()
        .await
        .context("Failed to fetch PR for self-approval check")?;

    if !pr_response.status().is_success() {
        let status = pr_response.status();
        let error_body = pr_response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Ok(Some(ExecutionResult::failure(format!(
            "Failed to fetch PR #{} for self-approval check (HTTP {}): {}",
            pull_request_id, status, error_body
        ))));
    }

    let pr_body: serde_json::Value = pr_response
        .json()
        .await
        .context("Failed to parse PR response")?;

    let creator_id = pr_body
        .get("createdBy")
        .and_then(|cb| cb.get("id"))
        .and_then(|id| id.as_str());

    if creator_id == Some(user_id) {
        return Ok(Some(ExecutionResult::failure(format!(
            "Self-approval blocked: the authenticated identity created PR #{} \
             and cannot cast a positive vote ('{}') on it",
            pull_request_id, event
        ))));
    }

    Ok(None)
}

/// PUTs the review vote to the ADO reviewers endpoint.
/// Returns `Err` on network errors, `Ok(Some(failure))` on HTTP errors, `Ok(None)` on success.
async fn submit_vote(
    ctx: &PrVoteCtx<'_>,
    encoded_user_id: &str,
) -> anyhow::Result<Option<ExecutionResult>> {
    let PrVoteCtx {
        client,
        base_url,
        encoded_repo,
        pull_request_id,
        event,
        vote_value,
        token,
    } = ctx;
    let (pull_request_id, vote_value) = (*pull_request_id, *vote_value);
    let vote_url = format!(
        "{}/{}/pullRequests/{}/reviewers/{}?api-version=7.1",
        base_url, encoded_repo, pull_request_id, encoded_user_id
    );
    info!(
        "Voting '{}' ({}) on PR #{}",
        event, vote_value, pull_request_id
    );
    let response = client
        .put(&vote_url)
        .header("Content-Type", "application/json")
        .basic_auth("", Some(token))
        .json(&serde_json::json!({ "vote": vote_value }))
        .send()
        .await
        .context("Failed to submit vote")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Ok(Some(ExecutionResult::failure(format!(
            "Failed to submit vote on PR #{} (HTTP {}): {}",
            pull_request_id, status, error_body
        ))));
    }

    info!("Vote '{}' submitted on PR #{}", event, pull_request_id);
    Ok(None)
}

/// POSTs an optional review comment thread. Returns the ADO thread ID on success, or a failure.
async fn post_review_comment_thread(
    client: &reqwest::Client,
    base_url: &str,
    encoded_repo: &str,
    pull_request_id: i32,
    body: &str,
    token: &str,
) -> anyhow::Result<Result<i64, ExecutionResult>> {
    let thread_url = format!(
        "{}/{}/pullRequests/{}/threads?api-version=7.1",
        base_url, encoded_repo, pull_request_id
    );
    info!(
        "Posting review comment on PR #{} ({} chars)",
        pull_request_id,
        body.len()
    );
    let response = client
        .post(&thread_url)
        .header("Content-Type", "application/json")
        .basic_auth("", Some(token))
        .json(&serde_json::json!({
            "comments": [{"parentCommentId": 0, "content": body, "commentType": 1}],
            "status": 1
        }))
        .send()
        .await
        .context("Failed to post review comment thread")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Ok(Err(ExecutionResult::failure(format!(
            "Vote submitted but failed to post review comment on PR #{} (HTTP {}): {}",
            pull_request_id, status, error_body
        ))));
    }

    let thread_resp: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse comment thread response")?;

    let thread_id = thread_resp.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
    info!(
        "Review comment thread #{} posted on PR #{}",
        thread_id, pull_request_id
    );
    Ok(Ok(thread_id))
}

#[async_trait::async_trait]
impl Executor for SubmitPrReviewResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "submit '{}' review on PR #{}",
            self.event, self.pull_request_id
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Submitting review on PR #{} — event: {}",
            self.pull_request_id, self.event
        );
        debug!(
            "submit-pr-review: pr_id={}, event='{}'",
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
            && !config
                .allowed_repositories
                .contains(&repo_alias.to_string())
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
        let user_id = match fetch_authenticated_user_id(&client, org_url, token).await? {
            Ok(id) => id,
            Err(failure) => return Ok(failure),
        };

        // Self-approval guard: prevent the agent from approving PRs it created.
        let vote_ctx = PrVoteCtx {
            client: &client,
            base_url: &base_url,
            encoded_repo: &encoded_repo,
            pull_request_id: self.pull_request_id,
            event: &self.event,
            vote_value,
            token,
        };
        if let Some(failure) = check_self_approval(&vote_ctx, &user_id).await? {
            return Ok(failure);
        }

        // PUT vote to reviewers endpoint
        let encoded_user_id = utf8_percent_encode(&user_id, PATH_SEGMENT).to_string();
        if let Some(failure) = submit_vote(&vote_ctx, &encoded_user_id).await? {
            return Ok(failure);
        }

        // If body is provided, also POST a comment thread with the review rationale
        if let Some(ref body) = self.body {
            let thread_id = match post_review_comment_thread(
                &client,
                &base_url,
                &encoded_repo,
                self.pull_request_id,
                body,
                token,
            )
            .await?
            {
                Ok(id) => id,
                Err(failure) => return Ok(failure),
            };

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
    use crate::safe_outputs::ToolResult;

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
        let err = <SubmitPrReviewResult as TryFrom<_>>::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("pull_request_id must be a positive integer"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_invalid_event() {
        let params = SubmitPrReviewParams {
            pull_request_id: 1,
            event: "merge".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let err = <SubmitPrReviewResult as TryFrom<_>>::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("event must be one of"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_request_changes_without_body() {
        let params = SubmitPrReviewParams {
            pull_request_id: 1,
            event: "request-changes".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let err = <SubmitPrReviewResult as TryFrom<_>>::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("body is required when event is 'request-changes'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_repository_pipeline_command() {
        let params = SubmitPrReviewParams {
            pull_request_id: 1,
            event: "approve".to_string(),
            body: None,
            repository: Some("##vso[task.setvariable variable=x]y".to_string()),
        };
        let err = <SubmitPrReviewResult as TryFrom<_>>::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("repository") || err.to_string().contains("##vso["),
            "unexpected error: {err}"
        );
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
