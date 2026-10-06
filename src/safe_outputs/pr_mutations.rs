//! Shared Azure DevOps PR mutations and legacy configuration validation.

use super::pr_http::BoundedPrResponse;
#[cfg(test)]
use super::pr_common::PullRequestReference;
use super::pr_common::repository_api_base;
#[cfg(test)]
use super::pr_common::resolve_pr_target;
use super::result::AdoRepositoryTarget;
#[cfg(test)]
use crate::safe_outputs::ExecutionContext;
use crate::safe_outputs::ExecutionResult;
use crate::secure::Guid;
#[cfg(test)]
use crate::secure::PullRequestTemporaryId;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

/// Valid merge strategy values accepted by ADO's completionOptions.mergeStrategy
const VALID_MERGE_STRATEGIES: &[&str] = &["squash", "noFastForward", "rebase", "rebaseMerge"];
const DEFAULT_MAX_REVIEWERS: usize = 3;
const MAX_REVIEWER_LEN: usize = 256;

pub(crate) fn validate_reviewer_inputs(reviewers: &[String]) -> anyhow::Result<()> {
    ensure!(!reviewers.is_empty(), "reviewers list must not be empty for add-reviewers operation");
    ensure!(reviewers.len() <= 100, "reviewers list must contain at most 100 entries");
    for reviewer in reviewers {
        let reviewer = reviewer.trim();
        ensure!(!reviewer.is_empty(), "reviewer must not be empty");
        ensure!(reviewer.len() <= MAX_REVIEWER_LEN, "reviewer must be {MAX_REVIEWER_LEN} characters or fewer");
        reject_pipeline_injection(reviewer, "reviewers")?;
    }
    Ok(())
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
    /// Empty or a literal "*" allows any valid reviewer.
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

pub(crate) struct UpdatePrContext<'a> {
    pub client: &'a reqwest::Client,
    pub target: AdoRepositoryTarget,
    pub pr_id: u64,
    pub token: &'a str,
    pub connection_type: Option<crate::compile::types::WriteConnectionType>,
}

impl UpdatePrContext<'_> {
    pub(crate) fn repository_api_base(&self) -> String {
        repository_api_base(&self.target)
    }
}

/// Outcome of a single reviewer resolution + add attempt.
#[derive(Debug)]
pub(crate) enum ReviewerAddResult {
    Added,
    AlreadyPresent,
    Failed(String),
}

#[derive(Default, Serialize)]
pub(crate) struct ReviewerChanges {
    pub added: Vec<String>,
    pub already_present: Vec<String>,
    pub failed: Vec<String>,
}

impl ReviewerChanges {
    pub(crate) fn record(&mut self, reviewer: &str, result: ReviewerAddResult) {
        match result {
            ReviewerAddResult::Added => self.added.push(reviewer.into()),
            ReviewerAddResult::AlreadyPresent => self.already_present.push(reviewer.into()),
            ReviewerAddResult::Failed(reason) => {
                warn!("Reviewer '{}' was not confirmed: {}", reviewer, reason);
                self.failed.push(format!("{reviewer} ({reason})"));
            }
        }
    }
}

fn reviewer_execution_result(
    pr_id: u64,
    added: Vec<String>,
    already_present: Vec<String>,
    failed: Vec<String>,
) -> ExecutionResult {
    let mut message = if added.is_empty() && !already_present.is_empty() {
        format!("{} reviewer(s) already present on PR #{}", already_present.len(), pr_id)
    } else {
        format!("Added {} reviewer(s) to PR #{}; {} already present", added.len(), pr_id, already_present.len())
    };
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
        "already_present": already_present,
        "failed": failed,
    });
    if has_failures {
        ExecutionResult::warning_with_data(message, data)
    } else {
        ExecutionResult::success_with_data(message, data)
    }
}

