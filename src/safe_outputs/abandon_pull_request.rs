//! `abandon-pull-request` Azure DevOps safe output.

use anyhow::ensure;
use log::{debug, info, warn};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::pr_common::{
    PrTargetPolicy, PullRequestReference, repository_api_base, resolve_pr_policy_target,
    validate_reference,
};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config, sanitize_markdown};
use crate::tool_result;
use ado_aw_derive::SanitizeConfig;

const MAX_COMMENT_LEN: usize = 4_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AbandonPullRequestTarget {
    #[default]
    Triggering,
    Any,
    Id(u64),
}

impl Serialize for AbandonPullRequestTarget {
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

impl<'de> Deserialize<'de> for AbandonPullRequestTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = AbandonPullRequestTarget;

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
                Ok(AbandonPullRequestTarget::Id(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value <= 0 {
                    return Err(E::custom("target pull request ID must be positive"));
                }
                Ok(AbandonPullRequestTarget::Id(value as u64))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "triggering" => Ok(AbandonPullRequestTarget::Triggering),
                    "*" => Ok(AbandonPullRequestTarget::Any),
                    other => {
                        let id = other.parse::<u64>().map_err(|_| {
                            E::custom("target must be \"triggering\", \"*\", or a positive pull request ID")
                        })?;
                        if id == 0 {
                            return Err(E::custom("target pull request ID must be positive"));
                        }
                        Ok(AbandonPullRequestTarget::Id(id))
                    }
                }
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct AbandonPullRequestParams {
    /// Positive Azure DevOps PR ID or same-run temporary ID. Required when target is "*".
    #[serde(default, alias = "pull_request_number")]
    pub pull_request_id: Option<PullRequestReference>,
    /// Optional abandonment comment.
    #[serde(default)]
    pub body: Option<String>,
    /// Optional repository alias/name.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for AbandonPullRequestParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(id) = &self.pull_request_id {
            validate_reference(id)?;
        }
        if let Some(body) = self.body.as_deref() {
            ensure!(!body.trim().is_empty(), "body must not be empty");
            ensure!(
                body.encode_utf16().count() <= MAX_COMMENT_LEN,
                "body must be {MAX_COMMENT_LEN} UTF-16 units or fewer"
            );
        }
        if let Some(repository) = self.repository.as_deref() {
            ensure!(
                !repository.trim().is_empty(),
                "repository must not be empty"
            );
            crate::validate::reject_pipeline_injection(repository, "repository")?;
        }
        Ok(())
    }
}

