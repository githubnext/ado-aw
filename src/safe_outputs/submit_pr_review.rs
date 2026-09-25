//! Submit PR review safe output tool

use ado_aw_derive::SanitizeConfig;
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ToolResult;
use super::pr_common::{PullRequestReference, legacy_policy, validate_reference};
use super::pr_inline::PrInlineComment;
use super::pr_mutations::UpdatePrContext;
use super::{PATH_SEGMENT, authenticate_ado_request};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config, sanitize_markdown};
use crate::secure::{CommitSha, Identifier};
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
        "reset" => Some(0),
        _ => None,
    }
}

/// Parameters for submitting a pull request review
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitPrReviewParams {
    /// Positive PR ID, or a same-run temporary ID when allow-temporary-ids is enabled.
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,

    /// Review decision: approve, approve-with-suggestions, request-changes, comment,
    /// wait-for-author, reject, or reset.
    pub event: String,

    /// Review rationale in markdown. Required for "request-changes", optional otherwise.
    /// Must be at least 10 characters when provided.
    #[serde(default)]
    pub body: Option<String>,
    /// Inline findings in this review only; requires configured max-comments.
    #[serde(default)]
    pub comments: Vec<PrInlineComment>,
    /// Exact reviewed source commit; required when comments is nonempty.
    #[serde(default)]
    pub expected_head_sha: Option<CommitSha>,

    /// Repository alias: "self" for pipeline repo, or an alias from the checkout list.
    /// Defaults to "self" if omitted.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for SubmitPrReviewParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(reference) = &self.pull_request_id {
            validate_reference(reference)?;
        }
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
        if self.event == "comment" {
            ensure!(
                self.body
                    .as_deref()
                    .is_some_and(|body| !body.trim().is_empty())
                    || !self.comments.is_empty(),
                "body or inline comments are required for a non-voting comment review"
            );
        }
        if let Some(ref body) = self.body {
            ensure!(body.len() >= 10, "body must be at least 10 characters");
            super::pr_comments::validate_body(body)?;
        }
        ensure!(
            self.comments.len() <= 100,
            "A review may contain at most 100 inline comments"
        );
        ensure!(
            self.comments.is_empty() || self.expected_head_sha.is_some(),
            "expected_head_sha is required for inline review comments"
        );
        for comment in &self.comments {
            comment.validate()?;
        }
        Ok(())
    }
}

tool_result! {
    name = "submit-pull-request-review",
    write = true,
    params = SubmitPrReviewParams,
    /// Result of submitting a pull request review
    #[serde(deny_unknown_fields)]
    pub struct SubmitPrReviewResult {
        #[serde(default)]
        pull_request_id: Option<PullRequestReference>,
        event: String,
        body: Option<String>,
        #[serde(default)]
        comments: Vec<PrInlineComment>,
        #[serde(default)]
        expected_head_sha: Option<CommitSha>,
        repository: Option<String>,
    }
}

