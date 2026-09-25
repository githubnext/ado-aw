//! Owned comment metadata and non-destructive lifecycle operations.
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::pr_common::collection_identity;
use super::pr_mutations::UpdatePrContext;
use super::{ExecutionContext, ExecutionResult, authenticate_ado_request};
use crate::secure::{Guid, Identifier};

const OWNER: &str = "ado-aw.owner";
const BODY_HASH: &str = "ado-aw.content-sha256";
const RUN: &str = "ado-aw.run-id";
const CONTENT_PROOF: &str = "\n\n<!-- ado-aw-content-sha256:";
pub(crate) const MAX_COMMENT_BYTES: usize = 65_536;

pub(crate) fn client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create PR comment client")
}

pub(crate) fn default_comment_key() -> Identifier {
    Identifier::parse("default").expect("constant comment key is valid")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Owner {
    schema: u8,
    collection: String,
    project: Guid,
    definition: u64,
    purpose: String,
    key: Identifier,
}

pub(crate) fn owner(
    ctx: &ExecutionContext,
    purpose: &str,
    key: &Identifier,
) -> anyhow::Result<Option<Owner>> {
    let (Some(collection), Some(project), Some(definition)) = (
        ctx.pipeline_collection_uri.as_deref(),
        ctx.ado_project_id.as_deref(),
        ctx.definition_id,
    ) else {
        log::debug!(
            "Complete pipeline identity is unavailable; comment ownership metadata is not asserted"
        );
        return Ok(None);
    };
    ensure!(
        definition > 0,
        "Pipeline definition ID must be positive for comment ownership"
    );
    Ok(Some(Owner {
        schema: 1,
        collection: collection_identity(collection)
            .context("Invalid pipeline collection identity")?,
        project: Guid::parse(project.to_ascii_lowercase())?,
        definition,
        purpose: purpose.into(),
        key: key.clone(),
    }))
}

fn string_property(value: impl Into<String>) -> Value {
    json!({"$type":"System.String","$value":value.into()})
}
fn property<'a>(properties: &'a Value, key: &str) -> Option<&'a str> {
    let value = properties.get(key)?;
    if value.get("$type")?.as_str()? != "System.String" {
        return None;
    }
    value.get("$value")?.as_str()
}

pub(crate) fn stamp(
    body: &mut Value,
    owner: Option<&Owner>,
    ctx: &ExecutionContext,
    content: &str,
) -> anyhow::Result<()> {
    if let Some(owner) = owner {
        let mut properties = json!({
            OWNER:string_property(serde_json::to_string(owner)?),
            BODY_HASH:string_property(crate::hash::sha256_hex(content.as_bytes())),
        });
        if let Some(run) = ctx.build_id {
            properties[RUN] = string_property(run.to_string());
        }
        body["properties"] = properties;
    }
    Ok(())
}

pub(crate) fn validate_body(body: &str) -> anyhow::Result<()> {
    ensure!(!body.trim().is_empty(), "Comment content must not be empty");
    ensure!(
        body.len() <= MAX_COMMENT_BYTES,
        "Comment content exceeds 65536 bytes"
    );
    Ok(())
}

