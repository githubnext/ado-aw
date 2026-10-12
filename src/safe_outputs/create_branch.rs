//! Create branch safe output tool

use ado_aw_derive::SanitizeConfig;
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PATH_SEGMENT, validate_git_ref_name};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
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
        ensure!(
            !self.branch_name.is_empty(),
            "branch_name must not be empty"
        );
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
            crate::validate::validate_commit_sha(commit, "source_commit")?;
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
        if let Some(repository) = &self.repository {
            reject_pipeline_injection(repository, "repository")?;
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
        self.branch_name = sanitize_config(&self.branch_name);
        self.repository = self.repository.as_deref().map(sanitize_config);
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
#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
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

/// Resolve a branch name to its latest commit SHA via the Azure DevOps refs API
async fn resolve_branch_to_commit(
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
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

    let response = crate::safe_outputs::authenticate_ado_request(
        client.get(&url).query(&[
            ("filter", format!("heads/{}", branch).as_str()),
            ("api-version", "7.1"),
        ]),
        token,
        connection_type,
    )
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

/// Check the branch name against the configured `branch-pattern` regex, if any.
///
/// Returns `Some(failure)` when the pattern is invalid or the name doesn't match.
fn check_branch_pattern(
    config: &CreateBranchConfig,
    branch_name: &str,
) -> anyhow::Result<Option<ExecutionResult>> {
    let Some(ref pattern) = config.branch_pattern else {
        return Ok(None);
    };
    let re = regex_lite::Regex::new(pattern)
        .context(format!("Invalid branch-pattern regex: '{}'", pattern))?;
    if !re.is_match(branch_name) {
        return Ok(Some(ExecutionResult::failure(format!(
            "Branch name '{}' does not match required pattern '{}'",
            branch_name, pattern
        ))));
    }
    debug!("Branch name matches pattern '{}'", pattern);
    Ok(None)
}

/// Check the repository alias against the configured `allowed-repositories` list, if any.
///
/// Validated before resolving the repository so operators see a policy error rather than a
/// confusing "not in checkout list" error.
fn check_allowed_repository(
    config: &CreateBranchConfig,
    repo_alias: &str,
) -> Option<ExecutionResult> {
    if !config.allowed_repositories.is_empty()
        && !config
            .allowed_repositories
            .contains(&repo_alias.to_string())
    {
        return Some(ExecutionResult::failure(format!(
            "Repository '{}' is not in the allowed-repositories list: [{}]",
            repo_alias,
            config.allowed_repositories.join(", ")
        )));
    }
    None
}

/// Check the source branch against the configured `allowed-source-branches` list, if any.
fn check_allowed_source_branch(
    config: &CreateBranchConfig,
    source_branch: &str,
) -> Option<ExecutionResult> {
    if !config.allowed_source_branches.is_empty()
        && !config
            .allowed_source_branches
            .contains(&source_branch.to_string())
    {
        return Some(ExecutionResult::failure(format!(
            "Source branch '{}' is not in the allowed-source-branches list",
            source_branch
        )));
    }
    None
}

/// Resolve the commit SHA to branch from: an explicit `source_commit` takes precedence,
/// otherwise the `source_branch` name is resolved via the refs API.
#[allow(clippy::too_many_arguments)]
async fn resolve_source_sha(
    source_commit: Option<&str>,
    source_branch: &str,
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
    repo_locator: &str,
) -> anyhow::Result<String> {
    if let Some(commit) = source_commit {
        debug!("Using explicit source commit: {}", commit);
        return Ok(commit.to_string());
    }
    debug!("Resolving source branch '{}' to commit", source_branch);
    resolve_branch_to_commit(
        client,
        org_url,
        project,
        token,
        connection_type,
        repo_locator,
        source_branch,
    )
    .await
}

/// Build the `refs/heads/...` ref name, leaving an already-qualified name untouched.
fn qualified_ref_name(branch_name: &str) -> String {
    if branch_name.starts_with("refs/heads/") {
        branch_name.to_string()
    } else {
        format!("refs/heads/{}", branch_name)
    }
}

/// Submit the ref-creation request to Azure DevOps and interpret the response, including
/// per-ref errors embedded in an otherwise-successful HTTP response.
#[allow(clippy::too_many_arguments)]
async fn submit_branch_creation(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
    branch_name: &str,
    ref_name: &str,
    source_sha: &str,
    repo_name: &str,
    target: &crate::safe_outputs::AdoRepositoryTarget,
) -> anyhow::Result<ExecutionResult> {
    let ref_updates = serde_json::json!([{
        "name": ref_name,
        "oldObjectId": "0000000000000000000000000000000000000000",
        "newObjectId": source_sha,
    }]);

    info!("Creating branch '{}' from commit {}", ref_name, source_sha);
    let response = crate::safe_outputs::authenticate_ado_request(
        client.post(url).header("Content-Type", "application/json"),
        token,
        connection_type,
    )
    .json(&ref_updates)
    .send()
    .await
    .context("Failed to send request to Azure DevOps")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());

        return Ok(ExecutionResult::failure(format!(
            "Failed to create branch '{}' (HTTP {}): {}",
            branch_name, status, error_body
        )));
    }

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
            branch_name, custom_message
        )));
    }

    info!("Branch '{}' created successfully", branch_name);

    Ok(ExecutionResult::success_with_data(
        format!(
            "Created branch '{}' in repository '{}' from commit {}",
            branch_name,
            repo_name,
            &source_sha[..8]
        ),
        serde_json::json!({
            "branch": branch_name,
            "ref": ref_name,
            "repository": repo_name,
            "source_commit": source_sha,
            "project": target.project,
            "organization": target.organization,
        }),
    ))
}

