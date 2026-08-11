//! `hide-github-issue-comment` safe output.

use anyhow::ensure;
use log::{debug, info};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubClient, GithubMutationFilters,
    GithubRepositoryPolicy, GithubTargetCapabilities, GithubTargetKind, GithubTargetMetadata,
    Validate, resolve_github_repository, validate_github_mutation_filter_config,
    validate_github_mutation_filters, validate_github_repository,
    validate_github_target_capability,
};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

const DEFAULT_REASON: &str = "OUTDATED";
const VALID_REASONS: &[&str] = &[
    "SPAM",
    "ABUSE",
    "OFF_TOPIC",
    "OUTDATED",
    "RESOLVED",
    "LOW_QUALITY",
];
const RESOLVE_COMMENT_QUERY: &str = r#"query ResolveGithubComment($id: ID!) {
  node(id: $id) {
    __typename
    ... on IssueComment {
      id
      url
      repository { nameWithOwner }
    }
    ... on PullRequestReviewComment {
      id
      url
      repository { nameWithOwner }
    }
    ... on DiscussionComment {
      id
      discussion {
        number
        title
        repository { nameWithOwner }
      }
    }
  }
}"#;
pub(crate) const MINIMIZE_COMMENT_MUTATION: &str = r#"mutation MinimizeGithubComment($input: MinimizeCommentInput!) {
  minimizeComment(input: $input) {
    minimizedComment {
      isMinimized
      minimizedReason
    }
  }
}"#;

/// Numeric REST issue-comment ID or GraphQL node ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum GithubCommentId {
    Numeric(u64),
    Node(String),
}

impl GithubCommentId {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Numeric(id) => ensure!(*id > 0, "comment_id must be positive"),
            Self::Node(id) => {
                ensure!(!id.trim().is_empty(), "comment_id must not be empty");
                ensure!(
                    id.len() <= 256,
                    "GraphQL comment_id must be 256 characters or fewer"
                );
                reject_pipeline_injection(id, "hide-github-issue-comment.comment_id")?;
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for GithubCommentId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Numeric(id) => write!(formatter, "{id}"),
            Self::Node(id) => formatter.write_str(id),
        }
    }
}

