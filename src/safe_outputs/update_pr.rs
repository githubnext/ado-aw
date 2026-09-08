//! Update pull request safe output tool

use ado_aw_derive::SanitizeConfig;
use log::{debug, info, warn};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

use super::result::AdoRepositoryTarget;
use super::{PATH_SEGMENT, canonical_repository_alias, resolve_repository_write_target};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::secure::PullRequestTemporaryId;
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use anyhow::{Context, ensure};

/// Valid operation names for update-pr
const VALID_OPERATIONS: &[&str] = &[
    "add-reviewers",
    "add-labels",
    "set-auto-complete",
    "vote",
    "update-description",
];

/// Valid vote values
const VALID_VOTES: &[&str] = &[
    "approve",
    "approve-with-suggestions",
    "wait-for-author",
    "reject",
    "reset",
];

/// Valid merge strategy values accepted by ADO's completionOptions.mergeStrategy
const VALID_MERGE_STRATEGIES: &[&str] = &["squash", "noFastForward", "rebase", "rebaseMerge"];
const DEFAULT_MAX_REVIEWERS: usize = 3;
const MAX_REVIEWER_LEN: usize = 256;

/// Positive Azure DevOps pull-request ID or a same-run temporary ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum PullRequestReference {
    Number(u64),
    Temporary(PullRequestTemporaryId),
}

impl fmt::Display for PullRequestReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(id) => write!(formatter, "{id}"),
            Self::Temporary(temporary_id) => formatter.write_str(&temporary_id.canonical()),
        }
    }
}

impl_temporary_reference_deserialize!(
    PullRequestReference,
    PullRequestTemporaryId,
    expecting = "a positive pull-request ID or #aw_ temporary ID",
    negative = "pull_request_id must be positive",
    quoted_out_of_range = "quoted pull_request_id is outside the u64 range",
);

/// Map a vote string to its ADO numeric value
fn vote_to_ado_value(vote: &str) -> Option<i32> {
    match vote {
        "approve" => Some(10),
        "approve-with-suggestions" => Some(5),
        "wait-for-author" => Some(-5),
        "reject" => Some(-10),
        "reset" => Some(0),
        _ => None,
    }
}

/// Parameters for updating a pull request
#[derive(Deserialize, JsonSchema)]
pub struct UpdatePrParams {
    /// Positive pull request ID or a temporary ID from create-pull-request.
    pub pull_request_id: PullRequestReference,

    /// Repository alias: "self" for the pipeline repo, or an alias from the checkout list
    #[serde(default)]
    pub repository: Option<String>,

    /// Operation to perform: "add-reviewers", "add-labels", "set-auto-complete", "vote", or "update-description"
    pub operation: String,

    /// Reviewer emails (required for add-reviewers operation)
    pub reviewers: Option<Vec<String>>,

    /// Label names (required for add-labels operation)
    pub labels: Option<Vec<String>>,

    /// Vote value: "approve", "approve-with-suggestions", "wait-for-author", "reject", or "reset"
    pub vote: Option<String>,

    /// New PR description in markdown (required for update-description, must be >= 10 chars)
    pub description: Option<String>,
}

impl Validate for UpdatePrParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let PullRequestReference::Number(id) = self.pull_request_id {
            ensure!(id > 0, "pull_request_id must be positive");
        }
        if let Some(repository) = &self.repository {
            reject_pipeline_injection(repository, "repository")?;
        }
        ensure!(
            VALID_OPERATIONS.contains(&self.operation.as_str()),
            "operation must be one of: {}",
            VALID_OPERATIONS.join(", ")
        );

        match self.operation.as_str() {
            "add-reviewers" => {
                let reviewers = self
                    .reviewers
                    .as_ref()
                    .context("reviewers must be provided for add-reviewers operation")?;
                ensure!(
                    !reviewers.is_empty(),
                    "reviewers list must not be empty for add-reviewers operation"
                );
                ensure!(
                    reviewers.len() <= 100,
                    "reviewers list must contain at most 100 entries"
                );
                for reviewer in reviewers {
                    let reviewer = reviewer.trim();
                    ensure!(!reviewer.is_empty(), "reviewer must not be empty");
                    ensure!(
                        reviewer.len() <= MAX_REVIEWER_LEN,
                        "reviewer must be {MAX_REVIEWER_LEN} characters or fewer"
                    );
                    reject_pipeline_injection(reviewer, "update-pr.reviewer")?;
                }
            }
            "add-labels" => {
                let labels = self
                    .labels
                    .as_ref()
                    .context("labels must be provided for add-labels operation")?;
                ensure!(
                    !labels.is_empty(),
                    "labels list must not be empty for add-labels operation"
                );
            }
            "vote" => {
                let vote = self
                    .vote
                    .as_ref()
                    .context("vote must be provided for vote operation")?;
                ensure!(
                    VALID_VOTES.contains(&vote.as_str()),
                    "vote must be one of: {}",
                    VALID_VOTES.join(", ")
                );
            }
            "update-description" => {
                let desc = self
                    .description
                    .as_ref()
                    .context("description must be provided for update-description operation")?;
                ensure!(
                    desc.len() >= 10,
                    "description must be at least 10 characters"
                );
            }
            _ => {} // set-auto-complete has no extra required fields
        }
        Ok(())
    }
}

tool_result! {
    name = "update-pr",
    write = true,
    params = UpdatePrParams,
    /// Result of updating a pull request
    pub struct UpdatePrResult {
        pull_request_id: PullRequestReference,
        repository: Option<String>,
        operation: String,
        reviewers: Option<Vec<String>>,
        labels: Option<Vec<String>>,
        vote: Option<String>,
        description: Option<String>,
    }
}

