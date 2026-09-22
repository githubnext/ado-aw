//! `update-pull-request` GitHub safe output.

use anyhow::ensure;
use log::{debug, info, warn};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use ado_aw_derive::SanitizeConfig;

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubBodyOperation, GithubClient,
    GithubMutationFilters, GithubRepositoryPolicy, GithubTargetCapabilities, GithubTargetKind,
    GithubTargetMetadata, Validate, build_github_trace_footer, resolve_github_repository,
    validate_github_mutation_filter_config, validate_github_mutation_filters,
    validate_github_repository, validate_github_target_capability,
};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;

const MAX_TITLE_LEN: usize = 256;
const MAX_BODY_LEN: usize = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum GithubPullRequestNumber {
    Number(u64),
    String(String),
}

impl GithubPullRequestNumber {
    fn parse(&self, field: &str) -> anyhow::Result<u64> {
        let number = match self {
            Self::Number(number) => *number,
            Self::String(value) => value
                .trim()
                .strip_prefix('#')
                .unwrap_or_else(|| value.trim())
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("{field} must be a positive pull request number"))?,
        };
        ensure!(number > 0, "{field} must be positive");
        Ok(number)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdatePullRequestParams {
    /// New pull request title.
    #[serde(default)]
    pub title: Option<String>,
    /// Pull request body content in Markdown.
    #[serde(default)]
    pub body: Option<String>,
    /// Body update operation. Defaults to the configured operation, then `replace`.
    #[serde(default)]
    pub operation: Option<GithubBodyOperation>,
    /// When true, update the PR branch from the base branch before other updates.
    #[serde(default, rename = "update_branch", alias = "updateBranch")]
    pub update_branch: Option<bool>,
    /// Pull request number. Required when front matter uses `target: "*"`.
    #[serde(default, rename = "pull_request_number", alias = "pullRequestNumber")]
    pub pull_request_number: Option<GithubPullRequestNumber>,
    /// Alias for pull_request_number.
    #[serde(default, rename = "pr_number", alias = "prNumber")]
    pub pr_number: Option<GithubPullRequestNumber>,
    /// Alias for pull_request_number.
    #[serde(default)]
    pub pr: Option<GithubPullRequestNumber>,
    /// Optional target repository.
    #[serde(default)]
    pub repository: Option<String>,
}

impl UpdatePullRequestParams {
    fn requested_number(&self) -> anyhow::Result<Option<u64>> {
        let mut found = None;
        for (field, value) in [
            ("pull_request_number", self.pull_request_number.as_ref()),
            ("pr_number", self.pr_number.as_ref()),
            ("pr", self.pr.as_ref()),
        ] {
            if let Some(value) = value {
                let number = value.parse(field)?;
                if let Some(existing) = found {
                    ensure!(
                        existing == number,
                        "pull request number aliases must all refer to the same PR"
                    );
                }
                found = Some(number);
            }
        }
        Ok(found)
    }
}

impl Validate for UpdatePullRequestParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.title.is_some() || self.body.is_some() || self.update_branch == Some(true),
            "at least one of title, body, or update_branch: true is required"
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
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        let _ = self.requested_number()?;
        Ok(())
    }
}