impl<'de> Deserialize<'de> for GithubCommentId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CommentIdVisitor;

        impl serde::de::Visitor<'_> for CommentIdVisitor {
            type Value = GithubCommentId;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a positive numeric REST comment ID or GraphQL node ID")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(GithubCommentId::Numeric(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                u64::try_from(value)
                    .map(GithubCommentId::Numeric)
                    .map_err(|_| E::custom("comment_id must be positive"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value.chars().all(|character| character.is_ascii_digit()) {
                    return value
                        .parse::<u64>()
                        .map(GithubCommentId::Numeric)
                        .map_err(|_| E::custom("numeric comment_id is outside the u64 range"));
                }
                Ok(GithubCommentId::Node(value.to_string()))
            }
        }

        deserializer.deserialize_any(CommentIdVisitor)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct HideGithubIssueCommentParams {
    /// Numeric REST issue-comment ID or GraphQL node ID.
    pub comment_id: GithubCommentId,
    /// GitHub minimization classifier. Defaults to `OUTDATED`.
    #[serde(default)]
    pub reason: Option<String>,
    /// Optional target repository. Required to resolve numeric IDs unless a
    /// default target repository is available.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for HideGithubIssueCommentParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.comment_id.validate()?;
        if let Some(reason) = self.reason.as_deref() {
            canonical_github_comment_reason(Some(reason))?;
        }
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "hide-github-issue-comment",
    write = true,
    params = HideGithubIssueCommentParams,
    default_max = 5,
    /// Result of minimizing a GitHub issue, pull-request, or discussion comment.
    pub struct HideGithubIssueCommentResult {
        comment_id: GithubCommentId,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for HideGithubIssueCommentResult {
    fn sanitize_content_fields(&mut self) {
        self.reason = self.reason.as_deref().map(sanitize_config);
        self.repository = self.repository.as_deref().map(sanitize_config);
        if let GithubCommentId::Node(id) = &mut self.comment_id {
            *id = sanitize_config(id);
        }
    }
}

#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HideGithubIssueCommentConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    /// Restrict the GraphQL minimization classifiers the agent may select.
    /// Empty permits every supported classifier.
    #[serde(default, rename = "allowed-reasons")]
    pub allowed_reasons: Vec<String>,
    /// Permit GraphQL discussion comments in addition to issue/PR comments.
    #[serde(default)]
    #[sanitize_config(skip)]
    pub discussions: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

pub(crate) fn canonical_github_comment_reason(reason: Option<&str>) -> anyhow::Result<String> {
    let reason = reason.unwrap_or(DEFAULT_REASON).to_ascii_uppercase();
    ensure!(
        VALID_REASONS.contains(&reason.as_str()),
        "reason must be one of: {}",
        VALID_REASONS.join(", ")
    );
    Ok(reason)
}

pub(crate) fn validate_github_comment_reason_policy(
    reason: &str,
    allowed_reasons: &[String],
) -> anyhow::Result<()> {
    for allowed in allowed_reasons {
        canonical_github_comment_reason(Some(allowed))?;
    }
    if !allowed_reasons.is_empty()
        && !allowed_reasons
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(reason))
    {
        anyhow::bail!(
            "reason '{}' is not in allowed-reasons: {}",
            crate::sanitize::neutralize_pipeline_commands(reason),
            allowed_reasons.join(", ")
        );
    }
    Ok(())
}

pub(crate) async fn minimize_github_comment(
    client: &GithubClient,
    node_id: &str,
    reason: &str,
) -> anyhow::Result<Result<(), ExecutionResult>> {
    let data = match client
        .graphql(
            "Failed to minimize GitHub comment",
            MINIMIZE_COMMENT_MUTATION,
            serde_json::json!({
                "input": {
                    "subjectId": node_id,
                    "classifier": reason,
                }
            }),
        )
        .await?
    {
        Ok(data) => data,
        Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
    };
    if data
        .pointer("/minimizeComment/minimizedComment/isMinimized")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Ok(Err(ExecutionResult::failure(
            "GitHub minimizeComment response did not confirm that the comment was minimized",
        )));
    }
    Ok(Ok(()))
}

#[derive(Debug, Deserialize)]
struct RestIssueComment {
    id: u64,
    node_id: Option<String>,
    issue_url: String,
    html_url: Option<String>,
}

#[derive(Debug)]
enum CommentParent {
    Issue {
        repository: String,
        number: u64,
        kind: GithubTargetKind,
        url: Option<String>,
    },
    Discussion {
        repository: String,
        number: u64,
        url: Option<String>,
    },
}

#[derive(Debug)]
struct ResolvedComment {
    node_id: String,
    parent: CommentParent,
}

pub(crate) fn validate_hide_github_issue_comment_config(
    config: &HideGithubIssueCommentConfig,
) -> anyhow::Result<()> {
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
    for reason in &config.allowed_reasons {
        canonical_github_comment_reason(Some(reason))?;
    }
    Ok(())
}

fn validate_actual_repository(
    selected_repository: &str,
    actual_repository: &str,
) -> Result<(), ExecutionResult> {
    if selected_repository.eq_ignore_ascii_case(actual_repository) {
        Ok(())
    } else {
        Err(ExecutionResult::failure(format!(
            "comment belongs to repository '{}', not selected repository '{}'",
            crate::sanitize::neutralize_pipeline_commands(actual_repository),
            crate::sanitize::neutralize_pipeline_commands(selected_repository)
        )))
    }
}

