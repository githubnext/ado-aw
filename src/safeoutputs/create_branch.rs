//! Create branch safe output tool

use ado_aw_derive::SanitizeConfig;
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PATH_SEGMENT, validate_git_ref_name};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use anyhow::{Context, ensure};

/// Parameters for creating a branch
#[derive(Deserialize, JsonSchema)]
pub struct CreateBranchParams {
    /// Branch name to create (e.g., "feature/my-analysis"). 1-200 characters.
    pub branch_name: String,

    /// Branch to create from (default: "main")
    pub source_branch: Option<String>,

    /// Specific commit SHA to branch from (overrides source_branch). Must be a valid 40-character hex string.
    pub source_commit: Option<String>,

    /// Repository alias: "self" for pipeline repo, or an alias from the checkout list (default: "self")
    pub repository: Option<String>,
}

impl Validate for CreateBranchParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(!self.branch_name.is_empty(), "branch_name must not be empty");
        ensure!(
            self.branch_name.len() <= 200,
            "branch_name must be at most 200 characters"
        );
        ensure!(
            !self.branch_name.contains(".."),
            "branch_name must not contain '..'"
        );
        ensure!(
            !self.branch_name.contains('\0'),
            "branch_name must not contain null bytes"
        );
        ensure!(
            !self.branch_name.starts_with('-'),
            "branch_name must not start with '-'"
        );
        ensure!(
            !self.branch_name.contains(' '),
            "branch_name must not contain spaces"
        );
        validate_git_ref_name(&self.branch_name, "branch_name")?;

        if let Some(ref commit) = self.source_commit {
            ensure!(
                commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit()),
                "source_commit must be a valid 40-character hex SHA"
            );
        }

        if let Some(ref branch) = self.source_branch {
            ensure!(
                !branch.contains(".."),
                "source_branch must not contain '..'"
            );
            ensure!(
                !branch.contains('\0'),
                "source_branch must not contain null bytes"
            );
            validate_git_ref_name(branch, "source_branch")?;
        }

        Ok(())
    }
}

tool_result! {
    name = "create-branch",
    write = true,
    params = CreateBranchParams,
    /// Result of creating a branch
    pub struct CreateBranchResult {
        branch_name: String,
        source_branch: Option<String>,
        source_commit: Option<String>,
        repository: Option<String>,
    }
}

impl SanitizeContent for CreateBranchResult {
    fn sanitize_content_fields(&mut self) {
        self.branch_name = sanitize_text(&self.branch_name);
    }
}

/// Configuration for the create-branch tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   create-branch:
///     branch-pattern: "^agent/.*"
///     allowed-repositories:
///       - self
///       - other-repo
///     allowed-source-branches:
///       - main
///       - develop
/// ```
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct CreateBranchConfig {
    /// Regex pattern that branch names must match
    #[serde(default, rename = "branch-pattern")]
    pub branch_pattern: Option<String>,

    /// Repositories the agent is allowed to create branches in
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,

    /// Source branches the agent is allowed to branch from
    #[serde(default, rename = "allowed-source-branches")]
    pub allowed_source_branches: Vec<String>,
}

impl Default for CreateBranchConfig {
    fn default() -> Self {
        Self {
            branch_pattern: None,
            allowed_repositories: Vec::new(),
            allowed_source_branches: Vec::new(),
        }
    }
}

/// Resolve a branch name to its latest commit SHA via the Azure DevOps refs API
async fn resolve_branch_to_commit(
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    token: &str,
    repo_name: &str,
    branch: &str,
) -> anyhow::Result<String> {
    let url = format!(
        "{}/{}/_apis/git/repositories/{}/refs",
        org_url.trim_end_matches('/'),
        utf8_percent_encode(project, PATH_SEGMENT),
        utf8_percent_encode(repo_name, PATH_SEGMENT),
    );
    debug!("Resolving branch '{}' via: {}", branch, url);

    let response = client
        .get(&url)
        .query(&[
            ("filter", format!("heads/{}", branch).as_str()),
            ("api-version", "7.1"),
        ])
        .basic_auth("", Some(token))
        .send()
        .await
        .context("Failed to query refs API")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        anyhow::bail!(
            "Failed to resolve branch '{}' (HTTP {}): {}",
            branch,
            status,
            error_body
        );
    }

    let body: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse refs response")?;

    body.get("value")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|r| r.get("objectId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .context(format!(
            "Branch '{}' not found in repository '{}'",
            branch, repo_name
        ))
}

