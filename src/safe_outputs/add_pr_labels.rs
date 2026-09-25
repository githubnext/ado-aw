//! Add labels without replacing or removing existing Azure DevOps PR labels.

use super::pr_common::{
    PullRequestReference, legacy_policy, validate_reference,
};
use super::pr_mutations::{UpdatePrContext, execute_add_labels};
use super::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use crate::secure::PrLabelName;
use super::ToolResult;
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddPrLabelsParams {
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,
    #[serde(default)]
    pub repository: Option<String>,
    /// Label names to add. The configured max-labels defaults to 10 unique names.
    pub labels: Vec<PrLabelName>,
}

impl Validate for AddPrLabelsParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(reference) = &self.pull_request_id {
            validate_reference(reference)?;
        }
        validate_label_input(&self.labels)?;
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
        #[serde(default)]
        pull_request_id: Option<PullRequestReference>,
        #[serde(default)]
        repository: Option<String>,
        labels: Vec<PrLabelName>,
    }
}

impl SanitizeContent for AddPrLabelsResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct AddPrLabelsConfig {
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
    #[serde(default, rename = "allowed-labels")]
    pub allowed_labels: Vec<String>,
    #[serde(default, rename = "blocked-labels")]
    pub blocked_labels: Vec<String>,
    #[serde(default = "default_max_labels", rename = "max-labels")]
    #[sanitize_config(skip)]
    pub max_labels: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

fn default_max_labels() -> usize { 10 }

impl Default for AddPrLabelsConfig {
    fn default() -> Self {
        Self {
            target: Default::default(), target_repo: None,
            required_labels: Vec::new(), required_title_prefix: None,
            allowed_repositories: Vec::new(), allowed_labels: Vec::new(),
            blocked_labels: Vec::new(), max_labels: default_max_labels(), max: None,
        }
    }
}

pub(crate) fn validate_label_input<T: AsRef<str>>(labels: &[T]) -> anyhow::Result<()> {
    ensure!(!labels.is_empty(), "labels list must not be empty");
    ensure!(labels.len() <= 1_000, "labels list must contain at most 1000 raw entries");
    for label in labels {
        crate::secure::PrLabelName::parse(label.as_ref())?;
    }
    Ok(())
}

pub(crate) fn normalize_label_batch<T: AsRef<str>>(
    labels: &[T], allowed: &[String], blocked: &[String], max_labels: usize,
) -> anyhow::Result<Vec<String>> {
    validate_label_input(labels)?;
    ensure!(max_labels > 0 && max_labels <= 1_000, "max-labels must be between 1 and 1000");
    let mut normalized = Vec::<String>::new();
    for label in labels {
        let label = label.as_ref().trim();
        ensure!(!blocked.iter().any(|item| item.trim().eq_ignore_ascii_case(label)),
            "Label '{label}' is blocked by blocked-labels");
        ensure!(allowed.is_empty() || allowed.iter().any(|item| item.trim().eq_ignore_ascii_case(label)),
            "Label '{label}' is not in allowed-labels");
        if !normalized.iter().any(|item| item.eq_ignore_ascii_case(label)) {
            normalized.push(label.to_string());
        }
    }
    ensure!(normalized.len() <= max_labels, "label batch exceeds max-labels: {max_labels}");
    Ok(normalized)
}

pub(crate) fn validate_add_pr_labels_config(config: &AddPrLabelsConfig) -> anyhow::Result<()> {
    ensure!(config.max_labels > 0 && config.max_labels <= 1_000, "max-labels must be between 1 and 1000");
    for label in config.allowed_labels.iter().chain(&config.blocked_labels) {
        crate::secure::PrLabelName::parse(label)?;
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
impl Executor for AddPrLabelsResult {
    fn dry_run_summary(&self) -> String {
        format!("add labels to {}", super::pr_common::describe_pr_reference(self.pull_request_id.as_ref()))
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
        let labels = match normalize_label_batch(
            &self.labels, &config.allowed_labels, &config.blocked_labels, config.max_labels,
        ) {
            Ok(labels) => labels,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        let (pr_id, target) = match super::pr_common::resolve_configured_pr_target(
            Self::NAME, self.pull_request_id.as_ref(), self.repository.as_deref(), ctx,
        ).await? {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        if let Some(legacy) = legacy_policy(ctx, "add-pull-request-labels", "add-labels")?
            && let Err(failure) = super::pr_common::validate_pr_repository_policy(
                &target, &legacy.allowed_repositories, ctx,
            )
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
            &labels,
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
    fn label_policy_limits_are_exact_and_blocking_wins() {
        for count in [0,1,10,11] {
            let labels=(0..count).map(|i|format!("label-{i}")).collect::<Vec<_>>();
            assert_eq!(normalize_label_batch(&labels,&[],&[],10).is_ok(),(1..=10).contains(&count));
        }
        assert!(normalize_label_batch(&vec!["label".to_string();1_001],&[],&[],10).is_err());
        assert_eq!(normalize_label_batch(&[" Label ".to_string(),"label".to_string()],&["LABEL".into()],&[],1).unwrap(),vec!["Label"]);
        assert!(normalize_label_batch(&["label"],&["label".into()],&["LABEL".into()],10).is_err());
        assert!(normalize_label_batch(&["other"],&["label".into()],&[],10).is_err());
        let labels=(0..11).map(|i|format!("label-{i}")).collect::<Vec<_>>();
        assert!(normalize_label_batch(&labels,&[],&[],11).is_ok());
        for name in [""," ","bad\nlabel","##vso[task.complete]x"] {
            assert!(validate_label_input(&[name]).is_err());
        }
    }

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
            .insert("add-pull-request-labels".into(), serde_json::json!({"target":"*"}));
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
