//! `close-pull-request` Azure DevOps safe output.

use anyhow::ensure;
use log::{debug, info, warn};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, PATH_SEGMENT, Validate, resolve_repo_name,
};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;
use ado_aw_derive::SanitizeConfig;
use percent_encoding::utf8_percent_encode;

const MAX_COMMENT_LEN: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosePullRequestTarget {
    Triggering,
    Any,
    Id(u64),
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
            Self::Id(id) => serializer.serialize_u64(*id),
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
                formatter.write_str(r#""triggering", "*", or a positive pull request ID"#)
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value == 0 {
                    return Err(E::custom("target pull request ID must be positive"));
                }
                Ok(ClosePullRequestTarget::Id(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value <= 0 {
                    return Err(E::custom("target pull request ID must be positive"));
                }
                Ok(ClosePullRequestTarget::Id(value as u64))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "triggering" => Ok(ClosePullRequestTarget::Triggering),
                    "*" => Ok(ClosePullRequestTarget::Any),
                    other => {
                        let id = other.parse::<u64>().map_err(|_| {
                            E::custom("target must be \"triggering\", \"*\", or a positive pull request ID")
                        })?;
                        if id == 0 {
                            return Err(E::custom("target pull request ID must be positive"));
                        }
                        Ok(ClosePullRequestTarget::Id(id))
                    }
                }
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ClosePullRequestParams {
    /// Positive Azure DevOps pull request ID. Required when config target is "*".
    #[serde(default, alias = "pull_request_number")]
    pub pull_request_id: Option<u64>,
    /// Optional closing comment.
    #[serde(default)]
    pub body: Option<String>,
    /// Optional repository alias/name.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for ClosePullRequestParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(id) = self.pull_request_id {
            ensure!(id > 0, "pull_request_id must be positive");
        }
        if let Some(body) = self.body.as_deref() {
            ensure!(!body.trim().is_empty(), "body must not be empty");
            ensure!(
                body.len() <= MAX_COMMENT_LEN,
                "body must be {MAX_COMMENT_LEN} characters or fewer"
            );
        }
        if let Some(repository) = self.repository.as_deref() {
            ensure!(
                !repository.trim().is_empty(),
                "repository must not be empty"
            );
        }
        Ok(())
    }
}

tool_result! {
    name = "close-pull-request",
    write = true,
    params = ClosePullRequestParams,
    default_max = 1,
    /// Result of abandoning an Azure DevOps pull request.
    pub struct ClosePullRequestResult {
        #[serde(default, alias = "pull_request_number")]
        pull_request_id: Option<u64>,
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
    /// Default repository alias/name when the agent does not pass `repository`.
    #[serde(default, rename = "target-repo", alias = "repository")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repositories", alias = "allowed-repos")]
    pub allowed_repositories: Vec<String>,
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
            allowed_repositories: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            max: None,
        }
    }
}

pub(crate) fn validate_close_pull_request_config(
    config: &ClosePullRequestConfig,
) -> anyhow::Result<()> {
    for label in &config.required_labels {
        ensure!(
            !label.trim().is_empty(),
            "required-labels must not contain empty labels"
        );
    }
    if let Some(prefix) = config.required_title_prefix.as_deref() {
        ensure!(
            !prefix.is_empty(),
            "required-title-prefix must not be empty when set"
        );
    }
    Ok(())
}

fn parse_positive(value: Option<&str>) -> Option<u64> {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|id| *id > 0)
}

fn pr_labels(pr: &serde_json::Value) -> Vec<String> {
    pr.get("labels")
        .and_then(|labels| labels.as_array())
        .into_iter()
        .flatten()
        .filter_map(|label| label.get("name").and_then(|name| name.as_str()))
        .map(ToOwned::to_owned)
        .collect()
}

