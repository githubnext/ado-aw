//! Publish an existing active draft PR without changing its vote or merge policy.
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::pr_common::{
    PullRequestReference, describe_pr_reference, repository_api_base, resolve_configured_pr_target,
    validate_reference,
};
use super::{
    ExecutionContext, ExecutionResult, Executor, ToolResult, Validate, authenticate_ado_request,
};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkPullRequestReadyParams {
    /// Omit only for a configured fixed or trusted triggering target.
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for MarkPullRequestReadyParams {
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
    name = "mark-pull-request-as-ready-for-review",
    write = true,
    params = MarkPullRequestReadyParams,
    #[serde(deny_unknown_fields)]
    pub struct MarkPullRequestReadyResult {
        #[serde(default)]
        pull_request_id:Option<PullRequestReference>,
        #[serde(default)]
        repository:Option<String>,
    }
}

impl SanitizeContent for MarkPullRequestReadyResult {
    fn sanitize_content_fields(&mut self) {
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct MarkPullRequestReadyConfig {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

#[derive(Deserialize)]
struct PrState {
    #[serde(rename = "pullRequestId")]
    id: u64,
    status: String,
    #[serde(rename = "isDraft")]
    draft: bool,
}

async fn read_pr(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    ctx: &ExecutionContext,
    id: u64,
) -> anyhow::Result<PrState> {
    let response = authenticate_ado_request(client.get(url), token, ctx.write_connection_type)
        .send()
        .await
        .context("Failed to fetch PR publication state")?;
    ensure!(
        response.status().is_success(),
        "Failed to fetch PR publication state (HTTP {})",
        response.status()
    );
    let state: PrState = response
        .json()
        .await
        .context("Malformed PR publication state")?;
    ensure!(
        state.id == id,
        "PR publication response identified a different PR"
    );
    Ok(state)
}

#[async_trait::async_trait]
impl Executor for MarkPullRequestReadyResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "mark {} ready for review",
            describe_pr_reference(self.pull_request_id.as_ref())
        )
    }
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        MarkPullRequestReadyParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
        }
        .validate()?;
        let _: MarkPullRequestReadyConfig = ctx.get_tool_config(Self::NAME)?;
        let (id, target) = match resolve_configured_pr_target(
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
        let token = ctx
            .access_token
            .as_deref()
            .context("No access token available")?;
        let url = format!(
            "{}/pullRequests/{id}?api-version=7.1",
            repository_api_base(&target)
        );
        let client = reqwest::Client::new();
        let before = read_pr(&client, &url, token, ctx, id).await?;
        ensure!(
            before.status.eq_ignore_ascii_case("active"),
            "Only an active PR can be marked ready for review"
        );
        let mut data =
            serde_json::json!({"pull_request_id":id,"repository":target.qualified_repository()});
        if !before.draft {
            data["already_ready"] = serde_json::json!(true);
            return Ok(ExecutionResult::success_with_data(
                "PR is already ready for review",
                data,
            ));
        }
        let response =
            authenticate_ado_request(client.patch(&url), token, ctx.write_connection_type)
                .json(&serde_json::json!({"isDraft":false}))
                .send()
                .await;
        match response {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                data["publication_status"] = serde_json::json!("failed");
                return Ok(ExecutionResult::failure_with_data(
                    format!("PR publication failed (HTTP {})", response.status()),
                    data,
                ));
            }
            Err(error) => {
                data["publication_status"] = serde_json::json!("uncertain");
                return Ok(ExecutionResult::failure_with_data(
                    format!("PR publication delivery is uncertain: {error}"),
                    data,
                ));
            }
        }
        match read_pr(&client, &url, token, ctx, id).await {
            Ok(after) if !after.draft && after.status.eq_ignore_ascii_case("active") => {
                data["publication_status"] = serde_json::json!("confirmed");
                Ok(ExecutionResult::success_with_data(
                    "PR publication confirmed",
                    data,
                ))
            }
            result => {
                data["publication_status"] = serde_json::json!("unconfirmed");
                let reason = match result {
                    Ok(_) => "PR is still draft or no longer active".to_string(),
                    Err(error) => format!("{error:#}"),
                };
                Ok(ExecutionResult::failure_with_data(
                    format!("Could not confirm PR publication: {reason}"),
                    data,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, method},
    };

    #[tokio::test]
    async fn publishes_only_is_draft_and_requires_authoritative_readback() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        for persists in [true, false] {
            let server = MockServer::start().await;
            let draft = Arc::new(AtomicBool::new(true));
            let read = draft.clone();
            Mock::given(method("GET")).respond_with(move |_:&wiremock::Request|{
                ResponseTemplate::new(200).set_body_json(json!({"pullRequestId":42,"status":"active","isDraft":read.load(Ordering::SeqCst)}))
            }).expect(2).mount(&server).await;
            Mock::given(method("PATCH"))
                .and(body_json(json!({"isDraft":false})))
                .respond_with(move |_: &wiremock::Request| {
                    if persists {
                        draft.store(false, Ordering::SeqCst);
                    }
                    ResponseTemplate::new(200)
                })
                .expect(1)
                .mount(&server)
                .await;
            let mut ctx = ExecutionContext {
                ado_org_url: Some(server.uri()),
                ado_organization: Some("org".into()),
                ado_project: Some("P".into()),
                repository_name: Some("repo".into()),
                access_token: Some("token".into()),
                ..Default::default()
            };
            ctx.tool_configs.insert(
                MarkPullRequestReadyResult::NAME.into(),
                json!({"target":"*"}),
            );
            let result = crate::execute::execute_safe_output(
                &json!({"name":MarkPullRequestReadyResult::NAME,"pull_request_id":42}),
                &ctx,
            )
            .await
            .unwrap()
            .1;
            assert_eq!(result.success, persists);
        }
    }

    #[tokio::test]
    async fn ready_is_a_noop_and_terminal_states_never_write() {
        for (status, draft, success) in [
            ("active", false, true),
            ("completed", false, false),
            ("abandoned", true, false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"pullRequestId":42,"status":status,"isDraft":draft})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let mut ctx = ExecutionContext {
                ado_org_url: Some(server.uri()),
                ado_organization: Some("org".into()),
                ado_project: Some("P".into()),
                repository_name: Some("repo".into()),
                access_token: Some("token".into()),
                ..Default::default()
            };
            ctx.tool_configs.insert(
                MarkPullRequestReadyResult::NAME.into(),
                json!({"target":"*"}),
            );
            let result = crate::execute::execute_safe_output(
                &json!({"name":MarkPullRequestReadyResult::NAME,"pull_request_id":42}),
                &ctx,
            )
            .await;
            assert_eq!(
                result.as_ref().is_ok_and(|(_, result)| result.success),
                success
            );
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        }
    }
}
