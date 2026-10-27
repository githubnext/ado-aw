//! Shared, explicit completion preferences and non-replayed completion writes.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::authenticate_ado_request;
use super::pr_http::BoundedPrResponse;
use super::pr_mutations::UpdatePrContext;
use super::result::{ActionEffect, EffectState};

pub(crate) const VALID_MERGE_STRATEGIES: &[&str] =
    &["squash", "noFastForward", "rebase", "rebaseMerge"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompletionPreferences {
    #[serde(
        default,
        rename = "merge-strategy",
        skip_serializing_if = "Option::is_none"
    )]
    pub merge_strategy: Option<String>,
    #[serde(
        default,
        rename = "delete-source-branch",
        skip_serializing_if = "Option::is_none"
    )]
    pub delete_source_branch: Option<bool>,
}

impl CompletionPreferences {
    pub(crate) fn validate(&self) -> Result<()> {
        if let Some(strategy) = &self.merge_strategy {
            ensure!(
                VALID_MERGE_STRATEGIES.contains(&strategy.as_str()),
                "Invalid merge-strategy '{strategy}'"
            );
        }
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.merge_strategy.is_none() && self.delete_source_branch.is_none()
    }
}

pub(crate) async fn apply(
    ctx: &UpdatePrContext<'_>,
    preferences: &CompletionPreferences,
    actor: Option<&str>,
) -> ActionEffect {
    let operation = if actor.is_some() {
        "enable-auto-complete"
    } else {
        "set-completion-preferences"
    };
    let target = format!("{}#{}", ctx.target.qualified_repository(), ctx.pr_id);
    let effect = |state| ActionEffect::new(operation, &target, state);
    match prepare(ctx, preferences, actor).await {
        Err(error) => effect(EffectState::Failed).detail(format!("{error:#}")),
        Ok(None) => effect(EffectState::AlreadySatisfied),
        Ok(Some(body)) => {
            let url = format!(
                "{}/pullRequests/{}?api-version=7.1",
                ctx.repository_api_base(),
                ctx.pr_id
            );
            match authenticate_ado_request(ctx.client.patch(&url), ctx.token, ctx.connection_type)
                .json(&body).send().await
            {
                Err(error) => effect(EffectState::Uncertain)
                    .detail(format!("Completion write delivery is uncertain; no retry: {error}")),
                Ok(response) if response.status().is_success() => {
                    match read_pr(ctx).await {
                        Ok(actual) if matches_requested(&actual, preferences, actor) =>
                            effect(EffectState::Applied),
                        Ok(_) => effect(EffectState::Uncertain)
                            .detail("Completion write was accepted but requested state is unconfirmed; no retry"),
                        Err(error) => effect(EffectState::Uncertain)
                            .detail(format!("Completion write was accepted but readback failed; no retry: {error:#}")),
                    }
                }
                Ok(response) => {
                    let state = if response.status().is_server_error() {
                        EffectState::Uncertain
                    } else {
                        EffectState::Failed
                    };
                    effect(state).detail(format!("Completion write returned HTTP {}; no retry", response.status()))
                }
            }
        }
    }
}

async fn read_pr(ctx: &UpdatePrContext<'_>) -> Result<Value> {
    let url = format!(
        "{}/pullRequests/{}?api-version=7.1",
        ctx.repository_api_base(),
        ctx.pr_id
    );
    let response = authenticate_ado_request(ctx.client.get(url), ctx.token, ctx.connection_type)
        .send()
        .await
        .context("Failed to read PR completion state")?;
    ensure!(
        response.status().is_success(),
        "Failed to read PR completion state (HTTP {})",
        response.status()
    );
    let value: Value = response
        .bounded_json()
        .await
        .context("Malformed PR completion state")?;
    ensure!(value.is_object(), "PR completion state must be an object");
    Ok(value)
}