impl ClosePullRequestResult {
    fn resolve_target_id(
        &self,
        config: &ClosePullRequestConfig,
        ctx: &ExecutionContext,
    ) -> Result<u64, ExecutionResult> {
        match config.target {
            ClosePullRequestTarget::Id(id) => Ok(id),
            ClosePullRequestTarget::Any => self.pull_request_id.ok_or_else(|| {
                ExecutionResult::failure(
                    "pull_request_id is required when safe-outputs.close-pull-request.target is '*'",
                )
            }),
            ClosePullRequestTarget::Triggering => {
                parse_positive(ctx.pull_request_id.as_deref()).ok_or_else(|| {
                    ExecutionResult::failure(
                        "safe-outputs.close-pull-request.target is 'triggering' but no Azure DevOps pull request context is available; use target: '*' and pass pull_request_id, or configure a numeric target",
                    )
                })
            }
        }
    }

    fn repository_selector<'a>(&'a self, config: &'a ClosePullRequestConfig) -> &'a str {
        self.repository
            .as_deref()
            .or(config.target_repo.as_deref())
            .unwrap_or("self")
    }

    fn resolve_repository(
        &self,
        config: &ClosePullRequestConfig,
        ctx: &ExecutionContext,
    ) -> Result<String, ExecutionResult> {
        let selector = self.repository_selector(config);
        if !config.allowed_repositories.is_empty()
            && !config
                .allowed_repositories
                .iter()
                .any(|allowed| allowed == selector)
        {
            return Err(ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed-repositories list: [{}]",
                selector,
                config.allowed_repositories.join(", ")
            )));
        }
        resolve_repo_name(Some(selector), ctx).map_err(ExecutionResult::failure)
    }

    fn validate_filters(
        &self,
        pr: &serde_json::Value,
        config: &ClosePullRequestConfig,
    ) -> Result<(), ExecutionResult> {
        if let Some(prefix) = config.required_title_prefix.as_deref() {
            let title = pr.get("title").and_then(|v| v.as_str()).unwrap_or_default();
            if !title.starts_with(prefix) {
                return Err(ExecutionResult::failure(format!(
                    "Pull request title does not start with required prefix '{}'",
                    prefix
                )));
            }
        }

        if !config.required_labels.is_empty() {
            let labels = pr_labels(pr);
            let missing: Vec<&str> = config
                .required_labels
                .iter()
                .map(String::as_str)
                .filter(|required| !labels.iter().any(|actual| actual == required))
                .collect();
            if !missing.is_empty() {
                return Err(ExecutionResult::failure(format!(
                    "Pull request is missing required label(s): {}",
                    missing.join(", ")
                )));
            }
        }
        Ok(())
    }
}

async fn fetch_pr(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    ctx: &ExecutionContext,
) -> anyhow::Result<Result<serde_json::Value, ExecutionResult>> {
    let response = crate::safe_outputs::authenticate_ado_request(
        client.get(url),
        token,
        ctx.write_connection_type,
    )
    .send()
    .await
    .map_err(|error| anyhow::anyhow!("Failed to fetch Azure DevOps pull request: {error}"))?;

    if response.status().is_success() {
        return Ok(Ok(response.json().await.map_err(|error| {
            anyhow::anyhow!("Failed to parse pull request response: {error}")
        })?));
    }

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok(Err(ExecutionResult::failure(format!(
        "Failed to fetch pull request (HTTP {}): {}",
        status, body
    ))))
}

async fn post_comment(
    client: &reqwest::Client,
    base_url: &str,
    repo_name: &str,
    pull_request_id: u64,
    token: &str,
    ctx: &ExecutionContext,
    body: Option<&str>,
) -> anyhow::Result<Result<bool, ExecutionResult>> {
    let Some(body) = body else {
        return Ok(Ok(false));
    };
    let url = format!(
        "{}/{}/pullRequests/{}/threads?api-version=7.1",
        base_url,
        utf8_percent_encode(repo_name, PATH_SEGMENT),
        pull_request_id,
    );
    let thread_body = serde_json::json!({
        "comments": [{
            "parentCommentId": 0,
            "content": body,
            "commentType": 1,
        }],
        "status": 1,
    });
    let response = crate::safe_outputs::authenticate_ado_request(
        client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&thread_body),
        token,
        ctx.write_connection_type,
    )
    .send()
    .await
    .map_err(|error| {
        anyhow::anyhow!("Failed to post Azure DevOps pull request comment: {error}")
    })?;

    if response.status().is_success() {
        return Ok(Ok(true));
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok(Err(ExecutionResult::failure(format!(
        "Failed to add closing comment to PR #{} (HTTP {}): {}",
        pull_request_id, status, body
    ))))
}

