//! Enable Azure DevOps auto-complete, without immediately merging.

use super::pr_common::{
    PullRequestReference, legacy_policy, validate_reference,
};
use super::pr_mutations::{UpdatePrContext, execute_set_auto_complete};
use super::update_pr::UpdatePrConfig;
use super::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use super::ToolResult;
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPrAutoCompleteParams {
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,
    #[serde(default)]
    pub repository: Option<String>,
}
impl Validate for SetPrAutoCompleteParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(reference) = &self.pull_request_id {
            validate_reference(reference)?;
        }
        if let Some(repository) = &self.repository {
            crate::validate::reject_pipeline_injection(repository, "repository")?;
        }
        Ok(())
    }
}
tool_result! {
    name = "set-pull-request-auto-complete",
    write = true,
    params = SetPrAutoCompleteParams,
    #[serde(deny_unknown_fields)]
    pub struct SetPrAutoCompleteResult {
        #[serde(default)]
        pull_request_id: Option<PullRequestReference>,
        #[serde(default)]
        repository: Option<String>,
    }
}
impl SanitizeContent for SetPrAutoCompleteResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}
fn default_true() -> bool {
    true
}
fn default_merge_strategy() -> String {
    "squash".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct SetPrAutoCompleteConfig {
    #[serde(default)]
    #[sanitize_config(skip)]
    pub target: super::update_pull_request::UpdatePullRequestTarget,
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
    #[serde(default = "default_true", rename = "delete-source-branch")]
    pub delete_source_branch: bool,
    #[serde(default = "default_merge_strategy", rename = "merge-strategy")]
    pub merge_strategy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}
impl Default for SetPrAutoCompleteConfig {
    fn default() -> Self {
        Self {
            target: Default::default(),
            target_repo: None,
            required_labels: Vec::new(),
            required_title_prefix: None,
            allowed_repositories: Vec::new(),
            delete_source_branch: true,
            merge_strategy: default_merge_strategy(),
            max: None,
        }
    }
}
pub(crate) fn validate_set_pr_auto_complete_config(
    config: &SetPrAutoCompleteConfig,
) -> anyhow::Result<()> {
    ensure!(
        ["squash", "noFastForward", "rebase", "rebaseMerge"]
            .contains(&config.merge_strategy.as_str()),
        "Invalid merge-strategy '{}'",
        config.merge_strategy
    );
    for repository in &config.allowed_repositories {
        ensure!(
            !repository.trim().is_empty(),
            "allowed-repositories entries must not be empty"
        );
        crate::validate::reject_pipeline_injection(repository, "allowed-repositories")?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl Executor for SetPrAutoCompleteResult {
    fn dry_run_summary(&self) -> String {
        format!("enable auto-complete on {}", super::pr_common::describe_pr_reference(self.pull_request_id.as_ref()))
    }
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if let Err(error) = (SetPrAutoCompleteParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
        })
        .validate()
        {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        ensure!(
            ctx.tool_configs
                .contains_key("set-pull-request-auto-complete"),
            "set-pull-request-auto-complete is not configured"
        );
        let config: SetPrAutoCompleteConfig =
            ctx.get_tool_config("set-pull-request-auto-complete")?;
        validate_set_pr_auto_complete_config(&config)?;
        let policy = UpdatePrConfig {
            allowed_repositories: config.allowed_repositories,
            delete_source_branch: config.delete_source_branch,
            merge_strategy: config.merge_strategy,
            ..Default::default()
        };
        let (pr_id, target) = match super::pr_common::resolve_configured_pr_target(
            Self::NAME, self.pull_request_id.as_ref(), self.repository.as_deref(), ctx,
        ).await? {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        let legacy = legacy_policy(ctx, "set-pull-request-auto-complete", "set-auto-complete")?;
        if let Some(legacy) = &legacy
            && let Err(failure) = super::pr_common::validate_pr_repository_policy(
                &target, &legacy.allowed_repositories, ctx,
            )
        {
            return Ok(failure);
        }
        let client = reqwest::Client::new();
        execute_set_auto_complete(
            &UpdatePrContext {
                client: &client,
                target,
                pr_id,
                token: ctx
                    .access_token
                    .as_deref()
                    .context("No access token available")?,
                connection_type: ctx.write_connection_type,
            },
            legacy.as_ref().unwrap_or(&policy),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_config_rejects_unknown_fields_and_invalid_allowlists() {
        for value in [
            serde_json::json!({"merge-immediately": true}),
            serde_json::json!({"delete-source-branch": "true"}),
            serde_json::json!({"allowed-repositories": "self"}),
        ] {
            assert!(serde_json::from_value::<SetPrAutoCompleteConfig>(value).is_err());
        }
        for repository in ["", " ", "##vso[task.setvariable variable=x]y"] {
            let config = SetPrAutoCompleteConfig {
                allowed_repositories: vec![repository.into()],
                ..Default::default()
            };
            assert!(validate_set_pr_auto_complete_config(&config).is_err());
        }
        for strategy in ["squash", "noFastForward", "rebase", "rebaseMerge"] {
            let config: SetPrAutoCompleteConfig =
                serde_json::from_value(serde_json::json!({"merge-strategy": strategy, "max": 0}))
                    .unwrap();
            assert!(validate_set_pr_auto_complete_config(&config).is_ok());
            assert_eq!(config.max, Some(0));
        }
    }

    #[test]
    fn completion_defaults_and_validation_match_legacy() {
        let config = SetPrAutoCompleteConfig::default();
        assert!(config.delete_source_branch);
        assert_eq!(config.merge_strategy, "squash");
        assert!(
            validate_set_pr_auto_complete_config(&SetPrAutoCompleteConfig {
                merge_strategy: "merge-immediately".into(),
                ..config
            })
            .is_err()
        );
    }

    #[tokio::test]
    async fn migrated_completion_uses_target_actor_and_exact_legacy_options() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, header, method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/_apis/connectiondata"))
            .and(header("authorization", "Bearer token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"authenticatedUser": {"id": "target-actor"}}),
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(
                "/Other/_apis/git/repositories/repo-id/pullRequests/4294967296",
            ))
            .and(header("authorization", "Bearer token"))
            .and(body_json(serde_json::json!({
                "autoCompleteSetBy": {"id": "target-actor"},
                "completionOptions": {"deleteSourceBranch": false, "mergeStrategy": "rebase"}
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = super::super::pr_common::tests::registered_context(
            &server.uri(),
            "set-pull-request-auto-complete",
            serde_json::json!({
                "delete-source-branch": false, "merge-strategy": "rebase",
                "legacy-update-pr": {"delete-source-branch": false, "merge-strategy": "rebase"}
            }),
        );
        ctx.write_connection_type = Some(crate::compile::types::WriteConnectionType::AzureDevOps);
        let mut result: SetPrAutoCompleteResult = serde_json::from_value(serde_json::json!({
            "name": "set-pull-request-auto-complete", "pull_request_id": "#aw_pr123"
        }))
        .unwrap();
        assert!(result.execute_sanitized(&ctx).await.unwrap().success);
    }
}
