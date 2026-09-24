//! Submit PR review safe output tool

use ado_aw_derive::SanitizeConfig;
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::pr_common::{
    PullRequestReference, legacy_policy, resolve_pr_target, validate_reference,
};
use super::pr_mutations::UpdatePrContext;
use super::{PATH_SEGMENT, authenticate_ado_request};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use anyhow::{Context, ensure};

/// Valid event values for submit-pull-request-review
const VALID_EVENTS: &[&str] = &[
    "approve",
    "approve-with-suggestions",
    "request-changes",
    "comment",
    "wait-for-author",
    "reject",
    "reset",
];

/// Map a review event string to its ADO vote numeric value
fn event_to_vote(event: &str) -> Option<i32> {
    match event {
        "approve" => Some(10),
        "approve-with-suggestions" => Some(5),
        "request-changes" | "wait-for-author" => Some(-5),
        "reject" => Some(-10),
        "comment" | "reset" => Some(0),
        _ => None,
    }
}

/// Parameters for submitting a pull request review
#[derive(Deserialize, JsonSchema)]
pub struct SubmitPrReviewParams {
    /// Positive PR ID, or a same-run temporary ID when allow-temporary-ids is enabled.
    pub pull_request_id: PullRequestReference,

    /// Review decision: approve, approve-with-suggestions, request-changes, comment,
    /// wait-for-author, reject, or reset.
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
        validate_reference(&self.pull_request_id)?;
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
    name = "submit-pull-request-review",
    write = true,
    params = SubmitPrReviewParams,
    /// Result of submitting a pull request review
    pub struct SubmitPrReviewResult {
        pull_request_id: PullRequestReference,
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

/// Configuration for the submit-pull-request-review tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   submit-pull-request-review:
///     allowed-events:
///       - approve
///       - comment
///     allowed-repositories:
///       - self
/// ```
#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
pub struct SubmitPrReviewConfig {
    /// Existing numeric-only configurations do not implicitly gain create-then-review authority.
    #[serde(default, rename = "allow-temporary-ids")]
    #[sanitize_config(skip)]
    pub allow_temporary_ids: bool,
    /// Which events are permitted. REQUIRED — empty list rejects all.
    #[serde(default, rename = "allowed-events")]
    pub allowed_events: Vec<String>,

    /// Which repositories the agent may target. Empty list means all allowed repos.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
}

pub(crate) fn validate_submit_pr_review_config(
    config: &SubmitPrReviewConfig,
) -> anyhow::Result<()> {
    for event in &config.allowed_events {
        ensure!(
            VALID_EVENTS.contains(&event.as_str()),
            "unknown submit-pull-request-review event '{event}'"
        );
    }
    for repository in &config.allowed_repositories {
        ensure!(
            !repository.trim().is_empty(),
            "allowed-repositories entries must not be empty"
        );
        reject_pipeline_injection(repository, "allowed-repositories")?;
    }
    Ok(())
}

/// Fetches the authenticated user's ID from the ADO connection data endpoint.
/// Returns `Ok(Err(ExecutionResult::failure(...)))` on HTTP errors, `Ok(Ok(user_id))` on success.
async fn fetch_authenticated_user_id(
    client: &reqwest::Client,
    org_url: &str,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
) -> anyhow::Result<Result<String, ExecutionResult>> {
    let connection_url = format!("{}/_apis/connectiondata", org_url.trim_end_matches('/'));
    debug!("Connection data URL: {}", connection_url);

    let response = authenticate_ado_request(client.get(&connection_url), token, connection_type)
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

/// Shared transport/auth context for the vote-related helpers, reducing per-call argument count.
type PrVoteCtx<'a> = UpdatePrContext<'a>;

/// Self-approval guard: returns `Some(failure)` when a positive vote targets a PR the
/// authenticated user created; returns `None` when the vote is allowed to proceed.
async fn check_self_approval(
    ctx: &PrVoteCtx<'_>,
    user_id: &str,
    event: &str,
    vote_value: i32,
) -> anyhow::Result<Option<ExecutionResult>> {
    if vote_value <= 0 {
        return Ok(None);
    }

    let pr_url = format!(
        "{}/pullRequests/{}?api-version=7.1",
        ctx.repository_api_base(),
        ctx.pr_id
    );
    let pr_response =
        authenticate_ado_request(ctx.client.get(&pr_url), ctx.token, ctx.connection_type)
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
            ctx.pr_id, status, error_body
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

    let Some(creator_id) = creator_id else {
        return Ok(Some(ExecutionResult::failure(
            "PR response missing createdBy.id for self-approval check",
        )));
    };
    if creator_id.eq_ignore_ascii_case(user_id) {
        return Ok(Some(ExecutionResult::failure(format!(
            "Self-approval blocked: the authenticated identity created PR #{} \
             and cannot cast a positive vote ('{}') on it",
            ctx.pr_id, event
        ))));
    }

    Ok(None)
}

/// PUTs the review vote to the ADO reviewers endpoint.
/// Returns `Err` on network errors, `Ok(Some(failure))` on HTTP errors, `Ok(None)` on success.
async fn submit_vote(
    ctx: &PrVoteCtx<'_>,
    encoded_user_id: &str,
    event: &str,
    vote_value: i32,
) -> anyhow::Result<Option<ExecutionResult>> {
    let vote_url = format!(
        "{}/pullRequests/{}/reviewers/{}?api-version=7.1",
        ctx.repository_api_base(),
        ctx.pr_id,
        encoded_user_id
    );
    info!("Voting '{}' ({}) on PR #{}", event, vote_value, ctx.pr_id);
    let response =
        authenticate_ado_request(ctx.client.put(&vote_url), ctx.token, ctx.connection_type)
            .header("Content-Type", "application/json")
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
            ctx.pr_id, status, error_body
        ))));
    }

    info!("Vote '{}' submitted on PR #{}", event, ctx.pr_id);
    Ok(None)
}

