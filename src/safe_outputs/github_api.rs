//! Shared GitHub REST and GraphQL client for Stage 3 safe outputs.
#![allow(dead_code)] // The remaining GitHub issue tools consume this shared surface in later slices.

use anyhow::{Context, ensure};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, LINK, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::Url;

use super::{GithubTargetKind, GithubTargetMetadata, validate_github_repository};

const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const GITHUB_API_VERSION: &str = "2022-11-28";
const MAX_ERROR_CHARS: usize = 4096;
const MAX_PAGES: usize = 1000;

/// Captured GitHub response with helpers for sanitized API failures.
#[derive(Debug)]
pub struct GithubResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    body: String,
}

impl GithubResponse {
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn json<T: DeserializeOwned>(&self, operation: &str) -> Result<T, GithubApiError> {
        serde_json::from_str(&self.body).map_err(|error| GithubApiError {
            operation: operation.to_string(),
            status: Some(self.status),
            message: format!("GitHub returned malformed JSON: {error}"),
        })
    }

    pub fn require_success(self, operation: &str) -> Result<Self, GithubApiError> {
        if self.is_success() {
            Ok(self)
        } else {
            Err(GithubApiError::from_response(operation, self))
        }
    }
}

/// Sanitized HTTP, GraphQL, or response-shape failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubApiError {
    pub operation: String,
    pub status: Option<StatusCode>,
    pub message: String,
}

impl GithubApiError {
    fn from_response(operation: &str, response: GithubResponse) -> Self {
        Self {
            operation: operation.to_string(),
            status: Some(response.status),
            message: sanitize_github_error_body(&response.body),
        }
    }

    fn graphql(operation: &str, status: StatusCode, errors: &[Value]) -> Self {
        let messages: Vec<String> = errors
            .iter()
            .map(|error| {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown GraphQL error");
                let kind = error.get("type").and_then(Value::as_str).or_else(|| {
                    error
                        .get("extensions")
                        .and_then(|extensions| extensions.get("type"))
                        .and_then(Value::as_str)
                });
                match kind {
                    Some(kind) => format!("{kind}: {message}"),
                    None => message.to_string(),
                }
            })
            .collect();
        Self {
            operation: operation.to_string(),
            status: Some(status),
            message: sanitize_github_error_body(&messages.join("; ")),
        }
    }
}

impl std::fmt::Display for GithubApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(
                formatter,
                "{} (HTTP {}): {}",
                self.operation, status, self.message
            ),
            None => write!(formatter, "{}: {}", self.operation, self.message),
        }
    }
}

impl std::error::Error for GithubApiError {}

/// Minimal issue comment metadata needed by comment policy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GithubIssueComment {
    pub id: u64,
    pub node_id: Option<String>,
    #[serde(default)]
    pub body: String,
    pub html_url: Option<String>,
    pub user: Option<GithubUser>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GithubUser {
    pub login: String,
    pub id: Option<u64>,
    pub node_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubInstallation {
    app_slug: String,
}

/// Minimal milestone metadata needed by milestone assignment.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GithubMilestone {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub node_id: Option<String>,
}

/// GitHub client with fixed authentication, headers, REST base, and GraphQL URL.
#[derive(Clone)]
pub struct GithubClient {
    http: reqwest::Client,
    rest_api_url: Url,
    graphql_url: Url,
    token: String,
}

impl GithubClient {
    pub fn new(rest_api_url: &str, token: &str) -> anyhow::Result<Self> {
        ensure!(!token.is_empty(), "GitHub token must not be empty");
        let rest_api_url = validate_rest_api_url(rest_api_url)?;
        let graphql_url = graphql_url_from_rest_api_url(rest_api_url.as_str())?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("Failed to build GitHub API client")?;
        Ok(Self {
            http,
            rest_api_url,
            graphql_url,
            token: token.to_string(),
        })
    }

    pub fn rest_api_url(&self) -> &Url {
        &self.rest_api_url
    }

    pub fn graphql_url(&self) -> &Url {
        &self.graphql_url
    }

    pub fn issues_url(&self, repository: &str) -> anyhow::Result<Url> {
        self.repository_route(repository, &["issues"])
    }

    pub fn issue_url(&self, repository: &str, number: u64) -> anyhow::Result<Url> {
        ensure!(number > 0, "GitHub issue number must be positive");
        self.repository_route(repository, &["issues", &number.to_string()])
    }

    pub fn issue_comments_url(&self, repository: &str, number: u64) -> anyhow::Result<Url> {
        ensure!(number > 0, "GitHub issue number must be positive");
        self.repository_route(repository, &["issues", &number.to_string(), "comments"])
    }

