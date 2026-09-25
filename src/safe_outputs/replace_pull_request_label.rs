use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::add_pr_labels::normalize_label_batch;
use super::pr_common::{
    PullRequestReference, describe_pr_reference, resolve_configured_pr_target, validate_reference,
};
use super::pr_mutations::UpdatePrContext;
use super::{
    ExecutionContext, ExecutionResult, Executor, ToolResult, Validate, authenticate_ado_request,
};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::secure::PrLabelName;
use crate::tool_result;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplacePullRequestLabelParams {
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,
    #[serde(default)]
    pub repository: Option<String>,
    pub from: PrLabelName,
    pub to: PrLabelName,
}

impl Validate for ReplacePullRequestLabelParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(reference) = &self.pull_request_id {
            validate_reference(reference)?;
        }
        if let Some(repository) = &self.repository {
            crate::validate::reject_pipeline_injection(repository, "repository")?;
        }
        ensure!(
            !self.from.trim().eq_ignore_ascii_case(self.to.trim()),
            "Replacement labels must differ"
        );
        Ok(())
    }
}

tool_result! {
    name = "replace-pull-request-label",
    write = true,
    params = ReplacePullRequestLabelParams,
    #[serde(deny_unknown_fields)]
    pub struct ReplacePullRequestLabelResult {
        #[serde(default)]
        pull_request_id: Option<PullRequestReference>,
        #[serde(default)]
        repository: Option<String>,
        from: PrLabelName,
        to: PrLabelName,
    }
}

