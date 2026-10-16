//! `update-pull-request` Azure DevOps safe output.

use super::pr_http::BoundedPrResponse;
use anyhow::{Context, ensure};
use log::{debug, info};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use ado_aw_derive::SanitizeConfig;

use super::authenticate_ado_request;
use super::pr_common::{
    PrTargetPolicy, PullRequestReference, fetch_pr_labels, legacy_policy, repository_api_base,
    resolve_pr_policy_target, validate_description, validate_reference, validate_title,
};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, ToolResult, Validate};
use crate::sanitize::{
    SanitizeContent, sanitize as sanitize_text, sanitize_config, sanitize_markdown,
};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;


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
    /// Replacement PR title; at most 400 Unicode scalar values (compiler resource limit).
    #[serde(default)]
    pub title: Option<String>,
    /// Pull request description content in Markdown.
    #[serde(default)]
    pub body: Option<String>,
    /// Description update operation. Defaults to the configured operation, then `replace`.
    #[serde(default)]
    pub operation: Option<AdoPullRequestBodyOperation>,
    /// Positive Azure DevOps PR ID or same-run temporary ID. Required when target is "*".
    #[serde(default)]
    pub pull_request_id: Option<AdoPullRequestId>,
    /// Repository alias: "self" for the pipeline repo, or an alias from the checkout list.
    #[serde(default)]
    pub repository: Option<String>,
}

impl UpdatePullRequestParams {
    fn requested_id(&self) -> anyhow::Result<Option<PullRequestReference>> {
        if let Some(value) = &self.pull_request_id {
            validate_reference(value)?;
        }
        Ok(self.pull_request_id.clone())
    }
}

