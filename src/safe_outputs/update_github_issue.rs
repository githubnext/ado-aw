//! `update-github-issue` safe output.

use anyhow::ensure;
use log::{debug, info};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubClient, GithubIssueNumber,
    GithubMutationFilters, GithubRepositoryPolicy, GithubTargetCapabilities, GithubTargetKind,
    GithubTargetMetadata, Validate, build_github_trace_footer, resolve_github_issue_target,
    validate_blocked_first_globs, validate_github_mutation_filter_config,
    validate_github_mutation_filters, validate_github_repository,
    validate_github_target_capability,
};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

const MAX_TITLE_LEN: usize = 256;
const MAX_BODY_LEN: usize = 65_536;
const MAX_LABELS: usize = 100;
const MAX_ASSIGNEES: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GithubIssueStatus {
    Open,
    Closed,
}

impl GithubIssueStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum GithubBodyOperation {
    Append,
    Prepend,
    Replace,
    ReplaceIsland,
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdateGithubIssueParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// Set the issue or pull request state.
    #[serde(default)]
    pub status: Option<GithubIssueStatus>,
    /// Replace the title.
    #[serde(default)]
    pub title: Option<String>,
    /// Body content used by `operation`.
    #[serde(default)]
    pub body: Option<String>,
    /// Body update operation. Defaults to `append`.
    #[serde(default)]
    pub operation: Option<GithubBodyOperation>,
    /// Replace all labels with this list.
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    /// Replace all assignees with this list.
    #[serde(default)]
    pub assignees: Option<Vec<String>>,
    /// Assign an existing milestone by number.
    #[serde(default)]
    pub milestone: Option<u64>,
    /// Optional target repository.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for UpdateGithubIssueParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.issue_number.validate("issue_number")?;
        ensure!(
            self.status.is_some()
                || self.title.is_some()
                || self.body.is_some()
                || self.labels.is_some()
                || self.assignees.is_some()
                || self.milestone.is_some(),
            "at least one of status, title, body, labels, assignees, or milestone is required"
        );
        if let Some(title) = self.title.as_deref() {
            ensure!(!title.trim().is_empty(), "title must not be empty");
            ensure!(
                title.len() <= MAX_TITLE_LEN,
                "title must be {MAX_TITLE_LEN} characters or fewer"
            );
        }
        if let Some(body) = self.body.as_deref() {
            ensure!(
                body.len() <= MAX_BODY_LEN,
                "body must be {MAX_BODY_LEN} characters or fewer"
            );
        } else {
            ensure!(
                self.operation.is_none(),
                "operation may only be provided when body is provided"
            );
        }
        if let Some(labels) = &self.labels {
            ensure!(
                labels.len() <= MAX_LABELS,
                "labels must contain at most {MAX_LABELS} entries"
            );
            for label in labels {
                ensure!(!label.is_empty(), "labels entries must not be empty");
                reject_pipeline_injection(label, "update-github-issue.labels")?;
            }
        }
        if let Some(assignees) = &self.assignees {
            ensure!(
                assignees.len() <= MAX_ASSIGNEES,
                "assignees must contain at most {MAX_ASSIGNEES} entries"
            );
            for assignee in assignees {
                ensure!(!assignee.is_empty(), "assignees entries must not be empty");
                reject_pipeline_injection(assignee, "update-github-issue.assignees")?;
            }
        }
        if let Some(milestone) = self.milestone {
            ensure!(milestone > 0, "milestone must be positive");
        }
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "update-github-issue",
    write = true,
    params = UpdateGithubIssueParams,
    default_max = 1,
    /// Result of updating a GitHub issue or pull request.
    pub struct UpdateGithubIssueResult {
        issue_number: GithubIssueNumber,
        #[serde(default)]
        status: Option<GithubIssueStatus>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        operation: Option<GithubBodyOperation>,
        #[serde(default)]
        labels: Option<Vec<String>>,
        #[serde(default)]
        assignees: Option<Vec<String>>,
        #[serde(default)]
        milestone: Option<u64>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for UpdateGithubIssueResult {
    fn sanitize_content_fields(&mut self) {
        self.title = self.title.as_deref().map(sanitize_text);
        self.body = self.body.as_deref().map(sanitize_text);
        self.labels = self
            .labels
            .as_ref()
            .map(|labels| labels.iter().map(|label| sanitize_config(label)).collect());
        self.assignees = self.assignees.as_ref().map(|assignees| {
            assignees
                .iter()
                .map(|assignee| sanitize_config(assignee))
                .collect()
        });
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateGithubIssueConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    /// Allow state changes.
    #[serde(default)]
    #[sanitize_config(skip)]
    pub status: bool,
    /// Allow title replacement.
    #[serde(default)]
    #[sanitize_config(skip)]
    pub title: bool,
    /// Allow body changes.
    #[serde(default)]
    #[sanitize_config(skip)]
    pub body: bool,
    /// Allow replacing labels.
    #[serde(default)]
    #[sanitize_config(skip)]
    pub labels: bool,
    /// Allow replacing assignees.
    #[serde(default)]
    #[sanitize_config(skip)]
    pub assignees: bool,
    /// Allow milestone assignment.
    #[serde(default)]
    #[sanitize_config(skip)]
    pub milestone: bool,
    /// Case-insensitive `*` glob allowlist for requested labels.
    #[serde(default, rename = "allowed-labels")]
    pub allowed_labels: Vec<String>,
    /// Include the standard ado-aw trace footer in body updates.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub footer: bool,
    /// Permit issue targets.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub issues: bool,
    /// Permit pull request targets through the shared issues endpoint.
    #[serde(default = "default_true", rename = "pull-requests")]
    #[sanitize_config(skip)]
    pub pull_requests: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

impl Default for UpdateGithubIssueConfig {
    fn default() -> Self {
        Self {
            target_repo: None,
            allowed_repos: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            status: false,
            title: false,
            body: false,
            labels: false,
            assignees: false,
            milestone: false,
            allowed_labels: Vec::new(),
            footer: true,
            issues: true,
            pull_requests: true,
            max: None,
        }
    }
}

pub(crate) fn validate_update_github_issue_config(
    config: &UpdateGithubIssueConfig,
) -> anyhow::Result<()> {
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
    ensure!(
        config.issues || config.pull_requests,
        "at least one of issues or pull-requests must be true"
    );
    for label in &config.allowed_labels {
        ensure!(
            !label.is_empty(),
            "allowed-labels entries must not be empty"
        );
        reject_pipeline_injection(label, "update-github-issue.allowed-labels")?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RawUpdateTarget {
    number: u64,
    node_id: Option<String>,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    #[serde(default)]
    labels: Vec<RawLabel>,
    #[serde(default)]
    assignees: Vec<RawAssignee>,
    milestone: Option<RawMilestone>,
    pull_request: Option<Value>,
    html_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawAssignee {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RawMilestone {
    number: u64,
    title: String,
}

impl RawUpdateTarget {
    fn metadata(&self) -> GithubTargetMetadata {
        GithubTargetMetadata {
            number: self.number,
            node_id: self.node_id.clone(),
            title: self.title.clone(),
            state: self.state.clone(),
            labels: self.labels.iter().map(|label| label.name.clone()).collect(),
            kind: if self.pull_request.is_some() {
                GithubTargetKind::PullRequest
            } else {
                GithubTargetKind::Issue
            },
            html_url: self.html_url.clone(),
        }
    }
}

fn repository_route(client: &GithubClient, repository: &str, tail: &[&str]) -> anyhow::Result<Url> {
    validate_github_repository(repository)?;
    let (owner, name) = repository
        .split_once('/')
        .expect("validated GitHub repository contains slash");
    let mut url = client.rest_api_url().clone();
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("GitHub API URL cannot be a base URL"))?;
        path.pop_if_empty();
        path.push("repos");
        path.push(owner);
        path.push(name);
        for segment in tail {
            path.push(segment);
        }
    }
    Ok(url)
}

fn body_with_footer(body: &str, include_footer: bool, ctx: &ExecutionContext) -> String {
    if include_footer {
        format!("{body}\n\n{}", build_github_trace_footer(ctx))
    } else {
        body.to_string()
    }
}

fn status_island_markers(ctx: &ExecutionContext) -> Result<(String, String), ExecutionResult> {
    let Some(definition_id) = ctx.definition_id else {
        return Err(ExecutionResult::failure(
            "SYSTEM_DEFINITIONID is required for replace-island",
        ));
    };
    Ok((
        format!("<!-- ado-aw-status-island-start:pipeline-definition-id={definition_id} -->"),
        format!("<!-- ado-aw-status-island-end:pipeline-definition-id={definition_id} -->"),
    ))
}

fn replace_status_island(
    current: &str,
    replacement: &str,
    ctx: &ExecutionContext,
) -> Result<String, ExecutionResult> {
    let (start_marker, end_marker) = status_island_markers(ctx)?;
    let starts: Vec<usize> = current
        .match_indices(&start_marker)
        .map(|(index, _)| index)
        .collect();
    let ends: Vec<usize> = current
        .match_indices(&end_marker)
        .map(|(index, _)| index)
        .collect();
    if starts.len() != 1 || ends.len() != 1 {
        return Err(ExecutionResult::failure(format!(
            "replace-island requires exactly one matching status island for pipeline \
             definition {}; found {} start marker(s) and {} end marker(s)",
            ctx.definition_id.unwrap_or_default(),
            starts.len(),
            ends.len()
        )));
    }
    let start = starts[0];
    let end = ends[0];
    if end <= start {
        return Err(ExecutionResult::failure(
            "replace-island status island markers are out of order",
        ));
    }
    let end_after_marker = end + end_marker.len();
    Ok(format!(
        "{}{}\n{}\n{}{}",
        &current[..start],
        start_marker,
        replacement,
        end_marker,
        &current[end_after_marker..]
    ))
}

fn build_updated_body(
    current: &str,
    new_content: &str,
    operation: GithubBodyOperation,
    include_footer: bool,
    ctx: &ExecutionContext,
) -> Result<String, ExecutionResult> {
    let section = body_with_footer(new_content, include_footer, ctx);
    let updated = match operation {
        GithubBodyOperation::Append => {
            if current.is_empty() {
                section
            } else {
                format!("{current}\n\n---\n\n{section}")
            }
        }
        GithubBodyOperation::Prepend => {
            if current.is_empty() {
                section
            } else {
                format!("{section}\n\n---\n\n{current}")
            }
        }
        GithubBodyOperation::Replace => section,
        GithubBodyOperation::ReplaceIsland => replace_status_island(current, &section, ctx)?,
    };
    if updated.len() > MAX_BODY_LEN {
        return Err(ExecutionResult::failure(format!(
            "updated body exceeds GitHub's {MAX_BODY_LEN}-character limit"
        )));
    }
    Ok(updated)
}

impl UpdateGithubIssueResult {
    fn requested_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.status.is_some() {
            fields.push("status");
        }
        if self.title.is_some() {
            fields.push("title");
        }
        if self.body.is_some() {
            fields.push("body");
        }
        if self.labels.is_some() {
            fields.push("labels");
        }
        if self.assignees.is_some() {
            fields.push("assignees");
        }
        if self.milestone.is_some() {
            fields.push("milestone");
        }
        fields
    }

    fn validate_opt_ins(&self, config: &UpdateGithubIssueConfig) -> Result<(), ExecutionResult> {
        for (requested, enabled, field) in [
            (self.status.is_some(), config.status, "status"),
            (self.title.is_some(), config.title, "title"),
            (self.body.is_some(), config.body, "body"),
            (self.labels.is_some(), config.labels, "labels"),
            (self.assignees.is_some(), config.assignees, "assignees"),
            (self.milestone.is_some(), config.milestone, "milestone"),
        ] {
            if requested && !enabled {
                return Err(ExecutionResult::failure(format!(
                    "update-github-issue field '{field}' is not enabled by configuration"
                )));
            }
        }
        if let Some(labels) = &self.labels
            && let Err(result) =
                validate_blocked_first_globs(labels, &config.allowed_labels, &[], "label")
        {
            return Err(result);
        }
        Ok(())
    }

    async fn fetch_target(
        &self,
        client: &GithubClient,
        repository: &str,
        number: u64,
    ) -> anyhow::Result<Result<RawUpdateTarget, ExecutionResult>> {
        let response = client
            .send(Method::GET, client.issue_url(repository, number)?, None)
            .await?;
        let response = match response.require_success("Failed to fetch GitHub issue") {
            Ok(response) => response,
            Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
        };
        match response.json("Failed to parse GitHub issue") {
            Ok(target) => Ok(Ok(target)),
            Err(error) => Ok(Err(ExecutionResult::failure(error.to_string()))),
        }
    }

    async fn preflight_labels(
        &self,
        client: &GithubClient,
        repository: &str,
    ) -> anyhow::Result<Result<(), ExecutionResult>> {
        let Some(labels) = &self.labels else {
            return Ok(Ok(()));
        };
        for label in labels {
            let response = client
                .send(
                    Method::GET,
                    repository_route(client, repository, &["labels", label])?,
                    None,
                )
                .await?;
            if !response.is_success() {
                let error = response
                    .require_success("Failed to validate GitHub label")
                    .expect_err("non-success response must produce an API error");
                return Ok(Err(ExecutionResult::failure(format!(
                    "Label '{}' failed preflight: {}",
                    crate::sanitize::neutralize_pipeline_commands(label),
                    error
                ))));
            }
        }
        Ok(Ok(()))
    }

    async fn preflight_assignees(
        &self,
        client: &GithubClient,
        repository: &str,
    ) -> anyhow::Result<Result<(), ExecutionResult>> {
        let Some(assignees) = &self.assignees else {
            return Ok(Ok(()));
        };
        for assignee in assignees {
            let response = client
                .send(
                    Method::GET,
                    repository_route(client, repository, &["assignees", assignee])?,
                    None,
                )
                .await?;
            if !response.is_success() {
                let error = response
                    .require_success("Failed to validate GitHub assignee")
                    .expect_err("non-success response must produce an API error");
                return Ok(Err(ExecutionResult::failure(format!(
                    "Assignee '{}' failed preflight: {}",
                    crate::sanitize::neutralize_pipeline_commands(assignee),
                    error
                ))));
            }
        }
        Ok(Ok(()))
    }

    async fn preflight_milestone(
        &self,
        client: &GithubClient,
        repository: &str,
    ) -> anyhow::Result<Result<(), ExecutionResult>> {
        let Some(milestone) = self.milestone else {
            return Ok(Ok(()));
        };
        let milestones = match client.list_milestones(repository).await? {
            Ok(milestones) => milestones,
            Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
        };
        if milestones
            .iter()
            .any(|candidate| candidate.number == milestone)
        {
            Ok(Ok(()))
        } else {
            Ok(Err(ExecutionResult::failure(format!(
                "Milestone #{milestone} does not exist in repository {repository}"
            ))))
        }
    }
}

#[async_trait::async_trait]
impl Executor for UpdateGithubIssueResult {
    fn dry_run_summary(&self) -> String {
        let target = match &self.issue_number {
            GithubIssueNumber::Number(number) => format!("#{number}"),
            GithubIssueNumber::Temporary(id) => id.canonical(),
        };
        format!(
            "update GitHub issue {target}: {}",
            self.requested_fields().join(", ")
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if !ctx.tool_configs.contains_key("update-github-issue") {
            return Ok(ExecutionResult::failure(
                "update-github-issue is not configured for this workflow",
            ));
        }
        let Some(token) = ctx.github_token.as_ref() else {
            return Ok(ExecutionResult::failure(
                "ADO_AW_GITHUB_TOKEN is not set; configure safe-outputs.github-token \
                 or safe-outputs.github-app",
            ));
        };
        let config: UpdateGithubIssueConfig = ctx.get_tool_config("update-github-issue")?;
        validate_update_github_issue_config(&config)?;
        if let Err(result) = self.validate_opt_ins(&config) {
            return Ok(result);
        }
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };
        if let Err(error) = validate_github_mutation_filter_config(filters) {
            return Ok(ExecutionResult::failure(error.to_string()));
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

        // Fetch and validate every dependency before the single PATCH write.
        let current = match self
            .fetch_target(&client, &target.repository, target.number)
            .await?
        {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        let metadata = current.metadata();
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
        if let Err(result) = self.preflight_labels(&client, &target.repository).await? {
            return Ok(result);
        }
        if let Err(result) = self
            .preflight_assignees(&client, &target.repository)
            .await?
        {
            return Ok(result);
        }
        if let Err(result) = self
            .preflight_milestone(&client, &target.repository)
            .await?
        {
            return Ok(result);
        }

        let mut payload = Map::new();
        if let Some(status) = self.status {
            payload.insert(
                "state".to_string(),
                Value::String(status.as_str().to_string()),
            );
        }
        if let Some(title) = self.title.as_ref() {
            payload.insert("title".to_string(), Value::String(title.clone()));
        }
        if let Some(body) = self.body.as_deref() {
            let updated = match build_updated_body(
                current.body.as_deref().unwrap_or_default(),
                body,
                self.operation.unwrap_or(GithubBodyOperation::Append),
                config.footer,
                ctx,
            ) {
                Ok(body) => body,
                Err(result) => return Ok(result),
            };
            payload.insert("body".to_string(), Value::String(updated));
        }
        if let Some(labels) = self.labels.as_ref() {
            payload.insert("labels".to_string(), serde_json::json!(labels));
        }
        if let Some(assignees) = self.assignees.as_ref() {
            payload.insert("assignees".to_string(), serde_json::json!(assignees));
        }
        if let Some(milestone) = self.milestone {
            payload.insert("milestone".to_string(), serde_json::json!(milestone));
        }

        debug!(
            "Updating GitHub target {}#{} fields: {}",
            target.repository,
            target.number,
            self.requested_fields().join(", ")
        );
        let response = client
            .send(
                Method::PATCH,
                client.issue_url(&target.repository, target.number)?,
                Some(&Value::Object(payload)),
            )
            .await?;
        if !response.is_success() {
            let error = response
                .require_success("Failed to update GitHub issue")
                .expect_err("non-success response must produce an API error");
            return Ok(ExecutionResult::failure(error.to_string()));
        }

        info!(
            "Updated GitHub target {}#{}",
            target.repository, target.number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Updated GitHub target {}#{}: {}",
                target.repository,
                target.number,
                self.requested_fields().join(", ")
            ),
            serde_json::json!({
                "number": target.number,
                "target_repo": target.repository,
                "target_kind": match metadata.kind {
                    GithubTargetKind::Issue => "issue",
                    GithubTargetKind::PullRequest => "pull_request",
                },
                "fields": self.requested_fields(),
                "previous": {
                    "title": current.title,
                    "state": current.state,
                    "labels": current.labels.into_iter().map(|label| label.name).collect::<Vec<_>>(),
                    "assignees": current.assignees.into_iter().map(|user| user.login).collect::<Vec<_>>(),
                    "milestone": current.milestone.map(|milestone| {
                        serde_json::json!({
                            "number": milestone.number,
                            "title": milestone.title,
                        })
                    }),
                },
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
        tool_configs.insert("update-github-issue".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            definition_id: Some(123),
            ..Default::default()
        }
    }

    fn target(number: u64, pull_request: bool) -> serde_json::Value {
        let mut target = serde_json::json!({
            "number": number,
            "node_id": format!("I_{number}"),
            "title": "[agent] Existing",
            "body": "Existing body",
            "state": "open",
            "labels": [{"name": "bug"}],
            "assignees": [{"login": "octocat"}],
            "milestone": {"number": 1, "title": "v1"},
            "html_url": format!("https://github.example/octo/repo/issues/{number}")
        });
        if pull_request {
            target["pull_request"] = serde_json::json!({"url": "https://api.example/pr"});
        }
        target
    }

    fn params() -> UpdateGithubIssueParams {
        UpdateGithubIssueParams {
            issue_number: GithubIssueNumber::Number(7),
            status: None,
            title: Some("Updated title".to_string()),
            body: None,
            operation: None,
            labels: None,
            assignees: None,
            milestone: None,
            repository: None,
        }
    }

    #[test]
    fn contract_name_and_budget() {
        assert_eq!(UpdateGithubIssueResult::NAME, "update-github-issue");
        assert_eq!(UpdateGithubIssueResult::DEFAULT_MAX, 1);
    }

    #[test]
    fn requires_at_least_one_change() {
        let mut params = params();
        params.title = None;
        assert!(params.validate().is_err());
    }

    #[test]
    fn validates_operations_and_collection_limits() {
        let mut operation_without_body = params();
        operation_without_body.operation = Some(GithubBodyOperation::Replace);
        assert!(operation_without_body.validate().is_err());

        let mut too_many_labels = params();
        too_many_labels.labels = Some(vec!["label".to_string(); MAX_LABELS + 1]);
        assert!(too_many_labels.validate().is_err());
    }

    #[test]
    fn config_is_strict_and_fields_default_to_opt_out() {
        assert!(
            serde_json::from_value::<UpdateGithubIssueConfig>(serde_json::json!({
                "allow-body": true
            }))
            .is_err()
        );
        let config: UpdateGithubIssueConfig =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!config.status);
        assert!(!config.title);
        assert!(!config.body);
        assert!(!config.labels);
        assert!(!config.assignees);
        assert!(!config.milestone);
        assert!(config.issues);
        assert!(config.pull_requests);
        assert!(config.footer);
    }

    #[test]
    fn body_operations_are_deterministic() {
        let ctx = ExecutionContext {
            definition_id: Some(123),
            ..Default::default()
        };
        assert_eq!(
            build_updated_body("old", "new", GithubBodyOperation::Append, false, &ctx).unwrap(),
            "old\n\n---\n\nnew"
        );
        assert_eq!(
            build_updated_body("old", "new", GithubBodyOperation::Prepend, false, &ctx).unwrap(),
            "new\n\n---\n\nold"
        );
        assert_eq!(
            build_updated_body("old", "new", GithubBodyOperation::Replace, false, &ctx).unwrap(),
            "new"
        );
    }

    #[test]
    fn replace_island_is_strict_and_preserves_surrounding_body() {
        let ctx = ExecutionContext {
            definition_id: Some(123),
            ..Default::default()
        };
        let existing = "before\n<!-- ado-aw-status-island-start:pipeline-definition-id=123 -->\nold\n<!-- ado-aw-status-island-end:pipeline-definition-id=123 -->\nafter";
        let updated = build_updated_body(
            existing,
            "new",
            GithubBodyOperation::ReplaceIsland,
            false,
            &ctx,
        )
        .unwrap();
        assert!(updated.starts_with("before\n"));
        assert!(updated.ends_with("\nafter"));
        assert!(updated.contains("\nnew\n"));
        assert!(!updated.contains("\nold\n"));
        assert!(
            build_updated_body(
                "no markers",
                "new",
                GithubBodyOperation::ReplaceIsland,
                false,
                &ctx
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn all_requested_values_are_preflighted_before_single_patch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(target(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/labels/enhancement"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "enhancement"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/assignees/hubot"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/milestones"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "number": 2,
                    "title": "v2",
                    "state": "open",
                    "node_id": "M_2"
                }])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({
                "state": "closed",
                "title": "Updated title",
                "body": "Replacement",
                "labels": ["enhancement"],
                "assignees": ["hubot"],
                "milestone": 2
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "status": true,
                "title": true,
                "body": true,
                "labels": true,
                "assignees": true,
                "milestone": true,
                "allowed-labels": ["enhance*"],
                "footer": false,
                "required-labels": ["BUG"],
                "required-title-prefix": "[agent]"
            }),
        );
        let mut result: UpdateGithubIssueResult = UpdateGithubIssueParams {
            issue_number: GithubIssueNumber::Number(7),
            status: Some(GithubIssueStatus::Closed),
            title: Some("Updated title".to_string()),
            body: Some("Replacement".to_string()),
            operation: Some(GithubBodyOperation::Replace),
            labels: Some(vec!["enhancement".to_string()]),
            assignees: Some(vec!["hubot".to_string()]),
            milestone: Some(2),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
    }

    #[tokio::test]
    async fn preflight_failure_prevents_patch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(target(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/labels/missing"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "message": "Not Found ##vso[task.setvariable variable=oops]x"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "labels": true,
                "allowed-labels": ["*"]
            }),
        );
        let mut result: UpdateGithubIssueResult = UpdateGithubIssueParams {
            issue_number: GithubIssueNumber::Number(7),
            status: None,
            title: None,
            body: None,
            operation: None,
            labels: Some(vec!["missing".to_string()]),
            assignees: None,
            milestone: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("`##vso[`"));
        assert!(!execution.message.contains("##vso[task."));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn pull_request_parity_is_configurable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(target(7, true)))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "title": true,
                "issues": true,
                "pull-requests": false
            }),
        );
        let mut result: UpdateGithubIssueResult = params().try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("pull requests"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn disabled_field_and_filter_fail_before_patch() {
        let server = MockServer::start().await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "body": true
            }),
        );
        let mut disabled: UpdateGithubIssueResult = params().try_into().unwrap();
        assert!(!disabled.execute_sanitized(&ctx).await.unwrap().success);
        assert!(server.received_requests().await.unwrap().is_empty());

        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(target(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "title": true,
                "required-title-prefix": "[other]"
            }),
        );
        let mut filtered: UpdateGithubIssueResult = params().try_into().unwrap();
        let execution = filtered.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("required-title-prefix"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dry_run_makes_no_requests() {
        let server = MockServer::start().await;
        let mut ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "title": true
            }),
        );
        ctx.dry_run = true;
        let mut result: UpdateGithubIssueResult = params().try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success);
        assert!(execution.message.contains("[DRY-RUN]"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