pub(crate) fn validate_and_normalize_reviewers(
    reviewers: &[String],
    config: &UpdatePrConfig,
) -> Result<Vec<String>, ExecutionResult> {
    if config.max_reviewers == 0 {
        return Err(ExecutionResult::failure(
            "update-pr.max-reviewers must be greater than zero",
        ));
    }
    let allow_any = config.allowed_reviewers.is_empty()
        || config
            .allowed_reviewers
            .iter()
            .any(|allowed| allowed == "*");

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

/// Set auto-complete on a pull request.
///
/// Resolves the authenticated user identity via `_apis/connectiondata`, then
/// patches the PR with `autoCompleteSetBy` and default completion options.
/// Uses the agent's own identity (not the PR creator) for proper audit trail.
pub(crate) async fn execute_set_auto_complete(
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
            .bounded_text()
            .await
            .unwrap_or_else(|error| format!("Failed to read PR error response: {error}"));
        return Ok(ExecutionResult::failure(format!(
            "Failed to fetch connection data (HTTP {}): {}",
            status, error_body
        )));
    }

    let conn_body: serde_json::Value = conn_response
        .bounded_json()
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
            .bounded_text()
            .await
            .unwrap_or_else(|error| format!("Failed to read PR error response: {error}"));
        Ok(ExecutionResult::failure(format!(
            "Failed to set auto-complete on PR #{} (HTTP {}): {}",
            operation_ctx.pr_id, status, error_body
        )))
    }
}

/// Add reviewers to a pull request.
///
/// Resolves each identity, skips existing membership, and adds missing reviewers.
pub(crate) async fn execute_add_reviewers(
    operation_ctx: &UpdatePrContext<'_>,
    config: &UpdatePrConfig,
    requested_reviewers: &[String],
) -> anyhow::Result<ExecutionResult> {
    let reviewers = match validate_and_normalize_reviewers(requested_reviewers, config) {
        Ok(reviewers) => reviewers,
        Err(failure) => return Ok(failure),
    };

    let mut changes = ReviewerChanges::default();

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
        let result = resolve_and_add_reviewer(
            operation_ctx.client,
            &vssps_base,
            &operation_ctx.repository_api_base(),
            operation_ctx.pr_id,
            reviewer,
            operation_ctx.token,
            operation_ctx.connection_type,
        )
        .await;
        changes.record(reviewer, result);
    }

    Ok(reviewer_execution_result(
        operation_ctx.pr_id,
        changes.added,
        changes.already_present,
        changes.failed,
    ))
}

