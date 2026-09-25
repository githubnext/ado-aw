//! Resolve inline comments against the exact ADO PR iteration, not a local file.
use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::pr_comments::{get_json, validate_body};
use super::pr_mutations::UpdatePrContext;
use crate::secure::{CommitSha, RelativeSafePath};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PrCommentSide {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrInlineComment {
    pub file_path: RelativeSafePath,
    #[serde(default)]
    pub side: PrCommentSide,
    pub line: u32,
    #[serde(default)]
    pub start_line: Option<u32>,
    pub content: String,
}

impl PrInlineComment {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.line > 0 && self.line <= i32::MAX as u32,
            "Inline line must be a positive ADO line number"
        );
        if let Some(start) = self.start_line {
            ensure!(
                start > 0 && start < self.line,
                "start_line must be positive and less than line"
            );
        }
        validate_body(&self.content)
    }
}

#[derive(Clone, Deserialize)]
struct Commit {
    #[serde(rename = "commitId")]
    id: CommitSha,
}

#[derive(Clone, Deserialize)]
struct Iteration {
    id: u32,
    #[serde(rename = "sourceRefCommit")]
    source: Commit,
    #[serde(rename = "commonRefCommit")]
    common: Commit,
}

#[derive(Deserialize)]
struct Iterations {
    value: Vec<Iteration>,
}

#[derive(Deserialize)]
struct Item {
    path: Option<String>,
}

#[derive(Deserialize)]
struct Change {
    #[serde(rename = "changeTrackingId")]
    tracking_id: u32,
    #[serde(rename = "changeType")]
    kind: String,
    item: Item,
    #[serde(rename = "originalPath")]
    original: Option<String>,
}

#[derive(Deserialize)]
struct Changes {
    #[serde(rename = "changeEntries")]
    changes: Vec<Change>,
    #[serde(default, rename = "nextSkip")]
    next_skip: u32,
}

async fn latest_iteration(
    ctx: &UpdatePrContext<'_>,
    head: &CommitSha,
) -> anyhow::Result<Iteration> {
    let iterations: Iterations = get_json(
        ctx,
        &format!(
            "{}/pullRequests/{}/iterations?api-version=7.1",
            ctx.repository_api_base(),
            ctx.pr_id
        ),
    )
    .await?;
    ensure!(
        !iterations.value.is_empty() && iterations.value.len() <= 2_000,
        "PR iteration discovery is empty or exceeds the bound"
    );
    let mut ids = std::collections::HashSet::new();
    for iteration in &iterations.value {
        ensure!(
            iteration.id > 0 && ids.insert(iteration.id),
            "PR iteration identities are invalid or ambiguous"
        );
    }
    let latest = iterations
        .value
        .into_iter()
        .max_by_key(|iteration| iteration.id)
        .context("PR iteration unavailable")?;
    ensure!(
        latest.source.id.eq_ignore_ascii_case(head),
        "PR head changed since review; expected_head_sha does not match the latest iteration"
    );
    Ok(latest)
}

pub(crate) async fn verify_head(ctx: &UpdatePrContext<'_>, head: &CommitSha) -> anyhow::Result<()> {
    latest_iteration(ctx, head).await.map(|_| ())
}