fn parse_rest_issue_url(issue_url: &str, repository: &str) -> anyhow::Result<u64> {
    let url =
        Url::parse(issue_url).map_err(|error| anyhow::anyhow!("invalid issue_url: {error}"))?;
    let segments: Vec<&str> = url
        .path_segments()
        .ok_or_else(|| anyhow::anyhow!("GitHub issue_url cannot be a base URL"))?
        .collect();
    let Some(repos_index) = segments.iter().rposition(|segment| *segment == "repos") else {
        anyhow::bail!("GitHub issue comment response contained an unrecognized issue_url");
    };
    let tail = &segments[repos_index..];
    ensure!(
        tail.len() == 5 && tail[3] == "issues",
        "GitHub issue comment response contained an unrecognized issue_url"
    );
    let actual_repository = format!("{}/{}", tail[1], tail[2]);
    ensure!(
        actual_repository.eq_ignore_ascii_case(repository),
        "GitHub issue comment belongs to repository '{}', not selected repository '{}'",
        actual_repository,
        repository
    );
    let number = tail[4]
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("GitHub issue comment issue_url has no numeric target"))?;
    ensure!(number > 0, "GitHub issue comment target must be positive");
    Ok(number)
}

fn parse_html_target_url(target_url: &str, repository: &str) -> anyhow::Result<u64> {
    let url =
        Url::parse(target_url).map_err(|error| anyhow::anyhow!("invalid comment URL: {error}"))?;
    let segments: Vec<&str> = url
        .path_segments()
        .ok_or_else(|| anyhow::anyhow!("GitHub comment URL cannot be a base URL"))?
        .collect();
    ensure!(
        segments.len() >= 4,
        "GitHub comment URL did not identify an issue or pull request"
    );
    let actual_repository = format!("{}/{}", segments[0], segments[1]);
    ensure!(
        actual_repository.eq_ignore_ascii_case(repository),
        "GitHub comment URL repository '{}' does not match '{}'",
        actual_repository,
        repository
    );
    ensure!(
        matches!(segments[2], "issues" | "pull"),
        "GitHub comment URL did not identify an issue or pull request"
    );
    let number = segments[3]
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("GitHub comment URL has no numeric target"))?;
    ensure!(number > 0, "GitHub comment target must be positive");
    Ok(number)
}

fn repository_from_node(node: &Value, pointer: &str) -> Option<String> {
    node.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
}

async fn validate_issue_parent(
    client: &GithubClient,
    repository: String,
    number: u64,
    filters: GithubMutationFilters<'_>,
    url: Option<String>,
) -> anyhow::Result<Result<CommentParent, ExecutionResult>> {
    let metadata = match client.get_issue(&repository, number).await? {
        Ok(metadata) => metadata,
        Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
    };
    if let Err(result) = validate_github_target_capability(
        &metadata,
        GithubTargetCapabilities::ISSUES_AND_PULL_REQUESTS,
    ) {
        return Ok(Err(result));
    }
    if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
        return Ok(Err(result));
    }
    Ok(Ok(CommentParent::Issue {
        repository,
        number,
        kind: metadata.kind,
        url: url.or(metadata.html_url),
    }))
}

async fn resolve_numeric_comment(
    client: &GithubClient,
    id: u64,
    selected_repository: &str,
    filters: GithubMutationFilters<'_>,
) -> anyhow::Result<Result<ResolvedComment, ExecutionResult>> {
    let response = client
        .send(
            Method::GET,
            client.issue_comment_url(selected_repository, id)?,
            None,
        )
        .await?;
    let response = match response.require_success("Failed to fetch GitHub issue comment") {
        Ok(response) => response,
        Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
    };
    let comment: RestIssueComment = match response.json("Failed to parse GitHub issue comment") {
        Ok(comment) => comment,
        Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
    };
    if comment.id != id {
        return Ok(Err(ExecutionResult::failure(
            "GitHub issue comment response ID did not match the requested comment_id",
        )));
    }
    let Some(node_id) = comment.node_id.filter(|node_id| !node_id.is_empty()) else {
        return Ok(Err(ExecutionResult::failure(
            "GitHub issue comment response contained no GraphQL node_id",
        )));
    };
    let number = match parse_rest_issue_url(&comment.issue_url, selected_repository) {
        Ok(number) => number,
        Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
    };
    let parent = match validate_issue_parent(
        client,
        selected_repository.to_string(),
        number,
        filters,
        comment.html_url,
    )
    .await?
    {
        Ok(parent) => parent,
        Err(result) => return Ok(Err(result)),
    };
    Ok(Ok(ResolvedComment { node_id, parent }))
}

