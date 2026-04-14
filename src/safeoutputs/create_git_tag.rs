//! Create git tag safe output tool

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PATH_SEGMENT, validate_git_ref_name};
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use anyhow::{Context, ensure};

/// Parameters for creating a git tag (agent-provided)
#[derive(Deserialize, JsonSchema)]
pub struct CreateGitTagParams {
    /// Tag name (e.g., "v1.2.3"). Must be 3-100 characters, alphanumeric
    /// plus dots, dashes, underscores, and slashes only.
    pub tag_name: String,

    /// Commit SHA to tag. If omitted, the executor resolves HEAD of the
    /// default branch. Must be a valid 40-character hex string if present.
    pub commit: Option<String>,

    /// Tag annotation message. Must be at least 5 characters if present.
    pub message: Option<String>,

    /// Repository alias: "self" for the pipeline repo, or an alias from
    /// the `checkout:` list. Defaults to "self".
    pub repository: Option<String>,
}

/// Regex pattern for valid tag names: alphanumeric, dots, dashes, underscores, slashes.
static TAG_NAME_PATTERN: std::sync::LazyLock<regex_lite::Regex> =
    std::sync::LazyLock::new(|| regex_lite::Regex::new(r"^[a-zA-Z0-9._/\-]+$").unwrap());

impl Validate for CreateGitTagParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.tag_name.starts_with('-'),
            "tag_name must not start with '-'"
        );
        ensure!(
            self.tag_name.len() >= 3,
            "tag_name must be at least 3 characters"
        );
        ensure!(
            self.tag_name.len() <= 100,
            "tag_name must be at most 100 characters"
        );
        ensure!(
            TAG_NAME_PATTERN.is_match(&self.tag_name),
            "tag_name contains invalid characters (only alphanumeric, dots, dashes, underscores, and slashes are allowed): {}",
            self.tag_name
        );
        validate_git_ref_name(&self.tag_name, "tag_name")?;

        if let Some(commit) = &self.commit {
            ensure!(
                commit.len() == 40,
                "commit must be exactly 40 hex characters, got {} characters",
                commit.len()
            );
            ensure!(
                commit.chars().all(|c| c.is_ascii_hexdigit()),
                "commit must be a valid hex string: {}",
                commit
            );
        }

        if let Some(message) = &self.message {
            ensure!(
                message.len() >= 5,
                "message must be at least 5 characters"
            );
        }

        Ok(())
    }
}

tool_result! {
    name = "create-git-tag",
    write = true,
    params = CreateGitTagParams,
    /// Result of creating a git tag
    pub struct CreateGitTagResult {
        tag_name: String,
        commit: Option<String>,
        message: Option<String>,
        repository: Option<String>,
    }
}

impl Sanitize for CreateGitTagResult {
    fn sanitize_fields(&mut self) {
        // tag_name is a structural identifier — only strip control characters
        self.tag_name = self
            .tag_name
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        self.message = self.message.as_ref().map(|m| sanitize_text(m));
        // commit and repository are structural identifiers; strip control chars only
        self.commit = self.commit.as_ref().map(|c| {
            c.chars().filter(|ch| !ch.is_control()).collect()
        });
        self.repository = self.repository.as_ref().map(|r| {
            r.chars().filter(|ch| !ch.is_control()).collect()
        });
    }
}

/// Configuration for the create-git-tag tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   create-git-tag:
///     tag-pattern: "^v\\d+\\.\\d+\\.\\d+$"
///     allowed-repositories:
///       - self
///       - my-lib
///     message-prefix: "[release] "
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGitTagConfig {
    /// Regex pattern that tag names must match (if configured)
    #[serde(default, rename = "tag-pattern")]
    pub tag_pattern: Option<String>,

    /// Repositories the agent is allowed to tag (if empty, all allowed)
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,

    /// Prefix prepended to the tag message
    #[serde(default, rename = "message-prefix")]
    pub message_prefix: Option<String>,
}

impl Default for CreateGitTagConfig {
    fn default() -> Self {
        Self {
            tag_pattern: None,
            allowed_repositories: Vec::new(),
            message_prefix: None,
        }
    }
}