impl SanitizeContent for UpdatePrResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
        self.operation = sanitize_config(&self.operation);
        self.reviewers = self
            .reviewers
            .as_ref()
            .map(|rs| rs.iter().map(|r| sanitize_config(r)).collect());
        self.labels = self
            .labels
            .as_ref()
            .map(|ls| ls.iter().map(|l| sanitize_config(l)).collect());
        self.vote = self.vote.as_deref().map(sanitize_config);
        self.description = self.description.as_deref().map(sanitize_text);
    }
}

/// Configuration for the update-pr tool (specified in front matter)
///
/// **Allow-list semantics note:** `allowed-operations` and `allowed-repositories` use
/// permissive defaults (empty = all allowed), while `allowed-votes` uses a secure default
/// (empty = all rejected). This asymmetry is intentional — vote operations can auto-approve
/// PRs, so they require explicit opt-in to prevent accidental privilege escalation.
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   update-pr:
///     allowed-operations:
///       - add-reviewers
///       - set-auto-complete
///     allowed-repositories:
///       - self
///     allowed-votes:
///       - approve
///       - reject
/// ```
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct UpdatePrConfig {
    /// Which operations are permitted. Empty list means all operations are allowed.
    #[serde(default, rename = "allowed-operations")]
    pub allowed_operations: Vec<String>,

    /// Which repositories the agent may target. Empty list means all allowed repos.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,

    /// Which vote values are permitted. REQUIRED for vote operation —
    /// empty list rejects all votes to prevent accidental auto-approve.
    #[serde(default, rename = "allowed-votes")]
    pub allowed_votes: Vec<String>,

    /// Case-insensitive exact allowlist for model-selected reviewers.
    /// Empty rejects all reviewers; a literal "*" allows any valid reviewer.
    #[serde(default, rename = "allowed-reviewers")]
    pub allowed_reviewers: Vec<String>,

    /// Maximum reviewers accepted by one add-reviewers operation.
    #[serde(default = "default_max_reviewers", rename = "max-reviewers")]
    #[sanitize_config(skip)]
    pub max_reviewers: usize,

    /// Whether to delete the source branch after merge (for set-auto-complete, default: true)
    #[serde(default = "default_true", rename = "delete-source-branch")]
    pub delete_source_branch: bool,

    /// Merge strategy for auto-complete: "squash", "noFastForward", "rebase", "rebaseMerge" (default: "squash")
    #[serde(default = "default_merge_strategy", rename = "merge-strategy")]
    pub merge_strategy: String,
}

fn default_true() -> bool {
    true
}

fn default_merge_strategy() -> String {
    "squash".to_string()
}

fn default_max_reviewers() -> usize {
    DEFAULT_MAX_REVIEWERS
}

impl Default for UpdatePrConfig {
    fn default() -> Self {
        Self {
            allowed_operations: Vec::new(),
            allowed_repositories: Vec::new(),
            allowed_votes: Vec::new(),
            allowed_reviewers: Vec::new(),
            max_reviewers: default_max_reviewers(),
            delete_source_branch: true,
            merge_strategy: "squash".to_string(),
        }
    }
}

struct UpdatePrContext<'a> {
    client: &'a reqwest::Client,
    target: AdoRepositoryTarget,
    pr_id: u64,
    token: &'a str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
}

impl UpdatePrContext<'_> {
    fn repository_api_base(&self) -> String {
        format!(
            "{}/{}/_apis/git/repositories/{}",
            self.target.organization_url,
            utf8_percent_encode(&self.target.project, PATH_SEGMENT),
            utf8_percent_encode(self.target.repository_locator(), PATH_SEGMENT),
        )
    }
}

fn repository_is_allowed(config: &UpdatePrConfig, alias: &str) -> bool {
    config.allowed_repositories.is_empty()
        || config
            .allowed_repositories
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(alias))
}

fn resolve_update_pr_target(
    reference: &PullRequestReference,
    requested_repository: Option<&str>,
    config: &UpdatePrConfig,
    ctx: &ExecutionContext,
) -> anyhow::Result<Result<(u64, AdoRepositoryTarget), ExecutionResult>> {
    match reference {
        PullRequestReference::Number(id) => {
            if *id == 0 {
                return Ok(Err(ExecutionResult::failure(
                    "pull_request_id must be positive",
                )));
            }
            let selector = requested_repository.unwrap_or("self");
            let Some(alias) = canonical_repository_alias(selector, ctx) else {
                return Ok(Err(ExecutionResult::failure(format!(
                    "Repository '{}' is not in the allowed repository list",
                    crate::sanitize::neutralize_pipeline_commands(selector)
                ))));
            };
            if !repository_is_allowed(config, &alias) {
                return Ok(Err(ExecutionResult::failure(format!(
                    "Repository '{}' is not in the allowed-repositories list: [{}]",
                    alias,
                    config.allowed_repositories.join(", ")
                ))));
            }
            let target = match resolve_repository_write_target(Some(&alias), ctx) {
                Ok(target) => target,
                Err(error) => return Ok(Err(error)),
            };
            Ok(Ok((*id, target)))
        }
        PullRequestReference::Temporary(temporary_id) => {
            let Some(resolved) = ctx.resolve_pull_request(temporary_id)? else {
                return Ok(Err(ExecutionResult::failure(format!(
                    "temporary pull-request ID '{}' has not been resolved; \
                     create-pull-request must succeed earlier in the same SafeOutputs job",
                    temporary_id.canonical()
                ))));
            };
            if let Some(selector) = requested_repository {
                let Some(alias) = canonical_repository_alias(selector, ctx) else {
                    return Ok(Err(ExecutionResult::failure(format!(
                        "Repository '{}' is not in the allowed repository list",
                        crate::sanitize::neutralize_pipeline_commands(selector)
                    ))));
                };
                if !alias.eq_ignore_ascii_case(&resolved.target.alias) {
                    return Ok(Err(ExecutionResult::failure(format!(
                        "temporary pull-request ID '{}' resolved to repository '{}', which does \
                         not match requested repository '{}'",
                        temporary_id.canonical(),
                        resolved.target.alias,
                        crate::sanitize::neutralize_pipeline_commands(selector)
                    ))));
                }
            }
            if !repository_is_allowed(config, &resolved.target.alias) {
                return Ok(Err(ExecutionResult::failure(format!(
                    "Repository '{}' is not in the allowed-repositories list: [{}]",
                    resolved.target.alias,
                    config.allowed_repositories.join(", ")
                ))));
            }
            Ok(Ok((resolved.id, resolved.target)))
        }
    }
}