    pub fn issue_comment_url(&self, repository: &str, comment_id: u64) -> anyhow::Result<Url> {
        ensure!(comment_id > 0, "GitHub comment ID must be positive");
        self.repository_route(repository, &["issues", "comments", &comment_id.to_string()])
    }

    pub fn milestones_url(&self, repository: &str) -> anyhow::Result<Url> {
        self.repository_route(repository, &["milestones"])
    }

    pub async fn send(
        &self,
        method: Method,
        url: Url,
        body: Option<&Value>,
    ) -> anyhow::Result<GithubResponse> {
        self.ensure_same_origin(&url)?;
        let mut request = self
            .http
            .request(method, url)
            .header(ACCEPT, GITHUB_ACCEPT)
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .header(USER_AGENT, format!("ado-aw/{}", env!("CARGO_PKG_VERSION")))
            .header(AUTHORIZATION, format!("Bearer {}", self.token));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .context("Failed to send request to GitHub API")?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .text()
            .await
            .context("Failed to read response from GitHub API")?;
        Ok(GithubResponse {
            status,
            headers,
            body,
        })
    }

    pub async fn get_issue(
        &self,
        repository: &str,
        number: u64,
    ) -> anyhow::Result<Result<GithubTargetMetadata, GithubApiError>> {
        let response = self
            .send(Method::GET, self.issue_url(repository, number)?, None)
            .await?;
        let response = match response.require_success("Failed to fetch GitHub issue") {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        };
        let issue: RawGithubIssue = match response.json("Failed to parse GitHub issue") {
            Ok(issue) => issue,
            Err(error) => return Ok(Err(error)),
        };
        Ok(Ok(issue.into_metadata()))
    }

    pub async fn list_issue_comments(
        &self,
        repository: &str,
        number: u64,
    ) -> anyhow::Result<Result<Vec<GithubIssueComment>, GithubApiError>> {
        let url = self.issue_comments_url(repository, number)?;
        self.get_paginated(url, "Failed to list GitHub issue comments")
            .await
    }

    pub async fn list_milestones(
        &self,
        repository: &str,
    ) -> anyhow::Result<Result<Vec<GithubMilestone>, GithubApiError>> {
        let mut url = self.milestones_url(repository)?;
        url.query_pairs_mut().append_pair("state", "all");
        self.get_paginated(url, "Failed to list GitHub milestones")
            .await
    }

    pub async fn authenticated_user(&self) -> anyhow::Result<Result<GithubUser, GithubApiError>> {
        let response = self.send(Method::GET, self.route(&["user"])?, None).await?;
        let response = match response.require_success("Failed to fetch authenticated GitHub user") {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        };
        Ok(response.json("Failed to parse authenticated GitHub user"))
    }

    /// Resolve the actor identity used for issue comments.
    ///
    /// User/PAT tokens expose `GET /user`. Installation tokens do not, so on
    /// the installation-token 403 path derive the bot login from
    /// `GET /installation` instead. The caller can then compare the exact actor
    /// login and avoid minimizing comments written by a different actor.
    pub async fn authenticated_comment_actor(
        &self,
    ) -> anyhow::Result<Result<GithubUser, GithubApiError>> {
        let response = self.send(Method::GET, self.route(&["user"])?, None).await?;
        if response.is_success() {
            return Ok(response.json("Failed to parse authenticated GitHub user"));
        }
        if response.status != StatusCode::FORBIDDEN {
            return Ok(Err(GithubApiError::from_response(
                "Failed to fetch authenticated GitHub user",
                response,
            )));
        }

        let response = self
            .send(Method::GET, self.route(&["installation"])?, None)
            .await?;
        let response = match response.require_success("Failed to fetch GitHub App installation") {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        };
        let installation: GithubInstallation =
            match response.json("Failed to parse GitHub App installation") {
                Ok(installation) => installation,
                Err(error) => return Ok(Err(error)),
            };
        if installation.app_slug.trim().is_empty() {
            return Ok(Err(GithubApiError {
                operation: "Failed to parse GitHub App installation".to_string(),
                status: Some(response.status),
                message: "GitHub App installation contained no app_slug".to_string(),
            }));
        }
        Ok(Ok(GithubUser {
            login: format!("{}[bot]", installation.app_slug),
            id: None,
            node_id: None,
        }))
    }