pub(crate) async fn get_json<T: serde::de::DeserializeOwned>(
    ctx: &UpdatePrContext<'_>,
    url: &str,
) -> anyhow::Result<T> {
    let mut response =
        authenticate_ado_request(ctx.client.get(url), ctx.token, ctx.connection_type)
            .send()
            .await
            .context("PR comment metadata read failed")?;
    ensure!(
        response.status().is_success(),
        "PR comment metadata read failed (HTTP {})",
        response.status()
    );
    if let Some(token) = response.headers().get("x-ms-continuationtoken") {
        ensure!(
            token.to_str()?.trim().is_empty(),
            "Incomplete comment metadata cannot authorize mutation"
        );
    }
    const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_RESPONSE_BYTES as u64),
        "PR comment metadata exceeds the 8 MB response bound"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("PR comment metadata stream failed")?
    {
        ensure!(
            chunk.len() <= MAX_RESPONSE_BYTES.saturating_sub(bytes.len()),
            "PR comment metadata exceeds the 8 MB response bound"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("Malformed PR comment metadata")
}

pub(crate) async fn actor(ctx: &UpdatePrContext<'_>) -> anyhow::Result<String> {
    #[derive(Deserialize)]
    struct User {
        id: String,
    }
    #[derive(Deserialize)]
    struct Connection {
        #[serde(rename = "authenticatedUser")]
        user: User,
    }
    let connection: Connection = get_json(
        ctx,
        &format!(
            "{}/_apis/connectiondata",
            ctx.target.organization_url.trim_end_matches('/')
        ),
    )
    .await?;
    Guid::parse(&connection.user.id).context("Invalid authenticated comment actor")?;
    ensure!(
        connection.user.id != "00000000-0000-0000-0000-000000000000",
        "Anonymous identity cannot authorize owned-comment changes"
    );
    Ok(connection.user.id)
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Actor {
    id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(crate) struct Comment {
    pub id: i32,
    #[serde(default, rename = "parentCommentId")]
    parent: i32,
    pub content: Option<String>,
    author: Option<Actor>,
    #[serde(default, rename = "isDeleted")]
    deleted: bool,
    #[serde(rename = "lastContentUpdatedDate")]
    updated: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(crate) struct Thread {
    pub id: i32,
    pub comments: Vec<Comment>,
    #[serde(default)]
    properties: Value,
    status: Option<Value>,
    #[serde(rename = "lastUpdatedDate")]
    updated: Option<String>,
}

fn owned_root<'a>(thread: &'a Thread, owner: &Owner, actor: &str) -> anyhow::Result<&'a Comment> {
    let stored: Owner = serde_json::from_str(
        property(&thread.properties, OWNER).context("Thread has no verified workflow ownership")?,
    )
    .context("Malformed thread ownership metadata")?;
    ensure!(
        &stored == owner,
        "Thread belongs to a different workflow or report"
    );
    ensure!(
        thread.id > 0 && !thread.comments.is_empty(),
        "Thread metadata is incomplete"
    );
    ensure!(
        thread.comments.len() == 1,
        "Conversations with replies are protected; shared actor identity does not prove reply ownership"
    );
    ensure!(
        thread.comments.iter().all(|comment| comment
            .author
            .as_ref()
            .is_some_and(|author| author.id.eq_ignore_ascii_case(actor))),
        "Conversation contains human or unverified authors; it must remain untouched"
    );
    let mut roots = thread
        .comments
        .iter()
        .filter(|comment| comment.parent == 0 && !comment.deleted);
    let root = roots
        .next()
        .context("Owned root comment is missing or deleted")?;
    ensure!(root.id > 0, "Owned root comment has an invalid ID");
    ensure!(roots.next().is_none(), "Thread has ambiguous root comments");
    visible_content(thread, root)?;
    Ok(root)
}

fn visible_content<'a>(thread: &Thread, root: &'a Comment) -> anyhow::Result<&'a str> {
    let content = root
        .content
        .as_deref()
        .context("Owned root content is unavailable")?;
    let initial = property(&thread.properties, BODY_HASH)
        .context("Owned comment has no initial content hash")?;
    if initial == crate::hash::sha256_hex(content.as_bytes()) {
        return Ok(content);
    }
    let proof = content.rsplit_once(CONTENT_PROOF).and_then(|(body, tail)| {
        let hash = tail.strip_suffix(" -->")?;
        (hash.len() == 64
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            && hash == crate::hash::sha256_hex(body.as_bytes()))
        .then_some(body)
    });
    proof.context(
        "Owned comment no longer matches its confirmed content hash; refusing to overwrite it",
    )
}

fn updated_content(content: &str) -> anyhow::Result<String> {
    validate_body(content)?;
    let content = format!(
        "{content}{CONTENT_PROOF}{} -->",
        crate::hash::sha256_hex(content.as_bytes())
    );
    validate_body(&content)?;
    Ok(content)
}

pub(crate) fn thread_url(ctx: &UpdatePrContext<'_>, id: i32) -> String {
    format!(
        "{}/pullRequests/{}/threads/{id}?api-version=7.1",
        ctx.repository_api_base(),
        ctx.pr_id
    )
}

pub(crate) async fn read_thread(ctx: &UpdatePrContext<'_>, id: i32) -> anyhow::Result<Thread> {
    ensure!(id > 0, "thread_id must be positive");
    let thread: Thread = get_json(ctx, &thread_url(ctx, id)).await?;
    ensure!(
        thread.id == id,
        "Comment metadata returned a different thread"
    );
    ensure!(
        thread.comments.len() <= 2_000,
        "Comment conversation exceeds the discovery bound"
    );
    Ok(thread)
}

