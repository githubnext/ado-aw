//! `close-pull-request` safe output.

use anyhow::ensure;
use log::{info, warn};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubClient, GithubMutationFilters,
    GithubRepositoryPolicy, GithubTargetCapabilities, Validate, resolve_github_repository,
    validate_github_mutation_filter_config, validate_github_mutation_filters,
    validate_github_repository, validate_github_target_capability,
};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;
use ado_aw_derive::SanitizeConfig;

const MAX_COMMENT_LEN: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosePullRequestTarget {
    Triggering,
    Any,
    Number(u64),
}

impl Default for ClosePullRequestTarget {
    fn default() -> Self {
        Self::Triggering
    }
}

impl Serialize for ClosePullRequestTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Triggering => serializer.serialize_str("triggering"),
            Self::Any => serializer.serialize_str("*"),
            Self::Number(number) => serializer.serialize_u64(*number),
        }
    }
}

impl<'de> Deserialize<'de> for ClosePullRequestTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = ClosePullRequestTarget;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(r#""triggering", "*", or a positive pull request number"#)
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value == 0 {
                    return Err(E::custom("target pull request number must be positive"));
                }
                Ok(ClosePullRequestTarget::Number(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value <= 0 {
                    return Err(E::custom("target pull request number must be positive"));
                }
                Ok(ClosePullRequestTarget::Number(value as u64))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "triggering" => Ok(ClosePullRequestTarget::Triggering),
                    "*" => Ok(ClosePullRequestTarget::Any),
                    other => {
                        let number = other
                            .parse::<u64>()
                            .map_err(|_| E::custom("target must be \"triggering\", \"*\", or a positive pull request number"))?;
                        if number == 0 {
                            return Err(E::custom("target pull request number must be positive"));
                        }
                        Ok(ClosePullRequestTarget::Number(number))
                    }
                }
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ClosePullRequestParams {
    /// Positive GitHub pull request number. Required when config target is "*".
    #[serde(default)]
    pub pull_request_number: Option<u64>,
    /// Optional closing comment.
    #[serde(default)]
    pub body: Option<String>,
    /// Optional target repository.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for ClosePullRequestParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(number) = self.pull_request_number {
            ensure!(number > 0, "pull_request_number must be positive");
        }
        if let Some(body) = self.body.as_deref() {
            ensure!(!body.trim().is_empty(), "body must not be empty");
            ensure!(
                body.len() <= MAX_COMMENT_LEN,
                "body must be {MAX_COMMENT_LEN} characters or fewer"
            );
        }
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "close-pull-request",
    write = true,
    params = ClosePullRequestParams,
    default_max = 1,
    /// Result of closing a GitHub pull request.
    pub struct ClosePullRequestResult {
        #[serde(default)]
        pull_request_number: Option<u64>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for ClosePullRequestResult {
    fn sanitize_content_fields(&mut self) {
        self.body = self.body.as_deref().map(sanitize_text);
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosePullRequestConfig {
    #[serde(default)]
    #[sanitize_config(skip)]
    pub target: ClosePullRequestTarget,
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

impl Default for ClosePullRequestConfig {
    fn default() -> Self {
        Self {
            target: ClosePullRequestTarget::Triggering,
            target_repo: None,
            allowed_repos: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            max: None,
        }
    }
}

pub(crate) fn validate_close_pull_request_config(
    config: &ClosePullRequestConfig,
) -> anyhow::Result<()> {
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
    Ok(())
}

fn parse_positive(value: Option<&str>) -> Option<u64> {
    value.and_then(|value| value.parse::<u64>().ok())
        .filter(|number| *number > 0)
}

impl ClosePullRequestResult {
    fn resolve_target_number(
        &self,
        config: &ClosePullRequestConfig,
        ctx: &ExecutionContext,
    ) -> Result<u64, ExecutionResult> {
        match config.target {
            ClosePullRequestTarget::Number(number) => Ok(number),
            ClosePullRequestTarget::Any => self.pull_request_number.ok_or_else(|| {
                ExecutionResult::failure(
                    "pull_request_number is required when safe-outputs.close-pull-request.target is '*'",
                )
            }),
            ClosePullRequestTarget::Triggering => parse_positive(ctx.pull_request_number.as_deref())
                .or_else(|| parse_positive(ctx.pull_request_id.as_deref()))
                .ok_or_else(|| {
                    ExecutionResult::failure(
                        "safe-outputs.close-pull-request.target is 'triggering' but no GitHub pull request context is available; use target: '*' and pass pull_request_number, or configure a numeric target",
                    )
                }),
        }
    }

    fn resolve_repository(
        &self,
        config: &ClosePullRequestConfig,
        ctx: &ExecutionContext,
    ) -> Result<String, ExecutionResult> {
        resolve_github_repository(
            self.repository.as_deref(),
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos),
            ctx,
        )
    }
}

async fn post_comment(
    client: &GithubClient,
    repository: &str,
    number: u64,
    body: Option<&str>,
) -> anyhow::Result<Result<bool, ExecutionResult>> {
    let Some(body) = body else {
        return Ok(Ok(false));
    };
    let response = client
        .send(
            Method::POST,
            client.issue_comments_url(repository, number)?,
            Some(&serde_json::json!({ "body": body })),
        )
        .await?;
    if !response.is_success() {
        let error = response
            .require_success("Failed to add GitHub pull request closing comment")
            .expect_err("non-success response must produce an API error");
        return Ok(Err(ExecutionResult::failure(error.to_string())));
    }
    Ok(Ok(true))
}

async fn close_pull_request(
    client: &GithubClient,
    repository: &str,
    number: u64,
    already_closed: bool,
) -> anyhow::Result<Result<(), ExecutionResult>> {
    if already_closed {
        return Ok(Ok(()));
    }
    let response = client
        .send(
            Method::PATCH,
            client.pull_request_url(repository, number)?,
            Some(&serde_json::json!({ "state": "closed" })),
        )
        .await?;
    if !response.is_success() {
        let error = response
            .require_success("Failed to close GitHub pull request")
            .expect_err("non-success response must produce an API error");
        return Ok(Err(ExecutionResult::failure(error.to_string())));
    }
    Ok(Ok(()))
}

#[async_trait::async_trait]
impl Executor for ClosePullRequestResult {
    fn dry_run_summary(&self) -> String {
        let target = self
            .pull_request_number
            .map(|number| format!("#{number}"))
            .unwrap_or_else(|| "configured target".to_string());
        format!("close GitHub pull request {target}")
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if !ctx.tool_configs.contains_key("close-pull-request") {
            return Ok(ExecutionResult::failure(
                "close-pull-request is not configured for this workflow",
            ));
        }
        let Some(token) = ctx.github_token.as_ref() else {
            return Ok(ExecutionResult::failure(
                "ADO_AW_GITHUB_TOKEN is not set; configure safe-outputs.github-token \
                 or safe-outputs.github-app",
            ));
        };
        let config: ClosePullRequestConfig = ctx.get_tool_config("close-pull-request")?;
        validate_close_pull_request_config(&config)?;
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };
        if let Err(error) = validate_github_mutation_filter_config(filters) {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        let target_number = match self.resolve_target_number(&config, ctx) {
            Ok(number) => number,
            Err(result) => return Ok(result),
        };
        let repository = match self.resolve_repository(&config, ctx) {
            Ok(repository) => repository,
            Err(result) => return Ok(result),
        };
        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let metadata = match client.get_issue(&repository, target_number).await? {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        if let Err(result) =
            validate_github_target_capability(&metadata, GithubTargetCapabilities {
                issues: false,
                pull_requests: true,
            })
        {
            return Ok(result);
        }
        if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
            return Ok(result);
        }

        let already_closed = metadata.state.eq_ignore_ascii_case("closed");
        let comment_posted = match post_comment(&client, &repository, target_number, self.body.as_deref()).await? {
            Ok(posted) => posted,
            Err(result) => return Ok(result),
        };
        if let Err(result) =
            close_pull_request(&client, &repository, target_number, already_closed).await?
        {
            return Ok(result);
        }

        let action = if already_closed {
            "GitHub pull request was already closed"
        } else {
            "Closed GitHub pull request"
        };
        if already_closed {
            warn!("GitHub pull request {}#{} was already closed", repository, target_number);
        } else {
            info!("Closed GitHub pull request {}#{}", repository, target_number);
        }
        Ok(ExecutionResult::success_with_data(
            format!("{action} {repository}#{target_number}"),
            serde_json::json!({
                "number": target_number,
                "target_repo": repository,
                "already_closed": already_closed,
                "comment_posted": comment_posted,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn context(server: &MockServer, config: serde_json::Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("close-pull-request".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            repository_provider: Some("github".to_string()),
            repository_name: Some("octo/repo".to_string()),
            ..Default::default()
        }
    }

    fn open_pr(number: u64) -> serde_json::Value {
        serde_json::json!({
            "number": number,
            "node_id": format!("PR_{number}"),
            "title": "[bot] stale PR",
            "state": "open",
            "labels": [{"name": "automated"}, {"name": "stale"}],
            "html_url": format!("https://github.example/octo/repo/pull/{number}"),
            "pull_request": {"url": format!("https://api.github.example/repos/octo/repo/pulls/{number}")}
        })
    }

    #[test]
    fn contract_name_and_budget() {
        assert_eq!(ClosePullRequestResult::NAME, "close-pull-request");
        assert_eq!(ClosePullRequestResult::DEFAULT_MAX, 1);
    }

    #[test]
    fn config_accepts_gh_aw_target_forms() {
        let triggering: ClosePullRequestConfig =
            serde_json::from_value(serde_json::json!({"target": "triggering"})).unwrap();
        assert_eq!(triggering.target, ClosePullRequestTarget::Triggering);
        let any: ClosePullRequestConfig =
            serde_json::from_value(serde_json::json!({"target": "*"})).unwrap();
        assert_eq!(any.target, ClosePullRequestTarget::Any);
        let number: ClosePullRequestConfig =
            serde_json::from_value(serde_json::json!({"target": 42})).unwrap();
        assert_eq!(number.target, ClosePullRequestTarget::Number(42));
        assert!(serde_json::from_value::<ClosePullRequestConfig>(serde_json::json!({
            "target": 0
        })).is_err());
    }

    #[test]
    fn validates_optional_number_body_and_repository() {
        assert!(
            ClosePullRequestParams {
                pull_request_number: Some(42),
                body: Some("Closing as stale.".to_string()),
                repository: Some("octo/repo".to_string()),
            }
            .validate()
            .is_ok()
        );
        assert!(
            ClosePullRequestParams {
                pull_request_number: Some(0),
                body: None,
                repository: None,
            }
            .validate()
            .is_err()
        );
    }

    #[tokio::test]
    async fn closes_with_comment_and_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(open_pr(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .and(body_json(serde_json::json!({"body": "Closing as stale."})))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/pulls/7"))
            .and(body_json(serde_json::json!({"state": "closed"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target": "*",
                "target-repo": "octo/repo",
                "required-labels": ["automated", "stale"],
                "required-title-prefix": "[bot]"
            }),
        );
        let mut result: ClosePullRequestResult = ClosePullRequestParams {
            pull_request_number: Some(7),
            body: Some("Closing as stale.".to_string()),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(
            execution.data.as_ref().unwrap()["comment_posted"],
            serde_json::json!(true)
        );
    }

    #[tokio::test]
    async fn triggering_target_uses_context_pull_request_number() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/9"))
            .respond_with(ResponseTemplate::new(200).set_body_json(open_pr(9)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/pulls/9"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        ctx.pull_request_number = Some("9".to_string());
        let mut result: ClosePullRequestResult = ClosePullRequestParams {
            pull_request_number: None,
            body: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        assert!(result.execute_sanitized(&ctx).await.unwrap().success);
    }

    #[tokio::test]
    async fn non_pull_request_target_is_rejected_before_patch() {
        let server = MockServer::start().await;
        let mut issue = open_pr(7);
        issue.as_object_mut().unwrap().remove("pull_request");
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/pulls/7"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({"target": "*", "target-repo": "octo/repo"}),
        );
        let mut result: ClosePullRequestResult = ClosePullRequestParams {
            pull_request_number: Some(7),
            body: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        server.verify().await;
    }
}