/// Resolve HEAD commit for a repository by querying the repository's default branch.
async fn resolve_head_commit(
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    token: &str,
    repo_name: &str,
) -> anyhow::Result<String> {
    // First, discover the default branch from the repository metadata
    let repo_url = format!(
        "{}/{}/_apis/git/repositories/{}?api-version=7.1",
        org_url.trim_end_matches('/'),
        utf8_percent_encode(project, PATH_SEGMENT),
        utf8_percent_encode(repo_name, PATH_SEGMENT),
    );
    debug!("Fetching repository metadata: {}", repo_url);

    let repo_response = client
        .get(&repo_url)
        .basic_auth("", Some(token))
        .send()
        .await
        .context("Failed to query repository metadata")?;

    ensure!(
        repo_response.status().is_success(),
        "Failed to fetch repository metadata (HTTP {})",
        repo_response.status()
    );

    let repo_body: serde_json::Value = repo_response
        .json()
        .await
        .context("Failed to parse repository metadata")?;

    let default_branch = repo_body
        .get("defaultBranch")
        .and_then(|v| v.as_str())
        .unwrap_or("refs/heads/main");

    // Strip "refs/heads/" prefix for the filter parameter
    let branch_filter = default_branch
        .strip_prefix("refs/")
        .unwrap_or(default_branch);
    debug!("Default branch: {} (filter: {})", default_branch, branch_filter);

    // Now resolve the HEAD commit of the default branch
    let url = format!(
        "{}/{}/_apis/git/repositories/{}/refs?filter={}&api-version=7.1",
        org_url.trim_end_matches('/'),
        utf8_percent_encode(project, PATH_SEGMENT),
        utf8_percent_encode(repo_name, PATH_SEGMENT),
        branch_filter,
    );

    debug!("Resolving HEAD commit via: {}", url);

    let response = client
        .get(&url)
        .basic_auth("", Some(token))
        .send()
        .await
        .context("Failed to query refs for HEAD resolution")?;

    ensure!(
        response.status().is_success(),
        "Failed to resolve HEAD (HTTP {})",
        response.status()
    );

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
            "No refs found for {} — cannot resolve HEAD commit",
            default_branch
        ))
}