async fn resolve_node_comment(
    client: &GithubClient,
    node_id: &str,
    selected_repository: &str,
    filters: GithubMutationFilters<'_>,
    discussions: bool,
) -> anyhow::Result<Result<ResolvedComment, ExecutionResult>> {
    let data = match client
        .graphql(
            "Failed to resolve GitHub comment node",
            RESOLVE_COMMENT_QUERY,
            serde_json::json!({ "id": node_id }),
        )
        .await?
    {
        Ok(data) => data,
        Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
    };
    let Some(node) = data.get("node").filter(|node| !node.is_null()) else {
        return Ok(Err(ExecutionResult::failure(
            "GitHub comment node was not found",
        )));
    };
    let returned_id = node.get("id").and_then(Value::as_str).unwrap_or_default();
    if returned_id != node_id {
        return Ok(Err(ExecutionResult::failure(
            "GitHub comment node response ID did not match comment_id",
        )));
    }
    let typename = node
        .get("__typename")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match typename {
        "IssueComment" | "PullRequestReviewComment" => {
            let Some(repository) = repository_from_node(node, "/repository/nameWithOwner") else {
                return Ok(Err(ExecutionResult::failure(
                    "GitHub comment node contained no repository",
                )));
            };
            if let Err(result) = validate_actual_repository(selected_repository, &repository) {
                return Ok(Err(result));
            }
            let Some(url) = node.get("url").and_then(Value::as_str) else {
                return Ok(Err(ExecutionResult::failure(
                    "GitHub comment node contained no target URL",
                )));
            };
            let number = match parse_html_target_url(url, &repository) {
                Ok(number) => number,
                Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
            };
            let parent = match validate_issue_parent(
                client,
                repository,
                number,
                filters,
                Some(url.to_string()),
            )
            .await?
            {
                Ok(parent) => parent,
                Err(result) => return Ok(Err(result)),
            };
            Ok(Ok(ResolvedComment {
                node_id: node_id.to_string(),
                parent,
            }))
        }
        "DiscussionComment" => {
            if !discussions {
                return Ok(Err(ExecutionResult::failure(
                    "GitHub discussion comments are disabled; set discussions: true",
                )));
            }
            let Some(repository) =
                repository_from_node(node, "/discussion/repository/nameWithOwner")
            else {
                return Ok(Err(ExecutionResult::failure(
                    "GitHub discussion comment contained no repository",
                )));
            };
            if let Err(result) = validate_actual_repository(selected_repository, &repository) {
                return Ok(Err(result));
            }
            let number = node
                .pointer("/discussion/number")
                .and_then(Value::as_u64)
                .filter(|number| *number > 0);
            let Some(number) = number else {
                return Ok(Err(ExecutionResult::failure(
                    "GitHub discussion comment contained no positive discussion number",
                )));
            };
            let title = node
                .pointer("/discussion/title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let metadata = GithubTargetMetadata {
                number,
                node_id: None,
                title,
                state: String::new(),
                labels: Vec::new(),
                kind: GithubTargetKind::Issue,
                html_url: None,
            };
            if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
                return Ok(Err(result));
            }
            Ok(Ok(ResolvedComment {
                node_id: node_id.to_string(),
                parent: CommentParent::Discussion {
                    repository,
                    number,
                    url: None,
                },
            }))
        }
        _ => Ok(Err(ExecutionResult::failure(format!(
            "GraphQL node '{}' is not a minimizable GitHub issue, pull-request, or discussion comment",
            crate::sanitize::neutralize_pipeline_commands(node_id)
        )))),
    }
}

