//! `add-github-issue-labels` safe output.

use anyhow::ensure;
use log::{debug, info};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubClient, GithubIssueNumber,
    GithubMutationFilters, GithubRepositoryPolicy, GithubTargetCapabilities, Validate,
    merge_github_values, resolve_github_issue_target, validate_blocked_first_globs,
    validate_github_mutation_filter_config, validate_github_mutation_filters,
    validate_github_repository, validate_github_target_capability,
};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

#[derive(Deserialize, JsonSchema)]
pub struct AddGithubIssueLabelsParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// Labels to add. At least one label is required.
    pub labels: Vec<String>,
    /// Optional target repository. Must exactly match `target-repo` or an
    /// `allowed-repos` entry.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for AddGithubIssueLabelsParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.issue_number.validate("issue_number")?;
        ensure!(!self.labels.is_empty(), "labels must not be empty");
        for label in &self.labels {
            ensure!(!label.is_empty(), "labels entries must not be empty");
            reject_pipeline_injection(label, "add-github-issue-labels.labels")?;
        }
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "add-github-issue-labels",
    write = true,
    params = AddGithubIssueLabelsParams,
    default_max = 5,
    /// Result of adding labels to a GitHub issue or permitted pull request.
    pub struct AddGithubIssueLabelsResult {
        issue_number: GithubIssueNumber,
        labels: Vec<String>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for AddGithubIssueLabelsResult {
    fn sanitize_content_fields(&mut self) {
        for label in &mut self.labels {
            *label = sanitize_config(label);
        }
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddGithubIssueLabelsConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    /// Case-insensitive gh-aw-compatible glob allowlist. Empty permits any
    /// label not matched by `blocked`.
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Case-insensitive gh-aw-compatible glob denylist. Evaluated first.
    #[serde(default)]
    pub blocked: Vec<String>,
    /// Permit issue targets.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub issues: bool,
    /// Permit pull-request targets through GitHub's issue-label endpoint.
    #[serde(default, rename = "pull-requests")]
    #[sanitize_config(skip)]
    pub pull_requests: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

impl Default for AddGithubIssueLabelsConfig {
    fn default() -> Self {
        Self {
            target_repo: None,
            allowed_repos: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            allowed: Vec::new(),
            blocked: Vec::new(),
            issues: true,
            pull_requests: false,
            max: None,
        }
    }
}

const fn default_true() -> bool {
    true
}

pub(crate) fn validate_add_github_issue_labels_config(
    config: &AddGithubIssueLabelsConfig,
) -> anyhow::Result<()> {
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
    ensure!(
        config.issues || config.pull_requests,
        "at least one of issues or pull-requests must be true"
    );
    validate_label_policy_config(&config.allowed, &config.blocked)
}

fn validate_label_policy_config(allowed: &[String], blocked: &[String]) -> anyhow::Result<()> {
    for (field, patterns) in [("allowed", allowed), ("blocked", blocked)] {
        for pattern in patterns {
            ensure!(!pattern.is_empty(), "{field} entries must not be empty");
            reject_pipeline_injection(pattern, field)?;
        }
    }
    Ok(())
}

fn target_display(issue_number: &GithubIssueNumber, repository: Option<&str>) -> String {
    match repository {
        Some(repository) => format!("{repository}#{issue_number}"),
        None => format!("#{issue_number}"),
    }
}

#[async_trait::async_trait]
impl Executor for AddGithubIssueLabelsResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "add GitHub labels [{}] to {}",
            self.labels.join(", "),
            target_display(&self.issue_number, self.repository.as_deref())
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        const TOOL: &str = "add-github-issue-labels";
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
        let config: AddGithubIssueLabelsConfig = ctx.get_tool_config(TOOL)?;
        validate_add_github_issue_labels_config(&config)?;

        let labels = merge_github_values(&[], &self.labels);
        if let Err(result) =
            validate_blocked_first_globs(&labels, &config.allowed, &config.blocked, "label")
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
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };
        validate_github_mutation_filter_config(filters)?;

        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let metadata = match client.get_issue(&target.repository, target.number).await? {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        if let Err(result) = validate_github_target_capability(
            &metadata,
            GithubTargetCapabilities {
                issues: config.issues,
                pull_requests: config.pull_requests,
            },
        ) {
            return Ok(result);
        }
        if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
            return Ok(result);
        }

        let mut url = client.issue_url(&target.repository, target.number)?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("GitHub issue URL cannot be a base URL"))?
            .push("labels");
        debug!(
            "POSTing {} label(s) to GitHub target {}#{}",
            labels.len(),
            target.repository,
            target.number
        );
        let response = client
            .send(
                Method::POST,
                url,
                Some(&serde_json::json!({ "labels": labels })),
            )
            .await?;
        if !response.is_success() {
            let error = response
                .require_success("Failed to add GitHub issue labels")
                .expect_err("non-success response must produce an API error");
            return Ok(ExecutionResult::failure(error.to_string()));
        }

        info!(
            "Added {} label(s) to GitHub target {}#{}",
            labels.len(),
            target.repository,
            target.number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Added labels [{}] to {}#{}",
                labels.join(", "),
                target.repository,
                target.number
            ),
            serde_json::json!({
                "number": target.number,
                "target_repo": target.repository,
                "labels": labels,
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

    fn issue_json(number: u64, pull_request: bool) -> serde_json::Value {
        let mut value = serde_json::json!({
            "number": number,
            "node_id": format!("I_{number}"),
            "title": "[agent] Fix the build",
            "state": "open",
            "labels": [{"name": "managed"}, {"name": "bug"}],
            "html_url": format!("https://github.example/octo/repo/issues/{number}")
        });
        if pull_request {
            value["pull_request"] = serde_json::json!({});
        }
        value
    }

    fn context(server: &MockServer, config: serde_json::Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("add-github-issue-labels".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        }
    }

    fn result(labels: &[&str]) -> AddGithubIssueLabelsResult {
        AddGithubIssueLabelsParams {
            issue_number: GithubIssueNumber::Number(7),
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            repository: None,
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn params_require_labels_and_validate_targets() {
        assert!(
            AddGithubIssueLabelsParams {
                issue_number: GithubIssueNumber::Number(0),
                labels: vec!["bug".to_string()],
                repository: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            AddGithubIssueLabelsParams {
                issue_number: GithubIssueNumber::Number(1),
                labels: vec![],
                repository: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            AddGithubIssueLabelsParams {
                issue_number: GithubIssueNumber::Temporary(
                    GithubTemporaryId::parse("#aw_labels").unwrap()
                ),
                labels: vec!["triage".to_string()],
                repository: Some("octo/repo".to_string()),
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn params_reject_empty_or_injecting_labels() {
        for label in ["", "##vso[task.complete]", "$(TOKEN)"] {
            assert!(
                AddGithubIssueLabelsParams {
                    issue_number: GithubIssueNumber::Number(1),
                    labels: vec![label.to_string()],
                    repository: None,
                }
                .validate()
                .is_err(),
                "label should be rejected: {label}"
            );
        }
    }

    #[test]
    fn result_contract_and_dry_run_summary() {
        assert_eq!(AddGithubIssueLabelsResult::NAME, "add-github-issue-labels");
        assert_eq!(AddGithubIssueLabelsResult::DEFAULT_MAX, 5);
        assert_eq!(
            result(&["bug", "triage"]).dry_run_summary(),
            "add GitHub labels [bug, triage] to #7"
        );
    }

    #[test]
    fn config_is_strict_and_defaults_to_issues_only() {
        let config: AddGithubIssueLabelsConfig = serde_yaml::from_str(
            "target-repo: octo/repo\nallowed: ['agent-*']\nblocked: [security]\n",
        )
        .unwrap();
        assert!(config.issues);
        assert!(!config.pull_requests);
        assert!(
            serde_yaml::from_str::<AddGithubIssueLabelsConfig>(
                "target-repo: octo/repo\nunexpected: true\n"
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn blocked_wins_case_insensitively_before_http() {
        let server = MockServer::start().await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed": ["*"],
                "blocked": ["SECURITY-*"]
            }),
        );
        let execution = result(&["security-review"])
            .execute_impl(&ctx)
            .await
            .unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("blocked"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn omitted_allowed_permits_label_and_preflights_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/labels"))
            .and(body_json(serde_json::json!({"labels": ["Needs-Triage"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["MANAGED"],
                "required-title-prefix": "[agent]"
            }),
        );
        let execution = result(&["Needs-Triage"]).execute_impl(&ctx).await.unwrap();
        assert!(
            execution.success,
            "unexpected failure: {}",
            execution.message
        );
    }

    #[tokio::test]
    async fn all_policy_checks_finish_before_label_write() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
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
        let execution = result(&["triage"]).execute_impl(&ctx).await.unwrap();
        assert!(!execution.success);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method.as_str(), "GET");
    }

    #[tokio::test]
    async fn pull_requests_require_explicit_permission() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, true)))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/labels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let denied = result(&["triage"])
            .execute_impl(&context(
                &server,
                serde_json::json!({"target-repo": "octo/repo"}),
            ))
            .await
            .unwrap();
        assert!(!denied.success);
        assert!(denied.message.contains("pull requests"));

        let allowed = result(&["triage"])
            .execute_impl(&context(
                &server,
                serde_json::json!({
                    "target-repo": "octo/repo",
                    "pull-requests": true
                }),
            ))
            .await
            .unwrap();
        assert!(allowed.success, "unexpected failure: {}", allowed.message);
    }

    #[tokio::test]
    async fn temporary_id_uses_resolved_repository_and_number() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(42, false)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/42/labels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        let id = GithubTemporaryId::parse("#aw_labels").unwrap();
        ctx.register_resolved_github_issue(
            &id,
            ResolvedGithubIssue {
                repository: "octo/repo".to_string(),
                number: 42,
                url: "https://github.example/octo/repo/issues/42".to_string(),
            },
        )
        .unwrap();
        let execution = AddGithubIssueLabelsResult {
            name: "add-github-issue-labels".to_string(),
            issue_number: GithubIssueNumber::Temporary(id),
            labels: vec!["triage".to_string()],
            repository: None,
        }
        .execute_impl(&ctx)
        .await
        .unwrap();
        assert!(
            execution.success,
            "unexpected failure: {}",
            execution.message
        );
    }

    #[tokio::test]
    async fn github_failure_is_neutralized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/labels"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "message": "##vso[task.complete] rejected"
            })))
            .mount(&server)
            .await;
        let execution = result(&["triage"])
            .execute_impl(&context(
                &server,
                serde_json::json!({"target-repo": "octo/repo"}),
            ))
            .await
            .unwrap();
        assert!(!execution.success);
        assert!(
            !execution
                .message
                .lines()
                .any(|line| line.starts_with("##vso["))
        );
        assert!(execution.message.contains("`##vso[`"));
    }
}
