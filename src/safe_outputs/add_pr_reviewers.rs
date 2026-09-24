//! Add operator-authorized, verified reviewers to an Azure DevOps PR.

use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::pr_common::{PullRequestReference, legacy_policy, resolve_pr_target};
use super::pr_mutations::{
    UpdatePrContext, execute_add_reviewers, validate_and_normalize_reviewers,
};
use super::update_pr::{UpdatePrConfig, UpdatePrParams};
use super::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;

#[derive(Deserialize, JsonSchema)]
pub struct AddPrReviewersParams {
    pub pull_request_id: PullRequestReference,
    #[serde(default)]
    pub repository: Option<String>,
    /// Reviewer GUIDs, exact identity names or email addresses.
    pub reviewers: Vec<String>,
}

impl Validate for AddPrReviewersParams {
    fn validate(&self) -> anyhow::Result<()> {
        UpdatePrParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
            operation: "add-reviewers".into(),
            reviewers: Some(self.reviewers.clone()),
            labels: None,
            vote: None,
            description: None,
        }
        .validate()
    }
}

tool_result! {
    name = "add-pull-request-reviewers",
    write = true,
    params = AddPrReviewersParams,
    pub struct AddPrReviewersResult {
        pull_request_id: PullRequestReference,
        #[serde(default)]
        repository: Option<String>,
        reviewers: Vec<String>,
    }
}

impl SanitizeContent for AddPrReviewersResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
        self.reviewers = self.reviewers.iter().map(|v| sanitize_config(v)).collect();
    }
}