pub(crate) fn owner_for_thread(
    ctx: &ExecutionContext,
    key: &Identifier,
    thread: &Thread,
) -> anyhow::Result<Owner> {
    let stored: Owner = serde_json::from_str(
        property(&thread.properties, OWNER).context("Thread has no workflow ownership metadata")?,
    )?;
    ensure!(
        matches!(stored.purpose.as_str(), "comment" | "review"),
        "Unsupported owned-comment purpose"
    );
    let expected = owner(ctx, &stored.purpose, key)?
        .context("Owned updates require a complete trusted pipeline identity")?;
    ensure!(
        stored == expected,
        "Thread belongs to a different workflow or report"
    );
    Ok(expected)
}

pub(crate) async fn older_threads(
    ctx: &UpdatePrContext<'_>,
    owner: &Owner,
    actor: &str,
    run: u64,
    max: usize,
) -> anyhow::Result<(Vec<Thread>, Vec<Value>)> {
    ensure!(
        run > 0,
        "A positive build ID is required for comment supersession"
    );
    ensure!(
        max > 0 && max <= 100,
        "max-superseded-comments must be between 1 and 100"
    );
    #[derive(Deserialize)]
    struct Threads {
        value: Vec<Thread>,
    }
    let threads: Threads = get_json(
        ctx,
        &format!(
            "{}/pullRequests/{}/threads?api-version=7.1",
            ctx.repository_api_base(),
            ctx.pr_id
        ),
    )
    .await?;
    ensure!(
        threads.value.len() <= 2_000,
        "PR thread discovery exceeds the 2000-thread bound"
    );
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();
    for thread in threads.value {
        if property(&thread.properties, OWNER).is_none() {
            continue;
        }
        let stored = match serde_json::from_str::<Owner>(
            property(&thread.properties, OWNER).expect("property checked"),
        ) {
            Ok(stored) => stored,
            Err(error) => {
                skipped.push(json!({"thread_id":thread.id,"reason":format!("invalid ownership metadata: {error}")}));
                continue;
            }
        };
        if &stored != owner {
            continue;
        }
        if thread.status != Some(json!(1)) && thread.status != Some(json!("active")) {
            skipped.push(json!({"thread_id":thread.id,"reason":"thread is already resolved or its status is unknown"}));
            continue;
        }
        let older = property(&thread.properties, RUN)
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|value| value > 0 && value < run);
        if !older {
            skipped.push(json!({"thread_id":thread.id,"reason":"not a proven older run"}));
            continue;
        }
        if let Err(error) = owned_root(&thread, owner, actor) {
            skipped.push(json!({"thread_id":thread.id,"reason":format!("{error:#}")}));
            continue;
        }
        candidates.push(thread);
    }
    ensure!(
        candidates.len() <= max,
        "Eligible comments exceed max-superseded-comments; nothing was superseded"
    );
    Ok((candidates, skipped))
}

async fn patch(ctx: &UpdatePrContext<'_>, url: &str, body: &Value) -> anyhow::Result<()> {
    let response = authenticate_ado_request(ctx.client.patch(url), ctx.token, ctx.connection_type)
        .json(body)
        .send()
        .await
        .context("Comment mutation delivery is uncertain")?;
    ensure!(
        response.status().is_success(),
        "Comment mutation failed (HTTP {})",
        response.status()
    );
    Ok(())
}