async fn abandon_pr(
    client: &reqwest::Client,
    url: &str,
    pull_request_id: u64,
    token: &str,
    ctx: &ExecutionContext,
) -> anyhow::Result<Result<(), ExecutionResult>> {
    let response = crate::safe_outputs::authenticate_ado_request(
        client
            .patch(url)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "status": "abandoned" })),
        token,
        ctx.write_connection_type,
    )
    .send()
    .await
    .map_err(|error| anyhow::anyhow!("Failed to abandon Azure DevOps pull request: {error}"))?;

    if response.status().is_success() {
        return Ok(Ok(()));
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok(Err(ExecutionResult::failure(format!(
        "Failed to abandon PR #{} (HTTP {}): {}",
        pull_request_id, status, body
    ))))
}

#[async_trait::async_trait]
impl Executor for ClosePullRequestResult {
    fn dry_run_summary(&self) -> String {
        let target = self
            .pull_request_id
            .map(|id| format!("#{id}"))
            .unwrap_or_else(|| "the configured or triggering target".to_string());
        format!("abandon Azure DevOps pull request {target}")
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if !ctx.tool_configs.contains_key("close-pull-request") {
            return Ok(ExecutionResult::failure(
                "close-pull-request is not configured for this workflow",
            ));
        }
        let org_url = ctx
            .ado_org_url
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("AZURE_DEVOPS_ORG_URL not set"))?;
        let project = ctx
            .ado_project
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SYSTEM_TEAMPROJECT not set"))?;
        let token = ctx.access_token.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)"
            )
        })?;
        let config: ClosePullRequestConfig = ctx.get_tool_config("close-pull-request")?;
        if let Err(error) = validate_close_pull_request_config(&config) {
            return Ok(ExecutionResult::failure(error.to_string()));
        }

        let pull_request_id = match self.resolve_target_id(&config, ctx) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let repo_name = match self.resolve_repository(&config, ctx) {
            Ok(repo_name) => repo_name,
            Err(result) => return Ok(result),
        };
        let client = reqwest::Client::new();
        let base_url = format!(
            "{}/{}/_apis/git/repositories",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
        );
        let pr_url = format!(
            "{}/{}/pullRequests/{}?api-version=7.1",
            base_url,
            utf8_percent_encode(&repo_name, PATH_SEGMENT),
            pull_request_id,
        );
        debug!("close-pull-request API URL: {}", pr_url);

        let pr = match fetch_pr(&client, &pr_url, token, ctx).await? {
            Ok(pr) => pr,
            Err(result) => return Ok(result),
        };
        if let Err(result) = self.validate_filters(&pr, &config) {
            return Ok(result);
        }

        let status = pr
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if status.eq_ignore_ascii_case("abandoned") {
            warn!("Azure DevOps PR #{} was already abandoned", pull_request_id);
            return Ok(ExecutionResult::success_with_data(
                format!("Azure DevOps PR #{} was already abandoned", pull_request_id),
                serde_json::json!({
                    "pull_request_id": pull_request_id,
                    "repository": repo_name,
                    "already_closed": true,
                    "comment_posted": false,
                }),
            ));
        }
        if !status.is_empty() && !status.eq_ignore_ascii_case("active") {
            return Ok(ExecutionResult::failure(format!(
                "Cannot abandon PR #{} because its status is '{}' (expected active)",
                pull_request_id, status
            )));
        }

        let comment_posted = match post_comment(
            &client,
            &base_url,
            &repo_name,
            pull_request_id,
            token,
            ctx,
            self.body.as_deref(),
        )
        .await?
        {
            Ok(posted) => posted,
            Err(result) => return Ok(result),
        };
        if let Err(result) = abandon_pr(&client, &pr_url, pull_request_id, token, ctx).await? {
            return Ok(result);
        }

        info!("Abandoned Azure DevOps PR #{}", pull_request_id);
        Ok(ExecutionResult::success_with_data(
            format!("Abandoned Azure DevOps PR #{}", pull_request_id),
            serde_json::json!({
                "pull_request_id": pull_request_id,
                "repository": repo_name,
                "already_closed": false,
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
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn context(server: &MockServer, config: serde_json::Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("close-pull-request".to_string(), config);
        ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_project: Some("proj".to_string()),
            access_token: Some("token".to_string()),
            tool_configs,
            repository_name: Some("repo".to_string()),
            allowed_repositories: HashMap::from([("other".to_string(), "other-repo".to_string())]),
            ..Default::default()
        }
    }

    fn pr(status: &str) -> serde_json::Value {
        serde_json::json!({
            "pullRequestId": 7,
            "title": "[bot] stale PR",
            "status": status,
            "labels": [{"name": "automated"}, {"name": "stale"}]
        })
    }

    #[test]
    fn contract_name_and_budget() {
        assert_eq!(ClosePullRequestResult::NAME, "close-pull-request");
        assert_eq!(ClosePullRequestResult::DEFAULT_MAX, 1);
    }

    #[test]
    fn config_accepts_target_forms() {
        let triggering: ClosePullRequestConfig =
            serde_json::from_value(serde_json::json!({"target": "triggering"})).unwrap();
        assert_eq!(triggering.target, ClosePullRequestTarget::Triggering);
        let any: ClosePullRequestConfig =
            serde_json::from_value(serde_json::json!({"target": "*"})).unwrap();
        assert_eq!(any.target, ClosePullRequestTarget::Any);
        let id: ClosePullRequestConfig =
            serde_json::from_value(serde_json::json!({"target": 42})).unwrap();
        assert_eq!(id.target, ClosePullRequestTarget::Id(42));
        assert!(
            serde_json::from_value::<ClosePullRequestConfig>(serde_json::json!({
                "target": 0
            }))
            .is_err()
        );
    }

    #[test]
    fn validates_optional_id_body_and_repository() {
        assert!(
            ClosePullRequestParams {
                pull_request_id: Some(42),
                body: Some("Closing as stale.".to_string()),
                repository: Some("self".to_string()),
            }
            .validate()
            .is_ok()
        );
        assert!(
            ClosePullRequestParams {
                pull_request_id: Some(0),
                body: None,
                repository: None,
            }
            .validate()
            .is_err()
        );
    }

    #[tokio::test]
    async fn abandons_with_comment_and_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proj/_apis/git/repositories/repo/pullRequests/7"))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr("active")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/proj/_apis/git/repositories/repo/pullRequests/7/threads",
            ))
            .and(query_param("api-version", "7.1"))
            .and(body_json(serde_json::json!({
                "comments": [{
                    "parentCommentId": 0,
                    "content": "Closing as stale.",
                    "commentType": 1,
                }],
                "status": 1,
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/proj/_apis/git/repositories/repo/pullRequests/7"))
            .and(query_param("api-version", "7.1"))
            .and(body_json(serde_json::json!({"status": "abandoned"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target": "*",
                "required-labels": ["automated", "stale"],
                "required-title-prefix": "[bot]"
            }),
        );
        let mut result: ClosePullRequestResult = ClosePullRequestParams {
            pull_request_id: Some(7),
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
        server.verify().await;
    }

    #[tokio::test]
    async fn triggering_target_uses_context_pull_request_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proj/_apis/git/repositories/repo/pullRequests/9"))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr("active")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/proj/_apis/git/repositories/repo/pullRequests/9"))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = context(&server, serde_json::json!({}));
        ctx.pull_request_id = Some("9".to_string());
        let mut result: ClosePullRequestResult = ClosePullRequestParams {
            pull_request_id: None,
            body: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        assert!(result.execute_sanitized(&ctx).await.unwrap().success);
        server.verify().await;
    }

    #[tokio::test]
    async fn missing_label_rejects_before_patch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proj/_apis/git/repositories/repo/pullRequests/7"))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr("active")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/proj/_apis/git/repositories/repo/pullRequests/7"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({"target": "*", "required-labels": ["missing"]}),
        );
        let mut result: ClosePullRequestResult = ClosePullRequestParams {
            pull_request_id: Some(7),
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