#[async_trait::async_trait]
impl Executor for UpdatePrResult {
    fn dry_run_summary(&self) -> String {
        format!("{} on PR #{}", self.operation, self.pull_request_id)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Updating PR #{} — operation: {}",
            self.pull_request_id, self.operation
        );
        debug!(
            "update-pr: pr_id={}, operation='{}'",
            self.pull_request_id, self.operation
        );

        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;
        let config: UpdatePrConfig = ctx.get_tool_config("update-pr")?;
        debug!("Config: {:?}", config);

        // Validate operation against allowed-operations
        if !config.allowed_operations.is_empty()
            && !config.allowed_operations.contains(&self.operation)
        {
            return Ok(ExecutionResult::failure(format!(
                "Operation '{}' is not in the allowed-operations list: [{}]",
                self.operation,
                config.allowed_operations.join(", ")
            )));
        }

        let (pr_id, target) = match resolve_update_pr_target(
            &self.pull_request_id,
            self.repository.as_deref(),
            &config,
            ctx,
        )? {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        debug!("Resolved PR target: {} #{}", target.display_name(), pr_id);

        let client = reqwest::Client::new();
        let operation_ctx = UpdatePrContext {
            client: &client,
            target,
            pr_id,
            token,
            connection_type: ctx.write_connection_type,
        };

        match self.operation.as_str() {
            "set-auto-complete" => {
                self.execute_set_auto_complete(&operation_ctx, &config)
                    .await
            }
            "vote" => self.execute_vote(&operation_ctx, &config).await,
            "add-reviewers" => self.execute_add_reviewers(&operation_ctx, &config).await,
            "add-labels" => self.execute_add_labels(&operation_ctx).await,
            "update-description" => self.execute_update_description(&operation_ctx).await,
            _ => Ok(ExecutionResult::failure(format!(
                "Unknown operation: {}",
                self.operation
            ))),
        }
    }
}

/// Outcome of a single reviewer resolution + add attempt.
enum ReviewerAddResult {
    Added,
    Failed(String),
}

fn reviewer_execution_result(
    pr_id: u64,
    added: Vec<String>,
    failed: Vec<String>,
) -> ExecutionResult {
    let mut message = format!("Added {} reviewer(s) to PR #{}", added.len(), pr_id);
    if !failed.is_empty() {
        message.push_str(&format!(
            " ({} failed: {})",
            failed.len(),
            failed.join(", ")
        ));
    }
    let has_failures = !failed.is_empty();
    let data = serde_json::json!({
        "pull_request_id": pr_id,
        "operation": "add-reviewers",
        "added": added,
        "failed": failed,
    });
    if has_failures {
        ExecutionResult::warning_with_data(message, data)
    } else {
        ExecutionResult::success_with_data(message, data)
    }
}

fn validate_and_normalize_reviewers(
    reviewers: &[String],
    config: &UpdatePrConfig,
) -> Result<Vec<String>, ExecutionResult> {
    if config.max_reviewers == 0 {
        return Err(ExecutionResult::failure(
            "update-pr.max-reviewers must be greater than zero",
        ));
    }
    let allow_any = config
        .allowed_reviewers
        .iter()
        .any(|allowed| allowed == "*");
    if config.allowed_reviewers.is_empty() {
        return Err(ExecutionResult::failure(
            "add-reviewers requires allowed-reviewers to be configured; use \
             allowed-reviewers: [\"*\"] to permit any valid reviewer",
        ));
    }

    let mut normalized = Vec::new();
    for reviewer in reviewers {
        let reviewer = reviewer.trim();
        if !allow_any
            && !config
                .allowed_reviewers
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(reviewer))
        {
            return Err(ExecutionResult::failure(format!(
                "Reviewer '{}' is not in update-pr.allowed-reviewers",
                crate::sanitize::neutralize_pipeline_commands(reviewer)
            )));
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(reviewer))
        {
            normalized.push(reviewer.to_string());
        }
    }
    if normalized.len() > config.max_reviewers {
        return Err(ExecutionResult::failure(format!(
            "add-reviewers requested {} unique reviewers, exceeding max-reviewers: {}",
            normalized.len(),
            config.max_reviewers
        )));
    }
    Ok(normalized)
}