#[async_trait::async_trait]
impl Executor for CreateBranchResult {
    fn dry_run_summary(&self) -> String {
        format!("create branch '{}'", self.branch_name)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!("Creating branch: '{}'", self.branch_name);
        debug!("create-branch: branch_name='{}'", self.branch_name);

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

        let config: CreateBranchConfig = ctx.get_tool_config("create-branch");
        debug!("Branch pattern: {:?}", config.branch_pattern);
        debug!("Allowed repositories: {:?}", config.allowed_repositories);
        debug!(
            "Allowed source branches: {:?}",
            config.allowed_source_branches
        );

        // Validate branch name against branch-pattern regex (if configured)
        if let Some(ref pattern) = config.branch_pattern {
            let re = regex_lite::Regex::new(pattern).context(format!(
                "Invalid branch-pattern regex: '{}'",
                pattern
            ))?;
            if !re.is_match(&self.branch_name) {
                return Ok(ExecutionResult::failure(format!(
                    "Branch name '{}' does not match required pattern '{}'",
                    self.branch_name, pattern
                )));
            }
            debug!("Branch name matches pattern '{}'", pattern);
        }

        // Determine repository alias
        let repo_alias = self
            .repository
            .as_deref()
            .unwrap_or("self");

        // Validate repository against config policy BEFORE resolving the name,
        // so operators see a policy error rather than a confusing "not in checkout list" error.
        if !config.allowed_repositories.is_empty()
            && !config.allowed_repositories.contains(&repo_alias.to_string())
        {
            return Ok(ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed-repositories list: [{}]",
                repo_alias,
                config.allowed_repositories.join(", ")
            )));
        }

        // Resolve the alias to the actual ADO repo name
        let repo_name = if repo_alias == "self" {
            ctx.repository_name
                .as_deref()
                .context("BUILD_REPOSITORY_NAME not set")?
                .to_string()
        } else {
            ctx.allowed_repositories
                .get(repo_alias)
                .cloned()
                .context(format!(
                    "Repository alias '{}' is not in the allowed checkout list",
                    repo_alias
                ))?
        };
        debug!("Resolved repository: {}", repo_name);

        // Validate source_branch against allowed-source-branches (if configured)
        let source_branch = self.source_branch.as_deref().unwrap_or("main");
        if !config.allowed_source_branches.is_empty()
            && !config.allowed_source_branches.contains(&source_branch.to_string())
        {
            return Ok(ExecutionResult::failure(format!(
                "Source branch '{}' is not in the allowed-source-branches list",
                source_branch
            )));
        }

        let client = reqwest::Client::new();

        // Resolve the source commit SHA
        let source_sha = if let Some(ref commit) = self.source_commit {
            debug!("Using explicit source commit: {}", commit);
            commit.clone()
        } else {
            debug!("Resolving source branch '{}' to commit", source_branch);
            resolve_branch_to_commit(&client, org_url, project, token, &repo_name, source_branch)
                .await?
        };
        debug!("Source commit SHA: {}", source_sha);

        // Build the refs update URL
        let url = format!(
            "{}/{}/_apis/git/repositories/{}/refs?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            utf8_percent_encode(&repo_name, PATH_SEGMENT),
        );
        debug!("API URL: {}", url);

        // Build the ref update request body
        let ref_name = if self.branch_name.starts_with("refs/heads/") {
            self.branch_name.clone()
        } else {
            format!("refs/heads/{}", self.branch_name)
        };

        let ref_updates = serde_json::json!([{
            "name": ref_name,
            "oldObjectId": "0000000000000000000000000000000000000000",
            "newObjectId": source_sha,
        }]);

        info!("Creating branch '{}' from commit {}", ref_name, source_sha);
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&ref_updates)
            .send()
            .await
            .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            // Check for per-ref errors in the response
            let success = body
                .get("value")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|r| r.get("success"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if !success {
                let custom_message = body
                    .get("value")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|r| r.get("customMessage"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");

                return Ok(ExecutionResult::failure(format!(
                    "Failed to create branch '{}': {}",
                    self.branch_name, custom_message
                )));
            }

            info!("Branch '{}' created successfully", self.branch_name);

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Created branch '{}' in repository '{}' from commit {}",
                    self.branch_name, repo_name, &source_sha[..8]
                ),
                serde_json::json!({
                    "branch": self.branch_name,
                    "ref": ref_name,
                    "repository": repo_name,
                    "source_commit": source_sha,
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
                "Failed to create branch '{}' (HTTP {}): {}",
                self.branch_name, status, error_body
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
        assert_eq!(CreateBranchResult::NAME, "create-branch");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"branch_name": "feature/my-analysis", "source_branch": "main", "repository": "self"}"#;
        let params: CreateBranchParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.branch_name, "feature/my-analysis");
        assert_eq!(params.source_branch, Some("main".to_string()));
        assert_eq!(params.repository, Some("self".to_string()));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = CreateBranchParams {
            branch_name: "feature/new-work".to_string(),
            source_branch: Some("develop".to_string()),
            source_commit: None,
            repository: None,
        };
        let result: CreateBranchResult = params.try_into().unwrap();
        assert_eq!(result.name, "create-branch");
        assert_eq!(result.branch_name, "feature/new-work");
        assert_eq!(result.source_branch, Some("develop".to_string()));
    }

    #[test]
    fn test_validation_rejects_empty_branch() {
        let params = CreateBranchParams {
            branch_name: "".to_string(),
            source_branch: None,
            source_commit: None,
            repository: None,
        };
        let result: Result<CreateBranchResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_path_traversal() {
        let params = CreateBranchParams {
            branch_name: "feature/../main".to_string(),
            source_branch: None,
            source_commit: None,
            repository: None,
        };
        let result: Result<CreateBranchResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_invalid_commit() {
        let params = CreateBranchParams {
            branch_name: "feature/valid".to_string(),
            source_branch: None,
            source_commit: Some("not-a-valid-sha".to_string()),
            repository: None,
        };
        let result: Result<CreateBranchResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_branch_starting_with_dash() {
        let params = CreateBranchParams {
            branch_name: "-bad".to_string(),
            source_branch: None,
            source_commit: None,
            repository: None,
        };
        let result: Result<CreateBranchResult, _> = params.try_into();
        assert!(result.is_err(), "branch starting with '-' should be rejected");
    }

    #[test]
    fn test_validation_rejects_branch_with_spaces() {
        let params = CreateBranchParams {
            branch_name: "my branch".to_string(),
            source_branch: None,
            source_commit: None,
            repository: None,
        };
        let result: Result<CreateBranchResult, _> = params.try_into();
        assert!(result.is_err(), "branch with spaces should be rejected");
    }

    #[test]
    fn test_validation_rejects_branch_over_200_chars() {
        let params = CreateBranchParams {
            branch_name: "a".repeat(201),
            source_branch: None,
            source_commit: None,
            repository: None,
        };
        let result: Result<CreateBranchResult, _> = params.try_into();
        assert!(result.is_err(), "branch >200 chars should be rejected");
    }

    #[test]
    fn test_validation_rejects_source_branch_with_traversal() {
        let params = CreateBranchParams {
            branch_name: "feature/valid".to_string(),
            source_branch: Some("../evil".to_string()),
            source_commit: None,
            repository: None,
        };
        let result: Result<CreateBranchResult, _> = params.try_into();
        assert!(result.is_err(), "source_branch with '..' should be rejected");
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = CreateBranchParams {
            branch_name: "feature/test-branch".to_string(),
            source_branch: Some("main".to_string()),
            source_commit: None,
            repository: None,
        };
        let result: CreateBranchResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"create-branch""#));
        assert!(json.contains(r#""branch_name":"feature/test-branch""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = CreateBranchConfig::default();
        assert!(config.branch_pattern.is_none());
        assert!(config.allowed_repositories.is_empty());
        assert!(config.allowed_source_branches.is_empty());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
branch-pattern: "^agent/.*"
allowed-repositories:
  - self
  - other-repo
allowed-source-branches:
  - main
  - develop
"#;
        let config: CreateBranchConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.branch_pattern, Some("^agent/.*".to_string()));
        assert_eq!(config.allowed_repositories, vec!["self", "other-repo"]);
        assert_eq!(config.allowed_source_branches, vec!["main", "develop"]);
    }
}