/// POSTs an optional review comment thread. Returns the ADO thread ID on success, or a failure.
async fn post_review_comment_thread(
    ctx: &PrVoteCtx<'_>,
    body: &str,
) -> anyhow::Result<Result<i64, ExecutionResult>> {
    let pull_request_id = ctx.pr_id;
    let thread_url = format!(
        "{}/pullRequests/{}/threads?api-version=7.1",
        ctx.repository_api_base(),
        pull_request_id
    );
    info!(
        "Posting review comment on PR #{} ({} chars)",
        pull_request_id,
        body.len()
    );
    let response =
        authenticate_ado_request(ctx.client.post(&thread_url), ctx.token, ctx.connection_type)
            .header("Content-Type", "application/json")
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

/// Sole vote mutation implementation for pull-request reviews.
pub(crate) async fn execute_review_vote(
    ctx: &UpdatePrContext<'_>,
    event: &str,
    vote_value: i32,
) -> anyhow::Result<Option<ExecutionResult>> {
    let user_id = match fetch_authenticated_user_id(
        ctx.client,
        &ctx.target.organization_url,
        ctx.token,
        ctx.connection_type,
    )
    .await?
    {
        Ok(id) => id,
        Err(failure) => return Ok(Some(failure)),
    };
    if let Some(failure) = check_self_approval(ctx, &user_id, event, vote_value).await? {
        return Ok(Some(failure));
    }
    let encoded_id = utf8_percent_encode(&user_id, PATH_SEGMENT).to_string();
    submit_vote(ctx, &encoded_id, event, vote_value).await
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
            "submit-pull-request-review: pr_id={}, event='{}'",
            self.pull_request_id, self.event
        );

        if let Err(error) = (SubmitPrReviewParams {
            pull_request_id: self.pull_request_id.clone(),
            event: self.event.clone(),
            body: self.body.clone(),
            repository: self.repository.clone(),
        })
        .validate()
        {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;
        let config: SubmitPrReviewConfig = ctx.get_tool_config("submit-pull-request-review")?;
        validate_submit_pr_review_config(&config)?;
        if matches!(self.pull_request_id, PullRequestReference::Temporary(_))
            && !config.allow_temporary_ids
        {
            return Ok(ExecutionResult::failure(
                "submit-pull-request-review temporary IDs require allow-temporary-ids: true",
            ));
        }
        let legacy = legacy_policy(ctx, "submit-pull-request-review", "vote")?;
        if let Some(legacy) = &legacy {
            if self.body.is_some() {
                return Ok(ExecutionResult::failure(
                    "legacy update-pr vote does not permit a rationale comment",
                ));
            }
            if !legacy.allowed_votes.contains(&self.event) {
                return Ok(ExecutionResult::failure(
                    "event is not in the legacy allowed-votes list",
                ));
            }
        }
        debug!("Config: {:?}", config);

        // Validate event against allowed-events — REQUIRED.
        // An empty allowed-events list means the operator hasn't opted in, so reject.
        if config.allowed_events.is_empty() {
            return Ok(ExecutionResult::failure(
                "submit-pull-request-review requires 'allowed-events' to be configured in \
                 safe-outputs.submit-pull-request-review. This prevents agents from casting \
                 unrestricted review votes. Example:\n  safe-outputs:\n    submit-pull-request-review:\n      \
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

        let (pr_id, target) = match resolve_pr_target(
            &self.pull_request_id,
            self.repository.as_deref(),
            &config.allowed_repositories,
            ctx,
        )? {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        if let Some(legacy) = &legacy
            && let Err(failure) = resolve_pr_target(
                &self.pull_request_id,
                self.repository.as_deref(),
                &legacy.allowed_repositories,
                ctx,
            )?
        {
            return Ok(failure);
        }
        let repo_name = target.qualified_repository();

        // Map event to vote value
        let vote_value = event_to_vote(&self.event).context(format!(
            "Invalid event: '{}'. Must be one of: {}",
            self.event,
            VALID_EVENTS.join(", ")
        ))?;

        let client = reqwest::Client::new();
        let vote_ctx = PrVoteCtx {
            client: &client,
            target,
            pr_id,
            token,
            connection_type: ctx.write_connection_type,
        };
        if let Some(failure) = execute_review_vote(&vote_ctx, &self.event, vote_value).await? {
            return Ok(failure);
        }

        // If body is provided, also POST a comment thread with the review rationale
        if let Some(ref body) = self.body {
            let thread_id = match post_review_comment_thread(&vote_ctx, body).await? {
                Ok(id) => id,
                Err(failure) => return Ok(failure),
            };

            return Ok(ExecutionResult::success_with_data(
                format!(
                    "Review '{}' submitted on PR #{} with comment thread #{}",
                    self.event, self.pull_request_id, thread_id
                ),
                serde_json::json!({
                    "pull_request_id": pr_id,
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
                "pull_request_id": pr_id,
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
        assert_eq!(SubmitPrReviewResult::NAME, "submit-pull-request-review");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"pull_request_id": 42, "event": "approve"}"#;
        let params: SubmitPrReviewParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pull_request_id, PullRequestReference::Number(42));
        assert_eq!(params.event, "approve");
        assert!(params.body.is_none());
        assert!(params.repository.is_none());
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = SubmitPrReviewParams {
            pull_request_id: PullRequestReference::Number(42),
            event: "approve".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let result: SubmitPrReviewResult = params.try_into().unwrap();
        assert_eq!(result.name, "submit-pull-request-review");
        assert_eq!(result.pull_request_id, PullRequestReference::Number(42));
        assert_eq!(result.event, "approve");
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = SubmitPrReviewParams {
            pull_request_id: PullRequestReference::Number(0),
            event: "approve".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let err = <SubmitPrReviewResult as TryFrom<_>>::try_from(params).unwrap_err();
        assert!(
            err.to_string()
                .contains("pull_request_id must be a positive integer"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_invalid_event() {
        let params = SubmitPrReviewParams {
            pull_request_id: PullRequestReference::Number(1),
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
            pull_request_id: PullRequestReference::Number(1),
            event: "request-changes".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let err = <SubmitPrReviewResult as TryFrom<_>>::try_from(params).unwrap_err();
        assert!(
            err.to_string()
                .contains("body is required when event is 'request-changes'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_repository_pipeline_command() {
        let params = SubmitPrReviewParams {
            pull_request_id: PullRequestReference::Number(1),
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
            pull_request_id: PullRequestReference::Number(99),
            event: "request-changes".to_string(),
            body: Some("This needs significant rework before merging.".to_string()),
            repository: Some("self".to_string()),
        };
        let result: SubmitPrReviewResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"submit-pull-request-review""#));
        assert!(json.contains(r#""pull_request_id":99"#));
        assert!(json.contains(r#""event":"request-changes""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = SubmitPrReviewConfig::default();
        assert!(config.allowed_events.is_empty());
        assert!(config.allowed_repositories.is_empty());
        assert!(!config.allow_temporary_ids);
    }

    #[test]
    fn all_review_events_retain_exact_vote_values_and_rationale_rules() {
        for (event, vote) in [
            ("approve", 10),
            ("approve-with-suggestions", 5),
            ("request-changes", -5),
            ("comment", 0),
            ("wait-for-author", -5),
            ("reject", -10),
            ("reset", 0),
        ] {
            assert_eq!(event_to_vote(event), Some(vote));
            let params = SubmitPrReviewParams {
                pull_request_id: PullRequestReference::Number(u64::MAX),
                event: event.into(),
                body: None,
                repository: None,
            };
            assert_eq!(params.validate().is_ok(), event != "request-changes");
        }
    }

    #[tokio::test]
    async fn native_numeric_config_rejects_temporary_ids_before_network() {
        let server = wiremock::MockServer::start().await;
        let ctx = super::super::pr_common::tests::registered_context(
            &server.uri(),
            "submit-pull-request-review",
            serde_json::json!({"allowed-events": ["reset"]}),
        );
        let mut result: SubmitPrReviewResult = serde_json::from_value(serde_json::json!({
            "name": "submit-pull-request-review", "pull_request_id": "#aw_pr123", "event": "reset"
        }))
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("allow-temporary-ids"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn native_comment_event_still_writes_zero_vote_and_optional_thread() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/_apis/connectiondata"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"authenticatedUser": {"id": "actor"}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(
                "/Other/_apis/git/repositories/repo-id/pullRequests/4294967296/reviewers/actor",
            ))
            .and(body_json(serde_json::json!({"vote": 0})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path("/Other/_apis/git/repositories/repo-id/pullRequests/4294967296/threads"))
            .and(body_json(serde_json::json!({
                "comments": [{"parentCommentId": 0, "content": "Reviewed without objection.", "commentType": 1}],
                "status": 1
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": 12})))
            .expect(1).mount(&server).await;
        let ctx = super::super::pr_common::tests::registered_context(
            &server.uri(),
            "submit-pull-request-review",
            serde_json::json!({"allowed-events": ["comment"], "allow-temporary-ids": true}),
        );
        let mut result: SubmitPrReviewResult = serde_json::from_value(serde_json::json!({
            "name": "submit-pull-request-review", "pull_request_id": "#aw_pr123", "event": "comment",
            "body": "Reviewed without objection."
        }))
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(execution.data.unwrap()["thread_id"], 12);
    }

    #[tokio::test]
    async fn migrated_review_uses_exact_target_and_does_not_require_wait_rationale() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, header, method, path},
        };
        for (event, vote) in [("wait-for-author", -5), ("reject", -10), ("reset", 0)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/_apis/connectiondata"))
                .and(header("authorization", "Bearer token"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"authenticatedUser": {"id": "actor"}})),
                )
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("PUT"))
                .and(path(
                    "/Other/_apis/git/repositories/repo-id/pullRequests/4294967296/reviewers/actor",
                ))
                .and(header("authorization", "Bearer token"))
                .and(body_json(serde_json::json!({"vote": vote})))
                .respond_with(ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;
            let mut ctx = super::super::pr_common::tests::registered_context(
                &server.uri(),
                "submit-pull-request-review",
                serde_json::json!({
                    "allowed-events": [event], "allow-temporary-ids": true,
                    "legacy-update-pr": {"allowed-votes": [event]}
                }),
            );
            ctx.write_connection_type =
                Some(crate::compile::types::WriteConnectionType::AzureDevOps);
            let mut result: SubmitPrReviewResult = serde_json::from_value(serde_json::json!({
                "name": "submit-pull-request-review", "pull_request_id": "#aw_pr123", "event": event
            }))
            .unwrap();
            let execution = result.execute_sanitized(&ctx).await.unwrap();
            assert!(execution.success, "{}", execution.message);
            assert_eq!(execution.data.unwrap()["vote_value"], vote);
            assert_eq!(server.received_requests().await.unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn migrated_review_rejects_comments_and_legacy_vote_exclusions() {
        let server = wiremock::MockServer::start().await;
        for (body, allowed_votes) in [
            (Some("Not permitted as a new comment"), vec!["reset"]),
            (None, vec!["reject"]),
        ] {
            let ctx = super::super::pr_common::tests::registered_context(
                &server.uri(),
                "submit-pull-request-review",
                serde_json::json!({
                    "allowed-events": ["reset"], "allow-temporary-ids": true,
                    "legacy-update-pr": {"allowed-votes": allowed_votes}
                }),
            );
            let mut result: SubmitPrReviewResult = serde_json::from_value(serde_json::json!({
                "name": "submit-pull-request-review", "pull_request_id": "#aw_pr123", "event": "reset", "body": body
            })).unwrap();
            assert!(!result.execute_sanitized(&ctx).await.unwrap().success);
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn positive_review_rejects_self_approval_and_missing_creator() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        for creator in [serde_json::json!("ACTOR"), serde_json::Value::Null] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/_apis/connectiondata"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"authenticatedUser": {"id": "actor"}})),
                )
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(
                    "/Other/_apis/git/repositories/repo-id/pullRequests/4294967296",
                ))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"createdBy": {"id": creator}})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let ctx = super::super::pr_common::tests::registered_context(
                &server.uri(),
                "submit-pull-request-review",
                serde_json::json!({"allowed-events": ["approve"], "allow-temporary-ids": true}),
            );
            let mut result: SubmitPrReviewResult = serde_json::from_value(serde_json::json!({
                "name": "submit-pull-request-review", "pull_request_id": "#aw_pr123", "event": "approve"
            }))
            .unwrap();
            assert!(!result.execute_sanitized(&ctx).await.unwrap().success);
            assert_eq!(server.received_requests().await.unwrap().len(), 2);
        }
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
