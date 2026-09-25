//! Resolve PR review thread safe output tool

use ado_aw_derive::SanitizeConfig;
use log::{debug, info};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::pr_common::{PullRequestReference, describe_pr_reference, repository_api_base, resolve_configured_pr_target, validate_reference};
use super::{ToolResult, authenticate_ado_request};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use anyhow::{Context, ensure};

/// All valid thread status strings (lowercase, agent-facing)
const VALID_STATUSES: &[&str] = &["active", "fixed", "wont-fix", "closed", "by-design"];

/// Map a thread status string to the ADO API integer value.
///
/// ADO thread status values:
/// - 1 = Active
/// - 2 = Fixed (resolved)
/// - 3 = WontFix
/// - 4 = Closed
/// - 5 = ByDesign
fn status_to_int(status: &str) -> Option<i32> {
    match status {
        "active" => Some(1),
        "fixed" => Some(2),
        "wont-fix" => Some(3),
        "closed" => Some(4),
        "by-design" => Some(5),
        _ => None,
    }
}

/// Parameters for resolving or reactivating a PR review thread
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvePrThreadParams {
    /// The pull request ID containing the thread
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,

    /// The thread ID to resolve or reactivate
    pub thread_id: i32,

    /// Target status: "fixed", "wont-fix", "closed", "by-design", or "active" (to reactivate)
    pub status: String,

    /// Repository alias: "self" for pipeline repo, or an alias from the checkout list.
    /// Defaults to "self" if omitted.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for ResolvePrThreadParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(reference) = &self.pull_request_id {
            validate_reference(reference)?;
        }
        ensure!(self.thread_id > 0, "thread_id must be positive");
        ensure!(
            VALID_STATUSES.contains(&self.status.as_str()),
            "Invalid status '{}'. Valid statuses: {}",
            self.status,
            VALID_STATUSES.join(", ")
        );
        if let Some(repository) = &self.repository {
            reject_pipeline_injection(repository, "repository")?;
        }
        Ok(())
    }
}

tool_result! {
    name = "resolve-pull-request-thread",
    write = true,
    params = ResolvePrThreadParams,
    /// Result of resolving or reactivating a PR review thread
    #[serde(deny_unknown_fields)]
    pub struct ResolvePrThreadResult {
        #[serde(default)]
        pull_request_id: Option<PullRequestReference>,
        thread_id: i32,
        status: String,
        repository: Option<String>,
    }
}

impl SanitizeContent for ResolvePrThreadResult {
    fn sanitize_content_fields(&mut self) {
        self.status = sanitize_config(&self.status);
        if let Some(ref repo) = self.repository {
            self.repository = Some(sanitize_config(repo));
        }
    }
}

/// Configuration for the resolve-pull-request-thread tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   resolve-pull-request-thread:
///     allowed-repositories:
///       - self
///       - other-repo
///     allowed-statuses:
///       - fixed
///       - wont-fix
/// ```
#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvePrThreadConfig {
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
    /// Restrict which repositories the agent can operate on.
    /// If empty, all repositories in the checkout list (plus "self") are allowed.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,

    /// Restrict which thread statuses can be set.
    /// REQUIRED — empty list rejects all status transitions.
    #[serde(default, rename = "allowed-statuses")]
    pub allowed_statuses: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

