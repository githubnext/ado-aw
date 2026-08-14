//! `remove-github-issue-labels` safe output.

use anyhow::ensure;
use log::{debug, info};
use reqwest::{Method, StatusCode};
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
pub struct RemoveGithubIssueLabelsParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// Labels to remove. At least one label is required.
    pub labels: Vec<String>,
    /// Optional target repository. Must exactly match `target-repo` or an
    /// `allowed-repos` entry.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for RemoveGithubIssueLabelsParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.issue_number.validate("issue_number")?;
        ensure!(!self.labels.is_empty(), "labels must not be empty");
        for label in &self.labels {
            ensure!(!label.is_empty(), "labels entries must not be empty");
            reject_pipeline_injection(label, "remove-github-issue-labels.labels")?;
        }
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "remove-github-issue-labels",
    write = true,
    params = RemoveGithubIssueLabelsParams,
    default_max = 5,
    /// Result of removing labels from a GitHub issue.
    pub struct RemoveGithubIssueLabelsResult {
        issue_number: GithubIssueNumber,
        labels: Vec<String>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for RemoveGithubIssueLabelsResult {
    fn sanitize_content_fields(&mut self) {
        for label in &mut self.labels {
            *label = sanitize_config(label);
        }
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveGithubIssueLabelsConfig {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

pub(crate) fn validate_remove_github_issue_labels_config(
    config: &RemoveGithubIssueLabelsConfig,
) -> anyhow::Result<()> {
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
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
impl Executor for RemoveGithubIssueLabelsResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "remove GitHub labels [{}] from {}",
            self.labels.join(", "),
            target_display(&self.issue_number, self.repository.as_deref())
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        const TOOL: &str = "remove-github-issue-labels";
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
        let config: RemoveGithubIssueLabelsConfig = ctx.get_tool_config(TOOL)?;
        validate_remove_github_issue_labels_config(&config)?;

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
        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let metadata = match client.get_issue(&target.repository, target.number).await? {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        if let Err(result) = validate_github_target_capability(
            &metadata,
            GithubTargetCapabilities::ISSUES_AND_PULL_REQUESTS,
        ) {
            return Ok(result);
        }
        if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
            return Ok(result);
        }

        // Resolve all requested labels against the live target before the first
        // DELETE. This makes absent labels idempotent and prevents a denied
        // later value from leaving a partial mutation.
        let mut present = Vec::new();
        let mut absent = Vec::new();
        for requested in &labels {
            match metadata
                .labels
                .iter()
                .find(|existing| existing.eq_ignore_ascii_case(requested))
            {
                Some(existing) => present.push(existing.clone()),
                None => absent.push(requested.clone()),
            }
        }

        let mut removed = Vec::new();
        for label in present {
            let mut url = client.issue_url(&target.repository, target.number)?;
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("GitHub issue URL cannot be a base URL"))?
                .push("labels")
                .push(&label);
            debug!(
                "DELETEing label '{}' from GitHub issue {}#{}",
                label, target.repository, target.number
            );
            let response = client.send(Method::DELETE, url, None).await?;
            if response.status == StatusCode::NOT_FOUND {
                // A concurrent actor may remove the label after our preflight.
                // The requested end state is already satisfied.
                absent.push(label);
                continue;
            }
            if !response.is_success() {
                let error = response
                    .require_success("Failed to remove GitHub issue label")
                    .expect_err("non-success response must produce an API error");
                return Ok(ExecutionResult::failure(error.to_string()));
            }
            removed.push(label);
        }

        info!(
            "Removed {} label(s) from GitHub issue {}#{}; {} already absent",
            removed.len(),
            target.repository,
            target.number,
            absent.len()
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Removed labels [{}] from {}#{} ({} already absent)",
                removed.join(", "),
                target.repository,
                target.number,
                absent.len()
            ),
            serde_json::json!({
                "number": target.number,
                "target_repo": target.repository,
                "removed_labels": removed,
                "absent_labels": absent,
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn issue_json(number: u64, pull_request: bool) -> serde_json::Value {
        let mut value = serde_json::json!({
            "number": number,
            "node_id": format!("I_{number}"),
            "title": "[agent] Fix the build",
            "state": "open",
            "labels": [{"name": "Managed"}, {"name": "needs triage"}],
            "html_url": format!("https://github.example/octo/repo/issues/{number}")
        });
        if pull_request {
            value["pull_request"] = serde_json::json!({});
        }
        value
    }

    fn context(server: &MockServer, config: serde_json::Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("remove-github-issue-labels".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        }
    }

    fn result(labels: &[&str]) -> RemoveGithubIssueLabelsResult {
        RemoveGithubIssueLabelsParams {
            issue_number: GithubIssueNumber::Number(7),
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            repository: None,
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn params_require_labels_and_support_temporary_ids() {
        assert!(
            RemoveGithubIssueLabelsParams {
                issue_number: GithubIssueNumber::Number(1),
                labels: vec![],
                repository: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            RemoveGithubIssueLabelsParams {
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
        for label in ["", "##vso[task.complete]", "${{ variables.secret }}"] {
            assert!(
                RemoveGithubIssueLabelsParams {
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
        assert_eq!(
            RemoveGithubIssueLabelsResult::NAME,
            "remove-github-issue-labels"
        );
        assert_eq!(RemoveGithubIssueLabelsResult::DEFAULT_MAX, 5);
        assert_eq!(
            result(&["bug"]).dry_run_summary(),
            "remove GitHub labels [bug] from #7"
        );
    }

    #[test]
    fn config_is_strict() {
        let config: RemoveGithubIssueLabelsConfig = serde_yaml::from_str(
            "target-repo: octo/repo\nallowed: ['agent-*']\nblocked: [security]\n",
        )
        .unwrap();
        assert_eq!(config.allowed, vec!["agent-*"]);
        assert!(
            serde_yaml::from_str::<RemoveGithubIssueLabelsConfig>(
                "target-repo: octo/repo\npull-requests: true\n"
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn blocked_wins_case_insensitively_before_http() {
        let server = MockServer::start().await;
        let execution = result(&["security-review"])
            .execute_impl(&context(
                &server,
                serde_json::json!({
                    "target-repo": "octo/repo",
                    "allowed": ["*"],
                    "blocked": ["SECURITY-*"]
                }),
            ))
            .await
            .unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("blocked"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn omitted_allowed_permits_removal_with_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/octo/repo/issues/7/labels/needs%20triage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;
        let execution = result(&["NEEDS TRIAGE"])
            .execute_impl(&context(
                &server,
                serde_json::json!({
                    "target-repo": "octo/repo",
                    "required-labels": ["managed"],
                    "required-title-prefix": "[agent]"
                }),
            ))
            .await
            .unwrap();
        assert!(
            execution.success,
            "unexpected failure: {}",
            execution.message
        );
    }

    #[tokio::test]
    async fn absent_label_is_idempotent_without_delete() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        let execution = result(&["already-absent"])
            .execute_impl(&context(
                &server,
                serde_json::json!({"target-repo": "octo/repo"}),
            ))
            .await
            .unwrap();
        assert!(
            execution.success,
            "unexpected failure: {}",
            execution.message
        );
        assert_eq!(
            execution
                .data
                .as_ref()
                .and_then(|data| data["absent_labels"].as_array())
                .map(Vec::len),
            Some(1)
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method.as_str(), "GET");
    }

    #[tokio::test]
    async fn pull_request_target_allows_label_removal() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, true)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/octo/repo/issues/7/labels/Managed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;
        let execution = result(&["managed"])
            .execute_impl(&context(
                &server,
                serde_json::json!({"target-repo": "octo/repo"}),
            ))
            .await
            .unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn filter_failure_happens_before_any_delete() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        let execution = result(&["managed"])
            .execute_impl(&context(
                &server,
                serde_json::json!({
                    "target-repo": "octo/repo",
                    "required-title-prefix": "[other]"
                }),
            ))
            .await
            .unwrap();
        assert!(!execution.success);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method.as_str(), "GET");
    }

    #[tokio::test]
    async fn delete_not_found_is_idempotent_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/octo/repo/issues/7/labels/Managed"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "message": "Label does not exist"
            })))
            .mount(&server)
            .await;
        let execution = result(&["managed"])
            .execute_impl(&context(
                &server,
                serde_json::json!({"target-repo": "octo/repo"}),
            ))
            .await
            .unwrap();
        assert!(
            execution.success,
            "unexpected failure: {}",
            execution.message
        );
        assert_eq!(
            execution
                .data
                .as_ref()
                .and_then(|data| data["absent_labels"].as_array())
                .map(Vec::len),
            Some(1)
        );
    }

    #[tokio::test]
    async fn temporary_id_resolves_before_removal() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(42, false)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/octo/repo/issues/42/labels/Managed"))
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
        let execution = RemoveGithubIssueLabelsResult {
            name: "remove-github-issue-labels".to_string(),
            issue_number: GithubIssueNumber::Temporary(id),
            labels: vec!["managed".to_string()],
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
        Mock::given(method("DELETE"))
            .and(path("/repos/octo/repo/issues/7/labels/Managed"))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "message": "##vso[task.complete] rejected"
            })))
            .mount(&server)
            .await;
        let execution = result(&["managed"])
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