impl SanitizeContent for ReplacePullRequestLabelResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrLabelTransition {
    pub from: PrLabelName,
    pub to: PrLabelName,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct ReplacePullRequestLabelConfig {
    #[serde(default)]
    #[sanitize_config(skip)]
    pub target: super::update_pull_request::UpdatePullRequestTarget,
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    #[serde(default, rename = "allowed-add")]
    pub allowed_add: Vec<String>,
    #[serde(default, rename = "allowed-remove")]
    pub allowed_remove: Vec<String>,
    #[serde(default, rename = "blocked-labels")]
    pub blocked_labels: Vec<String>,
    #[serde(default, rename = "allowed-transitions")]
    #[sanitize_config(skip)]
    pub allowed_transitions: Vec<PrLabelTransition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

pub(crate) fn validate_replace_pr_label_config(
    config: &ReplacePullRequestLabelConfig,
) -> anyhow::Result<()> {
    for label in config
        .allowed_add
        .iter()
        .chain(&config.allowed_remove)
        .chain(&config.blocked_labels)
    {
        PrLabelName::parse(label)?;
    }
    for transition in &config.allowed_transitions {
        ensure!(
            !transition
                .from
                .trim()
                .eq_ignore_ascii_case(transition.to.trim()),
            "Allowed label transitions must change the label"
        );
    }
    Ok(())
}

#[async_trait::async_trait]
impl Executor for ReplacePullRequestLabelResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "replace label '{}' with '{}' on {}",
            self.from,
            self.to,
            describe_pr_reference(self.pull_request_id.as_ref())
        )
    }
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        ReplacePullRequestLabelParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
        }
        .validate()?;
        let config: ReplacePullRequestLabelConfig = ctx.get_tool_config(Self::NAME)?;
        validate_replace_pr_label_config(&config)?;
        normalize_label_batch(
            std::slice::from_ref(&self.from),
            &config.allowed_remove,
            &config.blocked_labels,
            1,
        )?;
        normalize_label_batch(
            std::slice::from_ref(&self.to),
            &config.allowed_add,
            &config.blocked_labels,
            1,
        )?;
        let from = self.from.trim();
        let to = self.to.trim();
        ensure!(
            config.allowed_transitions.is_empty()
                || config
                    .allowed_transitions
                    .iter()
                    .any(
                        |transition| transition.from.trim().eq_ignore_ascii_case(from)
                            && transition.to.trim().eq_ignore_ascii_case(to)
                    ),
            "Label transition is not in allowed-transitions"
        );
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
        let operation = UpdatePrContext {
            client: &client,
            target,
            pr_id,
            token: ctx
                .access_token
                .as_deref()
                .context("No access token available")?,
            connection_type: ctx.write_connection_type,
        };
        let before = super::pr_labels::read_labels(&operation).await?;
        let has_from = before
            .iter()
            .any(|label| label.name.eq_ignore_ascii_case(from));
        let has_to = before
            .iter()
            .any(|label| label.name.eq_ignore_ascii_case(to));
        let mut data = serde_json::json!({"pull_request_id":pr_id,"repository":operation.target.qualified_repository(),
            "from":from,"to":to,"addition_status":if has_to {"already-present"} else {"not-attempted"},
            "removal_status":"not-attempted"});
        if !has_from {
            if has_to {
                data["already_replaced"] = serde_json::json!(true);
                return Ok(ExecutionResult::success_with_data(
                    "PR label transition already applied",
                    data,
                ));
            }
            return Ok(ExecutionResult::failure_with_data(
                "Replacement source label is absent",
                data,
            ));
        }
        if !has_to {
            let response = authenticate_ado_request(
                client.post(format!(
                    "{}/pullRequests/{pr_id}/labels?api-version=7.1",
                    operation.repository_api_base()
                )),
                operation.token,
                operation.connection_type,
            )
            .json(&serde_json::json!({"name":to}))
            .send()
            .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    data["addition_status"] = serde_json::json!("applied")
                }
                Ok(response) => {
                    data["addition_status"] = serde_json::json!("failed");
                    return Ok(ExecutionResult::failure_with_data(
                        format!(
                            "Replacement label addition failed (HTTP {})",
                            response.status()
                        ),
                        data,
                    ));
                }
                Err(error) => {
                    data["addition_status"] = serde_json::json!("uncertain");
                    return Ok(ExecutionResult::failure_with_data(
                        format!(
                            "Replacement label addition is uncertain; source label was not removed: {error}"
                        ),
                        data,
                    ));
                }
            }
        }
        let verified = match super::pr_labels::read_labels(&operation).await {
            Ok(labels) => labels,
            Err(error) => {
                return Ok(ExecutionResult::failure_with_data(
                    format!("Cannot verify replacement label; source not removed: {error:#}"),
                    data,
                ));
            }
        };
        if !verified
            .iter()
            .any(|label| label.name.eq_ignore_ascii_case(to))
        {
            return Ok(ExecutionResult::failure_with_data(
                "Replacement label not visible; source not removed",
                data,
            ));
        }
        let removed = match super::pr_labels::remove_labels(&operation, &[from.to_string()]).await {
            Ok(result) => result,
            Err(error) => {
                return Ok(ExecutionResult::failure_with_data(
                    format!("Replacement label present but removal could not proceed: {error:#}"),
                    data,
                ));
            }
        };
        data["removal"] = serde_json::json!(removed.data);
        if !removed.success {
            data["removal_status"] = data["removal"]["removal_status"].clone();
            return Ok(ExecutionResult::failure_with_data(removed.message, data));
        }
        data["removal_status"] = serde_json::json!("applied");
        let after = match super::pr_labels::read_labels(&operation).await {
            Ok(labels) => labels,
            Err(error) => {
                return Ok(ExecutionResult::failure_with_data(
                    format!("Replacement writes completed but readback is uncertain: {error:#}"),
                    data,
                ));
            }
        };
        if after
            .iter()
            .any(|label| label.name.eq_ignore_ascii_case(from))
            || !after
                .iter()
                .any(|label| label.name.eq_ignore_ascii_case(to))
        {
            return Ok(ExecutionResult::failure_with_data(
                "Concurrent label changes prevented confirmation of the replacement",
                data,
            ));
        }
        Ok(ExecutionResult::success_with_data(
            "PR label transition confirmed",
            data,
        ))
    }
}