#[async_trait::async_trait]
impl Executor for CreateBranchResult {
    fn dry_run_summary(&self) -> String {
        format!("create branch '{}'", self.branch_name)
    }

    async fn execute_sanitized(
        &mut self,
        ctx: &ExecutionContext,
    ) -> anyhow::Result<ExecutionResult> {
        self.sanitize_content_fields();
        self.execute_impl(ctx).await
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!("Creating branch: '{}'", self.branch_name);
        debug!("create-branch: branch_name='{}'", self.branch_name);

        let config: CreateBranchConfig = ctx.get_tool_config("create-branch")?;
        debug!("Branch pattern: {:?}", config.branch_pattern);
        debug!("Allowed repositories: {:?}", config.allowed_repositories);
        debug!(
            "Allowed source branches: {:?}",
            config.allowed_source_branches
        );

        if let Some(failure) = check_branch_pattern(&config, &self.branch_name)? {
            return Ok(failure);
        }

        // Determine repository alias
        let repo_alias = self.repository.as_deref().unwrap_or("self");
        if let Some(failure) = check_allowed_repository(&config, repo_alias) {
            return Ok(failure);
        }

        let target =
            match crate::safe_outputs::resolve_repository_write_target(Some(repo_alias), ctx) {
                Ok(target) => target,
                Err(failure) => return Ok(failure),
            };
        let repo_name = target.qualified_repository();
        debug!("Resolved repository target: {}", target.display_name());
        debug!("Resolved repository: {}", repo_name);

        let source_branch = self.source_branch.as_deref().unwrap_or("main");
        if let Some(failure) = check_allowed_source_branch(&config, source_branch) {
            return Ok(failure);
        }

        if ctx.dry_run {
            return Ok(ExecutionResult::success(format!(
                "[DRY-RUN] Would execute: create branch '{}' in {}",
                self.branch_name,
                target.display_name()
            )));
        }

        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;
        let client = reqwest::Client::new();

        let source_sha = resolve_source_sha(
            self.source_commit.as_deref(),
            source_branch,
            &client,
            &target.organization_url,
            &target.project,
            token,
            ctx.write_connection_type,
            target.repository_locator(),
        )
        .await?;
        debug!("Source commit SHA: {}", source_sha);

        let url = format!(
            "{}/{}/_apis/git/repositories/{}/refs?api-version=7.1",
            target.organization_url,
            utf8_percent_encode(&target.project, PATH_SEGMENT),
            utf8_percent_encode(target.repository_locator(), PATH_SEGMENT),
        );
        debug!("API URL: {}", url);

        let ref_name = qualified_ref_name(&self.branch_name);

        submit_branch_creation(
            &client,
            &url,
            token,
            ctx.write_connection_type,
            &self.branch_name,
            &ref_name,
            &source_sha,
            &repo_name,
            &target,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;

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
        assert_eq!(result.source_commit, None);
        assert_eq!(result.repository, None);
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
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("branch_name must not be empty"),
            "unexpected error: {err}"
        );
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
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("branch_name must not contain"),
            "unexpected error: {err}"
        );
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
        let err = result.unwrap_err().to_string();
        assert!(err.contains("source_commit"), "unexpected error: {err}");
    }

    #[test]
    fn test_validation_rejects_repository_pipeline_command() {
        let params = CreateBranchParams {
            branch_name: "feature/new-work".to_string(),
            source_branch: Some("main".to_string()),
            source_commit: None,
            repository: Some("##vso[task.setvariable variable=x]y".to_string()),
        };
        let result: Result<CreateBranchResult, _> = params.try_into();
        let err = result.unwrap_err().to_string();
        assert!(err.contains("repository"), "unexpected error: {err}");
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
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("must not start with '-'"),
            "unexpected error: {err}"
        );
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
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("must not contain spaces"),
            "unexpected error: {err}"
        );
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
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("must be at most 200 characters"),
            "unexpected error: {err}"
        );
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
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("source_branch must not contain '..'"),
            "unexpected error: {err}"
        );
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

    #[test]
    fn test_sanitize_content_neutralizes_repository_pipeline_command() {
        let mut result = CreateBranchResult {
            name: "create-branch".to_string(),
            branch_name: "feature/new-work".to_string(),
            source_branch: Some("main".to_string()),
            source_commit: None,
            repository: Some("##vso[task.setvariable variable=x]y".to_string()),
        };
        result.sanitize_content_fields();
        let repository = result.repository.as_deref().unwrap_or("");
        assert!(
            repository.contains("`##vso[`"),
            "repository pipeline command should be neutralized with backticks: {}",
            repository
        );
    }

    #[tokio::test]
    async fn test_dry_run_resolves_cross_org_target() {
        let mut ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/current-org".to_string()),
            ado_organization: Some("current-org".to_string()),
            ado_project: Some("Current Project".to_string()),
            dry_run: true,
            ..Default::default()
        };
        let allow = vec![crate::compile::types::AdoOrganizationScope {
            organization: crate::secure::AdoOrganization::parse("other-org").unwrap(),
            projects: vec![crate::compile::types::AdoProjectScope {
                project: crate::secure::AdoProject::parse("Other Project").unwrap(),
                project_id: None,
                repositories: vec![crate::secure::AdoRepository::parse("target-repo").unwrap()],
            }],
        }];
        crate::safe_outputs::configure_repository_write_context(
            &mut ctx,
            &["target".to_string()],
            vec![crate::safe_outputs::RepositoryTargetSpec {
                alias: "target".to_string(),
                repo_type: "git".to_string(),
                name: "Other Project/target-repo".to_string(),
                organization: Some("other-org".to_string()),
                endpoint: Some("cross-org-checkout".to_string()),
            }],
            Some(crate::compile::types::WriteConnectionType::AzureDevOps),
            &allow,
        );
        let mut result = CreateBranchResult {
            name: CreateBranchResult::NAME.to_string(),
            branch_name: "feature/cross-org".to_string(),
            source_branch: Some("main".to_string()),
            source_commit: None,
            repository: Some("target".to_string()),
        };

        let execution = result.execute_sanitized(&ctx).await.unwrap();

        assert!(execution.success, "{}", execution.message);
        assert!(
            execution
                .message
                .contains("other-org/Other Project/target-repo"),
            "{}",
            execution.message
        );
    }

    #[tokio::test]
    async fn test_dry_run_enforces_branch_pattern() {
        let mut ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/current-org".to_string()),
            ado_organization: Some("current-org".to_string()),
            ado_project: Some("Current Project".to_string()),
            repository_name: Some("self-repo".to_string()),
            dry_run: true,
            ..Default::default()
        };
        ctx.tool_configs.insert(
            "create-branch".to_string(),
            serde_json::json!({"branch-pattern": "^release/"}),
        );
        let mut result = CreateBranchResult {
            name: CreateBranchResult::NAME.to_string(),
            branch_name: "feature/not-allowed".to_string(),
            source_branch: Some("main".to_string()),
            source_commit: None,
            repository: None,
        };

        let execution = result.execute_sanitized(&ctx).await.unwrap();

        assert!(!execution.success);
        assert!(
            execution
                .message
                .contains("does not match required pattern")
        );
    }
}