tool_result! {
    name = "abandon-pull-request",
    write = true,
    params = AbandonPullRequestParams,
    default_max = 1,
    /// Result of abandoning an Azure DevOps pull request.
    pub struct AbandonPullRequestResult {
        #[serde(default, alias = "pull_request_number")]
        pull_request_id: Option<PullRequestReference>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for AbandonPullRequestResult {
    fn sanitize_content_fields(&mut self) {
        self.body = self.body.as_deref().map(sanitize_markdown);
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbandonPullRequestConfig {
    #[serde(default = "default_true", rename = "include-stats")]
    #[sanitize_config(skip)]
    pub include_stats: bool,
    #[serde(default)]
    #[sanitize_config(skip)]
    pub target: AbandonPullRequestTarget,
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

impl Default for AbandonPullRequestConfig {
    fn default() -> Self {
        Self {
            include_stats: true,
            target: AbandonPullRequestTarget::Triggering,
            target_repo: None,
            allowed_repositories: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            max: None,
        }
    }
}

fn default_true() -> bool {
    true
}

pub(crate) fn validate_abandon_pull_request_config(
    config: &AbandonPullRequestConfig,
) -> anyhow::Result<()> {
    if let AbandonPullRequestTarget::Id(id) = config.target {
        ensure!(id > 0, "target pull request ID must be positive");
    }
    for repository in config
        .allowed_repositories
        .iter()
        .chain(config.target_repo.iter())
    {
        ensure!(
            !repository.trim().is_empty(),
            "repository must not be empty"
        );
        crate::validate::reject_pipeline_injection(repository, "repository")?;
    }
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

impl AbandonPullRequestConfig {
    pub(crate) fn target_policy(&self) -> anyhow::Result<PrTargetPolicy> {
        match self.target {
            AbandonPullRequestTarget::Id(id) => PrTargetPolicy::fixed(id),
            AbandonPullRequestTarget::Triggering => Ok(PrTargetPolicy::Triggering),
            AbandonPullRequestTarget::Any => Ok(PrTargetPolicy::Explicit),
        }
    }
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

impl AbandonPullRequestResult {
    fn repository_selector<'a>(&'a self, config: &'a AbandonPullRequestConfig) -> Option<&'a str> {
        self.repository.as_deref().or(config.target_repo.as_deref())
    }

    fn validate_filters(
        &self,
        pr: &serde_json::Value,
        config: &AbandonPullRequestConfig,
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
                .filter(|required| {
                    !labels
                        .iter()
                        .any(|actual| actual.eq_ignore_ascii_case(required))
                })
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
    pull_request_id: u64,
    token: &str,
    ctx: &ExecutionContext,
    body: Option<&str>,
) -> anyhow::Result<Result<bool, ExecutionResult>> {
    let Some(body) = body else {
        return Ok(Ok(false));
    };
    let url = format!(
        "{}/pullRequests/{}/threads?api-version=7.1",
        base_url, pull_request_id,
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
        "Failed to add abandonment comment to PR #{} (HTTP {}): {}",
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
impl Executor for AbandonPullRequestResult {
    fn dry_run_summary(&self) -> String {
        let target = self
            .pull_request_id
            .as_ref()
            .map(|id| format!("#{id}"))
            .unwrap_or_else(|| "the configured or triggering target".to_string());
        format!("abandon Azure DevOps pull request {target}")
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        let params = AbandonPullRequestParams {
            pull_request_id: self.pull_request_id.clone(),
            body: self.body.clone(),
            repository: self.repository.clone(),
        };
        if let Err(error) = params.validate() {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        if !ctx.tool_configs.contains_key("abandon-pull-request") {
            return Ok(ExecutionResult::failure(
                "abandon-pull-request is not configured for this workflow",
            ));
        }
        let token = ctx.access_token.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)"
            )
        })?;
        let config: AbandonPullRequestConfig = ctx.get_tool_config("abandon-pull-request")?;
        if let Err(error) = validate_abandon_pull_request_config(&config) {
            return Ok(ExecutionResult::failure(error.to_string()));
        }

        let (pull_request_id, target) = match resolve_pr_policy_target(
            &config.target_policy()?,
            self.pull_request_id.as_ref(),
            self.repository_selector(&config),
            &config.allowed_repositories,
            ctx,
        )? {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        let repo_name = target.qualified_repository();
        let client = reqwest::Client::new();
        let base_url = repository_api_base(&target);
        let pr_url = format!(
            "{}/pullRequests/{}?api-version=7.1",
            base_url, pull_request_id,
        );
        debug!("abandon-pull-request API URL: {}", pr_url);

        let pr = match fetch_pr(&client, &pr_url, token, ctx).await? {
            Ok(pr) => pr,
            Err(result) => return Ok(result),
        };
        if let Err(result) = self.validate_filters(&pr, &config) {
            return Ok(result);
        }

        let Some(status) = pr.get("status").and_then(|v| v.as_str()) else {
            return Ok(ExecutionResult::failure(format!(
                "Cannot abandon PR #{} because the Azure DevOps response did not include a status",
                pull_request_id
            )));
        };
        if status.eq_ignore_ascii_case("abandoned") {
            warn!("Azure DevOps PR #{} was already abandoned", pull_request_id);
            return Ok(ExecutionResult::success_with_data(
                format!("Azure DevOps PR #{} was already abandoned", pull_request_id),
                serde_json::json!({
                    "pull_request_id": pull_request_id,
                    "repository": repo_name,
                    "already_abandoned": true,
                    "abandoned": true,
                    "comment_posted": false,
                    "comment_status": "not-attempted",
                }),
            ));
        }
        if !status.eq_ignore_ascii_case("active") {
            return Ok(ExecutionResult::failure(format!(
                "Cannot abandon PR #{} because its status is '{}' (expected active)",
                pull_request_id, status
            )));
        }

        let comment = self.body.as_deref().map(|body| {
            let body = sanitize_markdown(body);
            if config.include_stats {
                crate::agent_stats::append_stats_to_body(&body, ctx, true)
            } else {
                body
            }
        });
        if comment
            .as_deref()
            .is_some_and(|body| body.trim().is_empty())
        {
            return Ok(ExecutionResult::failure("sanitized body must not be empty"));
        }
        if comment
            .as_deref()
            .is_some_and(|body| body.encode_utf16().count() > MAX_COMMENT_LEN)
        {
            return Ok(ExecutionResult::failure(format!(
                "assembled abandonment comment exceeds {MAX_COMMENT_LEN} UTF-16 units"
            )));
        }
        if let Err(result) = abandon_pr(&client, &pr_url, pull_request_id, token, ctx).await? {
            return Ok(result);
        }

        let comment_posted = match post_comment(
            &client,
            &base_url,
            pull_request_id,
            token,
            ctx,
            comment.as_deref(),
        )
        .await
        {
            Ok(Ok(posted)) => posted,
            Ok(Err(result)) => {
                return Ok(abandoned_comment_warning(
                    pull_request_id,
                    &repo_name,
                    "failed",
                    &result.message,
                ));
            }
            Err(error) => {
                return Ok(abandoned_comment_warning(
                    pull_request_id,
                    &repo_name,
                    "uncertain",
                    &error.to_string(),
                ));
            }
        };

        info!("Abandoned Azure DevOps PR #{}", pull_request_id);
        Ok(ExecutionResult::success_with_data(
            format!("Abandoned Azure DevOps PR #{}", pull_request_id),
            serde_json::json!({
                "pull_request_id": pull_request_id,
                "repository": repo_name,
                "already_abandoned": false,
                "abandoned": true,
                "comment_posted": comment_posted,
                "comment_status": if comment_posted { "posted" } else { "not-requested" },
            }),
        ))
    }
}

fn abandoned_comment_warning(
    pr_id: u64,
    repository: &str,
    status: &str,
    reason: &str,
) -> ExecutionResult {
    ExecutionResult::warning_with_data(
        format!("Abandoned Azure DevOps PR #{pr_id} but failed to add comment: {reason}"),
        serde_json::json!({
            "pull_request_id": pr_id,
            "repository": repository,
            "abandoned": true,
            "already_abandoned": false,
            "comment_posted": false,
            "comment_status": status,
            "comment_error": reason,
        }),
    )
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
        tool_configs.insert("abandon-pull-request".to_string(), config);
        ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_organization: Some("org".to_string()),
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
        assert_eq!(AbandonPullRequestResult::NAME, "abandon-pull-request");
        assert_eq!(AbandonPullRequestResult::DEFAULT_MAX, 1);
    }

    #[test]
    fn config_accepts_target_forms() {
        let triggering: AbandonPullRequestConfig =
            serde_json::from_value(serde_json::json!({"target": "triggering"})).unwrap();
        assert_eq!(triggering.target, AbandonPullRequestTarget::Triggering);
        let any: AbandonPullRequestConfig =
            serde_json::from_value(serde_json::json!({"target": "*"})).unwrap();
        assert_eq!(any.target, AbandonPullRequestTarget::Any);
        let id: AbandonPullRequestConfig =
            serde_json::from_value(serde_json::json!({"target": 42})).unwrap();
        assert_eq!(id.target, AbandonPullRequestTarget::Id(42));
        assert!(
            serde_json::from_value::<AbandonPullRequestConfig>(serde_json::json!({
                "target": 0
            }))
            .is_err()
        );
    }

    #[test]
    fn validates_optional_id_body_and_repository() {
        assert!(
            AbandonPullRequestParams {
                pull_request_id: Some(PullRequestReference::Number(42)),
                body: Some("Closing as stale.".to_string()),
                repository: Some("self".to_string()),
            }
            .validate()
            .is_ok()
        );
        assert!(
            AbandonPullRequestParams {
                pull_request_id: Some(PullRequestReference::Number(0)),
                body: None,
                repository: None,
            }
            .validate()
            .is_err()
        );
    }

    #[tokio::test]
    async fn comment_http_failure_retains_abandonment_data() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proj/_apis/git/repositories/repo/pullRequests/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr("active")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/proj/_apis/git/repositories/repo/pullRequests/7"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/proj/_apis/git/repositories/repo/pullRequests/7/threads",
            ))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(&server, serde_json::json!({"target": "*"}));
        let mut result: AbandonPullRequestResult = serde_json::from_value(serde_json::json!({
            "name": "abandon-pull-request", "pull_request_id": "7", "body": "Closing as stale."
        }))
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success && execution.is_warning());
        let data = execution.data.unwrap();
        assert_eq!(data["abandoned"], true);
        assert_eq!(data["comment_posted"], false);
        assert_eq!(data["comment_status"], "failed");
        assert_eq!(data["pull_request_id"], 7);
    }

    #[tokio::test]
    async fn transport_failure_after_abandon_is_warning_without_retry() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for (index, expected_method) in ["GET", "PATCH", "POST"].iter().enumerate() {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(header_end) =
                        request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                    {
                        let header = String::from_utf8_lossy(&request[..header_end]);
                        let content_length = header
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                assert!(String::from_utf8_lossy(&request).starts_with(*expected_method));
                if index == 2 {
                    // The server may have received the comment, but its acknowledgement is lost.
                    drop(socket);
                    break;
                }
                let body = if index == 0 {
                    r#"{"status":"active"}"#
                } else {
                    "{}"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        let mut ctx = ExecutionContext {
            ado_org_url: Some(uri),
            ado_organization: Some("org".into()),
            ado_project: Some("proj".into()),
            repository_name: Some("repo".into()),
            access_token: Some("token".into()),
            ..Default::default()
        };
        ctx.tool_configs.insert(
            "abandon-pull-request".into(),
            serde_json::json!({"target": "*"}),
        );
        let mut result: AbandonPullRequestResult = serde_json::from_value(serde_json::json!({
            "name": "abandon-pull-request", "pull_request_id": 7, "body": "Closing as stale."
        }))
        .unwrap();
        let execution = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            result.execute_sanitized(&ctx),
        )
        .await
        .unwrap()
        .unwrap();
        server.await.unwrap();
        assert!(execution.success && execution.is_warning());
        let data = execution.data.unwrap();
        assert_eq!(data["abandoned"], true);
        assert_eq!(data["comment_status"], "uncertain");
    }

    #[tokio::test]
    async fn abandon_temp_reference_uses_registered_target_and_bearer_auth() {
        use wiremock::matchers::header;
        let server = MockServer::start().await;
        let route = "/Other/_apis/git/repositories/repo-id/pullRequests/4294967296";
        Mock::given(method("GET"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr("active")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(body_json(serde_json::json!({"status": "abandoned"})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = super::super::pr_common::tests::registered_context(
            &server.uri(),
            "abandon-pull-request",
            serde_json::json!({"target": "*", "allowed-repositories": ["other"]}),
        );
        ctx.write_connection_type = Some(crate::compile::types::WriteConnectionType::AzureDevOps);
        let mut result: AbandonPullRequestResult = serde_json::from_value(serde_json::json!({
            "name": "abandon-pull-request", "pull_request_id": "#aw_pr123"
        }))
        .unwrap();
        assert!(result.execute_sanitized(&ctx).await.unwrap().success);
    }

    #[tokio::test]
    async fn abandon_temp_reference_cannot_bypass_target_or_filters() {
        for target in [serde_json::json!(7), serde_json::json!("triggering")] {
            let server = MockServer::start().await;
            let mut ctx = super::super::pr_common::tests::registered_context(
                &server.uri(),
                "abandon-pull-request",
                serde_json::json!({"target": target}),
            );
            ctx.pull_request_id = Some("7".into());
            let mut result: AbandonPullRequestResult = serde_json::from_value(serde_json::json!({
                "name": "abandon-pull-request", "pull_request_id": "#aw_pr123"
            }))
            .unwrap();
            assert!(!result.execute_sanitized(&ctx).await.unwrap().success);
            assert!(server.received_requests().await.unwrap().is_empty());
        }
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/Other/_apis/git/repositories/repo-id/pullRequests/4294967296",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr("active")))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = super::super::pr_common::tests::registered_context(
            &server.uri(),
            "abandon-pull-request",
            serde_json::json!({"target": "*", "required-labels": ["missing"]}),
        );
        let mut result: AbandonPullRequestResult = serde_json::from_value(serde_json::json!({
            "name": "abandon-pull-request", "pull_request_id": "#aw_pr123"
        }))
        .unwrap();
        assert!(!result.execute_sanitized(&ctx).await.unwrap().success);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[test]
    fn abandonment_markdown_and_stats_respect_final_utf16_limit() {
        assert!(AbandonPullRequestConfig::default().include_stats);
        for (body, valid) in [("😀".repeat(2000), true), ("😀".repeat(2001), false)] {
            assert_eq!(
                AbandonPullRequestParams {
                    pull_request_id: None,
                    repository: None,
                    body: Some(body)
                }
                .validate()
                .is_ok(),
                valid
            );
        }
    }

    #[tokio::test]
    async fn assembled_abandonment_comment_is_checked_before_mutation() {
        for (body, expected_success) in [("`<safe>`".to_string(), true), ("a".repeat(4000), false)]
        {
            let server = MockServer::start().await;
            let route = "/proj/_apis/git/repositories/repo/pullRequests/7";
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_json(pr("active")))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("PATCH"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200))
                .expect(if expected_success { 1 } else { 0 })
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path(format!("{route}/threads")))
                .respond_with(ResponseTemplate::new(200))
                .expect(if expected_success { 1 } else { 0 })
                .mount(&server)
                .await;
            let mut ctx = context(&server, serde_json::json!({"target": "*"}));
            ctx.agent_stats = Some(crate::agent_stats::AgentStats {
                agent_name: "review-agent".into(),
                model: None,
                input_tokens: 1,
                output_tokens: 1,
                ai_credits: None,
                duration_seconds: 1.0,
                tool_calls: 1,
                turns: 1,
            });
            let mut result: AbandonPullRequestResult = serde_json::from_value(serde_json::json!({
                "name": "abandon-pull-request", "pull_request_id": 7, "body": body
            }))
            .unwrap();
            assert_eq!(
                result.execute_sanitized(&ctx).await.unwrap().success,
                expected_success
            );
            if expected_success {
                let requests = server.received_requests().await.unwrap();
                let posted = requests
                    .iter()
                    .find(|request| request.method.as_str() == "POST")
                    .unwrap();
                let body: serde_json::Value = serde_json::from_slice(&posted.body).unwrap();
                let content = body["comments"][0]["content"].as_str().unwrap();
                assert!(content.starts_with("`<safe>`"));
                assert!(content.contains("review-agent"));
            }
        }
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
                "include-stats": false,
                "required-labels": ["automated", "stale"],
                "required-title-prefix": "[bot]"
            }),
        );
        let mut result: AbandonPullRequestResult = AbandonPullRequestParams {
            pull_request_id: Some(PullRequestReference::Number(7)),
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
            .and(path(
                "/proj/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/9",
            ))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr("active")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(
                "/proj/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/9",
            ))
            .and(query_param("api-version", "7.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = context(&server, serde_json::json!({}));
        ctx.pull_request_id = Some("9".to_string());
        ctx.triggering_pr = Some(super::super::pr_common::TriggeringPullRequest {
            collection_uri: server.uri(),
            project: "proj".into(),
            repository_name: "repo".into(),
            repository_id: "11111111-1111-1111-1111-111111111111".into(),
            id: "9".into(),
        });
        let mut result: AbandonPullRequestResult = AbandonPullRequestParams {
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
        let mut result: AbandonPullRequestResult = AbandonPullRequestParams {
            pull_request_id: Some(PullRequestReference::Number(7)),
            body: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        server.verify().await;
    }

    #[tokio::test]
    async fn stage_three_revalidates_params() {
        let server = MockServer::start().await;
        let ctx = context(&server, serde_json::json!({"target": "*"}));
        let mut result: AbandonPullRequestResult = serde_json::from_value(serde_json::json!({
            "name": "abandon-pull-request",
            "pull_request_id": 7,
            "body": " "
        }))
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("body must not be empty"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_disallowed_repository_before_network() {
        let server = MockServer::start().await;
        let ctx = context(
            &server,
            serde_json::json!({"target": "*", "allowed-repositories": ["other"]}),
        );
        let mut result: AbandonPullRequestResult = AbandonPullRequestParams {
            pull_request_id: Some(PullRequestReference::Number(7)),
            body: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("allowed-repositories"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_title_prefix_before_patch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proj/_apis/git/repositories/repo/pullRequests/7"))
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
            serde_json::json!({"target": "*", "required-title-prefix": "[manual]"}),
        );
        let mut result: AbandonPullRequestResult = AbandonPullRequestParams {
            pull_request_id: Some(PullRequestReference::Number(7)),
            body: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("required prefix"));
        server.verify().await;
    }

    #[tokio::test]
    async fn handles_already_abandoned_and_rejects_completed() {
        for (status, success, expected) in [
            ("abandoned", true, "already abandoned"),
            ("completed", false, "expected active"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/proj/_apis/git/repositories/repo/pullRequests/7"))
                .respond_with(ResponseTemplate::new(200).set_body_json(pr(status)))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("PATCH"))
                .and(path("/proj/_apis/git/repositories/repo/pullRequests/7"))
                .respond_with(ResponseTemplate::new(200))
                .expect(0)
                .mount(&server)
                .await;
            let ctx = context(&server, serde_json::json!({"target": "*"}));
            let mut result: AbandonPullRequestResult = AbandonPullRequestParams {
                pull_request_id: Some(PullRequestReference::Number(7)),
                body: None,
                repository: None,
            }
            .try_into()
            .unwrap();
            let execution = result.execute_sanitized(&ctx).await.unwrap();
            assert_eq!(execution.success, success);
            assert!(
                execution.message.contains(expected),
                "{}",
                execution.message
            );
            server.verify().await;
        }
    }

    #[tokio::test]
    async fn fixed_target_rejects_mismatched_request_id() {
        let server = MockServer::start().await;
        let ctx = context(&server, serde_json::json!({"target": 42}));
        let mut result: AbandonPullRequestResult = AbandonPullRequestParams {
            pull_request_id: Some(PullRequestReference::Number(7)),
            body: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("#7"));
        assert!(execution.message.contains("#42"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
