//! Add labels without replacing or removing existing Azure DevOps PR labels.

use super::pr_common::{
    PullRequestReference, legacy_policy, resolve_pr_target, validate_reference,
};
use super::pr_mutations::{UpdatePrContext, execute_add_labels};
use super::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddPrLabelsParams {
    pub pull_request_id: PullRequestReference,
    #[serde(default)]
    pub repository: Option<String>,
    pub labels: Vec<String>,
}

impl Validate for AddPrLabelsParams {
    fn validate(&self) -> anyhow::Result<()> {
        validate_reference(&self.pull_request_id)?;
        ensure!(!self.labels.is_empty(), "labels list must not be empty");
        if let Some(repository) = &self.repository {
            crate::validate::reject_pipeline_injection(repository, "repository")?;
        }
        Ok(())
    }
}

tool_result! {
    name = "add-pull-request-labels",
    write = true,
    params = AddPrLabelsParams,
    #[serde(deny_unknown_fields)]
    pub struct AddPrLabelsResult {
        pull_request_id: PullRequestReference,
        #[serde(default)]
        repository: Option<String>,
        labels: Vec<String>,
    }
}

impl SanitizeContent for AddPrLabelsResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
        self.labels = self
            .labels
            .iter()
            .map(|value| sanitize_config(value))
            .collect();
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct AddPrLabelsConfig {
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

pub(crate) fn validate_add_pr_labels_config(config: &AddPrLabelsConfig) -> anyhow::Result<()> {
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
impl Executor for AddPrLabelsResult {
    fn dry_run_summary(&self) -> String {
        format!("add labels to PR #{}", self.pull_request_id)
    }
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if let Err(error) = (AddPrLabelsParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
            labels: self.labels.clone(),
        })
        .validate()
        {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        ensure!(
            ctx.tool_configs.contains_key("add-pull-request-labels"),
            "add-pull-request-labels is not configured"
        );
        let config: AddPrLabelsConfig = ctx.get_tool_config("add-pull-request-labels")?;
        validate_add_pr_labels_config(&config)?;
        let (pr_id, target) = match resolve_pr_target(
            &self.pull_request_id,
            self.repository.as_deref(),
            &config.allowed_repositories,
            ctx,
        )? {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        if let Some(legacy) = legacy_policy(ctx, "add-pull-request-labels", "add-labels")?
            && let Err(failure) = resolve_pr_target(
                &self.pull_request_id,
                self.repository.as_deref(),
                &legacy.allowed_repositories,
                ctx,
            )?
        {
            return Ok(failure);
        }
        let client = reqwest::Client::new();
        execute_add_labels(
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
            &self.labels,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, method, path},
    };

    #[test]
    fn typed_config_rejects_unknown_fields_and_invalid_allowlists() {
        for value in [
            serde_json::json!({"replace-labels": true}),
            serde_json::json!({"allowed-repositories": "self"}),
        ] {
            assert!(serde_json::from_value::<AddPrLabelsConfig>(value).is_err());
        }
        for repository in ["", " ", "##vso[task.setvariable variable=x]y"] {
            let config = AddPrLabelsConfig {
                allowed_repositories: vec![repository.into()],
                ..Default::default()
            };
            assert!(validate_add_pr_labels_config(&config).is_err());
        }
        assert!(validate_add_pr_labels_config(&AddPrLabelsConfig::default()).is_ok());
        let config: AddPrLabelsConfig =
            serde_json::from_value(serde_json::json!({"max": 0})).unwrap();
        assert_eq!(config.max, Some(0));
    }

    #[tokio::test]
    async fn adds_labels_to_exact_cross_project_target() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/Other/_apis/git/repositories/repo/pullRequests/4294967296/labels",
            ))
            .and(body_json(serde_json::json!({"name": "ready"})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_organization: Some("org".into()),
            ado_project: Some("Current".into()),
            access_token: Some("token".into()),
            repository_name: Some("self".into()),
            ..Default::default()
        };
        ctx.allowed_repositories
            .insert("other".into(), "Other/repo".into());
        ctx.tool_configs
            .insert("add-pull-request-labels".into(), serde_json::json!({}));
        let result: AddPrLabelsResult = serde_json::from_value(serde_json::json!({
            "name": "add-pull-request-labels", "pull_request_id": "4294967296",
            "repository": "other", "labels": ["ready"]
        }))
        .unwrap();
        assert!(result.execute_impl(&ctx).await.unwrap().success);
    }

    #[tokio::test]
    async fn temporary_labels_keep_registered_target_and_legacy_scope() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/Other/_apis/git/repositories/repo-id/pullRequests/4294967296/labels",
            ))
            .and(body_json(serde_json::json!({"name": "ready"})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = super::super::pr_common::tests::registered_context(
            &server.uri(),
            "add-pull-request-labels",
            serde_json::json!({"legacy-update-pr": {"allowed-repositories": ["other"]}}),
        );
        let mut result: AddPrLabelsResult = serde_json::from_value(serde_json::json!({
            "name": "add-pull-request-labels", "pull_request_id": "#aw_pr123", "labels": ["ready"]
        }))
        .unwrap();
        assert!(result.execute_sanitized(&ctx).await.unwrap().success);
        ctx.tool_configs.insert(
            "add-pull-request-labels".into(),
            serde_json::json!({"legacy-update-pr": {"allowed-repositories": ["self"]}}),
        );
        assert!(!result.execute_sanitized(&ctx).await.unwrap().success);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
