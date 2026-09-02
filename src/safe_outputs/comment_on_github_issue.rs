//! `comment-on-github-issue` safe output.

use anyhow::ensure;
use log::{debug, info};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::hide_github_issue_comment::{
    canonical_github_comment_reason, minimize_github_comment, validate_github_comment_reason_policy,
};
use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GITHUB_COMMENT_MARKER, GithubClient,
    GithubIssueComment, GithubIssueNumber, GithubMutationFilters, GithubRepositoryPolicy,
    GithubTargetCapabilities, GithubUser, Validate, build_github_trace_footer,
    github_pipeline_comment_marker, resolve_github_issue_target,
    validate_github_mutation_filter_config, validate_github_mutation_filters,
    validate_github_repository, validate_github_target_capability,
};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;
use ado_aw_derive::SanitizeConfig;

const HIDE_OLDER_REASON: &str = "OUTDATED";

#[derive(Deserialize, JsonSchema)]
pub struct CommentOnGithubIssueParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// Comment body in Markdown.
    pub body: String,
    /// Optional target repository. Must exactly match `target-repo` or an
    /// `allowed-repos` entry.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for CommentOnGithubIssueParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.issue_number.validate("issue_number")?;
        ensure!(self.body.len() >= 10, "body must be at least 10 characters");
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "comment-on-github-issue",
    write = true,
    params = CommentOnGithubIssueParams,
    default_max = 1,
    /// Result of commenting on a GitHub issue or permitted pull request.
    pub struct CommentOnGithubIssueResult {
        issue_number: GithubIssueNumber,
        body: String,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for CommentOnGithubIssueResult {
    fn sanitize_content_fields(&mut self) {
        self.body = sanitize_text(&self.body);
        self.repository = self.repository.as_deref().map(sanitize_config);
    }
}

#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentOnGithubIssueConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    /// Minimize older comments from this pipeline definition and authenticated
    /// GitHub actor before posting the replacement comment.
    #[serde(default, rename = "hide-older-comments")]
    #[sanitize_config(skip)]
    pub hide_older_comments: bool,
    /// Restrict classifiers available to hide-older-comments. The replacement
    /// operation uses `OUTDATED`.
    #[serde(default, rename = "allowed-reasons")]
    pub allowed_reasons: Vec<String>,
    /// Permit issue targets.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub issues: bool,
    /// Permit pull-request targets through GitHub's issue-comments endpoint.
    #[serde(default, rename = "pull-requests")]
    #[sanitize_config(skip)]
    pub pull_requests: bool,
    /// Include the visible pipeline trace footer. The stable hidden pipeline
    /// marker is always included.
    #[serde(default = "default_true")]
    #[sanitize_config(skip)]
    pub footer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

impl Default for CommentOnGithubIssueConfig {
    fn default() -> Self {
        Self {
            target_repo: None,
            allowed_repos: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            hide_older_comments: false,
            allowed_reasons: Vec::new(),
            issues: true,
            pull_requests: false,
            footer: true,
            max: None,
        }
    }
}

const fn default_true() -> bool {
    true
}

pub(crate) fn validate_comment_on_github_issue_config(
    config: &CommentOnGithubIssueConfig,
) -> anyhow::Result<()> {
    ensure!(
        config.issues || config.pull_requests,
        "at least one of issues or pull-requests must be true"
    );
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
    for reason in &config.allowed_reasons {
        canonical_github_comment_reason(Some(reason))?;
    }
    if config.hide_older_comments {
        validate_github_comment_reason_policy(HIDE_OLDER_REASON, &config.allowed_reasons)?;
    }
    Ok(())
}

fn actor_matches(comment_user: Option<&GithubUser>, authenticated: &GithubUser) -> bool {
    let Some(comment_user) = comment_user else {
        return false;
    };
    if let Some(authenticated_node_id) = authenticated.node_id.as_deref() {
        return comment_user.node_id.as_deref() == Some(authenticated_node_id);
    }
    if let Some(authenticated_id) = authenticated.id {
        return comment_user.id == Some(authenticated_id);
    }
    comment_user
        .login
        .eq_ignore_ascii_case(&authenticated.login)
}

fn build_comment_body(body: &str, marker: &str, footer: bool, ctx: &ExecutionContext) -> String {
    let mut sections = vec![body.to_string(), marker.to_string()];
    if footer {
        sections.push(build_github_trace_footer(ctx));
    }
    sections.join("\n\n")
}

#[async_trait::async_trait]
impl Executor for CommentOnGithubIssueResult {
    fn dry_run_summary(&self) -> String {
        format!("comment on GitHub issue #{}", self.issue_number)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        const TOOL: &str = "comment-on-github-issue";
        if !ctx.tool_configs.contains_key(TOOL) {
            return Ok(ExecutionResult::failure(format!(
                "{TOOL} is not configured for this workflow"
            )));
        }
        let token = match ctx.github_token.as_ref() {
            Some(token) => token,
            None => {
                return Ok(ExecutionResult::failure(
                    "ADO_AW_GITHUB_TOKEN is not set; configure safe-outputs.github-token \
                     or safe-outputs.github-app",
                ));
            }
        };
        let config: CommentOnGithubIssueConfig = ctx.get_tool_config(TOOL)?;
        validate_comment_on_github_issue_config(&config)?;

        let target = match resolve_github_issue_target(
            &self.issue_number,
            self.repository.as_deref(),
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos),
            ctx,
        )? {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        let marker = if config.hide_older_comments {
            match github_pipeline_comment_marker(ctx) {
                Ok(marker) => marker,
                Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
            }
        } else {
            GITHUB_COMMENT_MARKER.to_string()
        };
        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };

        // Resolve live target metadata and all older comments before the first
        // write. This prevents a denied target or malformed prior comment from
        // leaving a partial mutation.
        let metadata = match client.get_issue(&target.repository, target.number).await? {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        if let Err(result) = validate_github_target_capability(
            &metadata,
            GithubTargetCapabilities {
                issues: config.issues,
                pull_requests: config.pull_requests,
            },
        ) {
            return Ok(result);
        }
        if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
            return Ok(result);
        }

        let mut older_node_ids = Vec::new();
        if config.hide_older_comments {
            let authenticated = match ctx.github_actor_login.as_deref() {
                Some(login) if !login.trim().is_empty() => GithubUser {
                    login: login.trim().to_string(),
                    id: None,
                    node_id: None,
                },
                Some(_) => {
                    return Ok(ExecutionResult::failure(
                        "ADO_AW_GITHUB_ACTOR_LOGIN is empty; cannot safely identify older GitHub App comments",
                    ));
                }
                None => match client.authenticated_user().await? {
                    Ok(user) => user,
                    Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
                },
            };
            let comments = match client
                .list_issue_comments(&target.repository, target.number)
                .await?
            {
                Ok(comments) => comments,
                Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
            };
            for comment in comments {
                if comment.body.contains(&marker)
                    && actor_matches(comment.user.as_ref(), &authenticated)
                {
                    let Some(node_id) = comment.node_id.filter(|node_id| !node_id.is_empty())
                    else {
                        return Ok(ExecutionResult::failure(format!(
                            "matching older GitHub comment {} contained no GraphQL node_id",
                            comment.id
                        )));
                    };
                    older_node_ids.push(node_id);
                }
            }
        }

        for node_id in &older_node_ids {
            if let Err(result) =
                minimize_github_comment(&client, node_id, HIDE_OLDER_REASON).await?
            {
                return Ok(result);
            }
        }

        let comment_body = build_comment_body(&self.body, &marker, config.footer, ctx);
        debug!(
            "POSTing GitHub comment to {}#{} after minimizing {} older comment(s)",
            target.repository,
            target.number,
            older_node_ids.len()
        );
        let response = client
            .send(
                Method::POST,
                client.issue_comments_url(&target.repository, target.number)?,
                Some(&serde_json::json!({ "body": comment_body })),
            )
            .await?;
        let response = match response.require_success("Failed to create GitHub issue comment") {
            Ok(response) => response,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        let comment: GithubIssueComment =
            match response.json("Failed to parse GitHub issue comment response") {
                Ok(comment) => comment,
                Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
            };
        if comment.id == 0 {
            return Ok(ExecutionResult::failure(
                "GitHub issue comment response contained no positive comment ID",
            ));
        }

        info!(
            "Created GitHub comment {} on {}#{}",
            comment.id, target.repository, target.number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "Commented on {}#{}: {}",
                target.repository,
                target.number,
                comment.html_url.as_deref().unwrap_or("")
            ),
            serde_json::json!({
                "comment_id": comment.id,
                "comment_node_id": comment.node_id,
                "url": comment.html_url,
                "number": target.number,
                "target_repo": target.repository,
                "hidden_older_comments": older_node_ids.len(),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::{ResolvedGithubIssue, ToolResult};
    use crate::secure::GithubTemporaryId;
    use serde_json::Value;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn issue_json(number: u64, pull_request: bool) -> Value {
        let mut issue = serde_json::json!({
            "number": number,
            "node_id": format!("I_{number}"),
            "title": "[agent] Managed target",
            "state": "open",
            "labels": [{"name": "managed"}],
            "html_url": format!("https://github.example/octo/repo/issues/{number}")
        });
        if pull_request {
            issue["pull_request"] = serde_json::json!({});
        }
        issue
    }

    fn created_comment(id: u64) -> Value {
        serde_json::json!({
            "id": id,
            "node_id": format!("IC_{id}"),
            "body": "created",
            "html_url": format!("https://github.example/octo/repo/issues/7#issuecomment-{id}"),
            "user": {"login": "ado-aw", "id": 10, "node_id": "U_10"}
        })
    }

    fn context(server: &MockServer, config: Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("comment-on-github-issue".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            definition_id: Some(123),
            definition_name: Some("Agent Pipeline".to_string()),
            build_id: Some(456),
            build_reason: Some("Manual".to_string()),
            tool_configs,
            ..Default::default()
        }
    }

    fn make_result(number: GithubIssueNumber) -> CommentOnGithubIssueResult {
        CommentOnGithubIssueParams {
            issue_number: number,
            body: "A useful status update.".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap()
    }

    async fn mount_issue_and_comment(server: &MockServer, pull_request: bool) {
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, pull_request)))
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(created_comment(99)))
            .expect(1)
            .mount(server)
            .await;
    }

    #[test]
    fn result_contract_and_parameter_validation() {
        assert_eq!(CommentOnGithubIssueResult::NAME, "comment-on-github-issue");
        assert_eq!(CommentOnGithubIssueResult::DEFAULT_MAX, 1);
        assert!(
            CommentOnGithubIssueParams {
                issue_number: GithubIssueNumber::Number(0),
                body: "A useful body".to_string(),
                repository: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            CommentOnGithubIssueParams {
                issue_number: GithubIssueNumber::Number(1),
                body: "short".to_string(),
                repository: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            CommentOnGithubIssueParams {
                issue_number: GithubIssueNumber::Temporary(
                    GithubTemporaryId::parse("#aw_issue1").unwrap()
                ),
                body: "A useful status update".to_string(),
                repository: Some("octo/repo".to_string()),
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn strict_config_defaults_and_rejects_unknown_fields() {
        let defaults = CommentOnGithubIssueConfig::default();
        assert!(defaults.issues);
        assert!(!defaults.pull_requests);
        assert!(defaults.footer);
        assert!(!defaults.hide_older_comments);

        let config: CommentOnGithubIssueConfig = serde_yaml::from_str(
            r#"
target-repo: octo/repo
allowed-repos: [octo/other]
required-labels: [managed]
required-title-prefix: "[agent]"
hide-older-comments: true
allowed-reasons: [OUTDATED]
issues: false
pull-requests: true
footer: false
max: 2
"#,
        )
        .unwrap();
        assert!(!config.issues);
        assert!(config.pull_requests);
        assert!(!config.footer);
        assert_eq!(config.max, Some(2));
        assert!(serde_yaml::from_str::<CommentOnGithubIssueConfig>("unexpected: true").is_err());
    }

    #[test]
    fn sanitizes_agent_body_and_repository() {
        let mut result = CommentOnGithubIssueResult {
            name: "comment-on-github-issue".to_string(),
            issue_number: GithubIssueNumber::Number(7),
            body: "hello\u{0007} <!-- forged marker --> @octocat".to_string(),
            repository: Some("octo/re\u{0008}po".to_string()),
        };
        result.sanitize_content_fields();
        assert!(!result.body.contains('\u{0007}'));
        assert!(!result.body.contains("forged marker"));
        assert!(result.body.contains("`@octocat`"));
        assert_eq!(result.repository.as_deref(), Some("octo/repo"));
    }

    #[tokio::test]
    async fn dry_run_performs_no_http() {
        let server = MockServer::start().await;
        let mut ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        ctx.dry_run = true;
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success);
        assert!(execution.message.contains("[DRY-RUN]"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn creates_comment_with_generic_marker_and_footer() {
        let server = MockServer::start().await;
        mount_issue_and_comment(&server, false).await;
        let ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);

        let requests = server.received_requests().await.unwrap();
        let create = requests
            .iter()
            .find(|request| request.method.as_str() == "POST")
            .unwrap();
        let payload: Value = serde_json::from_slice(&create.body).unwrap();
        let body = payload["body"].as_str().unwrap();
        assert!(body.contains("A useful status update."));
        assert!(body.contains(GITHUB_COMMENT_MARKER));
        assert!(!body.contains("pipeline-definition-id=123"));
        assert!(body.contains("<!-- ado-aw -->"));
        assert!(body.contains("Pipeline: `Agent Pipeline`"));
    }

    #[tokio::test]
    async fn footer_can_be_disabled_but_generic_marker_cannot() {
        let server = MockServer::start().await;
        mount_issue_and_comment(&server, false).await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "footer": false
            }),
        );
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success);
        let requests = server.received_requests().await.unwrap();
        let create = requests
            .iter()
            .find(|request| request.method.as_str() == "POST")
            .unwrap();
        let payload: Value = serde_json::from_slice(&create.body).unwrap();
        let body = payload["body"].as_str().unwrap();
        assert!(body.contains(GITHUB_COMMENT_MARKER));
        assert!(!body.contains("pipeline-definition-id=123"));
        assert!(!body.contains("Pipeline:"));
        assert!(!body.contains("<!-- ado-aw -->"));
    }

    #[tokio::test]
    async fn pull_requests_are_default_denied_and_can_be_enabled() {
        let denied_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, true)))
            .expect(1)
            .mount(&denied_server)
            .await;
        let denied_ctx = context(
            &denied_server,
            serde_json::json!({"target-repo": "octo/repo"}),
        );
        let mut denied = make_result(GithubIssueNumber::Number(7));
        let execution = denied.execute_sanitized(&denied_ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("pull requests"));
        assert_eq!(denied_server.received_requests().await.unwrap().len(), 1);

        let enabled_server = MockServer::start().await;
        mount_issue_and_comment(&enabled_server, true).await;
        let enabled_ctx = context(
            &enabled_server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "pull-requests": true
            }),
        );
        let mut enabled = make_result(GithubIssueNumber::Number(7));
        let execution = enabled.execute_sanitized(&enabled_ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
    }

    #[tokio::test]
    async fn live_filter_failure_performs_no_write() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["missing"]
            }),
        );
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("missing required labels"));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method.as_str(), "GET");
    }

    #[tokio::test]
    async fn temporary_issue_id_resolves_before_commenting() {
        let server = MockServer::start().await;
        mount_issue_and_comment(&server, false).await;
        let ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        let temporary_id = GithubTemporaryId::parse("#aw_issue1").unwrap();
        ctx.register_resolved_github_issue(
            &temporary_id,
            ResolvedGithubIssue {
                repository: "octo/repo".to_string(),
                number: 7,
                url: "https://github.example/octo/repo/issues/7".to_string(),
            },
        )
        .unwrap();
        let mut result = make_result(GithubIssueNumber::Temporary(temporary_id));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
    }

    #[tokio::test]
    async fn hide_older_paginates_and_only_minimizes_matching_actor_and_marker() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "ado-aw",
                "id": 10,
                "node_id": "U_10"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let next = format!(
            "<{}/page2/comments?per_page=100>; rel=\"next\"",
            server.uri()
        );
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .and(query_param("per_page", "100"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Link", next)
                    .set_body_json(serde_json::json!([
                        {
                            "id": 1,
                            "node_id": "IC_MATCH",
                            "body": "old\n<!-- ado-aw:github-comment:pipeline-definition-id=123 -->",
                            "user": {"login": "ado-aw", "id": 10, "node_id": "U_10"}
                        },
                        {
                            "id": 2,
                            "node_id": "IC_OTHER_ACTOR",
                            "body": "<!-- ado-aw:github-comment:pipeline-definition-id=123 -->",
                            "user": {"login": "attacker", "id": 11, "node_id": "U_11"}
                        }
                    ])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/page2/comments"))
            .and(query_param("per_page", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 3,
                    "node_id": "IC_OTHER_PIPELINE",
                    "body": "<!-- ado-aw:github-comment:pipeline-definition-id=999 -->",
                    "user": {"login": "ado-aw", "id": 10, "node_id": "U_10"}
                }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": super::super::hide_github_issue_comment::MINIMIZE_COMMENT_MUTATION,
                "variables": {
                    "input": {"subjectId": "IC_MATCH", "classifier": "OUTDATED"}
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "minimizeComment": {
                        "minimizedComment": {"isMinimized": true}
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(created_comment(99)))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "hide-older-comments": true,
                "allowed-reasons": ["OUTDATED"]
            }),
        );
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(
            execution.data.as_ref().unwrap()["hidden_older_comments"],
            serde_json::json!(1)
        );
    }

    #[tokio::test]
    async fn hide_older_uses_mint_derived_app_actor_without_discovery() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 1,
                    "node_id": "IC_APP",
                    "body": "<!-- ado-aw:github-comment:pipeline-definition-id=123 -->",
                    "user": {"login": "ado-aw-app[bot]", "id": 10, "node_id": "BOT_10"}
                },
                {
                    "id": 2,
                    "node_id": "IC_OTHER",
                    "body": "<!-- ado-aw:github-comment:pipeline-definition-id=123 -->",
                    "user": {"login": "other-app[bot]", "id": 11, "node_id": "BOT_11"}
                }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": super::super::hide_github_issue_comment::MINIMIZE_COMMENT_MUTATION,
                "variables": {
                    "input": {"subjectId": "IC_APP", "classifier": "OUTDATED"}
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "minimizeComment": {
                        "minimizedComment": {"isMinimized": true}
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(created_comment(99)))
            .expect(1)
            .mount(&server)
            .await;

        let mut ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "hide-older-comments": true
            }),
        );
        ctx.github_actor_login = Some("ado-aw-app[bot]".to_string());
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(
            execution.data.as_ref().unwrap()["hidden_older_comments"],
            serde_json::json!(1)
        );
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| {
                    request.url.path() != "/user" && request.url.path() != "/installation"
                })
        );
    }

    #[tokio::test]
    async fn hide_older_rejects_empty_app_actor_before_comment_writes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "hide-older-comments": true
            }),
        );
        ctx.github_actor_login = Some("  ".to_string());
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(
            execution
                .message
                .contains("ADO_AW_GITHUB_ACTOR_LOGIN is empty")
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn malformed_matching_old_comment_fails_before_any_write() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "ado-aw",
                "id": 10,
                "node_id": "U_10"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 1,
                    "body": "<!-- ado-aw:github-comment:pipeline-definition-id=123 -->",
                    "user": {"login": "ado-aw", "id": 10, "node_id": "U_10"}
                }
            ])))
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "hide-older-comments": true
            }),
        );
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("no GraphQL node_id"));
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
    async fn hide_older_reason_policy_and_missing_marker_identity_fail_before_http() {
        let server = MockServer::start().await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "hide-older-comments": true,
                "allowed-reasons": ["SPAM"]
            }),
        );
        let mut result = make_result(GithubIssueNumber::Number(7));
        let error = result.execute_sanitized(&ctx).await.unwrap_err();
        assert!(error.to_string().contains("allowed-reasons"));
        assert!(server.received_requests().await.unwrap().is_empty());

        let mut missing_id_ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "hide-older-comments": true
            }),
        );
        missing_id_ctx.definition_id = None;
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&missing_id_ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("SYSTEM_DEFINITIONID"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn ordinary_comment_does_not_require_definition_id() {
        let server = MockServer::start().await;
        mount_issue_and_comment(&server, false).await;
        let mut ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        ctx.definition_id = None;
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        let requests = server.received_requests().await.unwrap();
        let create = requests
            .iter()
            .find(|request| request.method.as_str() == "POST")
            .unwrap();
        let payload: Value = serde_json::from_slice(&create.body).unwrap();
        let body = payload["body"].as_str().unwrap();
        assert!(body.contains(GITHUB_COMMENT_MARKER));
        assert!(!body.contains("pipeline-definition-id="));
    }

    #[tokio::test]
    async fn creation_api_failures_are_explicit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, false)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "message": "Resource not accessible"
            })))
            .mount(&server)
            .await;
        let ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("HTTP 403"));
        assert!(execution.message.contains("Resource not accessible"));
    }

    #[tokio::test]
    async fn missing_configuration_token_and_invalid_config_fail_cleanly() {
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result
            .execute_sanitized(&ExecutionContext::default())
            .await
            .unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("not configured"));

        let server = MockServer::start().await;
        let mut no_token_ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        no_token_ctx.github_token = None;
        let mut result = make_result(GithubIssueNumber::Number(7));
        let execution = result.execute_sanitized(&no_token_ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("ADO_AW_GITHUB_TOKEN"));

        let invalid_ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "issues": false,
                "pull-requests": false
            }),
        );
        let mut result = make_result(GithubIssueNumber::Number(7));
        let error = result.execute_sanitized(&invalid_ctx).await.unwrap_err();
        assert!(error.to_string().contains("at least one"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