#[async_trait::async_trait]
impl Executor for HideGithubIssueCommentResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "hide GitHub comment {} as {}",
            self.comment_id,
            self.reason.as_deref().unwrap_or(DEFAULT_REASON)
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        const TOOL: &str = "hide-github-issue-comment";
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
        let config: HideGithubIssueCommentConfig = ctx.get_tool_config(TOOL)?;
        validate_hide_github_issue_comment_config(&config)?;
        let reason = canonical_github_comment_reason(self.reason.as_deref())?;
        if let Err(error) = validate_github_comment_reason_policy(&reason, &config.allowed_reasons)
        {
            return Ok(ExecutionResult::failure(error.to_string()));
        }

        let selected_repository = match resolve_github_repository(
            self.repository.as_deref(),
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos),
            ctx,
        ) {
            Ok(repository) => repository,
            Err(result) => return Ok(result),
        };
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };
        let client = GithubClient::new(&ctx.github_api_url, token)?;

        // Resolve the owning target and apply repository/live-target policy
        // before the first mutation.
        let resolved = match &self.comment_id {
            GithubCommentId::Numeric(id) => {
                resolve_numeric_comment(&client, *id, &selected_repository, filters).await?
            }
            GithubCommentId::Node(node_id) => {
                resolve_node_comment(
                    &client,
                    node_id,
                    &selected_repository,
                    filters,
                    config.discussions,
                )
                .await?
            }
        };
        let resolved = match resolved {
            Ok(resolved) => resolved,
            Err(result) => return Ok(result),
        };

        debug!(
            "Minimizing GitHub comment {} with reason {}",
            self.comment_id, reason
        );
        if let Err(result) = minimize_github_comment(&client, &resolved.node_id, &reason).await? {
            return Ok(result);
        }

        let (repository, number, target_kind, url) = match resolved.parent {
            CommentParent::Issue {
                repository,
                number,
                kind,
                url,
            } => {
                let kind = match kind {
                    GithubTargetKind::Issue => "issue",
                    GithubTargetKind::PullRequest => "pull_request",
                };
                (repository, number, kind, url)
            }
            CommentParent::Discussion {
                repository,
                number,
                url,
            } => (repository, number, "discussion", url),
        };
        info!(
            "Minimized GitHub comment {} in {}#{}",
            self.comment_id, repository, number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Minimized GitHub comment {} in {}#{} as {}",
                self.comment_id, repository, number, reason
            ),
            serde_json::json!({
                "comment_id": self.comment_id.to_string(),
                "comment_node_id": resolved.node_id,
                "reason": reason,
                "target_repo": repository,
                "number": number,
                "target_kind": target_kind,
                "url": url,
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

    fn issue_json(number: u64, pull_request: bool) -> Value {
        let mut issue = serde_json::json!({
            "number": number,
            "node_id": format!("I_{number}"),
            "title": "[agent] Managed target",
            "state": "open",
            "labels": [{"name": "managed"}],
            "html_url": format!("https://github.example/octo/repo/issues/{number}")
        });
        if pull_request {
            issue["pull_request"] = serde_json::json!({});
        }
        issue
    }

    fn context(server: &MockServer, config: Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("hide-github-issue-comment".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        }
    }

    fn numeric_result(id: u64, reason: Option<&str>) -> HideGithubIssueCommentResult {
        HideGithubIssueCommentParams {
            comment_id: GithubCommentId::Numeric(id),
            reason: reason.map(str::to_string),
            repository: None,
        }
        .try_into()
        .unwrap()
    }

    fn minimize_response() -> Value {
        serde_json::json!({
            "data": {
                "minimizeComment": {
                    "minimizedComment": {
                        "isMinimized": true,
                        "minimizedReason": "OUTDATED"
                    }
                }
            }
        })
    }

    #[test]
    fn result_contract_and_comment_id_deserialization() {
        assert_eq!(
            HideGithubIssueCommentResult::NAME,
            "hide-github-issue-comment"
        );
        assert_eq!(HideGithubIssueCommentResult::DEFAULT_MAX, 5);
        let numeric: HideGithubIssueCommentParams =
            serde_json::from_value(serde_json::json!({"comment_id": "42"})).unwrap();
        assert_eq!(numeric.comment_id, GithubCommentId::Numeric(42));
        let node: HideGithubIssueCommentParams =
            serde_json::from_value(serde_json::json!({"comment_id": "IC_kwDOAA"})).unwrap();
        assert_eq!(
            node.comment_id,
            GithubCommentId::Node("IC_kwDOAA".to_string())
        );
    }

    #[test]
    fn validates_ids_reasons_and_repository() {
        assert!(
            HideGithubIssueCommentParams {
                comment_id: GithubCommentId::Numeric(0),
                reason: None,
                repository: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            HideGithubIssueCommentParams {
                comment_id: GithubCommentId::Node("".to_string()),
                reason: None,
                repository: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            HideGithubIssueCommentParams {
                comment_id: GithubCommentId::Numeric(1),
                reason: Some("not-a-reason".to_string()),
                repository: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            HideGithubIssueCommentParams {
                comment_id: GithubCommentId::Numeric(1),
                reason: Some("spam".to_string()),
                repository: Some("octo/$(TOKEN)".to_string()),
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn strict_config_defaults_and_rejects_unknown_fields() {
        let config: HideGithubIssueCommentConfig = serde_yaml::from_str(
            r#"
target-repo: octo/repo
allowed-repos: [octo/other]
required-labels: [managed]
required-title-prefix: "[agent]"
allowed-reasons: [spam, OUTDATED]
discussions: true
max: 3
"#,
        )
        .unwrap();
        assert!(config.discussions);
        assert_eq!(config.max, Some(3));
        assert!(serde_yaml::from_str::<HideGithubIssueCommentConfig>("unknown: true").is_err());
        assert!(!HideGithubIssueCommentConfig::default().discussions);
    }

    #[test]
    fn sanitizes_structural_text_and_formats_dry_run() {
        let mut result = HideGithubIssueCommentResult {
            name: "hide-github-issue-comment".to_string(),
            comment_id: GithubCommentId::Node("IC_\u{0007}1".to_string()),
            reason: Some("out\u{0008}dated".to_string()),
            repository: Some("octo/re\u{0007}po".to_string()),
        };
        result.sanitize_content_fields();
        assert_eq!(result.comment_id, GithubCommentId::Node("IC_1".to_string()));
        assert_eq!(result.reason.as_deref(), Some("outdated"));
        assert_eq!(
            result.dry_run_summary(),
            "hide GitHub comment IC_1 as outdated"
        );
    }

    #[tokio::test]
    async fn dry_run_performs_no_http() {
        let server = MockServer::start().await;
        let mut ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        ctx.dry_run = true;
        let mut result = numeric_result(7, None);
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success);
        assert!(execution.message.contains("[DRY-RUN]"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn policy_rejections_happen_before_http() {
        let server = MockServer::start().await;
        let mut result = numeric_result(7, Some("spam"));
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed-reasons": ["OUTDATED"]
            }),
        );
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("allowed-reasons"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn numeric_id_resolves_owner_and_minimizes_after_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/comments/99"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 99,
                "node_id": "IC_99",
                "issue_url": format!("{}/repos/octo/repo/issues/7", server.uri()),
                "html_url": "https://github.example/octo/repo/issues/7#issuecomment-99"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": MINIMIZE_COMMENT_MUTATION,
                "variables": {
                    "input": {"subjectId": "IC_99", "classifier": "SPAM"}
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(minimize_response()))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["MANAGED"],
                "required-title-prefix": "[agent]",
                "allowed-reasons": ["SPAM"]
            }),
        );
        let mut result = numeric_result(99, Some("spam"));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(
            execution.data.as_ref().unwrap()["comment_node_id"],
            serde_json::json!("IC_99")
        );
    }

    #[tokio::test]
    async fn failed_live_filter_performs_no_graphql_mutation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/comments/99"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 99,
                "node_id": "IC_99",
                "issue_url": format!("{}/repos/octo/repo/issues/7", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["missing"]
            }),
        );
        let mut result = numeric_result(99, None);
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("missing required labels"));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|request| request.method.as_str() == "GET")
        );
    }

    #[tokio::test]
    async fn node_id_resolves_repository_and_pull_request_before_minimizing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": RESOLVE_COMMENT_QUERY,
                "variables": {"id": "PRRC_1"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "node": {
                        "__typename": "PullRequestReviewComment",
                        "id": "PRRC_1",
                        "url": "https://github.example/octo/repo/pull/7#discussion_r1",
                        "repository": {"nameWithOwner": "octo/repo"}
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, true)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": MINIMIZE_COMMENT_MUTATION,
                "variables": {
                    "input": {"subjectId": "PRRC_1", "classifier": "OUTDATED"}
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(minimize_response()))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        let mut result: HideGithubIssueCommentResult = HideGithubIssueCommentParams {
            comment_id: GithubCommentId::Node("PRRC_1".to_string()),
            reason: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(
            execution.data.as_ref().unwrap()["target_kind"],
            serde_json::json!("pull_request")
        );
    }

    #[tokio::test]
    async fn discussion_requires_opt_in_and_applies_title_filter() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": RESOLVE_COMMENT_QUERY,
                "variables": {"id": "DC_1"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "node": {
                        "__typename": "DiscussionComment",
                        "id": "DC_1",
                        "discussion": {
                            "number": 4,
                            "title": "[agent] Discussion",
                            "repository": {"nameWithOwner": "octo/repo"}
                        }
                    }
                }
            })))
            .mount(&server)
            .await;

        let mut disabled: HideGithubIssueCommentResult = HideGithubIssueCommentParams {
            comment_id: GithubCommentId::Node("DC_1".to_string()),
            reason: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let disabled_ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        let execution = disabled.execute_sanitized(&disabled_ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("discussions: true"));

        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": MINIMIZE_COMMENT_MUTATION,
                "variables": {
                    "input": {"subjectId": "DC_1", "classifier": "OUTDATED"}
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(minimize_response()))
            .expect(1)
            .mount(&server)
            .await;
        let enabled_ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "discussions": true,
                "required-title-prefix": "[agent]"
            }),
        );
        let mut enabled: HideGithubIssueCommentResult = HideGithubIssueCommentParams {
            comment_id: GithubCommentId::Node("DC_1".to_string()),
            reason: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = enabled.execute_sanitized(&enabled_ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
    }

    #[tokio::test]
    async fn rest_and_graphql_failures_are_explicit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/comments/99"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "message": "comment not found"
            })))
            .mount(&server)
            .await;
        let ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        let mut missing = numeric_result(99, None);
        let execution = missing.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("HTTP 404"));

        let graphql_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/comments/99"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 99,
                "node_id": "IC_99",
                "issue_url": format!("{}/repos/octo/repo/issues/7", graphql_server.uri())
            })))
            .mount(&graphql_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .mount(&graphql_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [{"type": "FORBIDDEN", "message": "cannot minimize"}]
            })))
            .mount(&graphql_server)
            .await;
        let graphql_ctx = context(
            &graphql_server,
            serde_json::json!({"target-repo": "octo/repo"}),
        );
        let mut denied = numeric_result(99, None);
        let execution = denied.execute_sanitized(&graphql_ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("FORBIDDEN"));
        assert!(execution.message.contains("cannot minimize"));
    }

    #[tokio::test]
    async fn missing_configuration_and_token_fail_cleanly() {
        let mut result = numeric_result(1, None);
        let execution = result
            .execute_sanitized(&ExecutionContext::default())
            .await
            .unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("not configured"));

        let server = MockServer::start().await;
        let mut ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        ctx.github_token = None;
        let mut result = numeric_result(1, None);
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("ADO_AW_GITHUB_TOKEN"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
