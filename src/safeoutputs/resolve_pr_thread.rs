//! Resolve PR review thread safe output tool

use ado_aw_derive::SanitizeConfig;
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::resolve_repo_name;
use super::PATH_SEGMENT;
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
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

fn default_repository() -> Option<String> {
    Some("self".to_string())
}

/// Parameters for resolving or reactivating a PR review thread
#[derive(Deserialize, JsonSchema)]
pub struct ResolvePrThreadParams {
    /// The pull request ID containing the thread
    pub pull_request_id: i32,

    /// The thread ID to resolve or reactivate
    pub thread_id: i32,

    /// Target status: "fixed", "wont-fix", "closed", "by-design", or "active" (to reactivate)
    pub status: String,

    /// Repository alias: "self" for pipeline repo, or an alias from the checkout list.
    /// Defaults to "self" if omitted.
    #[serde(default = "default_repository")]
    pub repository: Option<String>,
}

impl Validate for ResolvePrThreadParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.pull_request_id > 0,
            "pull_request_id must be positive"
        );
        ensure!(self.thread_id > 0, "thread_id must be positive");
        ensure!(
            VALID_STATUSES.contains(&self.status.as_str()),
            "Invalid status '{}'. Valid statuses: {}",
            self.status,
            VALID_STATUSES.join(", ")
        );
        Ok(())
    }
}

tool_result! {
    name = "resolve-pr-review-thread",
    write = true,
    params = ResolvePrThreadParams,
    /// Result of resolving or reactivating a PR review thread
    pub struct ResolvePrThreadResult {
        pull_request_id: i32,
        thread_id: i32,
        status: String,
        repository: Option<String>,
    }
}

impl SanitizeContent for ResolvePrThreadResult {
    fn sanitize_content_fields(&mut self) {
        self.status = sanitize_text(&self.status);
        if let Some(ref repo) = self.repository {
            self.repository = Some(sanitize_text(repo));
        }
    }
}

/// Configuration for the resolve-pr-review-thread tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   resolve-pr-review-thread:
///     allowed-repositories:
///       - self
///       - other-repo
///     allowed-statuses:
///       - fixed
///       - wont-fix
/// ```
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct ResolvePrThreadConfig {
    /// Restrict which repositories the agent can operate on.
    /// If empty, all repositories in the checkout list (plus "self") are allowed.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,

    /// Restrict which thread statuses can be set.
    /// REQUIRED — empty list rejects all status transitions.
    #[serde(default, rename = "allowed-statuses")]
    pub allowed_statuses: Vec<String>,
}

impl Default for ResolvePrThreadConfig {
    fn default() -> Self {
        Self {
            allowed_repositories: Vec::new(),
            allowed_statuses: Vec::new(),
        }
    }
}

#[async_trait::async_trait]
impl Executor for ResolvePrThreadResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Resolving thread #{} on PR #{} with status '{}'",
            self.thread_id, self.pull_request_id, self.status
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

        let config: ResolvePrThreadConfig = ctx.get_tool_config("resolve-pr-review-thread");
        debug!("Config: {:?}", config);

        // Validate status against allowed-statuses — REQUIRED.
        // An empty allowed-statuses list means the operator hasn't opted in, so reject.
        // This prevents agents from resolving review threads (e.g. marking security
        // concerns as "fixed") without explicit operator consent.
        if config.allowed_statuses.is_empty() {
            return Ok(ExecutionResult::failure(
                "resolve-pr-review-thread requires 'allowed-statuses' to be configured in \
                 safe-outputs.resolve-pr-review-thread. This prevents agents from \
                 manipulating thread statuses without explicit operator consent. Example:\n  \
                 safe-outputs:\n    resolve-pr-review-thread:\n      allowed-statuses:\n        \
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

        let effective_repo = self
            .repository
            .as_deref()
            .unwrap_or("self");

        // Validate repository against allowed-repositories config
        if !config.allowed_repositories.is_empty()
            && !config.allowed_repositories.contains(&effective_repo.to_string())
        {
            return Ok(ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed-repositories list",
                effective_repo
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

        // Resolve repository alias to actual repo name via the shared helper.
        // Treat an empty string the same as "self" (pipeline repository).
        let alias = self.repository.as_deref().filter(|s| !s.is_empty());
        let repo_name = match resolve_repo_name(alias, ctx) {
            Ok(name) => name,
            Err(result) => return Ok(result),
        };

        // Build the Azure DevOps REST API URL for updating a thread
        // PATCH https://dev.azure.com/{org}/{project}/_apis/git/repositories/{repo}/pullRequests/{prId}/threads/{threadId}?api-version=7.1
        let url = format!(
            "{}/{}/_apis/git/repositories/{}/pullRequests/{}/threads/{}?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            utf8_percent_encode(&repo_name, PATH_SEGMENT),
            self.pull_request_id,
            self.thread_id,
        );
        debug!("API URL: {}", url);

        let body = serde_json::json!({
            "status": status_int
        });

        let client = reqwest::Client::new();

        info!(
            "Updating thread #{} on PR #{} to status '{}'",
            self.thread_id, self.pull_request_id, self.status
        );
        let response = client
            .patch(&url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&body)
            .send()
            .await
            .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let resp_body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            let returned_id = resp_body.get("id").and_then(|v| v.as_i64()).unwrap_or(0);

            info!(
                "Thread #{} on PR #{} updated to status '{}'",
                self.thread_id, self.pull_request_id, self.status
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Updated thread #{} on PR #{} to status '{}'",
                    self.thread_id, self.pull_request_id, self.status
                ),
                serde_json::json!({
                    "thread_id": returned_id,
                    "pull_request_id": self.pull_request_id,
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
                self.thread_id, self.pull_request_id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(ResolvePrThreadResult::NAME, "resolve-pr-review-thread");
    }

    #[test]
    fn test_params_deserializes() {
        let json =
            r#"{"pull_request_id": 42, "thread_id": 7, "status": "fixed"}"#;
        let params: ResolvePrThreadParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pull_request_id, 42);
        assert_eq!(params.thread_id, 7);
        assert_eq!(params.status, "fixed");
        assert_eq!(params.repository, Some("self".to_string()));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = ResolvePrThreadParams {
            pull_request_id: 42,
            thread_id: 7,
            status: "fixed".to_string(),
            repository: Some("self".to_string()),
        };
        let result: ResolvePrThreadResult = params.try_into().unwrap();
        assert_eq!(result.name, "resolve-pr-review-thread");
        assert_eq!(result.pull_request_id, 42);
        assert_eq!(result.thread_id, 7);
        assert_eq!(result.status, "fixed");
    }

    #[test]
    fn test_validation_rejects_zero_pr_id() {
        let params = ResolvePrThreadParams {
            pull_request_id: 0,
            thread_id: 7,
            status: "fixed".to_string(),
            repository: Some("self".to_string()),
        };
        let result: Result<ResolvePrThreadResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_zero_thread_id() {
        let params = ResolvePrThreadParams {
            pull_request_id: 42,
            thread_id: 0,
            status: "fixed".to_string(),
            repository: Some("self".to_string()),
        };
        let result: Result<ResolvePrThreadResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_invalid_status() {
        let params = ResolvePrThreadParams {
            pull_request_id: 42,
            thread_id: 7,
            status: "invalid-status".to_string(),
            repository: Some("self".to_string()),
        };
        let result: Result<ResolvePrThreadResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = ResolvePrThreadParams {
            pull_request_id: 42,
            thread_id: 7,
            status: "fixed".to_string(),
            repository: Some("self".to_string()),
        };
        let result: ResolvePrThreadResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"resolve-pr-review-thread""#));
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