/// Add labels to a pull request.
///
/// For each label, POSTs to the labels endpoint.
pub(crate) async fn execute_add_labels(
    operation_ctx: &UpdatePrContext<'_>,
    labels: &[String],
) -> anyhow::Result<ExecutionResult> {
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
                    .bounded_text()
                    .await
                    .unwrap_or_else(|error| format!("Failed to read PR error response: {error}"));
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
                failed.push(format!("{} (delivery uncertain)", label));
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
pub(crate) async fn execute_update_description(
    operation_ctx: &UpdatePrContext<'_>,
    description: &str,
) -> anyhow::Result<ExecutionResult> {
    if let Err(error) = super::pr_common::validate_description(description) {
        return Ok(ExecutionResult::failure(error.to_string()));
    }

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
            .bounded_text()
            .await
            .unwrap_or_else(|error| format!("Failed to read PR error response: {error}"));
        Ok(ExecutionResult::failure(format!(
            "Failed to update description on PR #{} (HTTP {}): {}",
            operation_ctx.pr_id, status, error_body
        )))
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
    if let Ok(reviewer_id) = Guid::parse(reviewer) {
        let identity_url = format!("{}/_apis/identities", vssps_base);
        debug!(
            "Verifying reviewer identity GUID '{}': {}",
            reviewer, identity_url
        );
        return match crate::safe_outputs::authenticate_ado_request(
            client.get(&identity_url).query(&[
                ("identityIds", reviewer_id.as_str()),
                ("api-version", "7.1"),
            ]),
            token,
            connection_type,
        )
        .send()
        .await
        {
            Ok(resp) if resp.status().is_success() => {
                match resp.bounded_json::<serde_json::Value>().await {
                    Ok(body) => {
                        let Some(identities) =
                            body.get("value").and_then(serde_json::Value::as_array)
                        else {
                            warn!(
                                "Identity lookup for GUID '{}' response missing 'value' array",
                                reviewer
                            );
                            return None;
                        };
                        if identities.len() != 1 {
                            warn!(
                                "Identity lookup for GUID '{}' returned {} identities",
                                reviewer,
                                identities.len()
                            );
                            return None;
                        }
                        identities[0]
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .filter(|id| id.eq_ignore_ascii_case(reviewer_id.as_str()))
                            .map(str::to_string)
                    }
                    Err(error) => {
                        warn!(
                            "Identity lookup for GUID '{}' returned invalid JSON: {}",
                            reviewer, error
                        );
                        None
                    }
                }
            }
            Ok(resp) => {
                warn!(
                    "Identity lookup for GUID '{}' returned HTTP {}",
                    reviewer,
                    resp.status()
                );
                None
            }
            Err(e) => {
                warn!("Identity lookup for GUID '{}' failed: {}", reviewer, e);
                None
            }
        };
    }

    let identity_url = format!("{}/_apis/identities", vssps_base);
    debug!("Resolving identity for '{}': {}", reviewer, identity_url);

    match crate::safe_outputs::authenticate_ado_request(
        client.get(&identity_url).query(&[
            ("searchFilter", "General"),
            ("filterValue", reviewer),
            ("api-version", "7.1"),
        ]),
        token,
        connection_type,
    )
    .send()
    .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = match resp.bounded_json().await {
                Ok(body) => body,
                Err(error) => {
                    warn!("Identity lookup for '{}' returned invalid metadata: {error}", reviewer);
                    return None;
                }
            };
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

#[derive(Deserialize)]
struct ReviewerIdentity {
    id: Guid,
}

#[derive(Deserialize)]
struct ReviewerList {
    value: Vec<ReviewerIdentity>,
    count: Option<usize>,
}

async fn reviewer_membership(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
) -> anyhow::Result<std::collections::HashSet<String>> {
    let response = crate::safe_outputs::authenticate_ado_request(
        client.get(url), token, connection_type,
    ).send().await.context("Reviewer membership read failed")?;
    ensure!(response.status().is_success(),
        "Reviewer membership read failed (HTTP {})", response.status());
    let list: ReviewerList = response.bounded_json().await?;
    ensure!(list.count.is_none_or(|count| count == list.value.len()),
        "Reviewer membership count is incomplete");
    let mut ids = std::collections::HashSet::new();
    for reviewer in list.value {
        ensure!(ids.insert(reviewer.id.as_str().to_ascii_lowercase()),
            "Reviewer membership contains duplicate identities");
    }
    Ok(ids)
}

/// This read-before-add preserves observed reviewer state, not concurrent additions.
/// ADO's ID-only POST can clear required status on a reviewer added after the read.
pub(crate) async fn add_reviewer_to_pr(
    client: &reqwest::Client,
    repository_api_base: &str,
    pr_id: u64,
    reviewer_id: &str,
    reviewer: &str,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
) -> ReviewerAddResult {
    let reviewer_id = match Guid::parse(reviewer_id) {
        Ok(id) => id.as_str().to_ascii_lowercase(),
        Err(error) => return ReviewerAddResult::Failed(format!("invalid reviewer identity: {error}")),
    };
    let reviewer_url = format!("{repository_api_base}/pullRequests/{pr_id}/reviewers?api-version=7.1");
    let members = match reviewer_membership(client, &reviewer_url, token, connection_type).await {
        Ok(members) => members,
        Err(error) => return ReviewerAddResult::Failed(format!("membership not checked: {error:#}")),
    };
    if members.contains(&reviewer_id) {
        return ReviewerAddResult::AlreadyPresent;
    }

    debug!("Adding reviewer '{}' to PR #{}", reviewer, pr_id);
    let response = crate::safe_outputs::authenticate_ado_request(
        client.post(&reviewer_url),
        token,
        connection_type,
    )
    .header("Content-Type", "application/json")
    .json(&serde_json::json!([{ "id": reviewer_id }]))
    .send()
    .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            match reviewer_membership(client, &reviewer_url, token, connection_type).await {
                Ok(members) if members.contains(&reviewer_id) => {
                    info!("Added reviewer '{}' to PR #{}", reviewer, pr_id);
                    ReviewerAddResult::Added
                }
                Ok(_) => ReviewerAddResult::Failed("addition accepted but membership is unconfirmed; no retry".into()),
                Err(error) => ReviewerAddResult::Failed(format!("addition accepted but membership is unconfirmed; no retry: {error:#}")),
            }
        }
        Ok(resp) => {
            let status = resp.status();
            let error_body = resp
                .bounded_text()
                .await
                .unwrap_or_else(|error| format!("Failed to read PR error response: {error}"));
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
            ReviewerAddResult::Failed("delivery uncertain".to_string())
        }
    }
}

