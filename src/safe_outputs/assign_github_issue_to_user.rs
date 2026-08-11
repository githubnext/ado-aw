//! `assign-github-issue-to-user` safe output.

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
pub struct AssignGithubIssueToUserParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// One GitHub username to assign.
    #[serde(default)]
    pub assignee: Option<String>,
    /// Multiple GitHub usernames to assign.
    #[serde(default)]
    pub assignees: Vec<String>,
    /// Optional target repository. Must exactly match `target-repo` or an
    /// `allowed-repos` entry.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for AssignGithubIssueToUserParams {
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
    name = "assign-github-issue-to-user",
    write = true,
    params = AssignGithubIssueToUserParams,
    default_max = 1,
    /// Result of assigning one or more users to a GitHub issue.
    pub struct AssignGithubIssueToUserResult {
        issue_number: GithubIssueNumber,
        #[serde(default)]
        assignee: Option<String>,
        #[serde(default)]
        assignees: Vec<String>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for AssignGithubIssueToUserResult {
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
pub struct AssignGithubIssueToUserConfig {
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
    /// Replace all existing assignees instead of adding to them.
    #[serde(default, rename = "unassign-first")]
    #[sanitize_config(skip)]
    pub unassign_first: bool,
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
    reject_pipeline_injection(assignee, "assign-github-issue-to-user.assignee")
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

pub(crate) fn validate_assign_github_issue_to_user_config(
    config: &AssignGithubIssueToUserConfig,
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

fn repository_assignee_url(
    client: &GithubClient,
    repository: &str,
    assignee: &str,
) -> anyhow::Result<Url> {
    let mut url = client.issues_url(repository)?;
    let mut path = url
        .path_segments_mut()
        .map_err(|_| anyhow::anyhow!("GitHub repository URL cannot be a base URL"))?;
    path.pop().push("assignees").push(assignee);
    drop(path);
    Ok(url)
}

#[async_trait::async_trait]
impl Executor for AssignGithubIssueToUserResult {
    fn dry_run_summary(&self) -> String {
        let assignees =
            normalized_assignees(self.assignee.as_deref(), &self.assignees).unwrap_or_default();
        format!(
            "assign GitHub issue {} to {}",
            self.issue_number,
            assignees.join(", ")
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        const TOOL: &str = "assign-github-issue-to-user";
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
        let config: AssignGithubIssueToUserConfig = ctx.get_tool_config(TOOL)?;
        validate_assign_github_issue_to_user_config(&config)?;
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

        // Resolve the live target and all policy filters before the first write.
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
        for assignee in &assignees {
            let response = client
                .send(
                    Method::GET,
                    repository_assignee_url(&client, &target.repository, assignee)?,
                    None,
                )
                .await?;
            if let Err(error) = response.require_success("Failed to validate GitHub assignee") {
                return Ok(ExecutionResult::failure(error.to_string()));
            }
        }

        let (method, url, operation) = if config.unassign_first {
            (
                Method::PATCH,
                client.issue_url(&target.repository, target.number)?,
                "Failed to replace GitHub issue assignees",
            )
        } else {
            (
                Method::POST,
                issue_assignees_url(&client, &target.repository, target.number)?,
                "Failed to assign GitHub issue users",
            )
        };
        debug!(
            "Assigning users [{}] to {}#{} (unassign-first={})",
            assignees.join(", "),
            target.repository,
            target.number,
            config.unassign_first
        );
        let response = client
            .send(
                method,
                url,
                Some(&serde_json::json!({ "assignees": assignees })),
            )
            .await?;
        if let Err(error) = response.require_success(operation) {
            return Ok(ExecutionResult::failure(error.to_string()));
        }

        info!(
            "Assigned users [{}] to {}#{}",
            assignees.join(", "),
            target.repository,
            target.number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Assigned {}#{} to {}",
                target.repository,
                target.number,
                assignees.join(", ")
            ),
            serde_json::json!({
                "number": target.number,
                "target_repo": target.repository,
                "assignees": assignees,
                "unassign_first": config.unassign_first,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::{ResolvedGithubIssue, ToolResult};
    use crate::secure::GithubTemporaryId;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn contract_validates_singular_plural_and_dedupes() {
        assert_eq!(
            AssignGithubIssueToUserResult::NAME,
            "assign-github-issue-to-user"
        );
        assert_eq!(AssignGithubIssueToUserResult::DEFAULT_MAX, 1);
        assert_eq!(
            normalized_assignees(
                None,
                &[
                    "Octocat".to_string(),
                    "octocat".to_string(),
                    "hubot".to_string()
                ]
            )
            .unwrap(),
            vec!["Octocat".to_string(), "hubot".to_string()]
        );
        assert!(
            AssignGithubIssueToUserParams {
                issue_number: GithubIssueNumber::Number(1),
                assignee: Some("octocat".to_string()),
                assignees: vec![],
                repository: None,
            }
            .validate()
            .is_ok()
        );
        assert!(
            AssignGithubIssueToUserParams {
                issue_number: GithubIssueNumber::Number(1),
                assignee: None,
                assignees: vec![],
                repository: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            AssignGithubIssueToUserParams {
                issue_number: GithubIssueNumber::Number(1),
                assignee: Some("octocat".to_string()),
                assignees: vec!["hubot".to_string()],
                repository: None,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn strict_config_round_trips_policy() {
        let config: AssignGithubIssueToUserConfig = serde_yaml::from_str(
            r#"
target-repo: octo/repo
allowed-repos: [octo/other]
required-labels: [managed]
required-title-prefix: "[agent]"
allowed: ["team-*"]
blocked: ["team-admin"]
unassign-first: true
max: 1
"#,
        )
        .unwrap();
        assert_eq!(config.allowed, vec!["team-*".to_string()]);
        assert_eq!(config.blocked, vec!["team-admin".to_string()]);
        assert!(config.unassign_first);
        assert!(
            serde_yaml::from_str::<AssignGithubIssueToUserConfig>(
                "target-repo: octo/repo\nunexpected: true\n"
            )
            .is_err()
        );
    }

    fn context(server: &MockServer, config: serde_json::Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("assign-github-issue-to-user".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        }
    }

    fn issue(number: u64) -> serde_json::Value {
        serde_json::json!({
            "number": number,
            "node_id": format!("I_{number}"),
            "title": "[agent] tracked",
            "state": "open",
            "labels": [{"name": "managed"}]
        })
    }

    #[tokio::test]
    async fn temporary_target_and_plural_values_add_once_after_deduplication() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue(42)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/42/assignees"))
            .and(body_json(
                serde_json::json!({"assignees": ["Octocat", "hubot"]}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(issue(42)))
            .expect(1)
            .mount(&server)
            .await;
        for assignee in ["Octocat", "hubot"] {
            Mock::given(method("GET"))
                .and(path(format!("/repos/octo/repo/assignees/{assignee}")))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(&server)
                .await;
        }

        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed": ["octo*", "hubot"]
            }),
        );
        let temporary_id = GithubTemporaryId::parse("#aw_created").unwrap();
        ctx.register_resolved_github_issue(
            &temporary_id,
            ResolvedGithubIssue {
                repository: "octo/repo".to_string(),
                number: 42,
                url: "https://github.example/octo/repo/issues/42".to_string(),
            },
        )
        .unwrap();
        let mut result: AssignGithubIssueToUserResult = AssignGithubIssueToUserParams {
            issue_number: GithubIssueNumber::Temporary(temporary_id),
            assignee: None,
            assignees: vec![
                "Octocat".to_string(),
                "octocat".to_string(),
                "hubot".to_string(),
            ],
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(
            execution.data.as_ref().unwrap()["assignees"],
            serde_json::json!(["Octocat", "hubot"])
        );
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
        let mut result: AssignGithubIssueToUserResult = AssignGithubIssueToUserParams {
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
    async fn unassign_first_replaces_assignees_after_filter_preflight() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({"assignees": ["octocat"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/assignees/octocat"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["MANAGED"],
                "required-title-prefix": "[agent]",
                "allowed": ["octo*"],
                "unassign-first": true
            }),
        );
        let mut result: AssignGithubIssueToUserResult = AssignGithubIssueToUserParams {
            issue_number: GithubIssueNumber::Number(7),
            assignee: Some("octocat".to_string()),
            assignees: vec![],
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].method.as_str(), "GET");
        assert_eq!(requests[1].method.as_str(), "GET");
        assert_eq!(requests[2].method.as_str(), "PATCH");
    }

    #[tokio::test]
    async fn failed_filter_performs_no_write() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue(7)))
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
        let mut result: AssignGithubIssueToUserResult = AssignGithubIssueToUserParams {
            issue_number: GithubIssueNumber::Number(7),
            assignee: Some("octocat".to_string()),
            assignees: vec![],
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
        let mut result: AssignGithubIssueToUserResult = AssignGithubIssueToUserParams {
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
