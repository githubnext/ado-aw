//! `unassign-github-issue-from-user` safe output.

use anyhow::ensure;
use log::{debug, info};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubClient, GithubIssueNumber,
    GithubMutationFilters, GithubRepositoryPolicy, GithubTargetCapabilities, Validate,
    resolve_github_issue_target, validate_blocked_first_globs,
    validate_github_mutation_filter_config, validate_github_mutation_filters,
    validate_github_repository, validate_github_target_capability,
};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

const MAX_ASSIGNEE_LEN: usize = 100;
const MAX_ASSIGNEES: usize = 100;

#[derive(Deserialize, JsonSchema)]
pub struct UnassignGithubIssueFromUserParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// One GitHub username to remove.
    #[serde(default)]
    pub assignee: Option<String>,
    /// Multiple GitHub usernames to remove.
    #[serde(default)]
    pub assignees: Vec<String>,
    /// Optional target repository. Must exactly match `target-repo` or an
    /// `allowed-repos` entry.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for UnassignGithubIssueFromUserParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.issue_number.validate("issue_number")?;
        normalized_assignees(self.assignee.as_deref(), &self.assignees)?;
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "unassign-github-issue-from-user",
    write = true,
    params = UnassignGithubIssueFromUserParams,
    default_max = 1,
    /// Result of removing one or more users from a GitHub issue.
    pub struct UnassignGithubIssueFromUserResult {
        issue_number: GithubIssueNumber,
        #[serde(default)]
        assignee: Option<String>,
        #[serde(default)]
        assignees: Vec<String>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for UnassignGithubIssueFromUserResult {
    fn sanitize_content_fields(&mut self) {
        self.assignee = self.assignee.as_deref().map(sanitize_config);
        self.assignees = self
            .assignees
            .iter()
            .map(|assignee| sanitize_config(assignee))
            .collect();
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnassignGithubIssueFromUserConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    /// Case-insensitive `*` glob allowlist. Empty permits any non-blocked user.
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Case-insensitive `*` glob blocklist, evaluated before `allowed`.
    #[serde(default)]
    pub blocked: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

fn validate_assignee(assignee: &str) -> anyhow::Result<()> {
    ensure!(!assignee.trim().is_empty(), "assignee must not be empty");
    ensure!(
        assignee.len() <= MAX_ASSIGNEE_LEN,
        "assignee must be {MAX_ASSIGNEE_LEN} characters or fewer"
    );
    reject_pipeline_injection(assignee, "unassign-github-issue-from-user.assignee")
}

fn normalized_assignees(
    assignee: Option<&str>,
    assignees: &[String],
) -> anyhow::Result<Vec<String>> {
    ensure!(
        assignee.is_none() || assignees.is_empty(),
        "provide assignee or assignees, not both"
    );
    ensure!(
        assignee.is_some() || !assignees.is_empty(),
        "assignee or assignees must be provided"
    );
    ensure!(
        assignees.len() <= MAX_ASSIGNEES,
        "assignees must contain at most {MAX_ASSIGNEES} entries"
    );
    let candidates: Vec<&str> = match assignee {
        Some(value) => vec![value],
        None => assignees.iter().map(String::as_str).collect(),
    };
    let mut normalized = Vec::new();
    for candidate in candidates {
        validate_assignee(candidate)?;
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(candidate))
        {
            normalized.push(candidate.to_string());
        }
    }
    Ok(normalized)
}

pub(crate) fn validate_unassign_github_issue_from_user_config(
    config: &UnassignGithubIssueFromUserConfig,
) -> anyhow::Result<()> {
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
    for (field, patterns) in [
        ("allowed", config.allowed.as_slice()),
        ("blocked", config.blocked.as_slice()),
    ] {
        for pattern in patterns {
            ensure!(!pattern.is_empty(), "{field} entries must not be empty");
            reject_pipeline_injection(pattern, field)?;
        }
    }
    Ok(())
}

fn issue_assignees_url(
    client: &GithubClient,
    repository: &str,
    number: u64,
) -> anyhow::Result<Url> {
    let mut url = client.issue_url(repository, number)?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("GitHub issue URL cannot be a base URL"))?
        .push("assignees");
    Ok(url)
}

#[async_trait::async_trait]
impl Executor for UnassignGithubIssueFromUserResult {
    fn dry_run_summary(&self) -> String {
        let assignees =
            normalized_assignees(self.assignee.as_deref(), &self.assignees).unwrap_or_default();
        format!(
            "remove {} from GitHub issue {}",
            assignees.join(", "),
            self.issue_number
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        const TOOL: &str = "unassign-github-issue-from-user";
        if !ctx.tool_configs.contains_key(TOOL) {
            return Ok(ExecutionResult::failure(format!(
                "{TOOL} is not configured for this workflow"
            )));
        }
        let token = match ctx.github_token.as_ref() {
            Some(token) => token,
            None => {
                return Ok(ExecutionResult::failure(
                    "ADO_AW_GITHUB_TOKEN is not set; configure safe-outputs.github-token \
                     or safe-outputs.github-app",
                ));
            }
        };
        let config: UnassignGithubIssueFromUserConfig = ctx.get_tool_config(TOOL)?;
        validate_unassign_github_issue_from_user_config(&config)?;
        let assignees = normalized_assignees(self.assignee.as_deref(), &self.assignees)?;
        if let Err(result) =
            validate_blocked_first_globs(&assignees, &config.allowed, &config.blocked, "assignee")
        {
            return Ok(result);
        }

        let target = match resolve_github_issue_target(
            &self.issue_number,
            self.repository.as_deref(),
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos),
            ctx,
        )? {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };

        // Resolve the live target and all policy filters before the write.
        let metadata = match client.get_issue(&target.repository, target.number).await? {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        if let Err(result) =
            validate_github_target_capability(&metadata, GithubTargetCapabilities::ISSUES_ONLY)
        {
            return Ok(result);
        }
        if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
            return Ok(result);
        }

        debug!(
            "Removing users [{}] from {}#{}",
            assignees.join(", "),
            target.repository,
            target.number
        );
        let response = client
            .send(
                Method::DELETE,
                issue_assignees_url(&client, &target.repository, target.number)?,
                Some(&serde_json::json!({ "assignees": assignees })),
            )
            .await?;
        if let Err(error) = response.require_success("Failed to remove GitHub issue assignees") {
            return Ok(ExecutionResult::failure(error.to_string()));
        }

        // GitHub ignores requested users who were already absent, making the
        // operation safely idempotent.
        info!(
            "Removed users [{}] from {}#{}",
            assignees.join(", "),
            target.repository,
            target.number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Removed {} from {}#{}",
                assignees.join(", "),
                target.repository,
                target.number
            ),
            serde_json::json!({
                "number": target.number,
                "target_repo": target.repository,
                "assignees": assignees,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;
    use crate::secure::GithubTemporaryId;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn contract_validates_numeric_temporary_singular_and_plural() {
        assert_eq!(
            UnassignGithubIssueFromUserResult::NAME,
            "unassign-github-issue-from-user"
        );
        assert_eq!(UnassignGithubIssueFromUserResult::DEFAULT_MAX, 1);
        for issue_number in [
            GithubIssueNumber::Number(7),
            GithubIssueNumber::Temporary(GithubTemporaryId::parse("#aw_created").unwrap()),
        ] {
            assert!(
                UnassignGithubIssueFromUserParams {
                    issue_number,
                    assignee: Some("octocat".to_string()),
                    assignees: vec![],
                    repository: Some("octo/repo".to_string()),
                }
                .validate()
                .is_ok()
            );
        }
        assert_eq!(
            normalized_assignees(None, &["Octocat".to_string(), "octocat".to_string()]).unwrap(),
            vec!["Octocat".to_string()]
        );
        assert!(
            UnassignGithubIssueFromUserParams {
                issue_number: GithubIssueNumber::Number(7),
                assignee: None,
                assignees: vec![],
                repository: None,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn strict_config_round_trips_policy() {
        let config: UnassignGithubIssueFromUserConfig = serde_yaml::from_str(
            r#"
target-repo: octo/repo
allowed-repos: [octo/other]
required-labels: [managed]
required-title-prefix: "[agent]"
allowed: ["team-*"]
blocked: ["team-admin"]
max: 1
"#,
        )
        .unwrap();
        assert_eq!(config.allowed, vec!["team-*".to_string()]);
        assert_eq!(config.blocked, vec!["team-admin".to_string()]);
        assert!(
            serde_yaml::from_str::<UnassignGithubIssueFromUserConfig>(
                "target-repo: octo/repo\nunexpected: true\n"
            )
            .is_err()
        );
    }

    fn context(server: &MockServer, config: serde_json::Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("unassign-github-issue-from-user".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        }
    }

    fn issue() -> serde_json::Value {
        serde_json::json!({
            "number": 7,
            "node_id": "I_7",
            "title": "[agent] tracked",
            "state": "open",
            "labels": [{"name": "managed"}],
            "assignees": []
        })
    }

    #[tokio::test]
    async fn already_absent_assignee_is_idempotent_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/octo/repo/issues/7/assignees"))
            .and(body_json(serde_json::json!({"assignees": ["octocat"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue()))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed": ["octo*"]
            }),
        );
        let mut result: UnassignGithubIssueFromUserResult = UnassignGithubIssueFromUserParams {
            issue_number: GithubIssueNumber::Number(7),
            assignee: Some("octocat".to_string()),
            assignees: vec![],
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
    }

    #[tokio::test]
    async fn blocked_policy_wins_before_any_http_request() {
        let server = MockServer::start().await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed": ["team-*"],
                "blocked": ["TEAM-ADMIN"]
            }),
        );
        let mut result: UnassignGithubIssueFromUserResult = UnassignGithubIssueFromUserParams {
            issue_number: GithubIssueNumber::Number(7),
            assignee: Some("team-admin".to_string()),
            assignees: vec![],
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("blocked"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn filter_failure_prevents_delete() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue()))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["missing"]
            }),
        );
        let mut result: UnassignGithubIssueFromUserResult = UnassignGithubIssueFromUserParams {
            issue_number: GithubIssueNumber::Number(7),
            assignee: None,
            assignees: vec!["octocat".to_string(), "hubot".to_string()],
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method.as_str(), "GET");
    }

    #[tokio::test]
    async fn dry_run_skips_configuration_and_network() {
        let ctx = ExecutionContext {
            dry_run: true,
            ..Default::default()
        };
        let mut result: UnassignGithubIssueFromUserResult = UnassignGithubIssueFromUserParams {
            issue_number: GithubIssueNumber::Number(7),
            assignee: Some("octocat".to_string()),
            assignees: vec![],
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success);
        assert!(execution.message.contains("[DRY-RUN]"));
        assert!(execution.message.contains("octocat"));
    }
}