#[async_trait::async_trait]
impl Executor for ResolvePrThreadResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "resolve thread #{} on {} as '{}'",
            self.thread_id, describe_pr_reference(self.pull_request_id.as_ref()), self.status
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if let Err(error) = (ResolvePrThreadParams {
            pull_request_id: self.pull_request_id.clone(),
            thread_id: self.thread_id,
            status: self.status.clone(),
            repository: self.repository.clone(),
        }).validate() {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;

        let config: ResolvePrThreadConfig = ctx.get_tool_config("resolve-pull-request-thread")?;
        debug!("Config: {:?}", config);

        // Validate status against allowed-statuses — REQUIRED.
        // An empty allowed-statuses list means the operator hasn't opted in, so reject.
        // This prevents agents from resolving review threads (e.g. marking security
        // concerns as "fixed") without explicit operator consent.
        if config.allowed_statuses.is_empty() {
            return Ok(ExecutionResult::failure(
                "resolve-pull-request-thread requires 'allowed-statuses' to be configured in \
                 safe-outputs.resolve-pull-request-thread. This prevents agents from \
                 manipulating thread statuses without explicit operator consent. Example:\n  \
                 safe-outputs:\n    resolve-pull-request-thread:\n      allowed-statuses:\n        \
                 - fixed\n\nValid statuses: active, fixed, wont-fix, closed, by-design"
                    .to_string(),
            ));
        }
        if !config.allowed_statuses.contains(&self.status) {
            return Ok(ExecutionResult::failure(format!(
                "Status '{}' is not in the allowed-statuses list",
                self.status
            )));
        }

        // Map status string to ADO integer
        let status_int = match status_to_int(&self.status) {
            Some(v) => v,
            None => {
                return Ok(ExecutionResult::failure(format!(
                    "Invalid status '{}'. Valid statuses: {}",
                    self.status,
                    VALID_STATUSES.join(", ")
                )));
            }
        };

        super::pr_common::validate_temporary_opt_in(self.pull_request_id.as_ref(), config.allow_temporary_ids)?;
        let (pull_request_id, target) = match resolve_configured_pr_target(
            Self::NAME, self.pull_request_id.as_ref(), self.repository.as_deref(), ctx,
        ).await? {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        let repo_name = target.qualified_repository();
        let project = &target.project;

        // Build the Azure DevOps REST API URL for updating a thread
        // PATCH https://dev.azure.com/{org}/{project}/_apis/git/repositories/{repo}/pullRequests/{prId}/threads/{threadId}?api-version=7.1
        let url = format!(
            "{}/pullRequests/{}/threads/{}?api-version=7.1",
            repository_api_base(&target),
            pull_request_id,
            self.thread_id,
        );
        debug!("API URL: {}", url);

        let body = serde_json::json!({
            "status": status_int
        });

        let client = reqwest::Client::new();

        info!(
            "Updating thread #{} on PR #{} to status '{}'",
            self.thread_id, pull_request_id, self.status
        );
        let response = authenticate_ado_request(client.patch(&url), token, ctx.write_connection_type)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let resp_body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            let returned_id = resp_body.get("id").and_then(|v| v.as_i64())
                .filter(|id| *id == i64::from(self.thread_id))
                .context("Thread update response missing the requested thread ID")?;

            info!(
                "Thread #{} on PR #{} updated to status '{}'",
                self.thread_id, pull_request_id, self.status
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Updated thread #{} on PR #{} to status '{}'",
                    self.thread_id, pull_request_id, self.status
                ),
                serde_json::json!({
                    "thread_id": returned_id,
                    "pull_request_id": pull_request_id,
                    "repository": repo_name,
                    "project": project,
                    "status": self.status,
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            Ok(ExecutionResult::failure(format!(
                "Failed to update thread #{} on PR #{} (HTTP {}): {}",
                self.thread_id, pull_request_id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"pull_request_id": 42, "thread_id": 7, "status": "fixed"}"#;
        let params: ResolvePrThreadParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pull_request_id, Some(PullRequestReference::Number(42)));
        assert_eq!(params.thread_id, 7);
        assert_eq!(params.status, "fixed");
        assert_eq!(params.repository, None);
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = ResolvePrThreadParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 7,
            status: "fixed".to_string(),
            repository: Some("self".to_string()),
        };
        let result: ResolvePrThreadResult = params.try_into().unwrap();
        assert_eq!(result.name, "resolve-pull-request-thread");
        assert_eq!(result.pull_request_id, Some(PullRequestReference::Number(42)));
        assert_eq!(result.thread_id, 7);
        assert_eq!(result.status, "fixed");
        assert_eq!(result.repository, Some("self".to_string()));
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = ResolvePrThreadParams {
            pull_request_id: Some(PullRequestReference::Number(0)),
            thread_id: 7,
            status: "fixed".to_string(),
            repository: Some("self".to_string()),
        };
        let err = <ResolvePrThreadResult as TryFrom<_>>::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("pull_request_id must be a positive integer"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_zero_thread_id() {
        let params = ResolvePrThreadParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 0,
            status: "fixed".to_string(),
            repository: Some("self".to_string()),
        };
        let err = <ResolvePrThreadResult as TryFrom<_>>::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("thread_id must be positive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_invalid_status() {
        let params = ResolvePrThreadParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 7,
            status: "invalid-status".to_string(),
            repository: Some("self".to_string()),
        };
        let err = <ResolvePrThreadResult as TryFrom<_>>::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("Invalid status"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains("invalid-status"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_repository_pipeline_command() {
        let params = ResolvePrThreadParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 7,
            status: "fixed".to_string(),
            repository: Some("##vso[task.setvariable variable=x]y".to_string()),
        };
        let err = <ResolvePrThreadResult as TryFrom<_>>::try_from(params).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("repository") || msg.contains("##vso["),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = ResolvePrThreadParams {
            pull_request_id: Some(PullRequestReference::Number(42)),
            thread_id: 7,
            status: "fixed".to_string(),
            repository: Some("self".to_string()),
        };
        let result: ResolvePrThreadResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"resolve-pull-request-thread""#));
        assert!(json.contains(r#""pull_request_id":42"#));
        assert!(json.contains(r#""thread_id":7"#));
    }

    #[test]
    fn test_config_defaults() {
        let config = ResolvePrThreadConfig::default();
        assert!(config.allowed_repositories.is_empty());
        assert!(config.allowed_statuses.is_empty());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
allowed-repositories:
  - self
  - other-repo
allowed-statuses:
  - fixed
  - wont-fix
"#;
        let config: ResolvePrThreadConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.allowed_repositories, vec!["self", "other-repo"]);
        assert_eq!(config.allowed_statuses, vec!["fixed", "wont-fix"]);
    }

    #[test]
    fn test_status_mapping() {
        assert_eq!(status_to_int("active"), Some(1));
        assert_eq!(status_to_int("fixed"), Some(2));
        assert_eq!(status_to_int("wont-fix"), Some(3));
        assert_eq!(status_to_int("closed"), Some(4));
        assert_eq!(status_to_int("by-design"), Some(5));
        assert_eq!(status_to_int("invalid"), None);
    }
}