fn default_max_reviewers() -> usize {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct AddPrReviewersConfig {
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
    /// Empty or literal "*" permits any otherwise-valid reviewer.
    #[serde(default, rename = "allowed-reviewers")]
    pub allowed_reviewers: Vec<String>,
    #[serde(default = "default_max_reviewers", rename = "max-reviewers")]
    #[sanitize_config(skip)]
    pub max_reviewers: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

impl Default for AddPrReviewersConfig {
    fn default() -> Self {
        Self {
            allowed_repositories: Vec::new(),
            allowed_reviewers: Vec::new(),
            max_reviewers: default_max_reviewers(),
            max: None,
        }
    }
}

pub(crate) fn validate_add_pr_reviewers_config(
    config: &AddPrReviewersConfig,
) -> anyhow::Result<()> {
    ensure!(
        config.max_reviewers > 0,
        "add-pull-request-reviewers.max-reviewers must be greater than zero"
    );
    for reviewer in &config.allowed_reviewers {
        ensure!(
            !reviewer.trim().is_empty(),
            "allowed-reviewers entries must not be empty"
        );
        ensure!(
            reviewer.len() <= 256,
            "allowed-reviewers entries must be 256 bytes or fewer"
        );
        crate::validate::reject_pipeline_injection(reviewer, "allowed-reviewers")?;
    }
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
impl Executor for AddPrReviewersResult {
    fn dry_run_summary(&self) -> String {
        format!("add reviewers to PR #{}", self.pull_request_id)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        let params = AddPrReviewersParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
            reviewers: self.reviewers.clone(),
        };
        if let Err(error) = params.validate() {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        ensure!(
            ctx.tool_configs.contains_key("add-pull-request-reviewers"),
            "add-pull-request-reviewers is not configured"
        );
        let config: AddPrReviewersConfig = ctx.get_tool_config("add-pull-request-reviewers")?;
        validate_add_pr_reviewers_config(&config)?;
        let policy = UpdatePrConfig {
            allowed_repositories: config.allowed_repositories,
            allowed_reviewers: config.allowed_reviewers,
            max_reviewers: config.max_reviewers,
            ..Default::default()
        };
        if let Err(failure) = validate_and_normalize_reviewers(&self.reviewers, &policy) {
            return Ok(failure);
        }
        let (pr_id, target) = match resolve_pr_target(
            &self.pull_request_id,
            self.repository.as_deref(),
            &policy.allowed_repositories,
            ctx,
        )? {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        let legacy = legacy_policy(ctx, "add-pull-request-reviewers", "add-reviewers")?;
        if let Some(legacy) = &legacy
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
        execute_add_reviewers(
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
            &self.reviewers,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_config_rejects_unknown_fields_and_invalid_policy() {
        for value in [
            serde_json::json!({"allowed-reviewer": ["owner"]}),
            serde_json::json!({"allowed-reviewers": "owner"}),
            serde_json::json!({"allowed-repositories": "self"}),
            serde_json::json!({"max-reviewers": -1}),
            serde_json::json!({"max-reviewers": 1.5}),
        ] {
            assert!(serde_json::from_value::<AddPrReviewersConfig>(value).is_err());
        }
        for value in [
            serde_json::json!({"max-reviewers": 0}),
            serde_json::json!({"allowed-reviewers": [" "]}),
            serde_json::json!({"allowed-reviewers": ["x".repeat(257)]}),
            serde_json::json!({"allowed-reviewers": ["##vso[task.setvariable variable=x]y"]}),
            serde_json::json!({"allowed-repositories": [""]}),
            serde_json::json!({"allowed-repositories": ["##vso[task.setvariable variable=x]y"]}),
        ] {
            let config = serde_json::from_value::<AddPrReviewersConfig>(value).unwrap();
            assert!(validate_add_pr_reviewers_config(&config).is_err());
        }
        for allowed in [vec![], vec!["*"], vec!["Owner@example.com"]] {
            let config: AddPrReviewersConfig = serde_json::from_value(serde_json::json!({
                "allowed-reviewers": allowed, "allowed-repositories": ["self"], "max": 0
            }))
            .unwrap();
            assert!(validate_add_pr_reviewers_config(&config).is_ok());
            assert_eq!(config.max_reviewers, 3);
            assert_eq!(config.max, Some(0));
        }
    }

    #[test]
    fn defaults_and_input_limits_match_legacy() {
        assert_eq!(AddPrReviewersConfig::default().max_reviewers, 3);
        for reviewers in [
            vec![],
            vec!["x".repeat(257)],
            vec!["x".into(); 101],
            vec![" ".into()],
        ] {
            assert!(
                AddPrReviewersParams {
                    pull_request_id: PullRequestReference::Number(1),
                    repository: None,
                    reviewers,
                }
                .validate()
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn historical_metadata_cannot_widen_reviewer_allowlist() {
        let mut ctx = ExecutionContext::default();
        ctx.tool_configs.insert(
            "add-pull-request-reviewers".into(),
            serde_json::json!({
                "allowed-reviewers": ["permitted"], "legacy-update-pr": {"allowed-reviewers": ["*"]}
            }),
        );
        let result: AddPrReviewersResult = AddPrReviewersParams {
            pull_request_id: PullRequestReference::Number(1),
            repository: None,
            reviewers: vec!["forbidden".into()],
        }
        .try_into()
        .unwrap();
        assert!(!result.execute_impl(&ctx).await.unwrap().success);
    }

    #[tokio::test]
    async fn legacy_reviewer_policy_remains_an_additional_restriction() {
        let server = wiremock::MockServer::start().await;
        for legacy in [
            serde_json::json!({"allowed-reviewers": ["permitted"]}),
            serde_json::json!({"max-reviewers": 1}),
        ] {
            let ctx = super::super::pr_common::tests::registered_context(
                &server.uri(),
                "add-pull-request-reviewers",
                serde_json::json!({"allowed-reviewers": ["*"], "max-reviewers": 3, "legacy-update-pr": legacy}),
            );
            let mut result: AddPrReviewersResult = serde_json::from_value(serde_json::json!({
                "name": "add-pull-request-reviewers", "pull_request_id": "#aw_pr123",
                "reviewers": ["forbidden", "second"]
            }))
            .unwrap();
            assert!(!result.execute_sanitized(&ctx).await.unwrap().success);
            assert!(server.received_requests().await.unwrap().is_empty());
        }
    }
}