impl Validate for UpdatePullRequestParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(title) = self.title.as_deref() {
            validate_title(title)?;
        }
        if let Some(body) = self.body.as_deref() {
            validate_description(body)?;
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
        #[serde(default)]
        pull_request_id: Option<AdoPullRequestId>,
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

fn default_allowed_operations() -> Vec<AdoPullRequestBodyOperation> {
    vec![AdoPullRequestBodyOperation::Replace]
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
    /// Include agent stats in body updates.
    #[serde(default, rename = "include-stats")]
    #[sanitize_config(skip)]
    pub include_stats: bool,
    /// Body operations the author permits; a default never grants authority.
    #[serde(default = "default_allowed_operations", rename = "allowed-operations")]
    #[sanitize_config(skip)]
    pub allowed_operations: Vec<AdoPullRequestBodyOperation>,
    #[serde(default, rename = "default-operation", skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub default_operation: Option<AdoPullRequestBodyOperation>,
    /// `"triggering"` (default), `"*"`, or a fixed PR ID.
    #[serde(default)]
    pub target: UpdatePullRequestTarget,
    /// Default repository selector, still constrained by the common target policy.
    #[serde(default, rename = "default-repository")]
    pub target_repo: Option<String>,
    /// Repository names/aliases permitted inside checkout authority. Empty permits none.
    #[serde(default = "super::pr_policy::unrestricted_names", rename = "allowed-repositories")]
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
            include_stats: false,
            allowed_operations: default_allowed_operations(),
            default_operation: None,
            target: UpdatePullRequestTarget::default(),
            target_repo: None,
            allowed_repositories: super::pr_policy::unrestricted_names(),
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
        config.allowed_operations.len() <= 1 || config.default_operation.is_some(),
        "multiple allowed-operations require an explicit default-operation"
    );
    if let Some(default) = config.default_operation {
        ensure!(config.allowed_operations.contains(&default),
            "default-operation must be permitted by allowed-operations");
    }
    match &config.target {
        UpdatePullRequestTarget::Id(id) => ensure!(*id > 0, "target PR ID must be positive"),
        UpdatePullRequestTarget::Named(target) => ensure!(
            matches!(target.as_str(), "triggering" | "created" | "*")
                || target.parse::<u64>().is_ok_and(|id| id > 0),
            "target must be \"triggering\", \"created\", \"*\", or a positive pull request ID"
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
    pub(crate) fn effective_default_operation(&self) -> Option<AdoPullRequestBodyOperation> {
        self.default_operation.or_else(|| {
            (self.allowed_operations.len() == 1).then(|| self.allowed_operations[0])
        })
    }

    pub(crate) fn resolve_operation(
        &self,
        requested: Option<AdoPullRequestBodyOperation>,
    ) -> anyhow::Result<AdoPullRequestBodyOperation> {
        let operation = requested.or_else(|| self.effective_default_operation())
            .context("no body operation is permitted by allowed-operations")?;
        ensure!(self.allowed_operations.contains(&operation),
            "requested body operation is not permitted by allowed-operations");
        Ok(operation)
    }

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
        if let Some(reference) = &self.pull_request_id { validate_reference(reference)?; }
        Ok(self.pull_request_id.clone())
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
                .bounded_text()
                .await
                .unwrap_or_else(|error| format!("Failed to read PR error response: {error}"));
            return Ok(Err(ExecutionResult::failure(format!(
                "Failed to fetch PR #{pr_id} (HTTP {status}): {body}"
            ))));
        }
        match response.bounded_json::<RawPullRequest>().await {
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
            pull_request_id: self.pull_request_id.clone(),
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
        if config.allowed_repositories.is_empty() {
            return Ok(ExecutionResult::failure("allowed-repositories permits no PR destination"));
        }
        let operation = if self.body.is_some() {
            match config.resolve_operation(self.operation) {
                Ok(operation) => operation,
                Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
            }
        } else {
            AdoPullRequestBodyOperation::Replace
        };
        let legacy = legacy_policy(ctx, "update-pull-request", "update-description")?;
        if legacy.is_some()
            && (self.title.is_some()
                || self.body.is_none()
                || operation != AdoPullRequestBodyOperation::Replace
                || config.include_stats)
        {
            return Ok(ExecutionResult::failure(
                "legacy update-description requires a replacement body, no title and include-stats: false",
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
        if self.title.is_none() && self.body.is_none() {
            return Ok(ExecutionResult::failure(
                "at least one of title or body is required",
            ));
        }
        let requested = self.requested_id()?;
        super::pr_common::validate_created_dependency(Self::NAME, requested.as_ref(), ctx)?;
        let (pr_id, target) = match resolve_pr_policy_target(
            &config.target_policy()?,
            requested.as_ref(),
            super::pr_common::repository_selection(
                &config.target_policy()?, requested.as_ref(), self.repository.as_deref(), config.target_repo.as_deref(),
            ),
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
        let client = super::pr_http::client()?;
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
            if let Err(error) = validate_title(&title) {
                return Ok(ExecutionResult::failure(error.to_string()));
            }
            patch.insert("title".to_string(), Value::String(title));
        }
        if let Some(body) = body.as_deref() {
            let description = match build_updated_body(
                current.description.as_deref().unwrap_or_default(),
                body,
                operation,
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
        .context("Failed to update Azure DevOps pull request; delivery is uncertain")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .bounded_text()
                .await
                .unwrap_or_else(|error| format!("Failed to read PR error response: {error}"));
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
            pull_request_id: None,
            repository: None,
        }
    }

    #[test]
    fn contract_name_and_budget() {
        assert_eq!(UpdatePullRequestResult::NAME, "update-pull-request");
        assert_eq!(UpdatePullRequestResult::DEFAULT_MAX, 1);
    }

    #[test]
    fn config_has_narrow_body_defaults_without_retired_branch_control() {
        let config: UpdatePullRequestConfig =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(config.title);
        assert!(config.body);
        assert!(!config.include_stats);
        assert_eq!(config.effective_default_operation(), Some(AdoPullRequestBodyOperation::Replace));

        for value in [true, false] {
            assert!(serde_json::from_value::<UpdatePullRequestConfig>(
                serde_json::json!({"update-branch":value})).is_err());
        }
    }

    #[test]
    fn body_operation_grants_are_independent_of_the_default() {
        for (value, valid) in [
            (serde_json::json!({"allowed-operations":["append"]}), true),
            (serde_json::json!({"allowed-operations":["append","replace"]}), false),
            (serde_json::json!({"allowed-operations":["append","replace"],"default-operation":"append"}), true),
            (serde_json::json!({"allowed-operations":["append"],"default-operation":"replace"}), false),
            (serde_json::json!({"allowed-operations":[]}), true),
        ] {
            let config: UpdatePullRequestConfig = serde_json::from_value(value).unwrap();
            assert_eq!(validate_update_pull_request_config(&config).is_ok(), valid);
        }
        let denied: UpdatePullRequestConfig = serde_json::from_value(
            serde_json::json!({"allowed-operations":[]}),
        ).unwrap();
        assert!(denied.resolve_operation(None).is_err());
        assert!(denied.resolve_operation(Some(AdoPullRequestBodyOperation::Replace)).is_err());
        assert!(serde_json::from_value::<UpdatePullRequestConfig>(
            serde_json::json!({"operation":"replace-island"}),
        ).is_err(), "the retired author default must not remain an alias");
    }

    #[tokio::test]
    async fn an_agent_cannot_expand_a_body_operation_grant() {
        let server = MockServer::start().await;
        let ctx = context(&server, serde_json::json!({
            "allowed-operations":["replace-island"], "include-stats":false
        }));
        let mut result: UpdatePullRequestResult = UpdatePullRequestParams {
            title: None, body: Some("requested body".into()),
            operation: Some(AdoPullRequestBodyOperation::Replace), ..params()
        }.try_into().unwrap();
        let result = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("allowed-operations"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn rejects_retired_reference_aliases_even_when_the_values_agree() {
        let empty = UpdatePullRequestParams {
            title: None,
            ..params()
        };
        assert!(empty.validate().is_ok());

        for alias in ["pullRequestId","pull_request_number","pullRequestNumber","pr_number","prNumber","pr","id"] {
            let mut value = serde_json::json!({"title":"Updated title","pull_request_id":1});
            value[alias] = serde_json::json!(1);
            assert!(serde_json::from_value::<UpdatePullRequestParams>(value).is_err(), "{alias}");
        }
    }

    #[test]
    fn include_stats_is_canonical_and_footer_requires_source_migration() {
        assert!(!serde_json::from_value::<UpdatePullRequestConfig>(
            serde_json::json!({"include-stats":false})).unwrap().include_stats);
        for footer in [true, false] {
            assert!(serde_json::from_value::<UpdatePullRequestConfig>(
                serde_json::json!({"footer":footer})).is_err());
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
    async fn migration_rejects_unauthorized_title_operation_and_decoration_before_network() {
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
                serde_json::json!({"allowed-operations": ["append"], "include-stats": false}),
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
                "include-stats": false,
                "allowed-operations": ["append"],
                "required-labels": ["automated"],
                "required-title-prefix": "[bot]"
            }),
        );
        let mut result: UpdatePullRequestResult = UpdatePullRequestParams {
            title: Some("Updated title".to_string()),
            body: Some("New body".to_string()),
            operation: None,
            pull_request_id: None,
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
        for value in [true, false] {
            let request = serde_json::json!({"name":"update-pull-request","title":"Updated title","update_branch":value});
            assert!(crate::execute::execute_safe_output(&request, &ctx).await.is_err());
        }
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
    async fn server_title_rejection_is_reported_without_truncation_or_retry() {
        let server = MockServer::start().await;
        let route = "/project/_apis/git/repositories/11111111-1111-1111-1111-111111111111/pullRequests/7";
        let title = "\u{1f600}".repeat(super::super::pr_common::MAX_TITLE_SCALARS);
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr(7)))
            .expect(1)
            .mount(&server).await;
        Mock::given(method("PATCH"))
            .and(path(route))
            .and(body_json(serde_json::json!({"title": title})))
            .respond_with(ResponseTemplate::new(400).set_body_string("Server title constraint rejected the supplied value"))
            .expect(1)
            .mount(&server).await;
        let ctx = context(&server, serde_json::json!({}));
        let mut result: UpdatePullRequestResult = UpdatePullRequestParams {
            title: Some(title), ..params()
        }.try_into().unwrap();
        let result = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!result.success && !result.is_warning());
        assert!(result.message.contains("HTTP 400"));
        assert!(result.message.contains("Server title constraint"));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
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