pub(crate) async fn update_owned(
    ctx: &UpdatePrContext<'_>,
    owner: &Owner,
    actor: &str,
    thread: &Thread,
    comment_id: i32,
    content: &str,
    superseded_by: Option<i32>,
) -> anyhow::Result<ExecutionResult> {
    validate_body(content)?;
    let root = owned_root(thread, owner, actor)?;
    ensure!(
        comment_id == root.id,
        "Only the verified owned root comment may be updated"
    );
    let fresh = read_thread(ctx, thread.id).await?;
    ensure!(
        &fresh == thread,
        "Comment conversation changed during preflight; no update attempted"
    );
    owned_root(&fresh, owner, actor)?;
    let encoded_content = updated_content(content)?;
    let mut data = json!({"pull_request_id":ctx.pr_id,"thread_id":thread.id,"comment_id":comment_id,
        "content_status":"not-attempted","metadata_status":"not-attempted"});
    let comment_url = format!(
        "{}/pullRequests/{}/threads/{}/comments/{comment_id}?api-version=7.1",
        ctx.repository_api_base(),
        ctx.pr_id,
        thread.id
    );
    if let Err(error) = patch(ctx, &comment_url, &json!({"content":encoded_content})).await {
        data["content_status"] = json!("uncertain");
        return Ok(ExecutionResult::failure_with_data(
            format!("{error:#}"),
            data,
        ));
    }
    data["content_status"] = json!("applied");
    let after = match read_thread(ctx, thread.id).await {
        Ok(after) => after,
        Err(error) => {
            return Ok(ExecutionResult::failure_with_data(
                format!("Comment changed but verification failed: {error:#}"),
                data,
            ));
        }
    };
    let expected_comments = thread
        .comments
        .iter()
        .map(|comment| {
            let mut copy = comment.clone();
            if copy.id == comment_id {
                copy.content = Some(encoded_content.clone());
            }
            copy.updated = None;
            copy
        })
        .collect::<Vec<_>>();
    let actual_comments = after
        .comments
        .iter()
        .map(|comment| {
            let mut copy = comment.clone();
            copy.updated = None;
            copy
        })
        .collect::<Vec<_>>();
    if after.properties != thread.properties
        || after.status != thread.status
        || actual_comments != expected_comments
    {
        return Ok(ExecutionResult::failure_with_data(
            "Conversation changed concurrently; ownership refresh and thread closure were not attempted",
            data,
        ));
    }
    // ADO thread properties are immutable after creation. Content and its
    // optimistic-concurrency hash move together; immutable ownership is still required.
    if superseded_by.is_some() {
        if let Err(error) = patch(ctx, &thread_url(ctx, thread.id), &json!({"status":4})).await {
            data["thread_status"] = json!("uncertain");
            return Ok(ExecutionResult::failure_with_data(
                format!("Comment content changed but thread closure failed: {error:#}"),
                data,
            ));
        }
        data["thread_status"] = json!("closed");
    }
    let final_state = match read_thread(ctx, thread.id).await {
        Ok(state) => state,
        Err(error) => {
            return Ok(ExecutionResult::failure_with_data(
                format!("Comment writes completed but final verification failed: {error:#}"),
                data,
            ));
        }
    };
    let verified = owned_root(&final_state, owner, actor);
    if !verified
        .is_ok_and(|root| visible_content(&final_state, root).is_ok_and(|actual| actual == content))
        || (superseded_by.is_some()
            && !matches!(final_state.status.as_ref(),Some(Value::String(status)) if status=="closed")
            && final_state.status != Some(json!(4)))
    {
        return Ok(ExecutionResult::failure_with_data(
            "Comment writes could not be confirmed; concurrent changes require attention",
            data,
        ));
    }
    data["metadata_status"] = json!("confirmed");
    Ok(ExecutionResult::success_with_data(
        "Owned comment update confirmed",
        data,
    ))
}