impl SanitizeContent for SubmitPrReviewResult {
    fn sanitize_content_fields(&mut self) {
        self.event = sanitize_config(&self.event);
        self.body = self.body.as_deref().map(sanitize_markdown);
        for comment in &mut self.comments {
            comment.content = sanitize_markdown(&comment.content);
        }
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
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitPrReviewConfig {
    #[serde(default, rename = "max-comments")]
    #[sanitize_config(skip)]
    pub max_comments: usize,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

fn default_max_superseded() -> usize {
    20
}

impl Default for SubmitPrReviewConfig {
    fn default() -> Self {
        Self {
            max_comments: 0,
            supersede_older_comments: false,
            comment_key: super::pr_comments::default_comment_key(),
            max_superseded_comments: default_max_superseded(),
            target: Default::default(),
            target_repo: None,
            required_labels: Vec::new(),
            required_title_prefix: None,
            allow_temporary_ids: false,
            allowed_events: Vec::new(),
            allowed_repositories: Vec::new(),
            max: None,
        }
    }
}

pub(crate) fn validate_submit_pr_review_config(
    config: &SubmitPrReviewConfig,
) -> anyhow::Result<()> {
    ensure!(
        config.max_comments <= 100,
        "max-comments must be between 0 and 100"
    );
    ensure!(
        config.max_superseded_comments > 0 && config.max_superseded_comments <= 100,
        "max-superseded-comments must be between 1 and 100"
    );
    ensure!(
        config.comment_key.len() <= 100,
        "comment-key must fit 100 bytes"
    );
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
    inline: Option<&serde_json::Value>,
    owner: Option<&super::pr_comments::Owner>,
    execution: &ExecutionContext,
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
    let mut payload = serde_json::json!({
        "comments": [{"parentCommentId": 0, "content": body, "commentType": 1}],
        "status": 1
    });
    if let Some(inline) = inline {
        payload["threadContext"] = inline["threadContext"].clone();
        payload["pullRequestThreadContext"] = inline["pullRequestThreadContext"].clone();
    }
    super::pr_comments::stamp(&mut payload, owner, execution, body)?;
    let response =
        authenticate_ado_request(ctx.client.post(&thread_url), ctx.token, ctx.connection_type)
            .header("Content-Type", "application/json")
            .json(&payload)
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
            "Failed to post review comment on PR #{} (HTTP {}): {}",
            pull_request_id, status, error_body
        ))));
    }

    let thread_resp: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse comment thread response")?;

    let thread_id = thread_resp
        .get("id")
        .and_then(|v| v.as_i64())
        .filter(|id| *id > 0)
        .context("Comment response missing a positive thread ID; delivery is uncertain")?;
    info!(
        "Review comment thread #{} posted on PR #{}",
        thread_id, pull_request_id
    );
    Ok(Ok(thread_id))
}

/// Validate the authenticated actor before any part of a voting review is written.
async fn prepare_review_actor(
    ctx: &UpdatePrContext<'_>,
    event: &str,
    vote_value: i32,
) -> anyhow::Result<Result<String, ExecutionResult>> {
    let user_id = match fetch_authenticated_user_id(
        ctx.client,
        &ctx.target.organization_url,
        ctx.token,
        ctx.connection_type,
    )
    .await?
    {
        Ok(id) => id,
        Err(failure) => return Ok(Err(failure)),
    };
    if let Some(failure) = check_self_approval(ctx, &user_id, event, vote_value).await? {
        return Ok(Err(failure));
    }
    Ok(Ok(user_id))
}