tool_result! {
    name = "update-pull-request",
    write = true,
    params = UpdatePullRequestParams,
    default_max = 1,
    /// Result of updating a GitHub pull request.
    pub struct UpdatePullRequestResult {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        operation: Option<GithubBodyOperation>,
        #[serde(default, rename = "update_branch")]
        update_branch: Option<bool>,
        #[serde(default, rename = "pull_request_number")]
        pull_request_number: Option<GithubPullRequestNumber>,
        #[serde(default, rename = "pr_number")]
        pr_number: Option<GithubPullRequestNumber>,
        #[serde(default)]
        pr: Option<GithubPullRequestNumber>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for UpdatePullRequestResult {
    fn sanitize_content_fields(&mut self) {
        self.title = self.title.as_deref().map(sanitize_text);
        self.body = self.body.as_deref().map(sanitize_text);
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

fn default_true() -> bool {
    true
}

fn default_operation() -> GithubBodyOperation {
    GithubBodyOperation::Replace
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UpdatePullRequestTarget {
    Number(u64),
    Named(String),
}

impl Default for UpdatePullRequestTarget {
    fn default() -> Self {
        Self::Named("triggering".to_string())
    }
}

#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePullRequestConfig {
    /// Whether title updates are enabled. Defaults to true.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub title: bool,
    /// Whether body updates are enabled. Defaults to true.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub body: bool,
    /// Default branch update policy. Defaults to false.
    #[serde(default, rename = "update-branch")]
    #[sanitize_config(skip)]
    pub update_branch: bool,
    /// gh-aw-compatible stacked-PR fallback knob. Parsed for config parity.
    #[serde(default = "default_true", rename = "sync-stack")]
    #[sanitize_config(skip)]
    pub sync_stack: bool,
    /// Include the standard ado-aw trace footer in body updates.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub footer: bool,
    /// Body update operation. Defaults to replace.
    #[serde(default = "default_operation")]
    #[sanitize_config(skip)]
    pub operation: GithubBodyOperation,
    /// `"triggering"` (default), `"*"`, or a fixed PR number.
    #[serde(default)]
    pub target: UpdatePullRequestTarget,
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

impl Default for UpdatePullRequestConfig {
    fn default() -> Self {
        Self {
            title: true,
            body: true,
            update_branch: false,
            sync_stack: true,
            footer: true,
            operation: GithubBodyOperation::Replace,
            target: UpdatePullRequestTarget::default(),
            target_repo: None,
            allowed_repos: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            max: None,
        }
    }
}

pub(crate) fn validate_update_pull_request_config(
    config: &UpdatePullRequestConfig,
) -> anyhow::Result<()> {
    match &config.target {
        UpdatePullRequestTarget::Number(number) => {
            ensure!(*number > 0, "target PR number must be positive");
        }
        UpdatePullRequestTarget::Named(target) => {
            ensure!(
                matches!(target.as_str(), "triggering" | "*"),
                "target must be \"triggering\", \"*\", or a positive pull request number"
            );
        }
    }
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RawPullRequestTarget {
    number: u64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    #[serde(default)]
    labels: Vec<RawLabel>,
    html_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawLabel {
    name: String,
}

impl RawPullRequestTarget {
    fn metadata(&self) -> GithubTargetMetadata {
        GithubTargetMetadata {
            number: self.number,
            node_id: None,
            title: self.title.clone(),
            state: self.state.clone(),
            labels: self.labels.iter().map(|label| label.name.clone()).collect(),
            kind: GithubTargetKind::PullRequest,
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

fn island_markers(ctx: &ExecutionContext) -> Result<(String, String), ExecutionResult> {
    let Some(definition_id) = ctx.definition_id else {
        return Err(ExecutionResult::failure(
            "SYSTEM_DEFINITIONID is required for replace-island",
        ));
    };
    Ok((
        format!("<!-- ado-aw-pr-island-start:pipeline-definition-id={definition_id} -->"),
        format!("<!-- ado-aw-pr-island-end:pipeline-definition-id={definition_id} -->"),
    ))
}

fn replace_island(
    current: &str,
    replacement: &str,
    ctx: &ExecutionContext,
) -> Result<String, ExecutionResult> {
    let (start_marker, end_marker) = island_markers(ctx)?;
    let starts: Vec<usize> = current
        .match_indices(&start_marker)
        .map(|(index, _)| index)
        .collect();
    let ends: Vec<usize> = current
        .match_indices(&end_marker)
        .map(|(index, _)| index)
        .collect();
    if starts.len() != 1 || ends.len() != 1 {
        let island = format!("{start_marker}\n{replacement}\n{end_marker}");
        return Ok(if current.is_empty() {
            island
        } else {
            format!("{current}\n\n---\n\n{island}")
        });
    }
    let start = starts[0];
    let end = ends[0];
    if end <= start {
        return Err(ExecutionResult::failure(
            "replace-island markers are out of order",
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
        GithubBodyOperation::ReplaceIsland => replace_island(current, &section, ctx)?,
    };
    if updated.len() > MAX_BODY_LEN {
        return Err(ExecutionResult::failure(format!(
            "updated body exceeds GitHub's {MAX_BODY_LEN}-character limit"
        )));
    }
    Ok(updated)
}

impl UpdatePullRequestResult {
    fn requested_number(&self) -> anyhow::Result<Option<u64>> {
        let params = UpdatePullRequestParams {
            title: self.title.clone(),
            body: self.body.clone(),
            operation: self.operation,
            update_branch: self.update_branch,
            pull_request_number: self.pull_request_number.clone(),
            pr_number: self.pr_number.clone(),
            pr: self.pr.clone(),
            repository: self.repository.clone(),
        };
        params.requested_number()
    }

    fn requested_fields(&self, config: &UpdatePullRequestConfig) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.title.is_some() {
            fields.push("title");
        }
        if self.body.is_some() {
            fields.push("body");
        }
        if self.update_branch.unwrap_or(config.update_branch) {
            fields.push("update_branch");
        }
        fields
    }

    fn resolve_number(
        &self,
        config: &UpdatePullRequestConfig,
    ) -> Result<u64, ExecutionResult> {
        let requested = self
            .requested_number()
            .map_err(|error| ExecutionResult::failure(error.to_string()))?;
        match &config.target {
            UpdatePullRequestTarget::Number(number) => {
                if let Some(requested) = requested
                    && requested != *number
                {
                    return Err(ExecutionResult::failure(format!(
                        "requested pull_request_number #{requested} does not match configured target #{number}"
                    )));
                }
                Ok(*number)
            }
            UpdatePullRequestTarget::Named(target) if target == "*" => {
                requested.ok_or_else(|| {
                    ExecutionResult::failure(
                        "pull_request_number is required when safe-outputs.update-pull-request.target is \"*\"",
                    )
                })
            }
            UpdatePullRequestTarget::Named(target) if target == "triggering" => {
                requested.ok_or_else(|| {
                    ExecutionResult::failure(
                        "SYSTEM_PULLREQUEST_PULLREQUESTID is required for target \"triggering\"",
                    )
                })
            }
            UpdatePullRequestTarget::Named(target) => Err(ExecutionResult::failure(format!(
                "unsupported update-pull-request target '{}'",
                crate::sanitize::neutralize_pipeline_commands(target)
            ))),
        }
    }

    async fn fetch_target(
        &self,
        client: &GithubClient,
        repository: &str,
        number: u64,
    ) -> anyhow::Result<Result<RawPullRequestTarget, ExecutionResult>> {
        let response = client
            .send(
                Method::GET,
                repository_route(client, repository, &["pulls", &number.to_string()])?,
                None,
            )
            .await?;
        let response = match response.require_success("Failed to fetch GitHub pull request") {
            Ok(response) => response,
            Err(error) => return Ok(Err(ExecutionResult::failure(error.to_string()))),
        };
        match response.json("Failed to parse GitHub pull request") {
            Ok(target) => Ok(Ok(target)),
            Err(error) => Ok(Err(ExecutionResult::failure(error.to_string()))),
        }
    }

    async fn update_branch(
        &self,
        client: &GithubClient,
        repository: &str,
        number: u64,
    ) -> anyhow::Result<Result<(), ExecutionResult>> {
        let response = client
            .send(
                Method::PUT,
                repository_route(
                    client,
                    repository,
                    &["pulls", &number.to_string(), "update-branch"],
                )?,
                None,
            )
            .await?;
        if response.is_success() {
            return Ok(Ok(()));
        }
        let error = response
            .require_success("Failed to update GitHub pull request branch")
            .expect_err("non-success response must produce an API error");
        let message = error.message.to_ascii_lowercase();
        if message.contains("there are no new commits on the base branch")
            || message.contains("merge conflict between base and head")
            || message.contains("head ref does not exist")
        {
            warn!("Non-fatal update-pull-request branch update failure: {error}");
            return Ok(Ok(()));
        }
        Ok(Err(ExecutionResult::failure(error.to_string())))
    }
}

fn ctx_pull_request_id_from(ctx: &ExecutionContext) -> Result<u64, ExecutionResult> {
    let raw = ctx.pull_request_id.as_deref().ok_or_else(|| {
        ExecutionResult::failure("SYSTEM_PULLREQUEST_PULLREQUESTID is required for target \"triggering\"")
    })?;
    raw.parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| {
            ExecutionResult::failure(format!(
                "SYSTEM_PULLREQUEST_PULLREQUESTID '{}' is not a positive pull request number",
                crate::sanitize::neutralize_pipeline_commands(raw)
            ))
        })
}

#[async_trait::async_trait]
impl Executor for UpdatePullRequestResult {
    fn dry_run_summary(&self) -> String {
        "update GitHub pull request".to_string()
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if !ctx.tool_configs.contains_key("update-pull-request") {
            return Ok(ExecutionResult::failure(
                "update-pull-request is not configured for this workflow",
            ));
        }
        let Some(token) = ctx.github_token.as_ref() else {
            return Ok(ExecutionResult::failure(
                "ADO_AW_GITHUB_TOKEN is not set; configure safe-outputs.github-token \
                 or safe-outputs.github-app",
            ));
        };
        let config: UpdatePullRequestConfig = ctx.get_tool_config("update-pull-request")?;
        validate_update_pull_request_config(&config)?;
        if self.title.is_some() && !config.title {
            return Ok(ExecutionResult::failure(
                "update-pull-request field 'title' is not enabled by configuration",
            ));
        }
        if self.body.is_some() && !config.body {
            return Ok(ExecutionResult::failure(
                "update-pull-request field 'body' is not enabled by configuration",
            ));
        }
        let number = match &config.target {
            UpdatePullRequestTarget::Named(target) if target == "triggering" => {
                match self.requested_number()? {
                    Some(number) => number,
                    None => match ctx_pull_request_id_from(ctx) {
                        Ok(number) => number,
                        Err(result) => return Ok(result),
                    },
                }
            }
            _ => match self.resolve_number(&config) {
                Ok(number) => number,
                Err(result) => return Ok(result),
            },
        };
        let repository = match resolve_github_repository(
            self.repository.as_deref(),
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos),
            ctx,
        ) {
            Ok(repository) => repository,
            Err(result) => return Ok(result),
        };
        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let current = match self.fetch_target(&client, &repository, number).await? {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        let metadata = current.metadata();
        if let Err(result) = validate_github_target_capability(
            &metadata,
            GithubTargetCapabilities {
                issues: false,
                pull_requests: true,
            },
        ) {
            return Ok(result);
        }
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };
        if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
            return Ok(result);
        }
        let update_branch = self.update_branch.unwrap_or(config.update_branch);
        if update_branch
            && let Err(result) = self.update_branch(&client, &repository, number).await?
        {
            return Ok(result);
        }

        let mut payload = Map::new();
        if let Some(title) = self.title.as_ref() {
            payload.insert("title".to_string(), Value::String(title.clone()));
        }
        if let Some(body) = self.body.as_deref() {
            let updated = match build_updated_body(
                current.body.as_deref().unwrap_or_default(),
                body,
                self.operation.unwrap_or(config.operation),
                config.footer,
                ctx,
            ) {
                Ok(body) => body,
                Err(result) => return Ok(result),
            };
            payload.insert("body".to_string(), Value::String(updated));
        }
        if !payload.is_empty() {
            debug!("Updating GitHub pull request {repository}#{number}");
            let response = client
                .send(
                    Method::PATCH,
                    repository_route(&client, &repository, &["pulls", &number.to_string()])?,
                    Some(&Value::Object(payload)),
                )
                .await?;
            if !response.is_success() {
                let error = response
                    .require_success("Failed to update GitHub pull request")
                    .expect_err("non-success response must produce an API error");
                return Ok(ExecutionResult::failure(error.to_string()));
            }
        }

        info!("Updated GitHub pull request {repository}#{number}");
        Ok(ExecutionResult::success_with_data(
            format!(
                "Updated GitHub pull request {repository}#{number}: {}",
                self.requested_fields(&config).join(", ")
            ),
            serde_json::json!({
                "number": number,
                "target_repo": repository,
                "pull_request_url": metadata.html_url,
                "fields": self.requested_fields(&config),
            }),
        ))
    }
}
