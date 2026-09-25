//! Authoritative label identity reads and bounded, non-replayed removals.
use anyhow::{Context, ensure};
use serde::Deserialize;

use super::pr_mutations::UpdatePrContext;
use super::{ExecutionResult, authenticate_ado_request};
use crate::secure::Guid;

#[derive(Debug, Deserialize)]
pub(crate) struct PrLabel {
    pub id: Guid,
    pub name: String,
}

pub(crate) async fn read_labels(ctx: &UpdatePrContext<'_>) -> anyhow::Result<Vec<PrLabel>> {
    #[derive(Deserialize)]
    struct Labels {
        count: Option<usize>,
        value: Vec<PrLabel>,
    }
    let response = authenticate_ado_request(
        ctx.client.get(format!(
            "{}/pullRequests/{}/labels?api-version=7.1",
            ctx.repository_api_base(),
            ctx.pr_id
        )),
        ctx.token,
        ctx.connection_type,
    )
    .send()
    .await
    .context("Failed to read PR label identities")?;
    ensure!(
        response.status().is_success(),
        "Failed to read PR labels (HTTP {})",
        response.status()
    );
    if let Some(token) = response.headers().get("x-ms-continuationtoken") {
        ensure!(
            token.to_str()?.trim().is_empty(),
            "Incomplete PR label listing cannot authorize removals"
        );
    }
    let labels: Labels = response
        .json()
        .await
        .context("Malformed PR label identity list")?;
    ensure!(
        labels.count.is_none_or(|count| count == labels.value.len()),
        "Incomplete PR label identity list"
    );
    for (i, label) in labels.value.iter().enumerate() {
        ensure!(
            !label.name.trim().is_empty(),
            "PR label response contains an empty name"
        );
        ensure!(
            !labels.value[..i]
                .iter()
                .any(|prior| prior.name.eq_ignore_ascii_case(&label.name) || prior.id == label.id),
            "Ambiguous PR label identities"
        );
    }
    Ok(labels.value)
}

pub(crate) async fn remove_labels(
    ctx: &UpdatePrContext<'_>,
    names: &[String],
) -> anyhow::Result<ExecutionResult> {
    let labels = read_labels(ctx).await?;
    let mut removed = Vec::new();
    let mut absent = Vec::new();
    let mut data = serde_json::json!({"pull_request_id":ctx.pr_id,"repository":ctx.target.qualified_repository()});
    for name in names {
        let Some(label) = labels
            .iter()
            .find(|label| label.name.eq_ignore_ascii_case(name))
        else {
            absent.push(name);
            continue;
        };
        let response = authenticate_ado_request(
            ctx.client.delete(format!(
                "{}/pullRequests/{}/labels/{}?api-version=7.1",
                ctx.repository_api_base(),
                ctx.pr_id,
                label.id
            )),
            ctx.token,
            ctx.connection_type,
        )
        .send()
        .await;
        let (status, message) = match response {
            Ok(response) if response.status().is_success() => {
                removed.push(name);
                continue;
            }
            Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
                absent.push(name);
                continue;
            }
            Ok(response) => (
                "failed",
                format!(
                    "Failed to remove label '{name}' (HTTP {})",
                    response.status()
                ),
            ),
            Err(error) => (
                "uncertain",
                format!("Label removal for '{name}' has uncertain delivery: {error}"),
            ),
        };
        data["removed"] = serde_json::json!(removed);
        data["already_absent"] = serde_json::json!(absent);
        data["failed_label"] = serde_json::json!(name);
        data["removal_status"] = serde_json::json!(status);
        return Ok(ExecutionResult::failure_with_data(message, data));
    }
    data["removed"] = serde_json::json!(removed);
    data["already_absent"] = serde_json::json!(absent);
    Ok(ExecutionResult::success_with_data(
        "PR label removal completed",
        data,
    ))
}

#[cfg(test)]
mod tests {
    use crate::execute::execute_safe_output;
    use crate::safe_outputs::ExecutionContext;
    use serde_json::{Value, json};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn label(name: &str, id: &str) -> Value {
        json!({"id":id,"name":name})
    }
    const FROM: &str = "11111111-1111-1111-1111-111111111111";
    const TO: &str = "22222222-2222-2222-2222-222222222222";

    #[tokio::test]
    async fn new_label_tools_share_the_original_proposal_budget() {
        let directory = tempfile::tempdir().unwrap();
        let remove =
            json!({"name":"remove-pull-request-labels","pull_request_id":42,"labels":["old"]});
        let replace = json!({"name":"replace-pull-request-label","pull_request_id":42,"from":"old","to":"new"});
        std::fs::write(
            directory.path().join(crate::ndjson::SAFE_OUTPUT_FILENAME),
            format!("{remove}\n{replace}\n"),
        )
        .unwrap();
        let mut ctx = ExecutionContext {
            dry_run: true,
            ..Default::default()
        };
        for tool in ["remove-pull-request-labels", "replace-pull-request-label"] {
            ctx.tool_configs
                .insert(tool.into(), json!({"max":2,"target":"*"}));
        }
        ctx.budget_groups.insert(
            "labels".into(),
            crate::compile::pr_migration::BudgetGroup {
                max: 1,
                tools: vec![
                    "remove-pull-request-labels".into(),
                    "replace-pull-request-label".into(),
                ],
            },
        );
        let results = crate::execute::execute_safe_outputs(
            directory.path(),
            &ctx,
            &crate::execute::ToolFilter::default(),
        )
        .await
        .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert!(results[1].is_budget_exhausted());
    }