#[async_trait::async_trait]
impl Executor for SubmitPrReviewResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "submit '{}' review on {}",
            self.event,
            super::pr_common::describe_pr_reference(self.pull_request_id.as_ref())
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Submitting review on {:?} — event: {}",
            self.pull_request_id, self.event
        );
        debug!(
            "submit-pull-request-review: pr_id={:?}, event='{}'",
            self.pull_request_id, self.event
        );

        if let Err(error) = (SubmitPrReviewParams {
            pull_request_id: self.pull_request_id.clone(),
            event: self.event.clone(),
            body: self.body.clone(),
            comments: self.comments.clone(),
            expected_head_sha: self.expected_head_sha.clone(),
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
        ensure!(
            self.comments.len() <= config.max_comments,
            "Inline review comments require sufficient max-comments (default 0)"
        );
        ensure!(
            !config.supersede_older_comments || self.body.is_some() || !self.comments.is_empty(),
            "Supersession requires replacement review content, not a vote-only proposal"
        );
        if matches!(
            self.pull_request_id,
            Some(PullRequestReference::Temporary(_))
        ) && !config.allow_temporary_ids
        {
            return Ok(ExecutionResult::failure(
                "submit-pull-request-review temporary IDs require allow-temporary-ids: true",
            ));
        }
        let legacy = legacy_policy(ctx, "submit-pull-request-review", "vote")?;
        if let Some(legacy) = &legacy {
            if self.body.is_some() || !self.comments.is_empty() {
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

        let (pr_id, target) = match super::pr_common::resolve_configured_pr_target(
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
        if let Some(legacy) = &legacy
            && let Err(failure) = super::pr_common::validate_pr_repository_policy(
                &target,
                &legacy.allowed_repositories,
                ctx,
            )
        {
            return Ok(failure);
        }
        let repo_name = target.qualified_repository();

        let vote_value = event_to_vote(&self.event);

        let client = super::pr_comments::client()?;
        let vote_ctx = PrVoteCtx {
            client: &client,
            target,
            pr_id,
            token,
            connection_type: ctx.write_connection_type,
        };
        let inline_contexts = if self.comments.is_empty() {
            Vec::new()
        } else {
            super::pr_inline::prepare(
                &vote_ctx,
                self.expected_head_sha
                    .as_ref()
                    .context("Inline review requires expected_head_sha")?,
                &self.comments,
            )
            .await?
        };
        let owner = super::pr_comments::owner(ctx, "review", &config.comment_key)?;
        let actor = if let Some(vote) = vote_value {
            match prepare_review_actor(&vote_ctx, &self.event, vote).await? {
                Ok(actor) => Some(actor),
                Err(failure) => return Ok(failure),
            }
        } else {
            None
        };
        let supersession = if config.supersede_older_comments {
            let owner = owner
                .as_ref()
                .context("Supersession requires a complete trusted pipeline identity")?;
            let author = super::pr_comments::actor(&vote_ctx).await?;
            let (candidates, skipped) = super::pr_comments::older_threads(
                &vote_ctx,
                owner,
                &author,
                ctx.build_id.context("Supersession requires build ID")?,
                config.max_superseded_comments,
            )
            .await?;
            Some((author, candidates, skipped))
        } else {
            None
        };
        let mut data = serde_json::json!({
            "pull_request_id": pr_id, "event": self.event, "repository": repo_name,
            "vote_value": vote_value, "vote_changed": false,
            "vote_status": if vote_value.is_some() { "not-attempted" } else { "not-requested" },
            "comment_status": "not-requested",
            "inline_comments": [],
        });
        for (index, (comment, context)) in self.comments.iter().zip(&inline_contexts).enumerate() {
            if let Err(error) = super::pr_inline::verify_head(
                &vote_ctx,
                self.expected_head_sha
                    .as_ref()
                    .context("Inline review requires expected_head_sha")?,
            )
            .await
            {
                return Ok(ExecutionResult::failure_with_data(
                    format!("Review stopped before further writes: {error:#}"),
                    data,
                ));
            }
            match post_review_comment_thread(
                &vote_ctx,
                &comment.content,
                Some(context),
                owner.as_ref(),
                ctx,
            )
            .await
            {
                Ok(Ok(id)) => data["inline_comments"]
                    .as_array_mut()
                    .context("Invalid internal review result")?
                    .push(serde_json::json!({"index":index,"thread_id":id,"status":"posted"})),
                outcome => {
                    let (status, message) = match outcome {
                        Ok(Err(failure)) => ("failed", failure.message),
                        Err(error) => ("uncertain", format!("{error:#}")),
                        Ok(Ok(_)) => unreachable!("successful branch handled above"),
                    };
                    data["inline_comments"]
                        .as_array_mut()
                        .context("Invalid internal review result")?
                        .push(serde_json::json!({"index":index,"status":status}));
                    return Ok(ExecutionResult::failure_with_data(
                        format!("Inline review stopped: {message}; vote not attempted"),
                        data,
                    ));
                }
            }
        }
        if let Some(body) = &self.body {
            if let Some(head) = &self.expected_head_sha
                && let Err(error) = super::pr_inline::verify_head(&vote_ctx, head).await
            {
                return Ok(ExecutionResult::failure_with_data(
                    format!("Review head changed before summary: {error:#}"),
                    data,
                ));
            }
            data["comment_status"] = serde_json::json!("uncertain");
            let thread_id = match post_review_comment_thread(
                &vote_ctx,
                body,
                None,
                owner.as_ref(),
                ctx,
            )
            .await
            {
                Ok(Ok(id)) => id,
                Ok(Err(failure)) => {
                    data["comment_status"] = serde_json::json!("failed");
                    return Ok(ExecutionResult::failure_with_data(failure.message, data));
                }
                Err(error) => {
                    return Ok(ExecutionResult::failure_with_data(
                        format!(
                            "Review comment delivery is uncertain; vote was not attempted: {error:#}"
                        ),
                        data,
                    ));
                }
            };
            data["thread_id"] = serde_json::json!(thread_id);
            data["comment_status"] = serde_json::json!("posted");
        }
        if let (Some(vote), Some(actor)) = (vote_value, actor) {
            if let Some(head) = &self.expected_head_sha
                && let Err(error) = super::pr_inline::verify_head(&vote_ctx, head).await
            {
                return Ok(ExecutionResult::failure_with_data(
                    format!("Review head changed before vote: {error:#}"),
                    data,
                ));
            }
            data["reviewer_id"] = serde_json::json!(actor);
            let encoded_actor = utf8_percent_encode(&actor, PATH_SEGMENT).to_string();
            match submit_vote(&vote_ctx, &encoded_actor, &self.event, vote).await {
                Ok(None) => {
                    data["vote_status"] = serde_json::json!("applied");
                    data["vote_changed"] = serde_json::json!(true);
                }
                Ok(Some(failure)) => {
                    data["vote_status"] = serde_json::json!("failed");
                    return Ok(ExecutionResult::failure_with_data(failure.message, data));
                }
                Err(error) => {
                    data["vote_status"] = serde_json::json!("uncertain");
                    return Ok(ExecutionResult::failure_with_data(
                        format!("Review vote delivery is uncertain: {error:#}"),
                        data,
                    ));
                }
            }
        }

        if let Some((author, candidates, skipped)) = supersession {
            let replacement = data["thread_id"]
                .as_i64()
                .or_else(|| data["inline_comments"][0]["thread_id"].as_i64())
                .context("Supersession requires a confirmed replacement thread")?;
            let result = super::pr_comments::supersede(
                &vote_ctx,
                owner.as_ref().context("Supersession requires ownership")?,
                &author,
                &candidates,
                i32::try_from(replacement)?,
                skipped,
            )
            .await;
            match result {
                Ok(details) => {
                    let failed = details["failures"]
                        .as_u64()
                        .context("Invalid supersession outcome")?
                        > 0;
                    data["supersession"] = details;
                    if failed {
                        return Ok(ExecutionResult::warning_with_data(
                            "Review completed, but older reports could not all be superseded",
                            data,
                        ));
                    }
                }
                Err(error) => {
                    data["supersession_error"] = serde_json::json!(format!("{error:#}"));
                    return Ok(ExecutionResult::warning_with_data(
                        "Review completed, but supersession failed",
                        data,
                    ));
                }
            }
        }
        Ok(ExecutionResult::success_with_data(
            format!("Review '{}' submitted on PR #{}", self.event, pr_id),
            data,
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
        assert_eq!(
            params.pull_request_id,
            Some(PullRequestReference::Number(42))
        );
        assert_eq!(params.event, "approve");
        assert!(params.body.is_none());
        assert!(params.repository.is_none());
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = SubmitPrReviewParams {
            comments: Vec::new(),
            expected_head_sha: None,
            pull_request_id: Some(PullRequestReference::Number(42)),
            event: "approve".to_string(),
            body: None,
            repository: Some("self".to_string()),
        };
        let result: SubmitPrReviewResult = params.try_into().unwrap();
        assert_eq!(result.name, "submit-pull-request-review");
        assert_eq!(
            result.pull_request_id,
            Some(PullRequestReference::Number(42))
        );
        assert_eq!(result.event, "approve");
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = SubmitPrReviewParams {
            comments: Vec::new(),
            expected_head_sha: None,
            pull_request_id: Some(PullRequestReference::Number(0)),
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
            comments: Vec::new(),
            expected_head_sha: None,
            pull_request_id: Some(PullRequestReference::Number(1)),
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
            comments: Vec::new(),
            expected_head_sha: None,
            pull_request_id: Some(PullRequestReference::Number(1)),
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
            comments: Vec::new(),
            expected_head_sha: None,
            pull_request_id: Some(PullRequestReference::Number(1)),
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
            comments: Vec::new(),
            expected_head_sha: None,
            pull_request_id: Some(PullRequestReference::Number(99)),
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
            ("wait-for-author", -5),
            ("reject", -10),
            ("reset", 0),
        ] {
            assert_eq!(event_to_vote(event), Some(vote));
            let params = SubmitPrReviewParams {
                comments: Vec::new(),
                expected_head_sha: None,
                pull_request_id: Some(PullRequestReference::Number(u64::MAX)),
                event: event.into(),
                body: None,
                repository: None,
            };
            assert_eq!(params.validate().is_ok(), event != "request-changes");
        }
        assert_eq!(event_to_vote("comment"), None);
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
    async fn native_comment_event_never_looks_up_actor_or_writes_a_vote() {
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
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(
                "/Other/_apis/git/repositories/repo-id/pullRequests/4294967296/reviewers/actor",
            ))
            .and(body_json(serde_json::json!({"vote": 0})))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
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
        let data = execution.data.unwrap();
        assert_eq!(data["thread_id"], 12);
        assert_eq!(data["vote_changed"], false);
        assert_eq!(data["vote_status"], "not-requested");
        assert!(data["vote_value"].is_null());
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
    fn comment_requires_content_and_preserves_markdown() {
        for body in [None, Some(""), Some("           ")] {
            let params = SubmitPrReviewParams {
                comments: Vec::new(),
                expected_head_sha: None,
                pull_request_id: Some(PullRequestReference::Number(1)),
                event: "comment".into(),
                body: body.map(str::to_string),
                repository: None,
            };
            assert!(params.validate().is_err());
        }
        let mut result: SubmitPrReviewResult = serde_json::from_value(serde_json::json!({
            "name":"submit-pull-request-review", "pull_request_id":1, "event":"comment",
            "body":"Check `Vec<T>` and this code:\n```rust\nlet x = a < b;\n```",
        }))
        .unwrap();
        result.sanitize_content_fields();
        assert_eq!(
            result.body.as_deref(),
            Some("Check `Vec<T>` and this code:\n```rust\nlet x = a < b;\n```")
        );
    }

    #[tokio::test]
    async fn malformed_comment_success_is_uncertain_and_cannot_cast_vote() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/_apis/connectiondata"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"authenticatedUser":{"id":"actor"}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = super::super::pr_common::tests::registered_context(
            &server.uri(),
            SubmitPrReviewResult::NAME,
            serde_json::json!({"allowed-events":["request-changes"],"allow-temporary-ids":true}),
        );
        let mut result: SubmitPrReviewResult = serde_json::from_value(serde_json::json!({
            "name":"submit-pull-request-review","pull_request_id":"#aw_pr123",
            "event":"request-changes","body":"Please correct this behavior."
        }))
        .unwrap();
        let result = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!result.success);
        let data = result.data.unwrap();
        assert_eq!(data["comment_status"], "uncertain");
        assert_eq!(data["vote_status"], "not-attempted");
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.method.as_str() != "PUT")
        );
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

    #[tokio::test]
    async fn review_batch_preflights_all_findings_and_never_votes_after_partial_failure() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        for scenario in [
            "success",
            "invalid-last",
            "not-enabled",
            "head-moved",
            "vote-failed",
        ] {
            let server = MockServer::start().await;
            let posts = Arc::new(AtomicUsize::new(0));
            let observed = posts.clone();
            let move_head = scenario == "head-moved";
            Mock::given(method("GET")).and(path("/P/_apis/git/repositories/repo/pullRequests/42/iterations"))
                .respond_with(move |_:&wiremock::Request|{
                    let head=if move_head&&observed.load(Ordering::SeqCst)>0 {"c"}else{"a"};
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"value":[{
                        "id":1,"sourceRefCommit":{"commitId":head.repeat(40)},"commonRefCommit":{"commitId":"b".repeat(40)}
                    }]}))
                }).mount(&server).await;
            Mock::given(method("GET"))
                .and(path(
                    "/P/_apis/git/repositories/repo/pullRequests/42/iterations/1/changes",
                ))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"changeEntries":[{
                        "changeTrackingId":1,"changeType":"edit","item":{"path":"/src.rs"}
                    }]}),
                ))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/P/_apis/git/repositories/repo/items"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"content":"first\nsecond\n"})),
                )
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/_apis/connectiondata"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"authenticatedUser":{"id":"actor"}})),
                )
                .mount(&server)
                .await;
            let posted = posts.clone();
            Mock::given(method("POST"))
                .respond_with(move |_: &wiremock::Request| {
                    let id = posted.fetch_add(1, Ordering::SeqCst) + 10;
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":id}))
                })
                .mount(&server)
                .await;
            Mock::given(method("PUT"))
                .respond_with(ResponseTemplate::new(if scenario == "vote-failed" {
                    500
                } else {
                    200
                }))
                .mount(&server)
                .await;
            let mut ctx = ExecutionContext {
                ado_org_url: Some(server.uri()),
                ado_organization: Some("org".into()),
                ado_project: Some("P".into()),
                repository_name: Some("repo".into()),
                access_token: Some("token".into()),
                ..Default::default()
            };
            ctx.tool_configs.insert(SubmitPrReviewResult::NAME.into(),serde_json::json!({
                "target":"*","allowed-events":["request-changes"],"max-comments":if scenario=="not-enabled"{0}else{2}
            }));
            let result=crate::execute::execute_safe_output(&serde_json::json!({
                "name":"submit-pull-request-review","pull_request_id":42,"event":"request-changes",
                "body":"Review summary requiring changes.","expected_head_sha":"a".repeat(40),
                "comments":[
                    {"file_path":"src.rs","line":1,"content":"First independent finding."},
                    {"file_path":"src.rs","line":if scenario=="invalid-last"{3}else{2},"content":"Second independent finding."}
                ]
            }),&ctx).await;
            let requests = server.received_requests().await.unwrap();
            let writes = requests
                .iter()
                .filter(|request| request.method.as_str() != "GET")
                .collect::<Vec<_>>();
            match scenario {
                "not-enabled" | "invalid-last" => {
                    assert!(result.is_err(), "{scenario}");
                    assert!(writes.is_empty(), "{scenario}");
                    if scenario == "not-enabled" {
                        assert!(requests.is_empty());
                    }
                }
                "head-moved" => {
                    let result = result.unwrap().1;
                    assert!(!result.success);
                    assert_eq!(writes.len(), 1);
                    assert_eq!(
                        result.data.unwrap()["inline_comments"][0]["status"],
                        "posted"
                    );
                }
                _ => {
                    let result = result.unwrap().1;
                    assert_eq!(result.success, scenario == "success");
                    assert_eq!(
                        writes
                            .iter()
                            .map(|request| request.method.as_str())
                            .collect::<Vec<_>>(),
                        vec!["POST", "POST", "POST", "PUT"]
                    );
                    let first: serde_json::Value = serde_json::from_slice(&writes[0].body).unwrap();
                    assert_eq!(first["threadContext"]["filePath"], "/src.rs");
                    assert_eq!(first["pullRequestThreadContext"]["changeTrackingId"], 1);
                    let data = result.data.unwrap();
                    assert_eq!(data["inline_comments"].as_array().unwrap().len(), 2);
                    assert_eq!(data["thread_id"], 12);
                    assert_eq!(
                        data["vote_status"],
                        if scenario == "success" {
                            "applied"
                        } else {
                            "failed"
                        }
                    );
                }
            }
        }
    }
}
