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
        let client = super::pr_http::client()?;
        let (pr_id, target) = match resolve_configured_pr_target(
            Self::NAME,
            self.pull_request_id.as_ref(),
            self.repository.as_deref(),
            ctx,
            &client,
        )
        .await?
        {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        super::pr_comments::owner(ctx, "comment", &config.comment_key)?
            .context("Owned updates require a complete trusted pipeline identity")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    const ACTOR: &str = "33333333-3333-3333-3333-333333333333";
    const ORIGINAL: &str = "Original owned report.";
    const REPLACEMENT: &str = "Replacement owned report.";

    fn context(server: &MockServer, key: &str) -> ExecutionContext {
        let mut ctx = ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_organization: Some("org".into()),
            ado_project: Some("P".into()),
            repository_name: Some("repo".into()),
            ado_project_id: Some("11111111-1111-1111-1111-111111111111".into()),
            pipeline_collection_uri: Some("https://dev.azure.com/source".into()),
            definition_id: Some(7),
            build_id: Some(100),
            access_token: Some("opaque-test-token".into()),
            ..Default::default()
        };
        ctx.tool_configs.insert(
            "update-pull-request-comment".into(),
            json!({"target":"*", "comment-key":key}),
        );
        ctx
    }

    fn proposal() -> Value {
        json!({"name":"update-pull-request-comment","pull_request_id":42,
                "thread_id":3,"comment_id":1,"content":REPLACEMENT})
    }

    fn owned_thread(ctx: &ExecutionContext, key: &str) -> Value {
        let owner =
            super::super::pr_comments::owner(ctx, "comment", &Identifier::parse(key).unwrap())
                .unwrap()
                .unwrap();
        let mut thread = json!({"id":3,"status":"active","comments":[{
            "id":1,"parentCommentId":0,"content":ORIGINAL,"author":{"id":ACTOR}
        }]});
        super::super::pr_comments::stamp(&mut thread, Some(&owner), ctx, ORIGINAL).unwrap();
        thread
    }

    #[tokio::test]
    async fn executor_updates_only_verified_owned_root_comments() {
        for case in [
            "owned",
            "key-limit",
            "actor",
            "pipeline",
            "key",
            "unmarked",
            "hash",
            "reply",
            "comment-id",
            "thread-id",
        ] {
            let server = MockServer::start().await;
            let key = if case == "key-limit" {
                "k".repeat(100)
            } else {
                "report".into()
            };
            let mut ctx = context(&server, &key);
            let mut thread = owned_thread(&ctx, &key);
            let mut entry = proposal();
            match case {
                    "actor" => thread["comments"][0]["author"]["id"] = json!("44444444-4444-4444-4444-444444444444"),
                    "pipeline" => ctx.definition_id = Some(8),
                    "key" => ctx.tool_configs.get_mut("update-pull-request-comment").unwrap()["comment-key"] = json!("different"),
                    "unmarked" => thread["properties"] = json!({}),
                    "hash" => thread["comments"][0]["content"] = json!("A manual edit must not be overwritten."),
                    "reply" => thread["comments"].as_array_mut().unwrap().push(json!({
                        "id":2,"parentCommentId":1,"content":"Reply from the same actor.","author":{"id":ACTOR}
                    })),
                    "comment-id" => entry["comment_id"] = json!(2),
                    "thread-id" => thread["id"] = json!(4),
                    _ => {}
                }
            let original = thread.clone();
            let state = Arc::new(Mutex::new(thread));
            let read = state.clone();
            let route = "/P/_apis/git/repositories/repo/pullRequests/42/threads/3";
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(move |_: &wiremock::Request| {
                    ResponseTemplate::new(200).set_body_json(read.lock().unwrap().clone())
                })
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/_apis/connectiondata"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"authenticatedUser":{"id":ACTOR}})),
                )
                .mount(&server)
                .await;
            let write = state.clone();
            let succeeds = matches!(case, "owned" | "key-limit");
            Mock::given(method("PATCH"))
                .and(path(format!("{route}/comments/1")))
                .respond_with(move |request: &wiremock::Request| {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    assert_eq!(body.as_object().unwrap().len(), 1);
                    write.lock().unwrap()["comments"][0]["content"] = body["content"].clone();
                    ResponseTemplate::new(200)
                })
                .expect(u64::from(succeeds))
                .mount(&server)
                .await;
            let outcome = crate::execute::execute_safe_output(&entry, &ctx).await;
            if succeeds {
                let (_, result) = outcome.unwrap();
                assert!(result.success, "{}", result.message);
                let after = state.lock().unwrap();
                assert!(
                    after["comments"][0]["content"]
                        .as_str()
                        .unwrap()
                        .starts_with(REPLACEMENT)
                );
                assert_eq!(after["properties"], original["properties"]);
                assert_eq!(after["status"], original["status"]);
                assert_eq!(result.data.as_ref().unwrap()["thread_id"], 3);
                assert_eq!(result.data.as_ref().unwrap()["comment_id"], 1);
            } else {
                assert!(outcome.is_err(), "{case} must fail before mutation");
                assert_eq!(*state.lock().unwrap(), original, "{case}");
                assert!(
                    server
                        .received_requests()
                        .await
                        .unwrap()
                        .iter()
                        .all(|request| request.method.as_str() == "GET"),
                    "{case}"
                );
            }
        }
    }

    #[tokio::test]
    async fn executor_rejects_invalid_ids_and_keys_before_network() {
        for case in [
            "zero-thread",
            "negative-thread",
            "zero-comment",
            "negative-comment",
            "long-key",
            "missing-pipeline",
        ] {
            let server = MockServer::start().await;
            let key = if case == "long-key" {
                "k".repeat(101)
            } else {
                "report".into()
            };
            let mut ctx = context(&server, &key);
            let mut entry = proposal();
            match case {
                "zero-thread" => entry["thread_id"] = json!(0),
                "negative-thread" => entry["thread_id"] = json!(-1),
                "zero-comment" => entry["comment_id"] = json!(0),
                "negative-comment" => entry["comment_id"] = json!(-1),
                "missing-pipeline" => ctx.definition_id = None,
                _ => {}
            }
            let error = crate::execute::execute_safe_output(&entry, &ctx)
                .await
                .unwrap_err();
            let expected = match case {
                "long-key" => "100 bytes",
                "missing-pipeline" => "pipeline identity",
                _ => "must be positive",
            };
            assert!(format!("{error:#}").contains(expected), "{case}: {error:#}");
            assert!(
                server.received_requests().await.unwrap().is_empty(),
                "{case}"
            );
        }
    }
}
