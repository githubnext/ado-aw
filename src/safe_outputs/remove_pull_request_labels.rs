use anyhow::Context;
use schemars::JsonSchema;
use serde::Deserialize;

use super::add_pr_labels::{
    normalize_label_batch, validate_add_pr_labels_config, validate_label_input,
};
use super::pr_common::{
    PullRequestReference, describe_pr_reference, resolve_configured_pr_target, validate_reference,
};
use super::pr_mutations::UpdatePrContext;
use super::{AddPrLabelsConfig, ExecutionContext, ExecutionResult, Executor, ToolResult, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::secure::PrLabelName;
use crate::tool_result;

pub type RemovePullRequestLabelsConfig = AddPrLabelsConfig;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemovePullRequestLabelsParams {
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,
    #[serde(default)]
    pub repository: Option<String>,
    pub labels: Vec<PrLabelName>,
}

impl Validate for RemovePullRequestLabelsParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(reference) = &self.pull_request_id {
            validate_reference(reference)?;
        }
        if let Some(repository) = &self.repository {
            crate::validate::reject_pipeline_injection(repository, "repository")?;
        }
        validate_label_input(&self.labels)
    }
}

tool_result! {
    name = "remove-pull-request-labels",
    write = true,
    params = RemovePullRequestLabelsParams,
    #[serde(deny_unknown_fields)]
    pub struct RemovePullRequestLabelsResult {
        #[serde(default)]
        pull_request_id: Option<PullRequestReference>,
        #[serde(default)]
        repository: Option<String>,
        labels: Vec<PrLabelName>,
    }
}

impl SanitizeContent for RemovePullRequestLabelsResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[async_trait::async_trait]
impl Executor for RemovePullRequestLabelsResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "remove labels from {}",
            describe_pr_reference(self.pull_request_id.as_ref())
        )
    }
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        RemovePullRequestLabelsParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
            labels: self.labels.clone(),
        }
        .validate()?;
        let config: RemovePullRequestLabelsConfig = ctx.get_tool_config(Self::NAME)?;
        validate_add_pr_labels_config(&config)?;
        let labels = normalize_label_batch(
            &self.labels,
            &config.allowed_labels,
            &config.blocked_labels,
            config.max_labels,
        )?;
        let (pr_id, target) = match resolve_configured_pr_target(
            Self::NAME,
            self.pull_request_id.as_ref(),
            self.repository.as_deref(),
            ctx,
        )
        .await?
        {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        let client = reqwest::Client::new();
        super::pr_labels::remove_labels(
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
            &labels,
        )
        .await
    }
}