pub(crate) async fn supersede(
    ctx: &UpdatePrContext<'_>,
    owner: &Owner,
    actor: &str,
    candidates: &[Thread],
    replacement: i32,
    mut skipped: Vec<Value>,
) -> anyhow::Result<Value> {
    let mut completed = Vec::new();
    let mut failures = 0;
    for thread in candidates {
        let root = owned_root(thread, owner, actor)?;
        let old = visible_content(thread, root)?;
        let content = format!(
            "{old}\n\n_Superseded by the newer automated report in thread #{replacement}._"
        );
        match update_owned(
            ctx,
            owner,
            actor,
            thread,
            root.id,
            &content,
            Some(replacement),
        )
        .await
        {
            Ok(result) if result.success => completed.push(thread.id),
            Ok(result) => {
                failures += 1;
                skipped.push(
                    json!({"thread_id":thread.id,"reason":result.message,"partial":result.data}),
                );
            }
            Err(error) => {
                failures += 1;
                skipped.push(json!({"thread_id":thread.id,"reason":format!("{error:#}")}));
            }
        }
    }
    Ok(json!({"superseded":completed,"not_superseded":skipped,"failures":failures}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    const ACTOR: &str = "33333333-3333-3333-3333-333333333333";
    const ORIGINAL: &str = "Original owned report with `code`.";

    fn context(server: &MockServer) -> ExecutionContext {
        ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_organization: Some("org".into()),
            ado_project: Some("P".into()),
            ado_project_id: Some("11111111-1111-1111-1111-111111111111".into()),
            pipeline_collection_uri: Some("https://dev.azure.com/source".into()),
            definition_id: Some(7),
            build_id: Some(100),
            repository_name: Some("repo".into()),
            access_token: Some("token".into()),
            ..Default::default()
        }
    }

    fn fixture(ctx: &ExecutionContext, id: i32, run: u64) -> Value {
        let owner = owner(ctx, "comment", &default_comment_key())
            .unwrap()
            .unwrap();
        let mut thread = json!({"id":id,"status":"active","comments":[
            {"id":1,"parentCommentId":0,"content":ORIGINAL,"author":{"id":ACTOR}}
        ]});
        stamp(&mut thread, Some(&owner), ctx, ORIGINAL).unwrap();
        thread["properties"][RUN] = string_property(run.to_string());
        thread
    }

    #[tokio::test]
    async fn markers_without_actor_namespace_and_content_proof_cannot_authorize_updates() {
        let server = MockServer::start().await;
        let ctx = context(&server);
        let expected = owner(&ctx, "comment", &default_comment_key())
            .unwrap()
            .unwrap();
        let base = fixture(&ctx, 3, 99);
        let good: Thread = serde_json::from_value(base.clone()).unwrap();
        assert!(owned_root(&good, &expected, ACTOR).is_ok());
        for failure in [
            "actor",
            "namespace",
            "hash",
            "unmarked",
            "same-actor-reply",
            "human-reply",
        ] {
            let mut value = base.clone();
            match failure {
                "actor"=>value["comments"][0]["author"]["id"]=json!("44444444-4444-4444-4444-444444444444"),
                "namespace"=>{
                    let mut other=expected.clone();other.definition+=1;
                    value["properties"][OWNER]=string_property(serde_json::to_string(&other).unwrap());
                },
                "hash"=>value["comments"][0]["content"]=json!("Human edited the old bot comment."),
                "unmarked"=>value["properties"]=json!({}),
                kind=>value["comments"].as_array_mut().unwrap().push(json!({
                    "id":2,"parentCommentId":1,"content":"A conversation reply.",
                    "author":{"id":if kind=="same-actor-reply" {ACTOR}else{"44444444-4444-4444-4444-444444444444"}}
                })),
            }
            let thread: Thread = serde_json::from_value(value).unwrap();
            assert!(owned_root(&thread, &expected, ACTOR).is_err(), "{failure}");
        }
        let mut edited=base.clone();
        edited["comments"][0]["content"]=json!(updated_content("A newer owned report.").unwrap());
        let verified:Thread=serde_json::from_value(edited.clone()).unwrap();
        let root=owned_root(&verified,&expected,ACTOR).unwrap();
        assert_eq!(visible_content(&verified,root).unwrap(),"A newer owned report.");
        edited["comments"][0]["content"]=json!(edited["comments"][0]["content"].as_str().unwrap().replace("newer","human-edited"));
        assert!(owned_root(&serde_json::from_value(edited).unwrap(),&expected,ACTOR).is_err());
        let mut unowned=base;
        unowned["properties"]=json!({});
        unowned["comments"][0]["content"]=json!(updated_content("A forged but correctly hashed footer.").unwrap());
        assert!(owned_root(&serde_json::from_value(unowned).unwrap(),&expected,ACTOR).is_err());
    }

    #[tokio::test]
    async fn discovery_protects_current_runs_and_replies_and_enforces_complete_bounds() {
        let server = MockServer::start().await;
        let ctx = context(&server);
        let expected = owner(&ctx, "comment", &default_comment_key())
            .unwrap()
            .unwrap();
        let mut replied = fixture(&ctx, 5, 99);
        replied["comments"].as_array_mut().unwrap().push(json!({
            "id":2,"content":"A reply from the same account is not provenance.","author":{"id":ACTOR}
        }));
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value":[fixture(&ctx,3,99),fixture(&ctx,4,100),replied]
            })))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        let op = UpdatePrContext {
            client: &client,
            target: super::super::resolve_repository_write_target(None, &ctx).unwrap(),
            pr_id: 42,
            token: "token",
            connection_type: None,
        };
        let (candidates, skipped) = older_threads(&op, &expected, ACTOR, 100, 1).await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, 3);
        assert_eq!(skipped.len(), 2);
        assert!(older_threads(&op, &expected, ACTOR, 100, 0).await.is_err());
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.method.as_str() == "GET")
        );
    }

    #[tokio::test]
    async fn updates_preserve_ownership_and_supersession_preserves_original_text() {
        for superseded_by in [None, Some(9)] {
            let server = MockServer::start().await;
            let ctx = context(&server);
            let expected = owner(&ctx, "comment", &default_comment_key())
                .unwrap()
                .unwrap();
            let initial = fixture(&ctx, 3, 99);
            let state = Arc::new(Mutex::new(initial.clone()));
            let reads = state.clone();
            let route = "/P/_apis/git/repositories/repo/pullRequests/42/threads/3";
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(move |_: &wiremock::Request| {
                    ResponseTemplate::new(200).set_body_json(reads.lock().unwrap().clone())
                })
                .mount(&server)
                .await;
            let comments = state.clone();
            Mock::given(method("PATCH"))
                .and(path(format!("{route}/comments/1")))
                .respond_with(move |request: &wiremock::Request| {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    comments.lock().unwrap()["comments"][0]["content"] = body["content"].clone();
                    ResponseTemplate::new(200).set_body_json(json!({"id":1}))
                })
                .expect(1)
                .mount(&server)
                .await;
            let metadata = state.clone();
            Mock::given(method("PATCH"))
                .and(path(route))
                .respond_with(move |request: &wiremock::Request| {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    let mut value = metadata.lock().unwrap();
                    assert!(
                        body.get("properties").is_none(),
                        "ADO thread properties are immutable"
                    );
                    if let Some(status) = body.get("status") {
                        value["status"] = status.clone();
                    }
                    ResponseTemplate::new(200).set_body_json(value.clone())
                })
                .expect(u64::from(superseded_by.is_some()))
                .mount(&server)
                .await;
            let client = reqwest::Client::new();
            let op = UpdatePrContext {
                client: &client,
                target: super::super::resolve_repository_write_target(None, &ctx).unwrap(),
                pr_id: 42,
                token: "token",
                connection_type: None,
            };
            let thread: Thread = serde_json::from_value(initial.clone()).unwrap();
            if superseded_by.is_some() {
                let outcome = supersede(&op, &expected, ACTOR, &[thread], 9, vec![])
                    .await
                    .unwrap();
                assert_eq!(outcome["superseded"], json!([3]));
                assert_eq!(outcome["failures"], 0);
                let value = state.lock().unwrap();
                assert!(
                    value["comments"][0]["content"]
                        .as_str()
                        .unwrap()
                        .starts_with(ORIGINAL)
                );
                assert_eq!(value["status"], 4);
            } else {
                let result = update_owned(
                    &op,
                    &expected,
                    ACTOR,
                    &thread,
                    1,
                    "Replacement owned content.",
                    None,
                )
                .await
                .unwrap();
                assert!(result.success, "{}", result.message);
                assert_eq!(state.lock().unwrap()["status"], "active");
            }
            let value = state.lock().unwrap();
            assert_eq!(value["properties"][OWNER], initial["properties"][OWNER]);
        }
    }

    #[tokio::test]
    async fn concurrent_edits_stop_before_the_first_write() {
        let server = MockServer::start().await;
        let ctx = context(&server);
        let expected = owner(&ctx, "comment", &default_comment_key())
            .unwrap()
            .unwrap();
        let before = fixture(&ctx, 3, 99);
        let mut changed = before.clone();
        changed["comments"][0]["content"] = json!("Concurrent human edit.");
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(changed))
            .expect(1)
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        let op = UpdatePrContext {
            client: &client,
            target: super::super::resolve_repository_write_target(None, &ctx).unwrap(),
            pr_id: 42,
            token: "token",
            connection_type: None,
        };
        let result = update_owned(
            &op,
            &expected,
            ACTOR,
            &serde_json::from_value(before).unwrap(),
            1,
            "New owned content.",
            None,
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("changed during preflight")
        );
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