/// Resolve an ADO identity for `reviewer` via the VSSPS identities API, then
/// check current membership and add the reviewer only if absent.
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

    #[tokio::test]
    async fn review3_existing_reviewer_is_not_mutated() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};
        let server = MockServer::start().await;
        let actor = "01234567-89ab-cdef-0123-456789abcdef";
        Mock::given(method("GET")).and(path("/pullRequests/42/reviewers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value":[{"id":actor,"vote":-10,"isRequired":true}]
            }))).mount(&server).await;
        Mock::given(method("PUT")).respond_with(ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"id":actor,"vote":0,"isRequired":false}))).mount(&server).await;
        let result = add_reviewer_to_pr(&super::super::pr_http::client().unwrap(),
            &server.uri(), 42, actor, actor, "test-token", None).await;
        assert!(matches!(result, ReviewerAddResult::AlreadyPresent));
        assert!(server.received_requests().await.unwrap().iter().all(|request| request.method.as_str() == "GET"),
            "An already-present reviewer must not receive a state-resetting write");
    }

    #[tokio::test]
    async fn reviewer_addition_uses_fresh_membership_and_identity_only_post() {
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path, body_json, header}};
        let actor = "01234567-89ab-cdef-0123-456789abcdef";
        for bearer in [false, true] {
            let server = MockServer::start().await;
            let present = Arc::new(AtomicBool::new(false));
            let read = present.clone();
            Mock::given(method("GET")).and(path("/pullRequests/42/reviewers"))
                .respond_with(move |_: &wiremock::Request| ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "value": if read.load(Ordering::SeqCst) { vec![serde_json::json!({"id":actor.to_uppercase(),"vote":-10,"isRequired":true})] } else {vec![]}
                }))).expect(5).mount(&server).await;
            let write = present.clone();
            Mock::given(method("POST")).and(path("/pullRequests/42/reviewers"))
                .and(body_json(serde_json::json!([{"id":actor}])))
                .and(header("authorization", if bearer { "Bearer test-token" } else { "Basic OnRlc3QtdG9rZW4=" }))
                .respond_with(move |_: &wiremock::Request| {
                    write.store(true, Ordering::SeqCst);
                    ResponseTemplate::new(200).set_body_json(serde_json::json!([{"id":actor}]))
                }).expect(2).mount(&server).await;
            let client = super::super::pr_http::client().unwrap();
            let auth = bearer.then_some(crate::compile::types::WriteConnectionType::AzureDevOps);
            let first = add_reviewer_to_pr(&client, &server.uri(), 42, actor, "first spelling", "test-token", auth).await;
            assert!(matches!(first, ReviewerAddResult::Added), "{first:?}");
            let duplicate = add_reviewer_to_pr(&client, &server.uri(), 42, &actor.to_uppercase(), "second spelling", "test-token", auth).await;
            assert!(matches!(duplicate, ReviewerAddResult::AlreadyPresent));
            // A later operation must read membership again, not reuse a cached authorization.
            present.store(false, Ordering::SeqCst);
            let later = add_reviewer_to_pr(&client, &server.uri(), 42, actor, "third spelling", "test-token", auth).await;
            assert!(matches!(later, ReviewerAddResult::Added));
        }
    }

    #[tokio::test]
    async fn focused_reviewer_execution_resolves_aliases_without_duplicate_writes() {
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path, body_json}};
        let server = MockServer::start().await;
        let actor = "01234567-89ab-cdef-0123-456789abcdef";
        Mock::given(method("GET")).and(path("/org/_apis/identities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value":[{"id":actor,"displayName":"Owner"}]
            }))).expect(2).mount(&server).await;
        let present = Arc::new(AtomicBool::new(false));
        let read = present.clone();
        Mock::given(method("GET")).and(path("/org/P/_apis/git/repositories/repo/pullRequests/42/reviewers"))
            .respond_with(move |_: &wiremock::Request| ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value":if read.load(Ordering::SeqCst) {vec![serde_json::json!({"id":actor})]} else {vec![]}
            }))).expect(3).mount(&server).await;
        Mock::given(method("POST")).and(path("/org/P/_apis/git/repositories/repo/pullRequests/42/reviewers"))
            .and(body_json(serde_json::json!([{"id":actor}])))
            .respond_with(move |_: &wiremock::Request| {
                present.store(true, Ordering::SeqCst);
                ResponseTemplate::new(200)
            }).expect(1).mount(&server).await;
        let client = reqwest::Client::builder().no_proxy()
            .resolve("dev.azure.com", *server.address())
            .resolve("vssps.dev.azure.com", *server.address())
            .timeout(std::time::Duration::from_secs(2)).build().unwrap();
        let operation = UpdatePrContext {
            client: &client,
            target: AdoRepositoryTarget {
                alias: "self".into(), organization: "org".into(), organization_url: "http://dev.azure.com/org".into(),
                project: "P".into(), repository: "repo".into(), repository_id: None, cross_organization: false,
            },
            pr_id: 42, token: "test-token", connection_type: None,
        };
        let result = execute_add_reviewers(&operation, &UpdatePrConfig::default(), &[actor.into(), "Owner".into()]).await.unwrap();
        assert!(result.success && !result.is_warning(), "{}", result.message);
        let data = result.data.unwrap();
        assert_eq!(data["added"], serde_json::json!([actor]));
        assert_eq!(data["already_present"], serde_json::json!(["Owner"]));
        assert_eq!(data["failed"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn existing_reviewer_votes_and_flags_need_no_state_bearing_request() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let actor = "01234567-89ab-cdef-0123-456789abcdef";
        for vote in [-10, -5, 0, 5, 10] {
            for required in [false, true] {
                let server = MockServer::start().await;
                Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "count":1,"value":[{"id":actor.to_uppercase(),"vote":vote,"isRequired":required,"isFlagged":true,"hasDeclined":true}]
                }))).expect(1).mount(&server).await;
                let result = add_reviewer_to_pr(&super::super::pr_http::client().unwrap(),
                    &server.uri(), 42, actor, actor, "test-token", None).await;
                assert!(matches!(result, ReviewerAddResult::AlreadyPresent));
                assert_eq!(server.received_requests().await.unwrap().len(), 1);
            }
        }
    }

    #[tokio::test]
    async fn incomplete_or_failed_reviewer_reads_never_authorize_addition() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let actor = "01234567-89ab-cdef-0123-456789abcdef";
        let bodies = vec![
            serde_json::json!({}), serde_json::json!({"value":null}),
            serde_json::json!({"value":[{}]}), serde_json::json!({"value":[{"id":"invalid"}]}),
            serde_json::json!({"value":[{"id":actor},{"id":actor.to_uppercase()}]}),
            serde_json::json!({"count":1,"value":[]}),
        ];
        let responses = bodies.into_iter().map(|body| ResponseTemplate::new(200).set_body_json(body))
            .chain([
                ResponseTemplate::new(401), ResponseTemplate::new(403),
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"value":[]}))
                    .insert_header("x-ms-continuationtoken", "more"),
                ResponseTemplate::new(200).set_body_string("not JSON"),
            ]);
        for response in responses {
            let server = MockServer::start().await;
            Mock::given(method("GET")).respond_with(response).expect(1).mount(&server).await;
            let result = add_reviewer_to_pr(&super::super::pr_http::client().unwrap(),
                &server.uri(), 42, actor, actor, "test-token", None).await;
            assert!(matches!(result, ReviewerAddResult::Failed(_)), "{result:?}");
            assert!(server.received_requests().await.unwrap().iter().all(|r| r.method.as_str() == "GET"));
        }
    }

    #[tokio::test]
    async fn reviewer_writes_are_not_replayed_on_errors_or_unconfirmed_membership() {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let actor = "01234567-89ab-cdef-0123-456789abcdef";
        for mode in ["forbidden", "missing", "invalid-readback", "timeout"] {
            let server = MockServer::start().await;
            let reads = Arc::new(AtomicUsize::new(0));
            let observed = reads.clone();
            Mock::given(method("GET")).respond_with(move |_: &wiremock::Request| {
                let count = observed.fetch_add(1, Ordering::SeqCst);
                if mode == "invalid-readback" && count > 0 {
                    ResponseTemplate::new(200).set_body_string("invalid")
                } else { ResponseTemplate::new(200).set_body_json(serde_json::json!({"value":[]})) }
            }).mount(&server).await;
            let response = match mode {
                "forbidden" => ResponseTemplate::new(403),
                "timeout" => ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(2)),
                _ => ResponseTemplate::new(200).set_body_json(serde_json::json!([{"id":actor}])),
            };
            Mock::given(method("POST")).respond_with(response).expect(1).mount(&server).await;
            let client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(300)).build().unwrap();
            let result = add_reviewer_to_pr(&client, &server.uri(), 42, actor, actor, "test-token", None).await;
            let ReviewerAddResult::Failed(reason) = result else { panic!("unexpected {result:?}") };
            assert!(reason.contains(match mode { "timeout" => "uncertain", "forbidden" => "403", _ => "unconfirmed" }), "{reason}");
            assert_eq!(reads.load(Ordering::SeqCst), if matches!(mode, "timeout" | "forbidden") {1} else {2});
        }
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
    fn reviewer_policy_allows_omitted_allowlist_and_explicit_wildcard() {
        let reviewers = vec!["owner@example.com".to_string()];
        assert_eq!(
            validate_and_normalize_reviewers(&reviewers, &UpdatePrConfig::default()).unwrap(),
            reviewers
        );

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
    fn reviewer_policy_restricts_non_empty_allowlist() {
        let result = validate_and_normalize_reviewers(
            &["other@example.com".to_string()],
            &UpdatePrConfig {
                allowed_reviewers: vec!["owner@example.com".to_string()],
                ..Default::default()
            },
        );
        assert!(result.unwrap_err().message.contains("allowed-reviewers"));
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
            vec!["existing@example.com".to_string()],
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
            reviewer_execution_result(42, vec!["added@example.com".to_string()], Vec::new(), Vec::new());
        assert!(success.success);
        assert!(!success.is_warning());
        let noop = reviewer_execution_result(42, vec![], vec!["existing".into()], vec![]);
        assert!(noop.success && !noop.is_warning());
        assert!(!noop.message.contains("Added"));
        assert_eq!(noop.data.unwrap()["already_present"], serde_json::json!(["existing"]));
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

        let resolved = resolve_pr_target(
            &PullRequestReference::Temporary(temporary_id),
            None,
            &[],
            &ctx,
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved, (42, target));
    }

    #[test]
    fn temporary_reference_rejects_unresolved_id() {
        let temporary_id = PullRequestTemporaryId::parse("#aw_pr123").unwrap();
        let result = resolve_pr_target(
            &PullRequestReference::Temporary(temporary_id),
            None,
            &[],
            &ExecutionContext::default(),
        )
        .unwrap()
        .unwrap_err();

        assert!(
            result
                .message
                .contains("temporary pull-request ID '#aw_pr123' has not been resolved"),
            "got: {}",
            result.message
        );
    }

    #[test]
    fn temporary_reference_rejects_requested_repository_mismatch() {
        let temporary_id = PullRequestTemporaryId::parse("#aw_pr123").unwrap();
        let ctx = ExecutionContext::default();
        ctx.register_resolved_pull_request(
            &temporary_id,
            crate::safe_outputs::ResolvedPullRequest {
                id: 42,
                url: "https://example.test/pr/42".to_string(),
                target: AdoRepositoryTarget {
                    alias: "tools".to_string(),
                    organization: "other-org".to_string(),
                    organization_url: "https://dev.azure.com/other-org".to_string(),
                    project: "Other Project".to_string(),
                    repository: "tools".to_string(),
                    repository_id: Some("repo-id".to_string()),
                    cross_organization: true,
                },
            },
        )
        .unwrap();

        let result = resolve_pr_target(
            &PullRequestReference::Temporary(temporary_id),
            Some("self"),
            &[],
            &ctx,
        )
        .unwrap()
        .unwrap_err();

        assert!(
            result.message.contains(
                "resolved to repository 'tools', which does not match requested repository 'self'"
            ),
            "got: {}",
            result.message
        );
    }

    #[test]
    fn temporary_reference_rejects_allowed_repositories_exclusion() {
        let temporary_id = PullRequestTemporaryId::parse("#aw_pr123").unwrap();
        let ctx = ExecutionContext::default();
        ctx.register_resolved_pull_request(
            &temporary_id,
            crate::safe_outputs::ResolvedPullRequest {
                id: 42,
                url: "https://example.test/pr/42".to_string(),
                target: AdoRepositoryTarget {
                    alias: "tools".to_string(),
                    organization: "other-org".to_string(),
                    organization_url: "https://dev.azure.com/other-org".to_string(),
                    project: "Other Project".to_string(),
                    repository: "tools".to_string(),
                    repository_id: Some("repo-id".to_string()),
                    cross_organization: true,
                },
            },
        )
        .unwrap();

        let result = resolve_pr_target(
            &PullRequestReference::Temporary(temporary_id),
            None,
            &["self".to_string()],
            &ctx,
        )
        .unwrap()
        .unwrap_err();

        assert!(
            result
                .message
                .contains("Repository 'tools' is not in the allowed-repositories list: [self]"),
            "got: {}",
            result.message
        );
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
    async fn reviewer_guid_lookup_verifies_existing_identity() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let reviewer = "12345678-1234-1234-1234-1234567890ab";
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("identityIds", reviewer))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 1,
                "value": [{
                    "id": "12345678-1234-1234-1234-1234567890AB",
                    "displayName": "Exact Person"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            reviewer,
            "token",
            None,
        )
        .await;
        assert_eq!(id.as_deref(), Some("12345678-1234-1234-1234-1234567890AB"));
    }

    #[tokio::test]
    async fn reviewer_guid_lookup_rejects_missing_identity() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let reviewer = "12345678-1234-1234-1234-1234567890ab";
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("identityIds", reviewer))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 0,
                "value": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            reviewer,
            "token",
            None,
        )
        .await;
        assert!(id.is_none());
    }

    #[tokio::test]
    async fn reviewer_guid_lookup_rejects_response_missing_value_array() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let reviewer = "12345678-1234-1234-1234-1234567890ab";
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("identityIds", reviewer))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 1
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            reviewer,
            "token",
            None,
        )
        .await;
        assert!(id.is_none());
    }

    #[tokio::test]
    async fn reviewer_guid_lookup_rejects_duplicate_identities() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let reviewer = "12345678-1234-1234-1234-1234567890ab";
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("identityIds", reviewer))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 2,
                "value": [
                    {"id": reviewer},
                    {"id": reviewer}
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            reviewer,
            "token",
            None,
        )
        .await;
        assert!(id.is_none());
    }

    #[tokio::test]
    async fn reviewer_guid_lookup_rejects_mismatched_identity() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let reviewer = "12345678-1234-1234-1234-1234567890ab";
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("identityIds", reviewer))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 1,
                "value": [{
                    "id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            reviewer,
            "token",
            None,
        )
        .await;
        assert!(id.is_none());
    }

    #[tokio::test]
    async fn reviewer_guid_lookup_rejects_http_errors_and_invalid_json() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let reviewer = "12345678-1234-1234-1234-1234567890ab";
        let http_error_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("identityIds", reviewer))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&http_error_server)
            .await;

        let http_error = lookup_reviewer_id(
            &reqwest::Client::new(),
            &http_error_server.uri(),
            reviewer,
            "token",
            None,
        )
        .await;
        assert!(http_error.is_none());

        let invalid_json_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("identityIds", reviewer))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("{", "application/json"))
            .expect(1)
            .mount(&invalid_json_server)
            .await;

        let invalid_json = lookup_reviewer_id(
            &reqwest::Client::new(),
            &invalid_json_server.uri(),
            reviewer,
            "token",
            None,
        )
        .await;
        assert!(invalid_json.is_none());
    }

    #[tokio::test]
    async fn reviewer_guid_lookup_preserves_bearer_auth_routing() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let reviewer = "12345678-1234-1234-1234-1234567890ab";
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("identityIds", reviewer))
            .and(query_param("api-version", "7.1"))
            .and(header("authorization", "Bearer entra-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 1,
                "value": [{"id": reviewer}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            reviewer,
            "entra-token",
            Some(crate::compile::types::WriteConnectionType::AzureDevOps),
        )
        .await;
        assert_eq!(id.as_deref(), Some(reviewer));
    }

    #[tokio::test]
    async fn malformed_guid_like_reviewer_uses_exact_identity_lookup() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let reviewer = "12345678-1234-1234-1234-1234567890ag";
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("searchFilter", "General"))
            .and(query_param("filterValue", reviewer))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            reviewer,
            "token",
            None,
        )
        .await;
        assert!(id.is_none());
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

    #[tokio::test]
    async fn reviewer_identity_lookup_encodes_filter_as_one_query_parameter() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let reviewer = "owner+alerts&team=core@example.com";
        Mock::given(method("GET"))
            .and(path("/_apis/identities"))
            .and(query_param("searchFilter", "General"))
            .and(query_param("filterValue", reviewer))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value": [{
                    "id": "exact-id",
                    "properties": {"Mail": {"$value": reviewer}}
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = lookup_reviewer_id(
            &reqwest::Client::new(),
            &server.uri(),
            reviewer,
            "token",
            None,
        )
        .await;
        assert_eq!(id.as_deref(), Some("exact-id"));
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