pub(crate) async fn prepare(
    ctx: &UpdatePrContext<'_>,
    head: &CommitSha,
    comments: &[PrInlineComment],
) -> anyhow::Result<Vec<Value>> {
    for comment in comments {
        comment.validate()?;
    }
    let iteration = latest_iteration(ctx, head).await?;
    let mut changes = Vec::new();
    let mut skip = 0;
    let mut pages = 0;
    loop {
        pages += 1;
        ensure!(
            pages <= 10,
            "PR change pagination exceeds the 10-page bound"
        );
        let page: Changes = get_json(
            ctx,
            &format!(
                "{}/pullRequests/{}/iterations/{}/changes?$top=200&$skip={skip}&api-version=7.1",
                ctx.repository_api_base(),
                ctx.pr_id,
                iteration.id
            ),
        )
        .await?;
        changes.extend(page.changes);
        ensure!(
            changes.len() <= 2_000,
            "PR change discovery exceeds the 2000-file bound"
        );
        if page.next_skip == 0 {
            break;
        }
        ensure!(
            page.next_skip > skip && page.next_skip <= 2_000,
            "PR change pagination is invalid or exceeds the bound"
        );
        skip = page.next_skip;
    }
    let mut prepared = Vec::new();
    for comment in comments {
        let requested = format!("/{}", comment.file_path.as_str().replace('\\', "/"));
        let matches = changes
            .iter()
            .filter(|change| {
                let path = if comment.side == PrCommentSide::Left {
                    change.original.as_deref().or(change.item.path.as_deref())
                } else {
                    change.item.path.as_deref().or(change.original.as_deref())
                };
                path == Some(requested.as_str())
            })
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1,
            "Inline path is absent or ambiguous in the selected PR diff: {requested}"
        );
        let change = matches[0];
        ensure!(change.tracking_id > 0, "Inline changeTrackingId is missing");
        let has_kind = |kind: &str| {
            change
                .kind
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(kind))
        };
        ensure!(
            !(comment.side == PrCommentSide::Left && has_kind("add")),
            "New files have no left-side content"
        );
        ensure!(
            !(comment.side == PrCommentSide::Right && has_kind("delete")),
            "Deleted files have no right-side content"
        );
        let canonical_path = change
            .item
            .path
            .as_deref()
            .or(change.original.as_deref())
            .context("Diff entry has no path")?;
        RelativeSafePath::parse(
            canonical_path
                .strip_prefix('/')
                .context("Invalid diff path")?,
        )?;
        let revision = if comment.side == PrCommentSide::Left {
            &iteration.common.id
        } else {
            &iteration.source.id
        };
        let mut url = reqwest::Url::parse(&format!("{}/items", ctx.repository_api_base()))?;
        url.query_pairs_mut()
            .append_pair("path", &requested)
            .append_pair("versionDescriptor.versionType", "commit")
            .append_pair("versionDescriptor.version", revision.as_str())
            .append_pair("includeContent", "true")
            .append_pair("$format", "json")
            .append_pair("api-version", "7.1");
        #[derive(Deserialize)]
        struct Content {
            content: String,
            #[serde(default, rename = "isBinary")]
            binary: bool,
        }
        let file: Content = get_json(ctx, url.as_str()).await?;
        ensure!(
            !file.binary && file.content.len() <= 4 * 1024 * 1024,
            "Inline content is binary or exceeds the inspection bound"
        );
        let end = usize::try_from(comment.line - 1)?;
        let text = file
            .content
            .lines()
            .nth(end)
            .context("Inline ending line is outside the selected revision")?;
        let offset =
            i32::try_from(text.encode_utf16().count() + 1).context("Inline line is too long")?;
        let mut thread = json!({"filePath":canonical_path});
        let (start_key, end_key) = match comment.side {
            PrCommentSide::Left => ("leftFileStart", "leftFileEnd"),
            PrCommentSide::Right => ("rightFileStart", "rightFileEnd"),
        };
        thread[start_key] = json!({"line":comment.start_line.unwrap_or(comment.line),"offset":1});
        thread[end_key] = json!({"line":comment.line,"offset":offset});
        prepared.push(json!({
            "threadContext":thread,
            "pullRequestThreadContext":{
                "changeTrackingId":change.tracking_id,
                "iterationContext":{"firstComparingIteration":iteration.id,"secondComparingIteration":iteration.id}
            }
        }));
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ExecutionContext;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    fn context(server: &MockServer) -> ExecutionContext {
        ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_organization: Some("org".into()),
            ado_project: Some("P".into()),
            repository_name: Some("repo".into()),
            access_token: Some("token".into()),
            ..Default::default()
        }
    }

    fn comment(file: &str, side: PrCommentSide) -> PrInlineComment {
        PrInlineComment {
            file_path: RelativeSafePath::parse(file).unwrap(),
            side,
            line: 2,
            start_line: Some(1),
            content: "Check this changed code.".into(),
        }
    }

    async fn iterations(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path(
                "/P/_apis/git/repositories/repo/pullRequests/42/iterations",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[{
                "id":2,"sourceRefCommit":{"commitId":"a".repeat(40)},
                "commonRefCommit":{"commitId":"b".repeat(40)}
            }]})))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn comment_context_uses_exact_revision_and_handles_renamed_left_side() {
        for side in [PrCommentSide::Left, PrCommentSide::Right] {
            let server = MockServer::start().await;
            iterations(&server).await;
            Mock::given(method("GET"))
                .and(path(
                    "/P/_apis/git/repositories/repo/pullRequests/42/iterations/2/changes",
                ))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"changeEntries":[{
                        "changeTrackingId":7,"changeType":"rename, edit",
                        "originalPath":"/old.rs","item":{"path":"/new.rs"}
                    }]})),
                )
                .mount(&server)
                .await;
            let file = if side == PrCommentSide::Left {
                "old.rs"
            } else {
                "new.rs"
            };
            let sha = if side == PrCommentSide::Left {
                "b".repeat(40)
            } else {
                "a".repeat(40)
            };
            Mock::given(method("GET"))
                .and(path("/P/_apis/git/repositories/repo/items"))
                .and(query_param("path", format!("/{file}")))
                .and(query_param("versionDescriptor.version", sha))
                .and(query_param("versionDescriptor.versionType", "commit"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"content":"first\n😀\n"})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let ctx = context(&server);
            let client = reqwest::Client::new();
            let op = UpdatePrContext {
                client: &client,
                target: super::super::resolve_repository_write_target(None, &ctx).unwrap(),
                pr_id: 42,
                token: "token",
                connection_type: None,
            };
            let result = prepare(
                &op,
                &CommitSha::parse("a".repeat(40)).unwrap(),
                &[comment(file, side)],
            )
            .await
            .unwrap();
            assert_eq!(result[0]["threadContext"]["filePath"], "/new.rs");
            let end = if side == PrCommentSide::Left {
                "leftFileEnd"
            } else {
                "rightFileEnd"
            };
            assert_eq!(
                result[0]["threadContext"][end],
                json!({"line":2,"offset":3})
            );
            assert_eq!(result[0]["pullRequestThreadContext"]["changeTrackingId"], 7);
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
    async fn stale_head_and_unsupported_diff_side_never_guess_a_local_anchor() {
        let server = MockServer::start().await;
        iterations(&server).await;
        Mock::given(method("GET"))
            .and(path(
                "/P/_apis/git/repositories/repo/pullRequests/42/iterations/2/changes",
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"changeEntries":[{
                    "changeTrackingId":7,"changeType":"delete","originalPath":"/deleted.rs","item":{"path":null}
                }]})),
            )
            .mount(&server)
            .await;
        let ctx = context(&server);
        let client = reqwest::Client::new();
        let op = UpdatePrContext {
            client: &client,
            target: super::super::resolve_repository_write_target(None, &ctx).unwrap(),
            pr_id: 42,
            token: "token",
            connection_type: None,
        };
        let error = prepare(
            &op,
            &CommitSha::parse("c".repeat(40)).unwrap(),
            &[comment("deleted.rs", PrCommentSide::Left)],
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("head changed"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let error = prepare(
            &op,
            &CommitSha::parse("a".repeat(40)).unwrap(),
            &[comment("deleted.rs", PrCommentSide::Right)],
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("no right-side"));
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| !request.url.path().ends_with("/items"))
        );
    }

    #[test]
    fn inline_comment_ranges_and_paths_fail_before_network() {
        let mut value = comment("src.rs", PrCommentSide::Right);
        value.line = 0;
        assert!(value.validate().is_err());
        value.line = 2;
        value.start_line = Some(2);
        assert!(value.validate().is_err());
        value.start_line = None;
        value.content = " ".into();
        assert!(value.validate().is_err());
        assert!(
            serde_json::from_value::<PrInlineComment>(json!({
                "file_path":"../escape","line":1,"content":"Never allowed"
            }))
            .is_err()
        );
    }
}