fn matches_requested(
    value: &Value,
    preferences: &CompletionPreferences,
    actor: Option<&str>,
) -> bool {
    let options = &value["completionOptions"];
    preferences
        .merge_strategy
        .as_ref()
        .is_none_or(|strategy| options["mergeStrategy"].as_str() == Some(strategy))
        && preferences
            .delete_source_branch
            .is_none_or(|delete| options["deleteSourceBranch"].as_bool() == Some(delete))
        && actor.is_none_or(|actor| {
            value["autoCompleteSetBy"]["id"]
                .as_str()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(actor))
        })
}

async fn prepare(
    ctx: &UpdatePrContext<'_>,
    preferences: &CompletionPreferences,
    actor: Option<&str>,
) -> Result<Option<Value>> {
    preferences.validate()?;
    if preferences.is_empty() && actor.is_none() {
        return Ok(None);
    }
    if let Some(actor) = actor {
        crate::secure::Guid::parse(actor).context("Invalid auto-complete actor identity")?;
    }
    let current = read_pr(ctx).await?;
    ensure!(
        current
            .get("completionOptions")
            .is_none_or(|value| value.is_null() || value.is_object()),
        "PR completionOptions must be an object"
    );
    if actor.is_some() {
        ensure!(
            current["isDraft"].as_bool() == Some(false),
            "Auto-complete requires an explicitly published, non-draft PR"
        );
        ensure!(
            current["status"].as_str() == Some("active"),
            "Auto-complete requires an active PR"
        );
        ensure!(
            current["completionOptions"]["bypassPolicy"].as_bool() != Some(true),
            "Auto-complete cannot enable a PR configured to bypass branch policies"
        );
    }
    if matches_requested(&current, preferences, actor) {
        return Ok(None);
    }
    let mut body = Map::new();
    if !preferences.is_empty() {
        let mut options = match current.get("completionOptions") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(options)) => options.clone(),
            Some(_) => anyhow::bail!("PR completionOptions must be an object"),
        };
        if let Some(strategy) = &preferences.merge_strategy {
            options.remove("squashMerge");
            options.insert("mergeStrategy".into(), json!(strategy));
        }
        if let Some(delete) = preferences.delete_source_branch {
            options.insert("deleteSourceBranch".into(), json!(delete));
        }
        body.insert("completionOptions".into(), Value::Object(options));
    }
    if let Some(actor) = actor {
        body.insert("autoCompleteSetBy".into(), json!({"id": actor}));
    }
    Ok(Some(Value::Object(body)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, method, path},
    };

    const ACTOR: &str = "01234567-89ab-cdef-0123-456789abcdef";
    const PATH: &str = "/P/_apis/git/repositories/repo/pullRequests/42";

    fn target(server: &MockServer) -> super::super::result::AdoRepositoryTarget {
        super::super::result::AdoRepositoryTarget {
            alias: "self".into(),
            organization: "org".into(),
            organization_url: server.uri(),
            project: "P".into(),
            repository: "repo".into(),
            repository_id: None,
            cross_organization: false,
        }
    }

    #[tokio::test]
    async fn omitted_preferences_are_not_forced_when_enabling_completion() {
        let server = MockServer::start().await;
        let applied = Arc::new(AtomicBool::new(false));
        let read = applied.clone();
        Mock::given(method("GET")).and(path(PATH)).respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(json!({
                "status":"active","isDraft":false,
                "completionOptions":{"mergeStrategy":"rebase","deleteSourceBranch":false},
                "autoCompleteSetBy":if read.load(Ordering::SeqCst) {json!({"id":ACTOR})} else {Value::Null}
            }))
        }).expect(2).mount(&server).await;
        Mock::given(method("PATCH"))
            .and(path(PATH))
            .and(body_json(json!({"autoCompleteSetBy":{"id":ACTOR}})))
            .respond_with(move |_: &wiremock::Request| {
                applied.store(true, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .expect(1)
            .mount(&server)
            .await;
        let client = super::super::pr_http::client().unwrap();
        let ctx = UpdatePrContext {
            client: &client,
            target: target(&server),
            pr_id: 42,
            token: "test",
            connection_type: None,
        };
        assert_eq!(
            apply(&ctx, &CompletionPreferences::default(), Some(ACTOR))
                .await
                .state,
            EffectState::Applied
        );
    }

    #[tokio::test]
    async fn initializing_one_preference_preserves_the_other_and_never_enables_completion() {
        let server = MockServer::start().await;
        let applied = Arc::new(AtomicBool::new(false));
        let read = applied.clone();
        Mock::given(method("GET")).and(path(PATH)).respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(json!({"isDraft":true,"status":"active",
                "completionOptions":{"mergeStrategy":"rebase","deleteSourceBranch":read.load(Ordering::SeqCst)}}))
        }).expect(2).mount(&server).await;
        Mock::given(method("PATCH"))
            .and(path(PATH))
            .and(body_json(
                json!({"completionOptions":{"mergeStrategy":"rebase","deleteSourceBranch":true}}),
            ))
            .respond_with(move |_: &wiremock::Request| {
                applied.store(true, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .expect(1)
            .mount(&server)
            .await;
        let client = super::super::pr_http::client().unwrap();
        let ctx = UpdatePrContext {
            client: &client,
            target: target(&server),
            pr_id: 42,
            token: "test",
            connection_type: None,
        };
        let preferences = CompletionPreferences {
            delete_source_branch: Some(true),
            ..Default::default()
        };
        assert_eq!(
            apply(&ctx, &preferences, None).await.state,
            EffectState::Applied
        );
    }

    #[tokio::test]
    async fn drafts_inactive_or_malformed_state_never_authorize_completion() {
        for current in [
            json!({"status":"active","isDraft":true}),
            json!({"status":"abandoned","isDraft":false}),
            json!({"status":"active"}),
            json!({"status":"active","isDraft":false,"completionOptions":[]}),
            json!({"status":"active","isDraft":false,"completionOptions":{"bypassPolicy":true}}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(current))
                .expect(1)
                .mount(&server)
                .await;
            let client = super::super::pr_http::client().unwrap();
            let ctx = UpdatePrContext {
                client: &client,
                target: target(&server),
                pr_id: 42,
                token: "test",
                connection_type: None,
            };
            assert_eq!(
                apply(&ctx, &CompletionPreferences::default(), Some(ACTOR))
                    .await
                    .state,
                EffectState::Failed
            );
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn unconfirmed_writes_fail_without_replay() {
        for (status, expected) in [
            (200, EffectState::Uncertain),
            (403, EffectState::Failed),
            (500, EffectState::Uncertain),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"status":"active","isDraft":false})),
                )
                .expect(if status == 200 { 2 } else { 1 })
                .mount(&server)
                .await;
            Mock::given(method("PATCH"))
                .respond_with(ResponseTemplate::new(status))
                .expect(1)
                .mount(&server)
                .await;
            let client = super::super::pr_http::client().unwrap();
            let ctx = UpdatePrContext {
                client: &client,
                target: target(&server),
                pr_id: 42,
                token: "test",
                connection_type: None,
            };
            let effect = apply(&ctx, &CompletionPreferences::default(), Some(ACTOR)).await;
            assert_eq!(effect.state, expected);
            let result =
                super::super::ExecutionResult::success("completion").with_effects(vec![effect]);
            assert!(!result.success && !result.is_warning());
        }
    }

    #[tokio::test]
    async fn absent_preferences_need_no_request_and_invalid_ones_fail_before_io() {
        let server = MockServer::start().await;
        let client = super::super::pr_http::client().unwrap();
        let ctx = UpdatePrContext {
            client: &client,
            target: target(&server),
            pr_id: 42,
            token: "test",
            connection_type: None,
        };
        assert_eq!(
            apply(&ctx, &CompletionPreferences::default(), None)
                .await
                .state,
            EffectState::AlreadySatisfied
        );
        let invalid = CompletionPreferences {
            merge_strategy: Some("merge-now".into()),
            ..Default::default()
        };
        assert_eq!(
            apply(&ctx, &invalid, Some(ACTOR)).await.state,
            EffectState::Failed
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