    pub async fn graphql(
        &self,
        operation: &str,
        query: &str,
        variables: Value,
    ) -> anyhow::Result<Result<Value, GithubApiError>> {
        let response = self
            .send(
                Method::POST,
                self.graphql_url.clone(),
                Some(&serde_json::json!({
                    "query": query,
                    "variables": variables,
                })),
            )
            .await?;
        let response = match response.require_success(operation) {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        };
        let payload: Value = match response.json(operation) {
            Ok(payload) => payload,
            Err(error) => return Ok(Err(error)),
        };
        let errors = payload
            .get("errors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !errors.is_empty() {
            return Ok(Err(GithubApiError::graphql(
                operation,
                response.status,
                &errors,
            )));
        }
        match payload.get("data") {
            Some(data) => Ok(Ok(data.clone())),
            None => Ok(Err(GithubApiError {
                operation: operation.to_string(),
                status: Some(response.status),
                message: "GitHub GraphQL response contained no data".to_string(),
            })),
        }
    }

    async fn get_paginated<T: DeserializeOwned>(
        &self,
        mut url: Url,
        operation: &str,
    ) -> anyhow::Result<Result<Vec<T>, GithubApiError>> {
        if !url.query_pairs().any(|(key, _)| key == "per_page") {
            url.query_pairs_mut().append_pair("per_page", "100");
        }
        let mut values = Vec::new();
        for _ in 0..MAX_PAGES {
            let response = self.send(Method::GET, url, None).await?;
            let response = match response.require_success(operation) {
                Ok(response) => response,
                Err(error) => return Ok(Err(error)),
            };
            let mut page: Vec<T> = match response.json(operation) {
                Ok(page) => page,
                Err(error) => return Ok(Err(error)),
            };
            values.append(&mut page);
            let Some(next) = next_link(&response.headers) else {
                return Ok(Ok(values));
            };
            let next = match Url::parse(&next) {
                Ok(next) => next,
                Err(error) => {
                    return Ok(Err(GithubApiError {
                        operation: operation.to_string(),
                        status: Some(response.status),
                        message: format!("GitHub returned an invalid pagination URL: {error}"),
                    }));
                }
            };
            if let Err(error) = self.ensure_same_origin(&next) {
                return Ok(Err(GithubApiError {
                    operation: operation.to_string(),
                    status: Some(response.status),
                    message: error.to_string(),
                }));
            }
            url = next;
        }
        Ok(Err(GithubApiError {
            operation: operation.to_string(),
            status: None,
            message: format!("GitHub pagination exceeded {MAX_PAGES} pages"),
        }))
    }

    fn repository_route(&self, repository: &str, tail: &[&str]) -> anyhow::Result<Url> {
        validate_github_repository(repository)?;
        let (owner, name) = repository
            .split_once('/')
            .expect("validated GitHub repository contains slash");
        let mut segments = vec!["repos", owner, name];
        segments.extend_from_slice(tail);
        self.route(&segments)
    }