    fn context(server: &MockServer, tool: &str, config: Value) -> ExecutionContext {
        let mut ctx = ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_organization: Some("org".into()),
            ado_project: Some("P".into()),
            repository_name: Some("repo".into()),
            access_token: Some("token".into()),
            ..Default::default()
        };
        let mut config = config;
        config["target"] = json!("*");
        ctx.tool_configs.insert(tool.into(), config);
        ctx
    }

    #[tokio::test]
    async fn removal_resolves_ids_and_preserves_unrelated_labels() {
        let server = MockServer::start().await;
        let route = "/P/_apis/git/repositories/repo/pullRequests/42/labels";
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"value":[label("Old",FROM),label("unrelated",TO)]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("{route}/{FROM}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            "remove-pull-request-labels",
            json!({"allowed-labels":["old","absent"]}),
        );
        let (_, result) = execute_safe_output(
            &json!({"name":"remove-pull-request-labels","pull_request_id":42,
                        "labels":["old","OLD","absent"]}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(result.success);
        let data = result.data.unwrap();
        assert_eq!(data["removed"], json!(["old"]));
        assert_eq!(data["already_absent"], json!(["absent"]));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn invalid_identity_lists_never_authorize_removal() {
        for body in [
            json!({}),
            json!({"value":[{"name":"old"}]}),
            json!({"value":[label("old",FROM),label("OLD",TO)]}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            let ctx = context(&server, "remove-pull-request-labels", json!({}));
            assert!(execute_safe_output(&json!({"name":"remove-pull-request-labels","pull_request_id":42,"labels":["old"]}),
                            &ctx).await.is_err());
            assert!(
                server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .all(|request| request.method.as_str() == "GET")
            );
        }
    }

    #[tokio::test]
    async fn replacement_adds_verifies_then_removes_with_honest_partial_results() {
        use std::sync::{Arc, Mutex};
        for delete_status in [204, 403] {
            let server = MockServer::start().await;
            let route = "/P/_apis/git/repositories/repo/pullRequests/42/labels";
            let state = Arc::new(Mutex::new(vec![label("old", FROM)]));
            let reads = state.clone();
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(move |_: &wiremock::Request| {
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"value":reads.lock().unwrap().clone()}))
                })
                .mount(&server)
                .await;
            let adds = state.clone();
            Mock::given(method("POST"))
                .and(path(route))
                .respond_with(move |_: &wiremock::Request| {
                    adds.lock().unwrap().push(label("new", TO));
                    ResponseTemplate::new(200).set_body_json(label("new", TO))
                })
                .expect(1)
                .mount(&server)
                .await;
            let deletes = state.clone();
            Mock::given(method("DELETE"))
                .and(path(format!("{route}/{FROM}")))
                .respond_with(move |_: &wiremock::Request| {
                    if delete_status == 204 {
                        deletes
                            .lock()
                            .unwrap()
                            .retain(|label| label["name"] != "old");
                    }
                    ResponseTemplate::new(delete_status)
                })
                .expect(1)
                .mount(&server)
                .await;
            let ctx = context(
                &server,
                "replace-pull-request-label",
                json!({"allowed-transitions":[{"from":"old","to":"new"}]}),
            );
            let (_, result) = execute_safe_output(
                &json!({"name":"replace-pull-request-label","pull_request_id":42,
                            "from":"old","to":"new"}),
                &ctx,
            )
            .await
            .unwrap();
            assert_eq!(result.success, delete_status == 204);
            assert_eq!(result.data.as_ref().unwrap()["addition_status"], "applied");
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests[0].method.as_str(), "GET");
            assert_eq!(requests[1].method.as_str(), "POST");
            assert_eq!(requests[2].method.as_str(), "GET");
            assert_eq!(requests[3].method.as_str(), "GET");
            assert_eq!(requests[4].method.as_str(), "DELETE");
            if delete_status == 403 {
                assert_eq!(result.data.unwrap()["removal"]["removal_status"], "failed");
                assert_eq!(state.lock().unwrap().len(), 2);
            }
        }
    }

    #[tokio::test]
    async fn unverified_addition_never_removes_source_and_transition_policy_precedes_reads() {
        for config in [
            json!({}),
            json!({"allowed-transitions":[{"from":"other","to":"new"}]}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"value":[label("old",FROM)]})),
                )
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
            let denied = config.get("allowed-transitions").is_some();
            let ctx = context(&server, "replace-pull-request-label", config);
            let result = execute_safe_output(
                &json!({"name":"replace-pull-request-label","pull_request_id":42,
                            "from":"old","to":"new"}),
                &ctx,
            )
            .await;
            if denied {
                assert!(result.is_err());
                assert!(server.received_requests().await.unwrap().is_empty());
            } else {
                let result = result.unwrap().1;
                assert!(!result.success);
                assert!(result.message.contains("not visible"));
                assert!(
                    server
                        .received_requests()
                        .await
                        .unwrap()
                        .iter()
                        .all(|request| request.method.as_str() != "DELETE")
                );
            }
        }
    }
}
