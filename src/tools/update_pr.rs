//! Update pull request safe output tool

use log::{debug, info, warn};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PATH_SEGMENT, resolve_repo_name};
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use crate::tool_result;
use crate::tools::{ExecutionContext, ExecutionResult, Executor, Validate};
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
    /// Pull request ID (must be positive)
    pub pull_request_id: i32,

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
        ensure!(
            self.pull_request_id > 0,
            "pull_request_id must be a positive integer"
        );
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
    params = UpdatePrParams,
    /// Result of updating a pull request
    pub struct UpdatePrResult {
        pull_request_id: i32,
        repository: Option<String>,
        operation: String,
        reviewers: Option<Vec<String>>,
        labels: Option<Vec<String>>,
        vote: Option<String>,
        description: Option<String>,
    }
}

impl Sanitize for UpdatePrResult {
    fn sanitize_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_text);
        self.operation = sanitize_text(&self.operation);
        self.reviewers = self
            .reviewers
            .as_ref()
            .map(|rs| rs.iter().map(|r| sanitize_text(r)).collect());
        self.labels = self
            .labels
            .as_ref()
            .map(|ls| ls.iter().map(|l| sanitize_text(l)).collect());
        self.vote = self.vote.as_deref().map(sanitize_text);
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for UpdatePrConfig {
    fn default() -> Self {
        Self {
            allowed_operations: Vec::new(),
            allowed_repositories: Vec::new(),
            allowed_votes: Vec::new(),
            delete_source_branch: true,
            merge_strategy: "squash".to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Executor for UpdatePrResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Updating PR #{} — operation: {}",
            self.pull_request_id, self.operation
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

        let config: UpdatePrConfig = ctx.get_tool_config("update-pr");
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

        // Validate repository against allowed-repositories
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

        let client = reqwest::Client::new();
        let encoded_project = utf8_percent_encode(project, PATH_SEGMENT).to_string();
        let base_url = format!(
            "{}/{}/_apis/git/repositories",
            org_url.trim_end_matches('/'),
            encoded_project,
        );

        match self.operation.as_str() {
            "set-auto-complete" => {
                self.execute_set_auto_complete(&client, &base_url, &repo_name, token, &config)
                    .await
            }
            "vote" => {
                self.execute_vote(
                    &client,
                    &base_url,
                    &repo_name,
                    token,
                    org_url,
                    &config,
                )
                .await
            }
            "add-reviewers" => {
                self.execute_add_reviewers(
                    &client,
                    &base_url,
                    &repo_name,
                    token,
                    org_url,
                )
                .await
            }
            "add-labels" => {
                self.execute_add_labels(&client, &base_url, &repo_name, token)
                    .await
            }
            "update-description" => {
                self.execute_update_description(&client, &base_url, &repo_name, token)
                    .await
            }
            _ => Ok(ExecutionResult::failure(format!(
                "Unknown operation: {}",
                self.operation
            ))),
        }
    }
}

impl UpdatePrResult {
    /// Set auto-complete on a pull request.
    ///
    /// First fetches the PR to get the `createdBy.id`, then patches the PR
    /// with `autoCompleteSetBy` and default completion options.
    async fn execute_set_auto_complete(
        &self,
        client: &reqwest::Client,
        base_url: &str,
        repo_name: &str,
        token: &str,
        config: &UpdatePrConfig,
    ) -> anyhow::Result<ExecutionResult> {
        let encoded_repo = utf8_percent_encode(repo_name, PATH_SEGMENT).to_string();

        // Fetch the PR to get createdBy.id
        let get_url = format!(
            "{}/{}/pullRequests/{}?api-version=7.1",
            base_url, encoded_repo, self.pull_request_id
        );
        debug!("GET PR URL: {}", get_url);

        let pr_response = client
            .get(&get_url)
            .basic_auth("", Some(token))
            .send()
            .await
            .context("Failed to fetch pull request")?;

        if !pr_response.status().is_success() {
            let status = pr_response.status();
            let error_body = pr_response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Ok(ExecutionResult::failure(format!(
                "Failed to fetch PR #{} (HTTP {}): {}",
                self.pull_request_id, status, error_body
            )));
        }

        let pr_body: serde_json::Value = pr_response
            .json()
            .await
            .context("Failed to parse PR response")?;

        let created_by_id = pr_body
            .get("createdBy")
            .and_then(|cb| cb.get("id"))
            .and_then(|id| id.as_str())
            .context("PR response missing createdBy.id")?;
        debug!("PR created by: {}", created_by_id);

        // PATCH to set auto-complete
        if !VALID_MERGE_STRATEGIES.contains(&config.merge_strategy.as_str()) {
            return Ok(ExecutionResult::failure(format!(
                "Invalid merge-strategy '{}'. Must be one of: {}",
                config.merge_strategy,
                VALID_MERGE_STRATEGIES.join(", ")
            )));
        }
        let patch_url = format!(
            "{}/{}/pullRequests/{}?api-version=7.1",
            base_url, encoded_repo, self.pull_request_id
        );
        let patch_body = serde_json::json!({
            "autoCompleteSetBy": {
                "id": created_by_id
            },
            "completionOptions": {
                "deleteSourceBranch": config.delete_source_branch,
                "mergeStrategy": config.merge_strategy
            }
        });

        info!("Setting auto-complete on PR #{}", self.pull_request_id);
        let response = client
            .patch(&patch_url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&patch_body)
            .send()
            .await
            .context("Failed to set auto-complete on PR")?;

        if response.status().is_success() {
            info!(
                "Auto-complete set on PR #{}",
                self.pull_request_id
            );
            Ok(ExecutionResult::success_with_data(
                format!("Auto-complete set on PR #{}", self.pull_request_id),
                serde_json::json!({
                    "pull_request_id": self.pull_request_id,
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
                self.pull_request_id, status, error_body
            )))
        }
    }

    /// Submit a vote on a pull request.
    ///
    /// Resolves the current user identity via `_apis/connectiondata`, then
    /// PUTs the vote to the reviewers endpoint.
    async fn execute_vote(
        &self,
        client: &reqwest::Client,
        base_url: &str,
        repo_name: &str,
        token: &str,
        org_url: &str,
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
        if !config.allowed_votes.contains(&vote_str.to_string())
        {
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
        let encoded_repo = utf8_percent_encode(repo_name, PATH_SEGMENT).to_string();
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
            vote_str, vote_value, self.pull_request_id
        );
        let response = client
            .put(&vote_url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&vote_body)
            .send()
            .await
            .context("Failed to submit vote")?;

        if response.status().is_success() {
            info!(
                "Vote '{}' submitted on PR #{}",
                vote_str, self.pull_request_id
            );
            Ok(ExecutionResult::success_with_data(
                format!(
                    "Vote '{}' submitted on PR #{}",
                    vote_str, self.pull_request_id
                ),
                serde_json::json!({
                    "pull_request_id": self.pull_request_id,
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
                self.pull_request_id, status, error_body
            )))
        }
    }

    /// Add reviewers to a pull request.
    ///
    /// For each reviewer email, resolves the identity via VSSPS, then PUTs to
    /// the reviewers endpoint with vote 0.
    async fn execute_add_reviewers(
        &self,
        client: &reqwest::Client,
        base_url: &str,
        repo_name: &str,
        token: &str,
        org_url: &str,
    ) -> anyhow::Result<ExecutionResult> {
        let reviewers = self
            .reviewers
            .as_ref()
            .context("reviewers list is required for add-reviewers operation")?;

        let encoded_repo = utf8_percent_encode(repo_name, PATH_SEGMENT).to_string();
        let mut added = Vec::new();
        let mut failed = Vec::new();

        // Derive VSSPS base URL once, before the loop.
        let trimmed_org = org_url.trim_end_matches('/');
        let vssps_base = trimmed_org
            .replace("://dev.azure.com/", "://vssps.dev.azure.com/");
        if vssps_base == trimmed_org {
            return Ok(ExecutionResult::failure(format!(
                "Cannot derive VSSPS identity endpoint from org URL '{}'. \
                 The add-reviewers operation requires dev.azure.com-style URLs \
                 to resolve reviewer identities. Legacy *.visualstudio.com \
                 organizations are not currently supported for this operation.",
                trimmed_org
            )));
        }

        for reviewer in reviewers {
            let identity_url = format!(
                "{}/_apis/identities?searchFilter=General&filterValue={}&api-version=7.1",
                vssps_base,
                utf8_percent_encode(reviewer, PATH_SEGMENT),
            );
            debug!("Resolving identity for '{}': {}", reviewer, identity_url);

            let identity_response = client
                .get(&identity_url)
                .basic_auth("", Some(token))
                .send()
                .await;

            let reviewer_id = match identity_response {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    body.get("value")
                        .and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|entry| entry.get("id"))
                        .and_then(|id| id.as_str())
                        .map(|s| s.to_string())
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
            };

            let reviewer_id = match reviewer_id {
                Some(id) => id,
                None => {
                    warn!(
                        "Could not resolve identity for '{}', skipping",
                        reviewer
                    );
                    failed.push(format!("{} (identity not found)", reviewer));
                    continue;
                }
            };

            let reviewer_url = format!(
                "{}/{}/pullRequests/{}/reviewers/{}?api-version=7.1",
                base_url,
                encoded_repo,
                self.pull_request_id,
                reviewer_id,
            );
            let reviewer_body = serde_json::json!({
                "vote": 0,
                "isRequired": false
            });

            debug!("Adding reviewer '{}' to PR #{}", reviewer, self.pull_request_id);
            let response = client
                .put(&reviewer_url)
                .header("Content-Type", "application/json")
                .basic_auth("", Some(token))
                .json(&reviewer_body)
                .send()
                .await;

            match response {
                Ok(resp) if resp.status().is_success() => {
                    info!("Added reviewer '{}' to PR #{}", reviewer, self.pull_request_id);
                    added.push(reviewer.clone());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let error_body = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());
                    warn!(
                        "Failed to add reviewer '{}' to PR #{} (HTTP {}): {}",
                        reviewer, self.pull_request_id, status, error_body
                    );
                    failed.push(format!("{} (HTTP {})", reviewer, status));
                }
                Err(e) => {
                    warn!(
                        "Request failed for reviewer '{}' on PR #{}: {}",
                        reviewer, self.pull_request_id, e
                    );
                    failed.push(format!("{} (request error)", reviewer));
                }
            }
        }

        if added.is_empty() && !failed.is_empty() {
            Ok(ExecutionResult::failure(format!(
                "Failed to add any reviewers to PR #{}: {}",
                self.pull_request_id,
                failed.join(", ")
            )))
        } else {
            let mut message = format!(
                "Added {} reviewer(s) to PR #{}",
                added.len(),
                self.pull_request_id
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
                    "pull_request_id": self.pull_request_id,
                    "operation": "add-reviewers",
                    "added": added,
                    "failed": failed,
                }),
            ))
        }
    }

    /// Add labels to a pull request.
    ///
    /// For each label, POSTs to the labels endpoint.
    async fn execute_add_labels(
        &self,
        client: &reqwest::Client,
        base_url: &str,
        repo_name: &str,
        token: &str,
    ) -> anyhow::Result<ExecutionResult> {
        let labels = self
            .labels
            .as_ref()
            .context("labels list is required for add-labels operation")?;

        let encoded_repo = utf8_percent_encode(repo_name, PATH_SEGMENT).to_string();
        let labels_url = format!(
            "{}/{}/pullRequests/{}/labels?api-version=7.1",
            base_url, encoded_repo, self.pull_request_id
        );

        let mut added = Vec::new();
        let mut failed = Vec::new();

        for label in labels {
            let label_body = serde_json::json!({
                "name": label
            });

            debug!(
                "Adding label '{}' to PR #{}",
                label, self.pull_request_id
            );
            let response = client
                .post(&labels_url)
                .header("Content-Type", "application/json")
                .basic_auth("", Some(token))
                .json(&label_body)
                .send()
                .await;

            match response {
                Ok(resp) if resp.status().is_success() => {
                    info!("Added label '{}' to PR #{}", label, self.pull_request_id);
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
                        label, self.pull_request_id, status, error_body
                    );
                    failed.push(format!("{} (HTTP {})", label, status));
                }
                Err(e) => {
                    warn!(
                        "Request failed for label '{}' on PR #{}: {}",
                        label, self.pull_request_id, e
                    );
                    failed.push(format!("{} (request error)", label));
                }
            }
        }

        if added.is_empty() && !failed.is_empty() {
            Ok(ExecutionResult::failure(format!(
                "Failed to add any labels to PR #{}: {}",
                self.pull_request_id,
                failed.join(", ")
            )))
        } else {
            let mut message = format!(
                "Added {} label(s) to PR #{}",
                added.len(),
                self.pull_request_id
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
                    "pull_request_id": self.pull_request_id,
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
        client: &reqwest::Client,
        base_url: &str,
        repo_name: &str,
        token: &str,
    ) -> anyhow::Result<ExecutionResult> {
        let description = self
            .description
            .as_ref()
            .context("description is required for update-description operation")?;

        let encoded_repo = utf8_percent_encode(repo_name, PATH_SEGMENT).to_string();
        let patch_url = format!(
            "{}/{}/pullRequests/{}?api-version=7.1",
            base_url, encoded_repo, self.pull_request_id
        );
        let patch_body = serde_json::json!({
            "description": description
        });

        info!(
            "Updating description on PR #{} ({} chars)",
            self.pull_request_id,
            description.len()
        );
        let response = client
            .patch(&patch_url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&patch_body)
            .send()
            .await
            .context("Failed to update PR description")?;

        if response.status().is_success() {
            info!("Description updated on PR #{}", self.pull_request_id);
            Ok(ExecutionResult::success_with_data(
                format!(
                    "Description updated on PR #{}",
                    self.pull_request_id
                ),
                serde_json::json!({
                    "pull_request_id": self.pull_request_id,
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
                self.pull_request_id, status, error_body
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
        assert_eq!(UpdatePrResult::NAME, "update-pr");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{
            "pull_request_id": 42,
            "operation": "set-auto-complete"
        }"#;
        let params: UpdatePrParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pull_request_id, 42);
        assert_eq!(params.operation, "set-auto-complete");
        assert!(params.repository.is_none());
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = UpdatePrParams {
            pull_request_id: 42,
            repository: Some("self".to_string()),
            operation: "set-auto-complete".to_string(),
            reviewers: None,
            labels: None,
            vote: None,
            description: None,
        };
        let result: UpdatePrResult = params.try_into().unwrap();
        assert_eq!(result.name, "update-pr");
        assert_eq!(result.pull_request_id, 42);
        assert_eq!(result.operation, "set-auto-complete");
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = UpdatePrParams {
            pull_request_id: 0,
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
            pull_request_id: 1,
            repository: None,
            operation: "delete-pr".to_string(),
            reviewers: None,
            labels: None,
            vote: None,
            description: None,
        };
        let result: Result<UpdatePrResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_vote_without_value() {
        let params = UpdatePrParams {
            pull_request_id: 1,
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
            pull_request_id: 1,
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
    fn test_result_serializes_correctly() {
        let params = UpdatePrParams {
            pull_request_id: 99,
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
        assert_eq!(config.merge_strategy, "squash");
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
"#;
        let config: UpdatePrConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.allowed_operations.len(), 2);
        assert!(config.allowed_operations.contains(&"add-reviewers".to_string()));
        assert!(config.allowed_operations.contains(&"set-auto-complete".to_string()));
        assert_eq!(config.allowed_repositories.len(), 1);
        assert_eq!(config.allowed_votes.len(), 2);
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

    #[test]
    fn test_valid_merge_strategies_are_recognized() {
        for strategy in VALID_MERGE_STRATEGIES {
            assert!(
                VALID_MERGE_STRATEGIES.contains(strategy),
                "'{}' should be a valid merge strategy",
                strategy
            );
        }
        // Ensure invalid strategy is NOT in the list
        assert!(!VALID_MERGE_STRATEGIES.contains(&"invalid"));
        assert!(!VALID_MERGE_STRATEGIES.contains(&"Squash"));
    }
}