#[async_trait::async_trait]
impl Executor for CreateGitTagResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!("Creating git tag: '{}'", self.tag_name);

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

        let config: CreateGitTagConfig = ctx.get_tool_config("create-git-tag");
        debug!("Tag pattern: {:?}", config.tag_pattern);
        debug!("Allowed repositories: {:?}", config.allowed_repositories);

        // Validate tag against configured pattern
        if let Some(pattern) = &config.tag_pattern {
            let re = regex_lite::Regex::new(pattern).context(format!(
                "Invalid tag-pattern regex in config: {}",
                pattern
            ))?;
            if !re.is_match(&self.tag_name) {
                return Ok(ExecutionResult::failure(format!(
                    "Tag name '{}' does not match required pattern '{}'",
                    self.tag_name, pattern
                )));
            }
        }

        // Resolve repository
        let repo_alias = self.repository.as_deref().unwrap_or("self");

        // Validate repository against config policy BEFORE resolving the name,
        // so operators see a policy error rather than a confusing resolution error.
        if !config.allowed_repositories.is_empty()
            && !config.allowed_repositories.contains(&repo_alias.to_string())
        {
            return Ok(ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed-repositories list: [{}]",
                repo_alias,
                config.allowed_repositories.join(", ")
            )));
        }

        let repo_name = if repo_alias == "self" {
            ctx.repository_name
                .as_deref()
                .context("BUILD_REPOSITORY_NAME not set and repository is 'self'")?
                .to_string()
        } else {
            ctx.allowed_repositories
                .get(repo_alias)
                .cloned()
                .context(format!(
                    "Repository alias '{}' not found in allowed repositories",
                    repo_alias
                ))?
        };

        let client = reqwest::Client::new();

        // Resolve commit SHA — use provided value or look up HEAD
        let commit_sha = match &self.commit {
            Some(sha) => sha.clone(),
            None => {
                info!("No commit specified, resolving HEAD of default branch");
                resolve_head_commit(&client, org_url, project, token, &repo_name).await?
            }
        };
        debug!("Tagging commit: {}", commit_sha);

        // Build tag message with optional prefix
        let tag_message = match (&config.message_prefix, &self.message) {
            (Some(prefix), Some(msg)) => format!("{}{}", prefix, msg),
            (Some(prefix), None) => format!("{}Tag {}", prefix, self.tag_name),
            (None, Some(msg)) => msg.clone(),
            (None, None) => format!("Tag {}", self.tag_name),
        };

        // POST annotated tag
        let url = format!(
            "{}/{}/_apis/git/repositories/{}/annotatedtags?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            utf8_percent_encode(&repo_name, PATH_SEGMENT),
        );
        debug!("API URL: {}", url);

        let body = serde_json::json!({
            "name": self.tag_name,
            "taggedObject": {
                "objectId": commit_sha
            },
            "message": tag_message
        });

        info!("Sending annotated tag creation request to ADO");
        let response = client
            .post(&url)
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

            let tag_url = resp_body
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            info!("Tag created: {} -> {}", self.tag_name, commit_sha);

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Created tag '{}' on commit {} in repository '{}'",
                    self.tag_name, commit_sha, repo_name
                ),
                serde_json::json!({
                    "tag": self.tag_name,
                    "commit": commit_sha,
                    "repository": repo_name,
                    "url": tag_url,
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            Ok(ExecutionResult::failure(format!(
                "Failed to create tag (HTTP {}): {}",
                status, error_body
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
        assert_eq!(CreateGitTagResult::NAME, "create-git-tag");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"tag_name": "v1.2.3", "commit": "abcdef1234567890abcdef1234567890abcdef12", "message": "Release v1.2.3"}"#;
        let params: CreateGitTagParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.tag_name, "v1.2.3");
        assert_eq!(
            params.commit.as_deref(),
            Some("abcdef1234567890abcdef1234567890abcdef12")
        );
        assert_eq!(params.message.as_deref(), Some("Release v1.2.3"));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = CreateGitTagParams {
            tag_name: "v1.0.0".to_string(),
            commit: Some("abcdef1234567890abcdef1234567890abcdef12".to_string()),
            message: Some("Initial release".to_string()),
            repository: None,
        };
        let result: CreateGitTagResult = params.try_into().unwrap();
        assert_eq!(result.name, "create-git-tag");
        assert_eq!(result.tag_name, "v1.0.0");
    }

    #[test]
    fn test_validation_rejects_empty_tag() {
        let params = CreateGitTagParams {
            tag_name: "ab".to_string(),
            commit: None,
            message: None,
            repository: None,
        };
        let result: Result<CreateGitTagResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_invalid_tag_chars() {
        let params = CreateGitTagParams {
            tag_name: "v1.0 invalid!".to_string(),
            commit: None,
            message: None,
            repository: None,
        };
        let result: Result<CreateGitTagResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_invalid_commit_sha() {
        let params = CreateGitTagParams {
            tag_name: "v1.0.0".to_string(),
            commit: Some("not-a-valid-sha".to_string()),
            message: None,
            repository: None,
        };
        let result: Result<CreateGitTagResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_short_message() {
        let params = CreateGitTagParams {
            tag_name: "v1.0.0".to_string(),
            commit: None,
            message: Some("Hi".to_string()),
            repository: None,
        };
        let result: Result<CreateGitTagResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = CreateGitTagParams {
            tag_name: "v2.0.0".to_string(),
            commit: Some("abcdef1234567890abcdef1234567890abcdef12".to_string()),
            message: Some("Major release".to_string()),
            repository: None,
        };
        let result: CreateGitTagResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"create-git-tag""#));
        assert!(json.contains(r#""tag_name":"v2.0.0""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = CreateGitTagConfig::default();
        assert!(config.tag_pattern.is_none());
        assert!(config.allowed_repositories.is_empty());
        assert!(config.message_prefix.is_none());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
tag-pattern: "^v\\d+\\.\\d+\\.\\d+$"
allowed-repositories:
  - self
  - my-lib
message-prefix: "[release] "
"#;
        let config: CreateGitTagConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.tag_pattern.as_deref(),
            Some("^v\\d+\\.\\d+\\.\\d+$")
        );
        assert_eq!(config.allowed_repositories, vec!["self", "my-lib"]);
        assert_eq!(config.message_prefix.as_deref(), Some("[release] "));
    }
}
