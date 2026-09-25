use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::pr_common::{
    PullRequestReference, describe_pr_reference, resolve_configured_pr_target, validate_reference,
};
use super::pr_mutations::UpdatePrContext;
use super::{ExecutionContext, ExecutionResult, Executor, ToolResult, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config, sanitize_markdown};
use crate::secure::Identifier;
use crate::tool_result;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePullRequestCommentParams {
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,
    #[serde(default)]
    pub repository: Option<String>,
    pub thread_id: i32,
    pub comment_id: i32,
    pub content: String,
}

impl Validate for UpdatePullRequestCommentParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(reference) = &self.pull_request_id {
            validate_reference(reference)?;
        }
        if let Some(repository) = &self.repository {
            crate::validate::reject_pipeline_injection(repository, "repository")?;
        }
        ensure!(
            self.thread_id > 0 && self.comment_id > 0,
            "thread_id and comment_id must be positive"
        );
        super::pr_comments::validate_body(&self.content)
    }
}

tool_result! {
    name = "update-pull-request-comment",
    write = true,
    params = UpdatePullRequestCommentParams,
    #[serde(deny_unknown_fields)]
    pub struct UpdatePullRequestCommentResult {
        #[serde(default)]
        pull_request_id:Option<PullRequestReference>,
        #[serde(default)]
        repository:Option<String>,
        thread_id:i32,
        comment_id:i32,
        content:String,
    }
}

impl SanitizeContent for UpdatePullRequestCommentResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
        self.content = sanitize_markdown(&self.content);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct UpdatePullRequestCommentConfig {
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
    #[serde(
        default = "super::pr_comments::default_comment_key",
        rename = "comment-key"
    )]
    #[sanitize_config(skip)]
    pub comment_key: Identifier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

impl Default for UpdatePullRequestCommentConfig {
    fn default() -> Self {
        Self {
            target: Default::default(),
            target_repo: None,
            allowed_repositories: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            comment_key: super::pr_comments::default_comment_key(),
            max: None,
        }
    }
}

#[async_trait::async_trait]
impl Executor for UpdatePullRequestCommentResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "update owned comment #{} in thread #{} on {}",
            self.comment_id,
            self.thread_id,
            describe_pr_reference(self.pull_request_id.as_ref())
        )
    }
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        UpdatePullRequestCommentParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
            thread_id: self.thread_id,
            comment_id: self.comment_id,
            content: self.content.clone(),
        }
        .validate()?;
        let config: UpdatePullRequestCommentConfig = ctx.get_tool_config(Self::NAME)?;
        ensure!(
            config.comment_key.len() <= 100,
            "comment-key must fit 100 bytes"
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
        super::pr_comments::owner(ctx, "comment", &config.comment_key)?
            .context("Owned updates require a complete trusted pipeline identity")?;
        let client = super::pr_comments::client()?;
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
        let thread = super::pr_comments::read_thread(&operation, self.thread_id).await?;
        let owner = super::pr_comments::owner_for_thread(ctx, &config.comment_key, &thread)?;
        let actor = super::pr_comments::actor(&operation).await?;
        super::pr_comments::update_owned(
            &operation,
            &owner,
            &actor,
            &thread,
            self.comment_id,
            &self.content,
            None,
        )
        .await
    }
}