    fn route(&self, segments: &[&str]) -> anyhow::Result<Url> {
        let mut url = self.rest_api_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("GitHub API URL cannot be a base URL"))?;
            path.pop_if_empty();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
    }

    fn ensure_same_origin(&self, url: &Url) -> anyhow::Result<()> {
        ensure!(
            url.scheme() == self.rest_api_url.scheme()
                && url.host_str() == self.rest_api_url.host_str()
                && url.port_or_known_default() == self.rest_api_url.port_or_known_default(),
            "GitHub API pagination or route attempted to leave the configured API origin"
        );
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct RawGithubIssue {
    number: u64,
    node_id: Option<String>,
    title: String,
    state: String,
    #[serde(default)]
    labels: Vec<RawGithubLabel>,
    pull_request: Option<Value>,
    html_url: Option<String>,
}

impl RawGithubIssue {
    fn into_metadata(self) -> GithubTargetMetadata {
        GithubTargetMetadata {
            number: self.number,
            node_id: self.node_id,
            title: self.title,
            state: self.state,
            labels: self.labels.into_iter().map(|label| label.name).collect(),
            kind: if self.pull_request.is_some() {
                GithubTargetKind::PullRequest
            } else {
                GithubTargetKind::Issue
            },
            html_url: self.html_url,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawGithubLabel {
    name: String,
}

fn validate_rest_api_url(raw: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(raw).with_context(|| format!("GitHub API URL '{raw}' is invalid"))?;
    ensure!(
        url.host_str().is_some(),
        "GitHub API URL must include a host"
    );
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && loopback),
        "GitHub API URL must use https"
    );
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "GitHub API URL must not contain a query string or fragment"
    );
    url.set_query(None);
    url.set_fragment(None);
    let trimmed = url.path().trim_end_matches('/').to_string();
    url.set_path(if trimmed.is_empty() { "/" } else { &trimmed });
    Ok(url)
}

/// Derive the GraphQL endpoint for GitHub.com or GitHub Enterprise Server.
pub fn graphql_url_from_rest_api_url(rest_api_url: &str) -> anyhow::Result<Url> {
    let mut url = validate_rest_api_url(rest_api_url)?;
    if url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.github.com"))
    {
        url.set_path("/graphql");
        return Ok(url);
    }

    let path = url.path().trim_end_matches('/');
    let graphql_path = match path.strip_suffix("/api/v3") {
        Some(prefix) => format!("{prefix}/api/graphql"),
        None => format!("{path}/graphql"),
    };
    url.set_path(&graphql_path);
    Ok(url)
}

fn next_link(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(LINK)?.to_str().ok()?;
    value.split(',').find_map(|part| {
        let mut pieces = part.trim().split(';');
        let url = pieces.next()?.trim().strip_prefix('<')?.strip_suffix('>')?;
        let is_next = pieces.any(|piece| piece.trim() == r#"rel="next""#);
        is_next.then(|| url.to_string())
    })
}

fn sanitize_github_error_body(body: &str) -> String {
    let structured = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            let message = value.get("message").and_then(Value::as_str)?;
            let mut rendered = message.to_string();
            if let Some(errors) = value.get("errors").and_then(Value::as_array)
                && !errors.is_empty()
            {
                let details: Vec<String> = errors
                    .iter()
                    .filter_map(|error| {
                        error
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| error.get("message")?.as_str().map(str::to_string))
                    })
                    .collect();
                if !details.is_empty() {
                    rendered.push_str(": ");
                    rendered.push_str(&details.join("; "));
                }
            }
            Some(rendered)
        })
        .unwrap_or_else(|| body.to_string());
    let neutralized = crate::sanitize::neutralize_pipeline_commands(&structured);
    let mut sanitized: String = neutralized
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(MAX_ERROR_CHARS)
        .collect();
    if neutralized.chars().count() > MAX_ERROR_CHARS {
        sanitized.push('…');
    }
    if sanitized.trim().is_empty() {
        "<empty response body>".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn issue_json(number: u64, pull_request: bool) -> Value {
        let mut value = serde_json::json!({
            "number": number,
            "node_id": format!("I_{number}"),
            "title": "Issue title",
            "state": "open",
            "labels": [{"name": "bug"}, {"name": "triage"}],
            "html_url": format!("https://github.example/octo/repo/issues/{number}")
        });
        if pull_request {
            value["pull_request"] = serde_json::json!({"url": "https://api.example/pulls/1"});
        }
        value
    }

    #[test]
    fn derives_dotcom_and_ghes_graphql_urls() {
        assert_eq!(
            graphql_url_from_rest_api_url("https://api.github.com")
                .unwrap()
                .as_str(),
            "https://api.github.com/graphql"
        );
        assert_eq!(
            graphql_url_from_rest_api_url("https://ghe.example.com/api/v3/")
                .unwrap()
                .as_str(),
            "https://ghe.example.com/api/graphql"
        );
        assert_eq!(
            graphql_url_from_rest_api_url("https://ghe.example.com/custom/api/v3")
                .unwrap()
                .as_str(),
            "https://ghe.example.com/custom/api/graphql"
        );
    }

    #[test]
    fn rejects_insecure_non_loopback_and_url_suffixes() {
        assert!(GithubClient::new("http://github.example", "token").is_err());
        assert!(GithubClient::new("https://api.github.com?token=x", "token").is_err());
        assert!(GithubClient::new("https://api.github.com", "").is_err());
    }

    #[tokio::test]
    async fn sends_standard_headers_and_repository_routes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/repos/octo/repo/issues"))
            .and(header("accept", GITHUB_ACCEPT))
            .and(header("x-github-api-version", GITHUB_API_VERSION))
            .and(header(
                "user-agent",
                format!("ado-aw/{}", env!("CARGO_PKG_VERSION")),
            ))
            .and(header("authorization", "Bearer secret"))
            .and(body_json(serde_json::json!({"title": "hello"})))
            .respond_with(ResponseTemplate::new(201).set_body_json(issue_json(1, false)))
            .expect(1)
            .mount(&server)
            .await;

        let client = GithubClient::new(&format!("{}/api/v3", server.uri()), "secret").unwrap();
        let url = client.issues_url("octo/repo").unwrap();
        let response = client
            .send(
                Method::POST,
                url,
                Some(&serde_json::json!({"title": "hello"})),
            )
            .await
            .unwrap();
        assert_eq!(response.status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn fetches_issue_and_distinguishes_pull_requests() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(7, true)))
            .mount(&server)
            .await;
        let client = GithubClient::new(&server.uri(), "token").unwrap();
        let metadata = client.get_issue("octo/repo", 7).await.unwrap().unwrap();
        assert_eq!(metadata.kind, GithubTargetKind::PullRequest);
        assert_eq!(metadata.labels, vec!["bug", "triage"]);
    }

    #[tokio::test]
    async fn paginates_comments_using_link_header() {
        let server = MockServer::start().await;
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
                    .set_body_json(serde_json::json!([{
                        "id": 1,
                        "node_id": "IC_1",
                        "body": "first",
                        "user": {"login": "octocat"}
                    }])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/page2/comments"))
            .and(query_param("per_page", "100"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "id": 2,
                    "node_id": "IC_2",
                    "body": "second",
                    "user": {"login": "octocat"}
                }])),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = GithubClient::new(&server.uri(), "token").unwrap();
        let comments = client
            .list_issue_comments("octo/repo", 7)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[1].id, 2);
    }

    #[tokio::test]
    async fn paginates_milestones_with_all_states() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/milestones"))
            .and(query_param("state", "all"))
            .and(query_param("per_page", "100"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "number": 3,
                    "title": "v1",
                    "state": "open",
                    "node_id": "MI_3"
                }])),
            )
            .mount(&server)
            .await;
        let client = GithubClient::new(&server.uri(), "token").unwrap();
        let milestones = client.list_milestones("octo/repo").await.unwrap().unwrap();
        assert_eq!(milestones[0].title, "v1");
    }

    #[tokio::test]
    async fn derives_comment_actor_from_app_installation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "message": "Resource not accessible by integration"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/installation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "app_slug": "ado-aw-app"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = GithubClient::new(&server.uri(), "installation-token").unwrap();
        let actor = client.authenticated_comment_actor().await.unwrap().unwrap();
        assert_eq!(actor.login, "ado-aw-app[bot]");
        assert_eq!(actor.id, None);
        assert_eq!(actor.node_id, None);
    }

    #[tokio::test]
    async fn sanitizes_structured_rest_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "message": "Validation failed\n##vso[task.complete]",
                "errors": [{"message": "bad label"}]
            })))
            .mount(&server)
            .await;
        let client = GithubClient::new(&server.uri(), "token").unwrap();
        let error = client.get_issue("octo/repo", 7).await.unwrap().unwrap_err();
        assert!(error.to_string().contains("Validation failed"));
        assert!(error.to_string().contains("bad label"));
        assert!(
            !error
                .to_string()
                .lines()
                .any(|line| line.starts_with("##vso["))
        );
    }

    #[tokio::test]
    async fn reports_malformed_rest_json_without_panicking() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{not-json"))
            .mount(&server)
            .await;
        let client = GithubClient::new(&server.uri(), "token").unwrap();
        let error = client.get_issue("octo/repo", 7).await.unwrap().unwrap_err();
        assert!(error.message.contains("malformed JSON"));
    }

    #[tokio::test]
    async fn graphql_returns_data_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {"viewer": {"login": "octocat"}}
            })))
            .mount(&server)
            .await;
        let client = GithubClient::new(&server.uri(), "token").unwrap();
        let data = client
            .graphql(
                "Fetch viewer",
                "query { viewer { login } }",
                serde_json::json!({}),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(data["viewer"]["login"], "octocat");
    }

    #[tokio::test]
    async fn graphql_uses_ghes_route_and_surfaces_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_json(serde_json::json!({
                "query": "query Test { viewer { login } }",
                "variables": {}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": null,
                "errors": [{
                    "type": "FORBIDDEN",
                    "message": "denied\n##vso[task.complete]"
                }]
            })))
            .mount(&server)
            .await;
        let client = GithubClient::new(&format!("{}/api/v3", server.uri()), "token").unwrap();
        let error = client
            .graphql(
                "Test GraphQL operation",
                "query Test { viewer { login } }",
                serde_json::json!({}),
            )
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("FORBIDDEN: denied"));
        assert!(
            !error
                .to_string()
                .lines()
                .any(|line| line.starts_with("##vso["))
        );
    }

    #[tokio::test]
    async fn rejects_cross_origin_pagination() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "Link",
                        "<https://evil.example/comments?page=2>; rel=\"next\"",
                    )
                    .set_body_json(serde_json::json!([])),
            )
            .mount(&server)
            .await;
        let client = GithubClient::new(&server.uri(), "token").unwrap();
        let error = client
            .list_issue_comments("octo/repo", 7)
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.message.contains("leave the configured API origin"));
    }
}