impl UpdatePrResult {
    /// Set auto-complete on a pull request.
    ///
    /// Resolves the authenticated user identity via `_apis/connectiondata`, then
    /// patches the PR with `autoCompleteSetBy` and default completion options.
    /// Uses the agent's own identity (not the PR creator) for proper audit trail.
    async fn execute_set_auto_complete(
        &self,
        operation_ctx: &UpdatePrContext<'_>,
        config: &UpdatePrConfig,
    ) -> anyhow::Result<ExecutionResult> {
        // Validate merge_strategy before any network I/O
        if !VALID_MERGE_STRATEGIES.contains(&config.merge_strategy.as_str()) {
            return Ok(ExecutionResult::failure(format!(
                "Invalid merge-strategy '{}'. Must be one of: {}",
                config.merge_strategy,
                VALID_MERGE_STRATEGIES.join(", ")
            )));
        }

        // Resolve the agent's identity via connection data
        let connection_url = format!(
            "{}/_apis/connectiondata",
            operation_ctx.target.organization_url.trim_end_matches('/')
        );
        let conn_response = crate::safe_outputs::authenticate_ado_request(
            operation_ctx.client.get(&connection_url),
            operation_ctx.token,
            operation_ctx.connection_type,
        )
        .send()
        .await
        .context("Failed to fetch connection data for auto-complete identity")?;

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

        let agent_user_id = conn_body
            .get("authenticatedUser")
            .and_then(|au| au.get("id"))
            .and_then(|id| id.as_str())
            .context("Connection data response missing authenticatedUser.id")?;
        debug!("Agent user ID for auto-complete: {}", agent_user_id);

        // PATCH to set auto-complete using the agent's identity
        let patch_url = format!(
            "{}/pullRequests/{}?api-version=7.1",
            operation_ctx.repository_api_base(),
            operation_ctx.pr_id
        );
        let patch_body = serde_json::json!({
            "autoCompleteSetBy": {
                "id": agent_user_id
            },
            "completionOptions": {
                "deleteSourceBranch": config.delete_source_branch,
                "mergeStrategy": config.merge_strategy
            }
        });

        info!("Setting auto-complete on PR #{}", operation_ctx.pr_id);
        let response = crate::safe_outputs::authenticate_ado_request(
            operation_ctx.client.patch(&patch_url),
            operation_ctx.token,
            operation_ctx.connection_type,
        )
        .header("Content-Type", "application/json")
        .json(&patch_body)
        .send()
        .await
        .context("Failed to set auto-complete on PR")?;

        if response.status().is_success() {
            info!("Auto-complete set on PR #{}", operation_ctx.pr_id);
            Ok(ExecutionResult::success_with_data(
                format!("Auto-complete set on PR #{}", operation_ctx.pr_id),
                serde_json::json!({
                    "pull_request_id": operation_ctx.pr_id,
                    "operation": "set-auto-complete",
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Ok(ExecutionResult::failure(format!(
                "Failed to set auto-complete on PR #{} (HTTP {}): {}",
                operation_ctx.pr_id, status, error_body
            )))
        }
    }

    /// Submit a vote on a pull request.
    ///
    /// Resolves the current user identity via `_apis/connectiondata`, then
    /// PUTs the vote to the reviewers endpoint.
    async fn execute_vote(
        &self,
        operation_ctx: &UpdatePrContext<'_>,
        config: &UpdatePrConfig,
    ) -> anyhow::Result<ExecutionResult> {
        let vote_str = self
            .vote
            .as_deref()
            .context("vote value is required for vote operation")?;

        // Validate against allowed-votes — REQUIRED for vote operation.
        // An empty allowed-votes list means the operator hasn't opted in, so reject.
        if config.allowed_votes.is_empty() {
            return Ok(ExecutionResult::failure(
                "vote operation requires 'allowed-votes' to be configured in safe-outputs.update-pr. \
                 This prevents agents from casting unrestricted votes (including approve). \
                 Example:\n  safe-outputs:\n    update-pr:\n      allowed-votes:\n        - approve-with-suggestions\n        - wait-for-author"
                    .to_string(),
            ));
        }
        if !config.allowed_votes.contains(&vote_str.to_string()) {
            return Ok(ExecutionResult::failure(format!(
                "Vote '{}' is not in the allowed-votes list: [{}]",
                vote_str,
                config.allowed_votes.join(", ")
            )));
        }

        let vote_value = vote_to_ado_value(vote_str).context(format!(
            "Invalid vote value: '{}'. Must be one of: {}",
            vote_str,
            VALID_VOTES.join(", ")
        ))?;

        // Resolve the current user identity.
        // Use the org URL for connection data — supports vanity domains and national clouds.
        let connection_url = format!(
            "{}/_apis/connectiondata",
            operation_ctx.target.organization_url.trim_end_matches('/')
        );
        debug!("Connection data URL: {}", connection_url);

        let conn_response = crate::safe_outputs::authenticate_ado_request(
            operation_ctx.client.get(&connection_url),
            operation_ctx.token,
            operation_ctx.connection_type,
        )
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

        // Self-approval guard: prevent the agent from approving PRs it created.
        // Positive votes (approve=10, approve-with-suggestions=5) are blocked when
        // the authenticated user is also the PR author.
        if vote_value > 0 {
            let pr_url = format!(
                "{}/pullRequests/{}?api-version=7.1",
                operation_ctx.repository_api_base(),
                operation_ctx.pr_id
            );
            let pr_response = crate::safe_outputs::authenticate_ado_request(
                operation_ctx.client.get(&pr_url),
                operation_ctx.token,
                operation_ctx.connection_type,
            )
            .send()
            .await
            .context("Failed to fetch PR for self-approval check")?;

            if pr_response.status().is_success() {
                let pr_body: serde_json::Value = pr_response
                    .json()
                    .await
                    .context("Failed to parse PR response")?;

                let creator_id = pr_body
                    .get("createdBy")
                    .and_then(|cb| cb.get("id"))
                    .and_then(|id| id.as_str());

                if creator_id == Some(user_id) {
                    return Ok(ExecutionResult::failure(format!(
                        "Self-approval blocked: the authenticated identity created PR #{} \
                         and cannot cast a positive vote ('{}') on it",
                        operation_ctx.pr_id, vote_str
                    )));
                }
            } else {
                let status = pr_response.status();
                let error_body = pr_response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string());
                return Ok(ExecutionResult::failure(format!(
                    "Failed to fetch PR #{} for self-approval check (HTTP {}): {}",
                    operation_ctx.pr_id, status, error_body
                )));
            }
        }

        // PUT vote to reviewers endpoint
        let encoded_user_id = utf8_percent_encode(user_id, PATH_SEGMENT).to_string();
        let vote_url = format!(
            "{}/pullRequests/{}/reviewers/{}?api-version=7.1",
            operation_ctx.repository_api_base(),
            operation_ctx.pr_id,
            encoded_user_id
        );
        let vote_body = serde_json::json!({
            "vote": vote_value
        });

        info!(
            "Voting '{}' ({}) on PR #{}",
            vote_str, vote_value, operation_ctx.pr_id
        );
        let response = crate::safe_outputs::authenticate_ado_request(
            operation_ctx.client.put(&vote_url),
            operation_ctx.token,
            operation_ctx.connection_type,
        )
        .header("Content-Type", "application/json")
        .json(&vote_body)
        .send()
        .await
        .context("Failed to submit vote")?;

        if response.status().is_success() {
            info!(
                "Vote '{}' submitted on PR #{}",
                vote_str, operation_ctx.pr_id
            );
            Ok(ExecutionResult::success_with_data(
                format!(
                    "Vote '{}' submitted on PR #{}",
                    vote_str, operation_ctx.pr_id
                ),
                serde_json::json!({
                    "pull_request_id": operation_ctx.pr_id,
                    "operation": "vote",
                    "vote": vote_str,
                    "vote_value": vote_value,
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Ok(ExecutionResult::failure(format!(
                "Failed to submit vote on PR #{} (HTTP {}): {}",
                operation_ctx.pr_id, status, error_body
            )))
        }
    }

    /// Add reviewers to a pull request.
    ///
    /// For each reviewer email, resolves the identity via VSSPS, then PUTs to
    /// the reviewers endpoint with vote 0.
    async fn execute_add_reviewers(
        &self,
        operation_ctx: &UpdatePrContext<'_>,
        config: &UpdatePrConfig,
    ) -> anyhow::Result<ExecutionResult> {
        let requested_reviewers = self
            .reviewers
            .as_ref()
            .context("reviewers list is required for add-reviewers operation")?;
        let reviewers = match validate_and_normalize_reviewers(requested_reviewers, config) {
            Ok(reviewers) => reviewers,
            Err(failure) => return Ok(failure),
        };

        let mut added = Vec::new();
        let mut failed = Vec::new();

        // Derive VSSPS base URL once, before the loop.
        let trimmed_org = operation_ctx.target.organization_url.trim_end_matches('/');
        let vssps_base = trimmed_org.replace("://dev.azure.com/", "://vssps.dev.azure.com/");
        if vssps_base == trimmed_org {
            return Ok(ExecutionResult::failure(format!(
                "Cannot derive VSSPS identity endpoint from org URL '{}'. \
                 The add-reviewers operation requires dev.azure.com-style URLs \
                 to resolve reviewer identities. Legacy *.visualstudio.com \
                 organizations are not currently supported for this operation.",
                trimmed_org
            )));
        }

        for reviewer in &reviewers {
            match resolve_and_add_reviewer(
                operation_ctx.client,
                &vssps_base,
                &operation_ctx.repository_api_base(),
                operation_ctx.pr_id,
                reviewer,
                operation_ctx.token,
                operation_ctx.connection_type,
            )
            .await
            {
                ReviewerAddResult::Added => added.push(reviewer.clone()),
                ReviewerAddResult::Failed(reason) => {
                    failed.push(format!("{} ({})", reviewer, reason));
                }
            }
        }

        Ok(reviewer_execution_result(
            operation_ctx.pr_id,
            added,
            failed,
        ))
    }

    /// Add labels to a pull request.
    ///
    /// For each label, POSTs to the labels endpoint.
    async fn execute_add_labels(
        &self,
        operation_ctx: &UpdatePrContext<'_>,
    ) -> anyhow::Result<ExecutionResult> {
        let labels = self
            .labels
            .as_ref()
            .context("labels list is required for add-labels operation")?;

        let labels_url = format!(
            "{}/pullRequests/{}/labels?api-version=7.1",
            operation_ctx.repository_api_base(),
            operation_ctx.pr_id
        );

        let mut added = Vec::new();
        let mut failed = Vec::new();

        for label in labels {
            let label_body = serde_json::json!({
                "name": label
            });

            debug!("Adding label '{}' to PR #{}", label, operation_ctx.pr_id);
            let response = crate::safe_outputs::authenticate_ado_request(
                operation_ctx.client.post(&labels_url),
                operation_ctx.token,
                operation_ctx.connection_type,
            )
            .header("Content-Type", "application/json")
            .json(&label_body)
            .send()
            .await;

            match response {
                Ok(resp) if resp.status().is_success() => {
                    info!("Added label '{}' to PR #{}", label, operation_ctx.pr_id);
                    added.push(label.clone());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let error_body = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());
                    warn!(
                        "Failed to add label '{}' to PR #{} (HTTP {}): {}",
                        label, operation_ctx.pr_id, status, error_body
                    );
                    failed.push(format!("{} (HTTP {})", label, status));
                }
                Err(e) => {
                    warn!(
                        "Request failed for label '{}' on PR #{}: {}",
                        label, operation_ctx.pr_id, e
                    );
                    failed.push(format!("{} (request error)", label));
                }
            }
        }

        if added.is_empty() && !failed.is_empty() {
            Ok(ExecutionResult::failure(format!(
                "Failed to add any labels to PR #{}: {}",
                operation_ctx.pr_id,
                failed.join(", ")
            )))
        } else {
            let mut message = format!(
                "Added {} label(s) to PR #{}",
                added.len(),
                operation_ctx.pr_id
            );
            if !failed.is_empty() {
                message.push_str(&format!(
                    " ({} failed: {})",
                    failed.len(),
                    failed.join(", ")
                ));
            }
            Ok(ExecutionResult::success_with_data(
                message,
                serde_json::json!({
                    "pull_request_id": operation_ctx.pr_id,
                    "operation": "add-labels",
                    "added": added,
                    "failed": failed,
                }),
            ))
        }
    }

    /// Update the description of a pull request.
    async fn execute_update_description(
        &self,
        operation_ctx: &UpdatePrContext<'_>,
    ) -> anyhow::Result<ExecutionResult> {
        let description = self
            .description
            .as_ref()
            .context("description is required for update-description operation")?;

        let patch_url = format!(
            "{}/pullRequests/{}?api-version=7.1",
            operation_ctx.repository_api_base(),
            operation_ctx.pr_id
        );
        let patch_body = serde_json::json!({
            "description": description
        });

        info!(
            "Updating description on PR #{} ({} chars)",
            operation_ctx.pr_id,
            description.len()
        );
        let response = crate::safe_outputs::authenticate_ado_request(
            operation_ctx.client.patch(&patch_url),
            operation_ctx.token,
            operation_ctx.connection_type,
        )
        .header("Content-Type", "application/json")
        .json(&patch_body)
        .send()
        .await
        .context("Failed to update PR description")?;

        if response.status().is_success() {
            info!("Description updated on PR #{}", operation_ctx.pr_id);
            Ok(ExecutionResult::success_with_data(
                format!("Description updated on PR #{}", operation_ctx.pr_id),
                serde_json::json!({
                    "pull_request_id": operation_ctx.pr_id,
                    "operation": "update-description",
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Ok(ExecutionResult::failure(format!(
                "Failed to update description on PR #{} (HTTP {}): {}",
                operation_ctx.pr_id, status, error_body
            )))
        }
    }
}

/// Look up the Azure DevOps identity GUID for `reviewer` via the VSSPS
/// identities API. Returns `Some(guid)` on success or `None` if the identity
/// cannot be resolved (warning is logged in that case).
async fn lookup_reviewer_id(
    client: &reqwest::Client,
    vssps_base: &str,
    reviewer: &str,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
) -> Option<String> {
    if reviewer.len() == 36
        && reviewer
            .chars()
            .filter(|character| *character == '-')
            .count()
            == 4
        && reviewer
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
    {
        return Some(reviewer.to_string());
    }

    let identity_url = format!(
        "{}/_apis/identities?searchFilter=General&filterValue={}&api-version=7.1",
        vssps_base,
        utf8_percent_encode(reviewer, PATH_SEGMENT),
    );
    debug!("Resolving identity for '{}': {}", reviewer, identity_url);

    match crate::safe_outputs::authenticate_ado_request(
        client.get(&identity_url),
        token,
        connection_type,
    )
    .send()
    .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let matching_ids = body
                .get("value")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter(|identity| {
                    let direct_match = ["providerDisplayName", "customDisplayName", "displayName"]
                        .iter()
                        .filter_map(|field| identity.get(field).and_then(serde_json::Value::as_str))
                        .any(|value| value.eq_ignore_ascii_case(reviewer));
                    let property_match = ["Account", "Mail"]
                        .iter()
                        .filter_map(|field| {
                            identity
                                .get("properties")
                                .and_then(|properties| properties.get(field))
                                .and_then(|property| property.get("$value"))
                                .and_then(serde_json::Value::as_str)
                        })
                        .any(|value| value.eq_ignore_ascii_case(reviewer));
                    direct_match || property_match
                })
                .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
                .collect::<std::collections::HashSet<_>>();
            if matching_ids.len() == 1 {
                matching_ids.into_iter().next().map(str::to_string)
            } else {
                if matching_ids.len() > 1 {
                    warn!(
                        "Identity lookup for '{}' returned multiple exact matches",
                        reviewer
                    );
                }
                None
            }
        }
        Ok(resp) => {
            warn!(
                "Identity lookup for '{}' returned HTTP {}",
                reviewer,
                resp.status()
            );
            None
        }
        Err(e) => {
            warn!("Identity lookup for '{}' failed: {}", reviewer, e);
            None
        }
    }
}

/// PUT `reviewer_id` as a reviewer onto `pr_id`. Returns
/// [`ReviewerAddResult::Added`] on success or [`ReviewerAddResult::Failed`]
/// with a short reason string on any HTTP or transport error.
async fn add_reviewer_to_pr(
    client: &reqwest::Client,
    repository_api_base: &str,
    pr_id: u64,
    reviewer_id: &str,
    reviewer: &str,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
) -> ReviewerAddResult {
    let reviewer_url = format!(
        "{}/pullRequests/{}/reviewers/{}?api-version=7.1",
        repository_api_base, pr_id, reviewer_id,
    );
    let reviewer_body = serde_json::json!({ "vote": 0, "isRequired": false });

    debug!("Adding reviewer '{}' to PR #{}", reviewer, pr_id);
    let response = crate::safe_outputs::authenticate_ado_request(
        client.put(&reviewer_url),
        token,
        connection_type,
    )
    .header("Content-Type", "application/json")
    .json(&reviewer_body)
    .send()
    .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            info!("Added reviewer '{}' to PR #{}", reviewer, pr_id);
            ReviewerAddResult::Added
        }
        Ok(resp) => {
            let status = resp.status();
            let error_body = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            warn!(
                "Failed to add reviewer '{}' to PR #{} (HTTP {}): {}",
                reviewer, pr_id, status, error_body
            );
            ReviewerAddResult::Failed(format!("HTTP {}", status))
        }
        Err(e) => {
            warn!(
                "Request failed for reviewer '{}' on PR #{}: {}",
                reviewer, pr_id, e
            );
            ReviewerAddResult::Failed("request error".to_string())
        }
    }
}

/// Resolve an ADO identity for `reviewer` via the VSSPS identities API, then
/// PUT the reviewer onto the PR. Returns [`ReviewerAddResult::Added`] on success
/// or [`ReviewerAddResult::Failed`] with a short reason string on any failure.
async fn resolve_and_add_reviewer(
    client: &reqwest::Client,
    vssps_base: &str,
    repository_api_base: &str,
    pr_id: u64,
    reviewer: &str,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
) -> ReviewerAddResult {
    let Some(reviewer_id) =
        lookup_reviewer_id(client, vssps_base, reviewer, token, connection_type).await
    else {
        warn!("Could not resolve identity for '{}', skipping", reviewer);
        return ReviewerAddResult::Failed("identity not found".to_string());
    };
    add_reviewer_to_pr(
        client,
        repository_api_base,
        pr_id,
        &reviewer_id,
        reviewer,
        token,
        connection_type,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(UpdatePrResult::NAME, "update-pr");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{
            "pull_request_id": 42,
            "operation": "set-auto-complete"
        }"#;
        let params: UpdatePrParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pull_request_id, PullRequestReference::Number(42));
        assert_eq!(params.operation, "set-auto-complete");
        assert!(params.repository.is_none());
    }

    #[test]
    fn pull_request_reference_accepts_quoted_numbers_and_temporary_ids() {
        let quoted: PullRequestReference = serde_json::from_str("\"42\"").unwrap();
        let temporary: PullRequestReference = serde_json::from_str("\"#aw_pr123\"").unwrap();
        assert_eq!(quoted, PullRequestReference::Number(42));
        assert!(matches!(temporary, PullRequestReference::Temporary(_)));
        assert!(serde_json::from_str::<PullRequestReference>("\"not-an-id\"").is_err());
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = UpdatePrParams {
            pull_request_id: PullRequestReference::Number(42),
            repository: Some("self".to_string()),
            operation: "set-auto-complete".to_string(),
            reviewers: None,
            labels: None,
            vote: None,
            description: None,
        };
        let result: UpdatePrResult = params.try_into().unwrap();
        assert_eq!(result.name, "update-pr");
        assert_eq!(result.pull_request_id, PullRequestReference::Number(42));
        assert_eq!(result.operation, "set-auto-complete");
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = UpdatePrParams {
            pull_request_id: PullRequestReference::Number(0),
            repository: None,
            operation: "set-auto-complete".to_string(),
            reviewers: None,
            labels: None,
            vote: None,
            description: None,
        };
        let result: Result<UpdatePrResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_invalid_operation() {
        let params = UpdatePrParams {
            pull_request_id: PullRequestReference::Number(1),
            repository: None,
            operation: "delete-pr".to_string(),
            reviewers: None,
            labels: None,
            vote: None,
            description: None,
        };
        let err: Result<UpdatePrResult, _> = params.try_into();
        let err = err.unwrap_err().to_string();
        assert!(err.contains("operation must be one of"), "got: {err}");
    }

    #[test]
    fn test_validation_rejects_vote_without_value() {
        let params = UpdatePrParams {
            pull_request_id: PullRequestReference::Number(1),
            repository: None,
            operation: "vote".to_string(),
            reviewers: None,
            labels: None,
            vote: None,
            description: None,
        };
        let result: Result<UpdatePrResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_reviewers_without_list() {
        let params = UpdatePrParams {
            pull_request_id: PullRequestReference::Number(1),
            repository: None,
            operation: "add-reviewers".to_string(),
            reviewers: None,
            labels: None,
            vote: None,
            description: None,
        };
        let result: Result<UpdatePrResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_repository_pipeline_command() {
        let params = UpdatePrParams {
            pull_request_id: PullRequestReference::Number(1),
            repository: Some("##vso[task.setvariable variable=x]y".to_string()),
            operation: "set-auto-complete".to_string(),
            reviewers: None,
            labels: None,
            vote: None,
            description: None,
        };
        let result: Result<UpdatePrResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = UpdatePrParams {
            pull_request_id: PullRequestReference::Number(99),
            repository: Some("self".to_string()),
            operation: "vote".to_string(),
            reviewers: None,
            labels: None,
            vote: Some("approve".to_string()),
            description: None,
        };
        let result: UpdatePrResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"update-pr""#));
        assert!(json.contains(r#""pull_request_id":99"#));
        assert!(json.contains(r#""operation":"vote""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = UpdatePrConfig::default();
        assert!(config.allowed_operations.is_empty());
        assert!(config.allowed_repositories.is_empty());
        assert!(config.allowed_votes.is_empty());
        assert!(config.allowed_reviewers.is_empty());
        assert_eq!(config.max_reviewers, DEFAULT_MAX_REVIEWERS);
        assert_eq!(config.merge_strategy, "squash");
    }

    #[test]
    fn reviewer_policy_is_default_deny_and_supports_explicit_wildcard() {
        let reviewers = vec!["owner@example.com".to_string()];
        let denied = validate_and_normalize_reviewers(&reviewers, &UpdatePrConfig::default());
        assert!(denied.unwrap_err().message.contains("allowed-reviewers"));

        let config = UpdatePrConfig {
            allowed_reviewers: vec!["*".to_string()],
            ..Default::default()
        };
        assert_eq!(
            validate_and_normalize_reviewers(&reviewers, &config).unwrap(),
            reviewers
        );
    }

    #[test]
    fn reviewer_policy_deduplicates_and_enforces_limit() {
        let config = UpdatePrConfig {
            allowed_reviewers: vec![
                "Owner@example.com".to_string(),
                "other@example.com".to_string(),
            ],
            max_reviewers: 2,
            ..Default::default()
        };
        let reviewers = validate_and_normalize_reviewers(
            &[
                "owner@example.com".to_string(),
                "OWNER@example.com".to_string(),
            ],
            &config,
        )
        .unwrap();
        assert_eq!(reviewers, ["owner@example.com"]);

        let too_many = validate_and_normalize_reviewers(
            &[
                "owner@example.com".to_string(),
                "other@example.com".to_string(),
                "third@example.com".to_string(),
            ],
            &UpdatePrConfig {
                allowed_reviewers: vec!["*".to_string()],
                max_reviewers: 2,
                ..Default::default()
            },
        );
        assert!(too_many.unwrap_err().message.contains("max-reviewers"));
    }

    #[test]
    fn reviewer_results_warn_for_partial_and_total_failures() {
        let partial = reviewer_execution_result(
            42,
            vec!["added@example.com".to_string()],
            vec!["failed@example.com (HTTP 403)".to_string()],
        );
        assert!(partial.success);
        assert!(partial.is_warning());
        assert_eq!(
            partial.data.as_ref().unwrap()["added"][0],
            "added@example.com"
        );

        let total = reviewer_execution_result(
            42,
            Vec::new(),
            vec!["failed@example.com (identity not found)".to_string()],
        );
        assert!(total.success);
        assert!(total.is_warning());
        assert_eq!(
            total.data.as_ref().unwrap()["failed"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let success =
            reviewer_execution_result(42, vec!["added@example.com".to_string()], Vec::new());
        assert!(success.success);
        assert!(!success.is_warning());
    }

    #[test]
    fn temporary_reference_resolves_exact_registered_target() {
        let temporary_id = PullRequestTemporaryId::parse("#aw_pr123").unwrap();
        let ctx = ExecutionContext::default();
        let target = AdoRepositoryTarget {
            alias: "tools".to_string(),
            organization: "other-org".to_string(),
            organization_url: "https://dev.azure.com/other-org".to_string(),
            project: "Other Project".to_string(),
            repository: "tools".to_string(),
            repository_id: Some("repo-id".to_string()),
            cross_organization: true,
        };
        ctx.register_resolved_pull_request(
            &temporary_id,
            crate::safe_outputs::ResolvedPullRequest {
                id: 42,
                url: "https://example.test/pr/42".to_string(),
                target: target.clone(),
            },
        )
        .unwrap();

        let resolved = resolve_update_pr_target(
            &PullRequestReference::Temporary(temporary_id),
            None,
            &UpdatePrConfig::default(),
            &ctx,
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved, (42, target));
    }

    #[tokio::test]
    async fn reviewer_identity_lookup_requires_exact_match() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value": [
                    {
                        "id": "wrong-id",
                        "providerDisplayName": "Similar Person",
                        "properties": {"Mail": {"$value": "similar@example.com"}}
                    },
                    {
                        "id": "exact-id",
                        "providerDisplayName": "Exact Person",
                        "properties": {"Mail": {"$value": "owner@example.com"}}
                    }
                ]
            })))
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            "owner@example.com",
            "token",
            None,
        )
        .await;
        assert_eq!(id.as_deref(), Some("exact-id"));

        let missing = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            "missing@example.com",
            "token",
            None,
        )
        .await;
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn reviewer_identity_lookup_rejects_ambiguous_exact_matches() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value": [
                    {
                        "id": "first-id",
                        "properties": {"Mail": {"$value": "owner@example.com"}}
                    },
                    {
                        "id": "second-id",
                        "properties": {"Mail": {"$value": "owner@example.com"}}
                    }
                ]
            })))
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            "owner@example.com",
            "token",
            None,
        )
        .await;
        assert!(id.is_none());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
allowed-operations:
  - add-reviewers
  - set-auto-complete
allowed-repositories:
  - self
allowed-votes:
  - approve
  - reject
allowed-reviewers:
  - owner@example.com
max-reviewers: 2
"#;
        let config: UpdatePrConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.allowed_operations.len(), 2);
        assert!(
            config
                .allowed_operations
                .contains(&"add-reviewers".to_string())
        );
        assert!(
            config
                .allowed_operations
                .contains(&"set-auto-complete".to_string())
        );
        assert_eq!(config.allowed_repositories.len(), 1);
        assert_eq!(config.allowed_votes.len(), 2);
        assert_eq!(config.allowed_reviewers, ["owner@example.com"]);
        assert_eq!(config.max_reviewers, 2);
    }

    #[test]
    fn test_valid_merge_strategies_are_expected_values() {
        assert_eq!(
            VALID_MERGE_STRATEGIES,
            &["squash", "noFastForward", "rebase", "rebaseMerge"]
        );
    }

    #[test]
    fn test_config_deserializes_merge_strategy() {
        let yaml = r#"merge-strategy: rebase"#;
        let config: UpdatePrConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.merge_strategy, "rebase");
    }
}
