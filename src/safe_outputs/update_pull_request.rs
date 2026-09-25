//! `update-pull-request` Azure DevOps safe output.

use anyhow::{Context, ensure};
use log::{debug, info};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use ado_aw_derive::SanitizeConfig;

use super::authenticate_ado_request;
use super::pr_common::{
    PrTargetPolicy, PullRequestReference, fetch_pr_labels, legacy_policy, repository_api_base,
    resolve_pr_policy_target, validate_description, validate_reference,
};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{
    SanitizeContent, sanitize as sanitize_text, sanitize_config, sanitize_markdown,
};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;

const MAX_TITLE_CHARS: usize = 256;
const MAX_BODY_CHARS: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AdoPullRequestBodyOperation {
    Replace,
    Append,
    Prepend,
    ReplaceIsland,
}

pub type AdoPullRequestId = PullRequestReference;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
    /// Positive Azure DevOps PR ID or same-run temporary ID. Required when target is "*".
    #[serde(
        default,
        rename = "pull_request_id",
        alias = "pullRequestId",
        alias = "id"
    )]
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
    fn requested_id(&self) -> anyhow::Result<Option<PullRequestReference>> {
        let mut found = None;
        for (_field, value) in [
            ("pull_request_id", self.pull_request_id.as_ref()),
            ("pull_request_number", self.pull_request_number.as_ref()),
            ("pr_number", self.pr_number.as_ref()),
            ("pr", self.pr.as_ref()),
        ] {
            if let Some(value) = value {
                validate_reference(value)?;
                let id = value.clone();
                if let Some(existing) = &found {
                    ensure!(
                        existing == &id,
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
                body.encode_utf16().count() <= MAX_BODY_CHARS,
                "body must be {MAX_BODY_CHARS} UTF-16 units or fewer"
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
    #[serde(deny_unknown_fields)]
    pub struct UpdatePullRequestResult {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        operation: Option<AdoPullRequestBodyOperation>,
        #[serde(default, rename = "update_branch")]
        update_branch: Option<bool>,
        #[serde(default, rename = "pull_request_id", alias = "pullRequestId", alias = "id")]
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
        // Rendering policy is selected in Stage 3; proposals still receive transport sanitization.
        self.title = self
            .title
            .as_deref()
            .map(crate::sanitize::sanitize_custom_payload);
        self.body = self
            .body
            .as_deref()
            .map(crate::sanitize::sanitize_custom_payload);
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
    Id(u64),
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
    /// Include agent stats in body updates.
    #[serde(default = "default_true", rename = "include-stats", alias = "footer")]
    #[sanitize_config(skip)]
    pub include_stats: bool,
    /// Body update operation. Defaults to replace.
    #[serde(default = "default_operation")]
    #[sanitize_config(skip)]
    pub operation: AdoPullRequestBodyOperation,
    /// `"triggering"` (default), `"*"`, or a fixed PR ID.
    #[serde(default)]
    pub target: UpdatePullRequestTarget,
    /// Default repository selector, still constrained by the common target policy.
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
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
            include_stats: true,
            operation: AdoPullRequestBodyOperation::Replace,
            target: UpdatePullRequestTarget::default(),
            target_repo: None,
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
    super::pr_common::PrMutationPolicy::parse(&serde_json::to_value(config)?)?;
    ensure!(
        !config.update_branch,
        "safe-outputs.update-pull-request.update-branch is not supported for Azure DevOps PRs"
    );
    match &config.target {
        UpdatePullRequestTarget::Id(id) => ensure!(*id > 0, "target PR ID must be positive"),
        UpdatePullRequestTarget::Named(target) => ensure!(
            matches!(target.as_str(), "triggering" | "*")
                || target.parse::<u64>().is_ok_and(|id| id > 0),
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

impl UpdatePullRequestConfig {
    pub(crate) fn target_policy(&self) -> anyhow::Result<PrTargetPolicy> {
        match &self.target {
            UpdatePullRequestTarget::Id(id) => PrTargetPolicy::fixed(*id),
            UpdatePullRequestTarget::Named(value) => PrTargetPolicy::named(value),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawPullRequest {
    #[serde(rename = "pullRequestId")]
    pull_request_id: u64,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: Option<String>,
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
    if starts.is_empty() && ends.is_empty() {
        let island = format!("{start_marker}\n{replacement}\n{end_marker}");
        return Ok(if current.is_empty() {
            island
        } else {
            format!("{current}\n\n---\n\n{island}")
        });
    }
    if starts.len() != 1 || ends.len() != 1 {
        return Err(ExecutionResult::failure(
            "replace-island requires exactly one matching marker pair; duplicate or partial markers are not safe to replace",
        ));
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
    if let Err(error) = validate_description(&updated) {
        return Err(ExecutionResult::failure(error.to_string()));
    }
    Ok(updated)
}

impl UpdatePullRequestResult {
    fn requested_id(&self) -> anyhow::Result<Option<PullRequestReference>> {
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
        token: &str,
        pr_id: u64,
        ctx: &ExecutionContext,
    ) -> anyhow::Result<Result<RawPullRequest, ExecutionResult>> {
        let url = format!("{base_url}/pullRequests/{pr_id}?api-version=7.1");
        let response = authenticate_ado_request(client.get(&url), token, ctx.write_connection_type)
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
        labels: &[String],
        config: &UpdatePullRequestConfig,
    ) -> Result<(), ExecutionResult> {
        let missing: Vec<&str> = config
            .required_labels
            .iter()
            .map(String::as_str)
            .filter(|required| {
                !labels
                    .iter()
                    .any(|label| label.eq_ignore_ascii_case(required))
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
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;
        let config: UpdatePullRequestConfig = ctx.get_tool_config("update-pull-request")?;
        validate_update_pull_request_config(&config)?;
        let legacy = legacy_policy(ctx, "update-pull-request", "update-description")?;
        if legacy.is_some()
            && (self.title.is_some()
                || self.body.as_deref().is_none_or(|body| body.len() < 10)
                || self.operation.unwrap_or(config.operation)
                    != AdoPullRequestBodyOperation::Replace
                || config.include_stats)
        {
            return Ok(ExecutionResult::failure(
                "legacy update-description requires replacement body of at least 10 characters, no title and include-stats: false",
            ));
        }
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
        let requested = self.requested_id()?;
        let (pr_id, target) = match resolve_pr_policy_target(
            &config.target_policy()?,
            requested.as_ref(),
            self.repository.as_deref().or(config.target_repo.as_deref()),
            &config.allowed_repositories,
            ctx,
        )? {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        if let Some(legacy) = &legacy
            && let Err(failure) = super::pr_common::validate_pr_repository_policy(
                &target, &legacy.allowed_repositories, ctx,
            )
        {
            return Ok(failure);
        }
        let client = reqwest::Client::new();
        let base_url = repository_api_base(&target);
        let body = self.body.as_deref().map(|body| {
            if legacy.is_some() {
                sanitize_text(body)
            } else {
                sanitize_markdown(body)
            }
        });
        if legacy.is_some()
            && config.required_labels.is_empty()
            && config.required_title_prefix.is_none()
        {
            let body = body.as_deref().context("legacy body must be provided")?;
            if body.len() < 10 {
                return Ok(ExecutionResult::failure(
                    "description must be at least 10 characters after sanitization",
                ));
            }
            return super::pr_mutations::execute_update_description(
                &super::pr_mutations::UpdatePrContext {
                    client: &client,
                    target,
                    pr_id,
                    token,
                    connection_type: ctx.write_connection_type,
                },
                body,
            )
            .await;
        }
        let current = match self.fetch_pr(&client, &base_url, token, pr_id, ctx).await? {
            Ok(pr) => pr,
            Err(result) => return Ok(result),
        };
        let labels = if config.required_labels.is_empty() {
            Vec::new()
        } else {
            match fetch_pr_labels(&client, &base_url, pr_id, token, ctx).await? {
                Ok(labels) => labels,
                Err(result) => return Ok(result),
            }
        };
        if let Err(result) = self.validate_filters(&current, &labels, &config) {
            return Ok(result);
        }
        let mut patch = Map::new();
        if let Some(title) = self.title.as_ref() {
            let title = sanitize_text(title);
            if title.trim().is_empty() || title.chars().count() > MAX_TITLE_CHARS {
                return Ok(ExecutionResult::failure(
                    "sanitized title must be nonempty and 256 characters or fewer",
                ));
            }
            patch.insert("title".to_string(), Value::String(title));
        }
        if let Some(body) = body.as_deref() {
            let description = match build_updated_body(
                current.description.as_deref().unwrap_or_default(),
                body,
                self.operation.unwrap_or(config.operation),
                config.include_stats,
                ctx,
            ) {
                Ok(description) => description,
                Err(result) => return Ok(result),
            };
            patch.insert("description".to_string(), Value::String(description));
        }
        let patch_url = format!("{base_url}/pullRequests/{pr_id}?api-version=7.1");
        debug!(
            "Updating Azure DevOps PR #{pr_id}: {}",
            self.requested_fields().join(", ")
        );
        let response = authenticate_ado_request(
            client
                .patch(&patch_url)
                .header("Content-Type", "application/json")
                .json(&Value::Object(patch)),
            token,
            ctx.write_connection_type,
        )
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
            ado_organization: Some("org".to_string()),
            ado_project: Some("project".to_string()),
            access_token: Some("token".to_string()),
            repository_name: Some("repo".to_string()),
            pull_request_id: Some("7".to_string()),
            triggering_pr: Some(super::super::pr_common::TriggeringPullRequest {
                collection_uri: server.uri(),
                project: "project".into(),
                repository_name: "repo".into(),
                repository_id: "11111111-1111-1111-1111-111111111111".into(),
                id: "7".into(),
            }),
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
        assert!(config.include_stats);
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
        aliases.pull_request_number = Some(serde_json::from_str("\"#2\"").unwrap());
        assert!(aliases.validate().is_err());
    }

    #[test]
    fn include_stats_alias_rejects_duplicate_spellings() {
        for config in [
            serde_json::json!({"include-stats": false}),
            serde_json::json!({"footer": false}),
        ] {
            assert!(
                !serde_json::from_value::<UpdatePullRequestConfig>(config)
                    .unwrap()
                    .include_stats
            );
        }
        for footer in [true, false] {
            assert!(
                serde_json::from_value::<UpdatePullRequestConfig>(
                    serde_json::json!({"include-stats": false, "footer": footer})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn island_rejects_partial_duplicate_and_reversed_markers() {
        let ctx = ExecutionContext {
            definition_id: Some(123),
            ..Default::default()
        };
        let (start, end) = island_markers(&ctx).unwrap();
        for existing in [
            start.clone(),
            end.clone(),
            format!("{end}{start}"),
            format!("{start}{end}{start}{end}"),
            format!("{start}{start}{end}"),
            format!("{start}{end}{end}"),
        ] {
            assert!(
                replace_island(&existing, "replacement", &ctx).is_err(),
                "{existing}"
            );
        }
        let other = "<!-- ado-aw-pr-island-start:pipeline-definition-id=456 -->\nother\n<!-- ado-aw-pr-island-end:pipeline-definition-id=456 -->";
        let current = format!("before\n{other}\n{start}\nold\n{end}\nafter");
        assert_eq!(
            replace_island(&current, "new", &ctx).unwrap(),
            format!("before\n{other}\n{start}\nnew\n{end}\nafter")
        );
    }

    #[test]
    fn final_description_bound_includes_existing_content_markers_and_stats() {
        let ctx = ExecutionContext {
            definition_id: Some(1),
            ..Default::default()
        };
        for operation in [
            AdoPullRequestBodyOperation::Append,
            AdoPullRequestBodyOperation::Prepend,
        ] {
            assert!(build_updated_body(&"a".repeat(3990), "bbbb", operation, false, &ctx).is_err());
        }
        assert!(
            build_updated_body(
                "",
                &"a".repeat(3990),
                AdoPullRequestBodyOperation::ReplaceIsland,
                false,
                &ctx
            )
            .is_err()
        );
        let mut with_stats = ctx;
        with_stats.agent_stats = Some(crate::agent_stats::AgentStats {
            agent_name: "agent".into(),
            model: None,
            input_tokens: 1,
            output_tokens: 1,
            ai_credits: None,
            duration_seconds: 1.0,
            tool_calls: 1,
            turns: 1,
        });
        assert!(
            build_updated_body(
                "",
                &"a".repeat(4000),
                AdoPullRequestBodyOperation::Replace,
                true,
                &with_stats
            )
            .is_err()
        );
        assert!(
            build_updated_body(
                "",
                &"a".repeat(4000),
                AdoPullRequestBodyOperation::Replace,
                false,
                &with_stats
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn migrated_description_uses_registered_target_plain_text_and_no_get_or_footer() {
        let server = MockServer::start().await;
        let text = "Code: `<safe>` and <strong>title</strong>.";
        Mock::given(method("PATCH"))
            .and(path(
                "/Other/_apis/git/repositories/repo-id/pullRequests/4294967296",
            ))
            .and(body_json(
                serde_json::json!({"description": sanitize_text(text)}),
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = super::super::pr_common::tests::registered_context(
            &server.uri(),
            "update-pull-request",
            serde_json::json!({
                "title": false, "body": true, "target": "*", "include-stats": false,
                "legacy-update-pr": {"allowed-operations": ["update-description"], "allowed-repositories": ["other"]}
            }),
        );
        let mut result: UpdatePullRequestResult = serde_json::from_value(serde_json::json!({
            "name": "update-pull-request", "pull_request_id": "#aw_pr123", "body": text
        }))
        .unwrap();
        assert!(result.execute_sanitized(&ctx).await.unwrap().success);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn migration_rejects_title_append_footer_and_short_body_before_network() {
        let server = MockServer::start().await;
        for (request, include_stats) in [
            (
                serde_json::json!({"body": "long enough body", "title": "new title"}),
                false,
            ),
            (
                serde_json::json!({"body": "long enough body", "operation": "append"}),
                false,
            ),
            (serde_json::json!({"body": "short"}), false),
            (serde_json::json!({"body": "long enough body"}), true),
        ] {
            let ctx = context(
                &server,
                serde_json::json!({
                    "target": "*", "include-stats": include_stats,
                    "legacy-update-pr": {"allowed-operations": ["update-description"]}
                }),
            );
            let mut request = request;
            request["name"] = serde_json::json!("update-pull-request");
            request["pull_request_id"] = serde_json::json!(7);
            let mut result: UpdatePullRequestResult = serde_json::from_value(request).unwrap();
            assert!(!result.execute_sanitized(&ctx).await.unwrap().success);
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn native_body_uses_markdown_and_rejects_assembled_overflow_before_patch() {
        for (body, expected_success) in [("`<safe>`".to_string(), true), ("a".repeat(4000), false)]
        {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
                .respond_with(ResponseTemplate::new(200).set_body_json(pr(7)))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("PATCH"))
                .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
                .and(body_json(
                    serde_json::json!({"description": "Existing body\n\n---\n\n`<safe>`"}),
                ))
                .respond_with(ResponseTemplate::new(200))
                .expect(if expected_success { 1 } else { 0 })
                .mount(&server)
                .await;
            let ctx = context(
                &server,
                serde_json::json!({"operation": "append", "include-stats": false}),
            );
            let mut result: UpdatePullRequestResult = UpdatePullRequestParams {
                title: None,
                body: Some(body),
                ..params()
            }
            .try_into()
            .unwrap();
            assert_eq!(
                result.execute_sanitized(&ctx).await.unwrap().success,
                expected_success
            );
        }
    }

    #[tokio::test]
    async fn temporary_reference_does_not_bypass_fixed_or_triggering_target() {
        for target in [serde_json::json!(7), serde_json::json!("triggering")] {
            let server = MockServer::start().await;
            let mut ctx = super::super::pr_common::tests::registered_context(
                &server.uri(),
                "update-pull-request",
                serde_json::json!({"target": target}),
            );
            ctx.pull_request_id = Some("7".into());
            let mut result: UpdatePullRequestResult = serde_json::from_value(serde_json::json!({
                "name": "update-pull-request", "pull_request_id": "#aw_pr123", "title": "new title"
            }))
            .unwrap();
            assert!(!result.execute_sanitized(&ctx).await.unwrap().success);
            assert!(server.received_requests().await.unwrap().is_empty());
        }
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
            .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7/labels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 1, "value": [{"name": "automated"}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
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

    #[tokio::test]
    async fn rejects_disallowed_repository_before_network() {
        let server = MockServer::start().await;
        let ctx = context(
            &server,
            serde_json::json!({"allowed-repositories": ["other"]}),
        );
        let mut result: UpdatePullRequestResult = params().try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("allowed-repositories"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn repository_allowlist_uses_canonical_aliases() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr(7)))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({"allowed-repositories": ["self"]}),
        );
        let mut request = params();
        request.repository = Some("REPO".to_string());
        let mut result: UpdatePullRequestResult = request.try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        server.verify().await;
    }

    #[tokio::test]
    async fn rejects_filters_before_patch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7/labels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 0, "value": []
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let ctx = context(&server, serde_json::json!({"required-labels": ["missing"]}));
        let mut result: UpdatePullRequestResult = params().try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("missing required labels"));
        server.verify().await;
    }

    #[tokio::test]
    async fn rejects_request_level_update_branch() {
        let server = MockServer::start().await;
        let ctx = context(&server, serde_json::json!({}));
        let mut request = params();
        request.update_branch = Some(true);
        let mut result: UpdatePullRequestResult = request.try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("not supported"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn reports_fetch_and_patch_failures() {
        for (get_status, patch_status, expected) in [
            (404, 200, "Failed to fetch PR #7"),
            (200, 500, "Failed to update PR #7"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
                .respond_with(ResponseTemplate::new(get_status).set_body_json(
                    if get_status == 200 {
                        pr(7)
                    } else {
                        serde_json::json!({})
                    },
                ))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("PATCH"))
                .and(path("/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7"))
                .respond_with(ResponseTemplate::new(patch_status))
                .expect(if get_status == 200 { 1 } else { 0 })
                .mount(&server)
                .await;
            let ctx = context(&server, serde_json::json!({}));
            let mut result: UpdatePullRequestResult = params().try_into().unwrap();
            let execution = result.execute_sanitized(&ctx).await.unwrap();
            assert!(!execution.success);
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
        let mut request = params();
        request.pull_request_id = Some(AdoPullRequestId::Number(7));
        let mut result: UpdatePullRequestResult = request.try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("#7"));
        assert!(execution.message.contains("#42"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
