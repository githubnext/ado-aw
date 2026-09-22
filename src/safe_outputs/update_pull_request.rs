//! `update-pull-request` Azure DevOps safe output.

use anyhow::{Context, ensure};
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use ado_aw_derive::SanitizeConfig;

use super::{PATH_SEGMENT, resolve_repo_name};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;

const MAX_TITLE_CHARS: usize = 256;
const MAX_BODY_CHARS: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AdoPullRequestBodyOperation {
    Replace,
    Append,
    Prepend,
    ReplaceIsland,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AdoPullRequestId {
    Number(i32),
    String(String),
}

impl AdoPullRequestId {
    fn parse(&self, field: &str) -> anyhow::Result<i32> {
        let id = match self {
            Self::Number(id) => *id,
            Self::String(value) => value
                .trim()
                .strip_prefix('#')
                .unwrap_or_else(|| value.trim())
                .parse::<i32>()
                .map_err(|_| anyhow::anyhow!("{field} must be a positive pull request ID"))?,
        };
        ensure!(id > 0, "{field} must be positive");
        Ok(id)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdatePullRequestParams {
    /// Replacement Azure DevOps pull request title.
    #[serde(default)]
    pub title: Option<String>,
    /// Pull request description content in Markdown.
    #[serde(default)]
    pub body: Option<String>,
    /// Description update operation. Defaults to the configured operation, then `replace`.
    #[serde(default)]
    pub operation: Option<AdoPullRequestBodyOperation>,
    /// Not supported for Azure DevOps PRs; accepted for gh-aw schema compatibility but must be false/omitted.
    #[serde(default, rename = "update_branch", alias = "updateBranch")]
    pub update_branch: Option<bool>,
    /// Azure DevOps pull request ID. Required when front matter uses `target: "*"`.
    #[serde(default, rename = "pull_request_id", alias = "pullRequestId")]
    pub pull_request_id: Option<AdoPullRequestId>,
    /// gh-aw-compatible alias for pull_request_id.
    #[serde(default, rename = "pull_request_number", alias = "pullRequestNumber")]
    pub pull_request_number: Option<AdoPullRequestId>,
    /// Alias for pull_request_id.
    #[serde(default, rename = "pr_number", alias = "prNumber")]
    pub pr_number: Option<AdoPullRequestId>,
    /// Alias for pull_request_id.
    #[serde(default)]
    pub pr: Option<AdoPullRequestId>,
    /// Repository alias: "self" for the pipeline repo, or an alias from the checkout list.
    #[serde(default)]
    pub repository: Option<String>,
}

impl UpdatePullRequestParams {
    fn requested_id(&self) -> anyhow::Result<Option<i32>> {
        let mut found = None;
        for (field, value) in [
            ("pull_request_id", self.pull_request_id.as_ref()),
            ("pull_request_number", self.pull_request_number.as_ref()),
            ("pr_number", self.pr_number.as_ref()),
            ("pr", self.pr.as_ref()),
        ] {
            if let Some(value) = value {
                let id = value.parse(field)?;
                if let Some(existing) = found {
                    ensure!(
                        existing == id,
                        "pull request ID aliases must all refer to the same PR"
                    );
                }
                found = Some(id);
            }
        }
        Ok(found)
    }
}

impl Validate for UpdatePullRequestParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(title) = self.title.as_deref() {
            ensure!(!title.trim().is_empty(), "title must not be empty");
            ensure!(
                title.chars().count() <= MAX_TITLE_CHARS,
                "title must be {MAX_TITLE_CHARS} characters or fewer"
            );
        }
        if let Some(body) = self.body.as_deref() {
            ensure!(
                body.chars().count() <= MAX_BODY_CHARS,
                "body must be {MAX_BODY_CHARS} characters or fewer"
            );
        } else {
            ensure!(
                self.operation.is_none(),
                "operation may only be provided when body is provided"
            );
        }
        if let Some(repository) = self.repository.as_deref() {
            reject_pipeline_injection(repository, "repository")?;
        }
        let _ = self.requested_id()?;
        Ok(())
    }
}

tool_result! {
    name = "update-pull-request",
    write = true,
    params = UpdatePullRequestParams,
    default_max = 1,
    /// Result of updating an Azure DevOps pull request.
    pub struct UpdatePullRequestResult {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        operation: Option<AdoPullRequestBodyOperation>,
        #[serde(default, rename = "update_branch")]
        update_branch: Option<bool>,
        #[serde(default, rename = "pull_request_id")]
        pull_request_id: Option<AdoPullRequestId>,
        #[serde(default, rename = "pull_request_number")]
        pull_request_number: Option<AdoPullRequestId>,
        #[serde(default, rename = "pr_number")]
        pr_number: Option<AdoPullRequestId>,
        #[serde(default)]
        pr: Option<AdoPullRequestId>,
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

fn default_operation() -> AdoPullRequestBodyOperation {
    AdoPullRequestBodyOperation::Replace
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UpdatePullRequestTarget {
    Id(i32),
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
    /// Whether title updates are enabled. Defaults to true to match gh-aw.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub title: bool,
    /// Whether description updates are enabled. Defaults to true to match gh-aw.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub body: bool,
    /// Not supported for Azure DevOps PRs; must remain false.
    #[serde(default, rename = "update-branch")]
    #[sanitize_config(skip)]
    pub update_branch: bool,
    /// Accepted for gh-aw front matter parity; unused for Azure DevOps.
    #[serde(default = "default_true", rename = "sync-stack")]
    #[sanitize_config(skip)]
    pub sync_stack: bool,
    /// Include agent stats in body updates.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub footer: bool,
    /// Body update operation. Defaults to replace.
    #[serde(default = "default_operation")]
    #[sanitize_config(skip)]
    pub operation: AdoPullRequestBodyOperation,
    /// `"triggering"` (default), `"*"`, or a fixed PR ID.
    #[serde(default)]
    pub target: UpdatePullRequestTarget,
    /// Repository aliases the agent may target. Empty means any checkout alias accepted by the compiler.
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
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
            operation: AdoPullRequestBodyOperation::Replace,
            target: UpdatePullRequestTarget::default(),
            allowed_repositories: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            max: None,
        }
    }
}

pub(crate) fn validate_update_pull_request_config(
    config: &UpdatePullRequestConfig,
) -> anyhow::Result<()> {
    ensure!(
        !config.update_branch,
        "safe-outputs.update-pull-request.update-branch is not supported for Azure DevOps PRs"
    );
    match &config.target {
        UpdatePullRequestTarget::Id(id) => ensure!(*id > 0, "target PR ID must be positive"),
        UpdatePullRequestTarget::Named(target) => ensure!(
            matches!(target.as_str(), "triggering" | "*"),
            "target must be \"triggering\", \"*\", or a positive pull request ID"
        ),
    }
    for repository in &config.allowed_repositories {
        ensure!(
            !repository.trim().is_empty(),
            "allowed-repositories entries must not be empty"
        );
        reject_pipeline_injection(
            repository,
            "safe-outputs.update-pull-request.allowed-repositories",
        )?;
    }
    for label in &config.required_labels {
        ensure!(
            !label.is_empty(),
            "required-labels entries must not be empty"
        );
        reject_pipeline_injection(label, "safe-outputs.update-pull-request.required-labels")?;
    }
    if let Some(prefix) = config.required_title_prefix.as_deref() {
        ensure!(
            !prefix.is_empty(),
            "required-title-prefix must not be empty"
        );
        reject_pipeline_injection(
            prefix,
            "safe-outputs.update-pull-request.required-title-prefix",
        )?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RawPullRequest {
    #[serde(rename = "pullRequestId")]
    pull_request_id: i32,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    labels: Vec<RawLabel>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawLabel {
    name: String,
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

fn body_with_footer(body: &str, include_footer: bool, ctx: &ExecutionContext) -> String {
    if include_footer {
        crate::agent_stats::append_stats_to_body(body, ctx, true)
    } else {
        body.to_string()
    }
}

fn build_updated_body(
    current: &str,
    new_content: &str,
    operation: AdoPullRequestBodyOperation,
    include_footer: bool,
    ctx: &ExecutionContext,
) -> Result<String, ExecutionResult> {
    let section = body_with_footer(new_content, include_footer, ctx);
    let updated = match operation {
        AdoPullRequestBodyOperation::Append => {
            if current.is_empty() {
                section
            } else {
                format!("{current}\n\n---\n\n{section}")
            }
        }
        AdoPullRequestBodyOperation::Prepend => {
            if current.is_empty() {
                section
            } else {
                format!("{section}\n\n---\n\n{current}")
            }
        }
        AdoPullRequestBodyOperation::Replace => section,
        AdoPullRequestBodyOperation::ReplaceIsland => replace_island(current, &section, ctx)?,
    };
    if updated.chars().count() > MAX_BODY_CHARS {
        return Err(ExecutionResult::failure(format!(
            "updated body exceeds Azure DevOps' {MAX_BODY_CHARS}-character limit"
        )));
    }
    Ok(updated)
}

fn ctx_pull_request_id(ctx: &ExecutionContext) -> Result<i32, ExecutionResult> {
    let raw = ctx.pull_request_id.as_deref().ok_or_else(|| {
        ExecutionResult::failure(
            "SYSTEM_PULLREQUEST_PULLREQUESTID is required for target \"triggering\"",
        )
    })?;
    raw.parse::<i32>().ok().filter(|id| *id > 0).ok_or_else(|| {
        ExecutionResult::failure(format!(
            "SYSTEM_PULLREQUEST_PULLREQUESTID '{}' is not a positive pull request ID",
            crate::sanitize::neutralize_pipeline_commands(raw)
        ))
    })
}

impl UpdatePullRequestResult {
    fn requested_id(&self) -> anyhow::Result<Option<i32>> {
        UpdatePullRequestParams {
            title: self.title.clone(),
            body: self.body.clone(),
            operation: self.operation,
            update_branch: self.update_branch,
            pull_request_id: self.pull_request_id.clone(),
            pull_request_number: self.pull_request_number.clone(),
            pr_number: self.pr_number.clone(),
            pr: self.pr.clone(),
            repository: self.repository.clone(),
        }
        .requested_id()
    }

    fn resolve_id(
        &self,
        config: &UpdatePullRequestConfig,
        ctx: &ExecutionContext,
    ) -> Result<i32, ExecutionResult> {
        let requested = self
            .requested_id()
            .map_err(|error| ExecutionResult::failure(error.to_string()))?;
        match &config.target {
            UpdatePullRequestTarget::Id(id) => {
                if let Some(requested) = requested
                    && requested != *id
                {
                    return Err(ExecutionResult::failure(format!(
                        "requested pull_request_id #{requested} does not match configured target #{id}"
                    )));
                }
                Ok(*id)
            }
            UpdatePullRequestTarget::Named(target) if target == "*" => requested.ok_or_else(|| {
                ExecutionResult::failure(
                    "pull_request_id is required when safe-outputs.update-pull-request.target is \"*\"",
                )
            }),
            UpdatePullRequestTarget::Named(target) if target == "triggering" => {
                let triggering = ctx_pull_request_id(ctx)?;
                if let Some(requested) = requested
                    && requested != triggering
                {
                    return Err(ExecutionResult::failure(format!(
                        "requested pull_request_id #{requested} does not match triggering pull request #{triggering}"
                    )));
                }
                Ok(triggering)
            }
            UpdatePullRequestTarget::Named(target) => Err(ExecutionResult::failure(format!(
                "unsupported update-pull-request target '{}'",
                crate::sanitize::neutralize_pipeline_commands(target)
            ))),
        }
    }

    fn requested_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.title.is_some() {
            fields.push("title");
        }
        if self.body.is_some() {
            fields.push("body");
        }
        fields
    }

    async fn fetch_pr(
        &self,
        client: &reqwest::Client,
        base_url: &str,
        repo_name: &str,
        token: &str,
        pr_id: i32,
    ) -> anyhow::Result<Result<RawPullRequest, ExecutionResult>> {
        let encoded_repo = utf8_percent_encode(repo_name, PATH_SEGMENT).to_string();
        let url = format!("{base_url}/{encoded_repo}/pullRequests/{pr_id}?api-version=7.1");
        let response = client
            .get(&url)
            .basic_auth("", Some(token))
            .send()
            .await
            .context("Failed to fetch Azure DevOps pull request")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Ok(Err(ExecutionResult::failure(format!(
                "Failed to fetch PR #{pr_id} (HTTP {status}): {body}"
            ))));
        }
        match response.json::<RawPullRequest>().await {
            Ok(pr) => Ok(Ok(pr)),
            Err(error) => Ok(Err(ExecutionResult::failure(format!(
                "Failed to parse PR #{pr_id}: {error}"
            )))),
        }
    }

    fn validate_filters(
        &self,
        pr: &RawPullRequest,
        config: &UpdatePullRequestConfig,
    ) -> Result<(), ExecutionResult> {
        let missing: Vec<&str> = config
            .required_labels
            .iter()
            .map(String::as_str)
            .filter(|required| {
                !pr.labels
                    .iter()
                    .any(|label| label.name.eq_ignore_ascii_case(required))
            })
            .collect();
        if !missing.is_empty() {
            return Err(ExecutionResult::failure(format!(
                "PR #{} is missing required labels: {}",
                pr.pull_request_id,
                missing.join(", ")
            )));
        }
        if let Some(prefix) = config.required_title_prefix.as_deref()
            && !pr.title.starts_with(prefix)
        {
            return Err(ExecutionResult::failure(format!(
                "PR #{} title does not start with required-title-prefix '{}'",
                pr.pull_request_id,
                crate::sanitize::neutralize_pipeline_commands(prefix)
            )));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Executor for UpdatePullRequestResult {
    fn dry_run_summary(&self) -> String {
        "update Azure DevOps pull request".to_string()
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        let params = UpdatePullRequestParams {
            title: self.title.clone(),
            body: self.body.clone(),
            operation: self.operation,
            update_branch: self.update_branch,
            pull_request_id: self.pull_request_id.clone(),
            pull_request_number: self.pull_request_number.clone(),
            pr_number: self.pr_number.clone(),
            pr: self.pr.clone(),
            repository: self.repository.clone(),
        };
        if let Err(error) = params.validate() {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        if !ctx.tool_configs.contains_key("update-pull-request") {
            return Ok(ExecutionResult::failure(
                "update-pull-request is not configured for this workflow",
            ));
        }
        let org_url = ctx
            .ado_org_url
            .as_ref()
            .context("AZURE_DEVOPS_ORG_URL not set")?;
        let project = ctx
            .ado_project
            .as_ref()
            .context("SYSTEM_TEAMPROJECT not set")?;
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;
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
        if self.update_branch == Some(true) {
            return Ok(ExecutionResult::failure(
                "update_branch is not supported for Azure DevOps PRs",
            ));
        }
        if self.title.is_none() && self.body.is_none() {
            return Ok(ExecutionResult::failure(
                "at least one of title or body is required",
            ));
        }
        let pr_id = match self.resolve_id(&config, ctx) {
            Ok(pr_id) => pr_id,
            Err(result) => return Ok(result),
        };
        let repo_alias = self.repository.as_deref().unwrap_or("self");
        if !config.allowed_repositories.is_empty()
            && !config
                .allowed_repositories
                .iter()
                .any(|allowed| allowed == repo_alias)
        {
            return Ok(ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed-repositories list: [{}]",
                crate::sanitize::neutralize_pipeline_commands(repo_alias),
                config.allowed_repositories.join(", ")
            )));
        }
        let repo_name = match resolve_repo_name(self.repository.as_deref(), ctx) {
            Ok(name) => name,
            Err(failure) => return Ok(failure),
        };
        let client = reqwest::Client::new();
        let encoded_project = utf8_percent_encode(project, PATH_SEGMENT).to_string();
        let base_url = format!(
            "{}/{}/_apis/git/repositories",
            org_url.trim_end_matches('/'),
            encoded_project,
        );
        let current = match self
            .fetch_pr(&client, &base_url, &repo_name, token, pr_id)
            .await?
        {
            Ok(pr) => pr,
            Err(result) => return Ok(result),
        };
        if let Err(result) = self.validate_filters(&current, &config) {
            return Ok(result);
        }
        let mut patch = Map::new();
        if let Some(title) = self.title.as_ref() {
            patch.insert("title".to_string(), Value::String(title.clone()));
        }
        if let Some(body) = self.body.as_deref() {
            let description = match build_updated_body(
                current.description.as_deref().unwrap_or_default(),
                body,
                self.operation.unwrap_or(config.operation),
                config.footer,
                ctx,
            ) {
                Ok(description) => description,
                Err(result) => return Ok(result),
            };
            patch.insert("description".to_string(), Value::String(description));
        }
        let encoded_repo = utf8_percent_encode(&repo_name, PATH_SEGMENT).to_string();
        let patch_url = format!("{base_url}/{encoded_repo}/pullRequests/{pr_id}?api-version=7.1");
        debug!(
            "Updating Azure DevOps PR #{pr_id}: {}",
            self.requested_fields().join(", ")
        );
        let response = client
            .patch(&patch_url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&Value::Object(patch))
            .send()
            .await
            .context("Failed to update Azure DevOps pull request")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Ok(ExecutionResult::failure(format!(
                "Failed to update PR #{pr_id} (HTTP {status}): {body}"
            )));
        }
        info!("Updated Azure DevOps PR #{pr_id}");
        Ok(ExecutionResult::success_with_data(
            format!(
                "Updated Azure DevOps PR #{pr_id}: {}",
                self.requested_fields().join(", ")
            ),
            serde_json::json!({
                "pull_request_id": pr_id,
                "operation": "update-pull-request",
                "fields": self.requested_fields(),
                "url": current.url,
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
        tool_configs.insert("update-pull-request".to_string(), config);
        ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_project: Some("project".to_string()),
            access_token: Some("token".to_string()),
            repository_name: Some("repo".to_string()),
            pull_request_id: Some("7".to_string()),
            tool_configs,
            definition_id: Some(123),
            ..Default::default()
        }
    }

    fn pr(id: i32) -> serde_json::Value {
        serde_json::json!({
            "pullRequestId": id,
            "title": "[bot] Existing",
            "description": "Existing body",
            "labels": [{"name": "automated"}],
            "url": format!("https://dev.azure.example/pr/{id}")
        })
    }

    fn params() -> UpdatePullRequestParams {
        UpdatePullRequestParams {
            title: Some("Updated title".to_string()),
            body: None,
            operation: None,
            update_branch: None,
            pull_request_id: None,
            pull_request_number: None,
            pr_number: None,
            pr: None,
            repository: None,
        }
    }

    #[test]
    fn contract_name_and_budget() {
        assert_eq!(UpdatePullRequestResult::NAME, "update-pull-request");
        assert_eq!(UpdatePullRequestResult::DEFAULT_MAX, 1);
    }

    #[test]
    fn config_matches_gh_aw_shape_but_rejects_update_branch_true() {
        let config: UpdatePullRequestConfig =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(config.title);
        assert!(config.body);
        assert!(!config.update_branch);
        assert!(config.sync_stack);
        assert!(config.footer);
        assert_eq!(config.operation, AdoPullRequestBodyOperation::Replace);

        let unsupported: UpdatePullRequestConfig = serde_json::from_value(serde_json::json!({
            "update-branch": true
        }))
        .unwrap();
        assert!(validate_update_pull_request_config(&unsupported).is_err());
    }

    #[test]
    fn validates_id_aliases() {
        let empty = UpdatePullRequestParams {
            title: None,
            ..params()
        };
        assert!(empty.validate().is_ok());

        let mut aliases = params();
        aliases.pull_request_id = Some(AdoPullRequestId::Number(1));
        aliases.pull_request_number = Some(AdoPullRequestId::String("#2".to_string()));
        assert!(aliases.validate().is_err());
    }

    #[test]
    fn replace_island_appends_then_replaces_pipeline_scoped_section() {
        let ctx = ExecutionContext {
            definition_id: Some(123),
            ..Default::default()
        };
        let first = build_updated_body(
            "before",
            "new",
            AdoPullRequestBodyOperation::ReplaceIsland,
            false,
            &ctx,
        )
        .unwrap();
        assert!(first.contains("before\n\n---\n\n"));
        assert!(first.contains("<!-- ado-aw-pr-island-start:pipeline-definition-id=123 -->"));
        let second = build_updated_body(
            &first,
            "next",
            AdoPullRequestBodyOperation::ReplaceIsland,
            false,
            &ctx,
        )
        .unwrap();
        assert!(second.contains("\nnext\n"));
        assert!(!second.contains("\nnew\n"));
    }

    #[tokio::test]
    async fn updates_triggering_pr_title_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/project/_apis/git/repositories/repo/pullRequests/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/project/_apis/git/repositories/repo/pullRequests/7"))
            .and(body_json(serde_json::json!({
                "title": "Updated title",
                "description": "Existing body\n\n---\n\nNew body"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr(7)))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "title": true,
                "body": true,
                "footer": false,
                "operation": "append",
                "required-labels": ["automated"],
                "required-title-prefix": "[bot]"
            }),
        );
        let mut result: UpdatePullRequestResult = UpdatePullRequestParams {
            title: Some("Updated title".to_string()),
            body: Some("New body".to_string()),
            operation: None,
            update_branch: None,
            pull_request_id: None,
            pull_request_number: None,
            pr_number: None,
            pr: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
    }

    #[tokio::test]
    async fn target_star_requires_agent_pr_id() {
        let server = MockServer::start().await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target": "*"
            }),
        );
        let mut result: UpdatePullRequestResult = params().try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("pull_request_id is required"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
