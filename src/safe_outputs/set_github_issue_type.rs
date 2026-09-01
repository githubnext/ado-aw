//! `set-github-issue-type` safe output.

use log::{debug, info};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubClient, GithubIssueNumber,
    GithubMutationFilters, GithubRepositoryPolicy, GithubTargetCapabilities, Validate,
    resolve_github_issue_target, validate_github_mutation_filter_config,
    validate_github_mutation_filters, validate_github_repository,
    validate_github_target_capability,
};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

#[derive(Deserialize, JsonSchema)]
pub struct SetGithubIssueTypeParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// Native issue type name. An empty string clears the type.
    pub issue_type: String,
    /// Optional target repository. Must exactly match `target-repo` or an
    /// `allowed-repos` entry.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for SetGithubIssueTypeParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.issue_number.validate("issue_number")?;
        anyhow::ensure!(
            self.issue_type.len() <= 128,
            "issue_type must be 128 characters or fewer"
        );
        reject_pipeline_injection(&self.issue_type, "set-github-issue-type.issue_type")?;
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "set-github-issue-type",
    write = true,
    params = SetGithubIssueTypeParams,
    default_max = 5,
    /// Result of setting or clearing a GitHub issue's native type.
    pub struct SetGithubIssueTypeResult {
        issue_number: GithubIssueNumber,
        issue_type: String,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for SetGithubIssueTypeResult {
    fn sanitize_content_fields(&mut self) {
        self.issue_type = sanitize_config(&self.issue_type);
        self.repository = self
            .repository
            .as_deref()
            .map(crate::sanitize::sanitize_config);
    }
}

#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetGithubIssueTypeConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

#[async_trait::async_trait]
impl Executor for SetGithubIssueTypeResult {
    fn dry_run_summary(&self) -> String {
        let target = match &self.issue_number {
            GithubIssueNumber::Number(number) => format!("#{number}"),
            GithubIssueNumber::Temporary(id) => id.canonical(),
        };
        if self.issue_type.is_empty() {
            format!("clear GitHub issue type on {target}")
        } else {
            format!("set GitHub issue type on {target} to '{}'", self.issue_type)
        }
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if !ctx.tool_configs.contains_key("set-github-issue-type") {
            return Ok(ExecutionResult::failure(
                "set-github-issue-type is not configured for this workflow",
            ));
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
        let config: SetGithubIssueTypeConfig = ctx.get_tool_config("set-github-issue-type")?;

        let resolved_type = if self.issue_type.is_empty() {
            String::new()
        } else if config.allowed.is_empty() {
            self.issue_type.clone()
        } else if let Some(allowed) = config
            .allowed
            .iter()
            .find(|allowed| allowed.eq_ignore_ascii_case(&self.issue_type))
        {
            allowed.clone()
        } else {
            return Ok(ExecutionResult::failure(format!(
                "Issue type '{}' is not in the allowed list: {}",
                crate::sanitize::neutralize_pipeline_commands(&self.issue_type),
                config.allowed.join(", ")
            )));
        };

        let target = match resolve_github_issue_target(
            &self.issue_number,
            self.repository.as_deref(),
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos),
            ctx,
        )? {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };

        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };
        validate_github_mutation_filter_config(filters)?;
        let metadata = match client.get_issue(&target.repository, target.number).await? {
            Ok(metadata) => metadata,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        if let Err(result) =
            validate_github_target_capability(&metadata, GithubTargetCapabilities::ISSUES_ONLY)
        {
            return Ok(result);
        }
        if let Err(result) = validate_github_mutation_filters(&metadata, filters) {
            return Ok(result);
        }

        let url = client.issue_url(&target.repository, target.number)?;
        debug!("PATCHing GitHub issue type at {url}");

        // gh-aw's set-github-issue-type contract uses an empty string to clear the
        // native type; preserve that wire behavior for front-matter parity.
        let response = client
            .send(
                Method::PATCH,
                url,
                Some(&serde_json::json!({ "type": resolved_type })),
            )
            .await?;

        if !response.is_success() {
            let error = response
                .require_success("Failed to set GitHub issue type")
                .expect_err("non-success response must produce an API error");
            return Ok(ExecutionResult::failure(error.to_string()));
        }

        let action = if resolved_type.is_empty() {
            "Cleared".to_string()
        } else {
            format!("Set to '{resolved_type}'")
        };
        info!(
            "{} native type for GitHub issue {}#{}",
            action, target.repository, target.number
        );
        Ok(ExecutionResult::success_with_data(
            format!(
                "{} issue type for {}#{}",
                action, target.repository, target.number
            ),
            serde_json::json!({
                "number": target.number,
                "target_repo": target.repository,
                "issue_type": resolved_type,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::{CreateGithubIssueParams, ToolResult};
    use crate::secure::GithubTemporaryId;
    use std::collections::HashMap;

    async fn mount_issue_get(server: &wiremock::MockServer, number: u64, pull_request: bool) {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};

        let mut body = serde_json::json!({
            "number": number,
            "node_id": format!("I_{number}"),
            "title": "Issue title",
            "state": "open",
            "labels": [],
            "html_url": format!("https://github.example/octo/repo/issues/{number}")
        });
        if pull_request {
            body["pull_request"] = serde_json::json!({});
        }
        Mock::given(method("GET"))
            .and(path(format!("/repos/octo/repo/issues/{number}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(server)
            .await;
    }

    #[test]
    fn result_name_and_default_budget_match_contract() {
        assert_eq!(SetGithubIssueTypeResult::NAME, "set-github-issue-type");
        assert_eq!(SetGithubIssueTypeResult::DEFAULT_MAX, 5);
    }

    #[test]
    fn validates_numeric_and_temporary_targets() {
        assert!(
            SetGithubIssueTypeParams {
                issue_number: GithubIssueNumber::Number(1),
                issue_type: "Bug".to_string(),
                repository: None,
            }
            .validate()
            .is_ok()
        );
        assert!(
            SetGithubIssueTypeParams {
                issue_number: GithubIssueNumber::Temporary(
                    GithubTemporaryId::parse("#aw_bug1").unwrap()
                ),
                issue_type: String::new(),
                repository: None,
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn rejects_zero_issue_number() {
        let result = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(0),
            issue_type: "Bug".to_string(),
            repository: None,
        }
        .validate();
        assert!(result.is_err());
    }

    #[test]
    fn rejects_malformed_repository() {
        let result = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(1),
            issue_type: "Bug".to_string(),
            repository: Some("octo/$(TOKEN)".to_string()),
        }
        .validate();
        assert!(result.is_err());
    }

    #[test]
    fn quoted_numeric_issue_number_deserializes_as_number() {
        let params: SetGithubIssueTypeParams = serde_json::from_value(serde_json::json!({
            "issue_number": "42",
            "issue_type": "Bug"
        }))
        .unwrap();
        assert!(matches!(params.issue_number, GithubIssueNumber::Number(42)));
    }

    #[tokio::test]
    async fn create_then_set_type_resolves_temporary_id() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "number": 42,
                "html_url": "https://github.example/octo/repo/issues/42"
            })))
            .expect(1)
            .mount(&server)
            .await;
        mount_issue_get(&server, 42, false).await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/42"))
            .and(body_json(serde_json::json!({ "type": "Bug" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "create-github-issue".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "require-temporary-id": true
            }),
        );
        tool_configs.insert(
            "set-github-issue-type".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed": ["Bug", "Feature"]
            }),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };

        let mut create: crate::safe_outputs::CreateGithubIssueResult = CreateGithubIssueParams {
            title: "A real issue title".to_string(),
            body: "A detailed issue body that is long enough for validation.".to_string(),
            labels: vec![],
            assignees: vec![],
            repository: None,
            temporary_id: Some(GithubTemporaryId::parse("#aw_bug1").unwrap()),
        }
        .try_into()
        .unwrap();
        let created = create.execute_sanitized(&ctx).await.unwrap();
        assert!(created.success, "create failed: {}", created.message);

        let mut duplicate: crate::safe_outputs::CreateGithubIssueResult = CreateGithubIssueParams {
            title: "A second real issue title".to_string(),
            body: "Another detailed issue body that is long enough for validation.".to_string(),
            labels: vec![],
            assignees: vec![],
            repository: None,
            temporary_id: Some(GithubTemporaryId::parse("#aw_bug1").unwrap()),
        }
        .try_into()
        .unwrap();
        let duplicate_result = duplicate.execute_sanitized(&ctx).await.unwrap();
        assert!(!duplicate_result.success);
        assert!(duplicate_result.message.contains("already used"));

        let mut set_type: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Temporary(
                GithubTemporaryId::parse("aw_bug1").unwrap(),
            ),
            issue_type: "bug".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let updated = set_type.execute_sanitized(&ctx).await.unwrap();
        assert!(updated.success, "set type failed: {}", updated.message);
        assert_eq!(
            updated
                .data
                .as_ref()
                .and_then(|data| data["number"].as_u64()),
            Some(42)
        );
    }

    #[tokio::test]
    async fn unresolved_temporary_id_fails_before_http() {
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-type".to_string(),
            serde_json::json!({"target-repo": "octo/repo"}),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Temporary(
                GithubTemporaryId::parse("#aw_missing").unwrap(),
            ),
            issue_type: "Bug".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("has not been resolved"));
    }

    #[tokio::test]
    async fn rejects_when_tool_not_configured() {
        let ctx = ExecutionContext {
            github_token: Some("token-that-must-not-be-used".to_string()),
            ..Default::default()
        };
        let mut result: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(1),
            issue_type: "Bug".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("not configured"));
    }

    #[tokio::test]
    async fn empty_issue_type_clears_with_rest_patch() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        mount_issue_get(&server, 7, false).await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({ "type": "" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-type".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed": ["Bug"]
            }),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(7),
            issue_type: String::new(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "clear failed: {}", execution.message);
    }

    /// Deliberate asymmetry with `create-github-issue.allowed-labels`, which is
    /// default-**deny**. Issue types are a closed set defined by the repository
    /// owner, so an empty `allowed:` means "any type the repo already defines"
    /// rather than "no types". Labels are free-form strings the agent can
    /// invent, hence the stricter default there. This test pins the documented
    /// behaviour so a well-meaning "make it consistent" change is caught.
    #[tokio::test]
    async fn empty_allowed_list_permits_any_issue_type() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        mount_issue_get(&server, 7, false).await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({ "type": "Epic" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-type".to_string(),
            serde_json::json!({ "target-repo": "octo/repo" }),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(7),
            issue_type: "Epic".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(
            execution.success,
            "empty allowed list must not deny: {}",
            execution.message
        );
    }

    /// The counterpart: once `allowed:` is non-empty it is strictly enforced,
    /// and the rejection happens before any HTTP call is made.
    #[tokio::test]
    async fn non_empty_allowed_list_rejects_unlisted_type_before_http() {
        use wiremock::MockServer;

        let server = MockServer::start().await;
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-type".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed": ["Bug", "Task"]
            }),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(7),
            issue_type: "Epic".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(
            execution.message.contains("not in the allowed list"),
            "unexpected message: {}",
            execution.message
        );
        // No mock was mounted, so any outbound PATCH would have surfaced as a
        // wiremock "unmatched request" rather than a clean allowlist failure.
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// A matching entry is echoed back using the *configured* casing, so the
    /// repository sees the canonical type name rather than the agent's casing.
    #[tokio::test]
    async fn allowed_match_is_case_insensitive_and_uses_configured_casing() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        mount_issue_get(&server, 7, false).await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({ "type": "Bug" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-type".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed": ["Bug"]
            }),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(7),
            issue_type: "bUg".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(
            execution.success,
            "case-insensitive match failed: {}",
            execution.message
        );
    }

    #[tokio::test]
    async fn required_filters_preflight_before_type_patch() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 7,
                "node_id": "I_7",
                "title": "[agent] Fix the build",
                "state": "open",
                "labels": [{"name": "bug"}],
                "html_url": "https://github.example/octo/repo/issues/7"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({ "type": "Bug" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-type".to_string(),
            serde_json::json!({
                "target-repo": "octo/default",
                "allowed-repos": ["octo/repo"],
                "required-labels": ["BUG"],
                "required-title-prefix": "[agent]",
                "allowed": ["Bug"]
            }),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(7),
            issue_type: "bug".to_string(),
            repository: Some("OCTO/REPO".to_string()),
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(
            execution.success,
            "filtered patch failed: {}",
            execution.message
        );
    }

    #[tokio::test]
    async fn pull_request_target_is_rejected_before_type_patch() {
        use wiremock::MockServer;

        let server = MockServer::start().await;
        mount_issue_get(&server, 7, true).await;
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-type".to_string(),
            serde_json::json!({"target-repo": "octo/repo"}),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(7),
            issue_type: "Bug".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("pull requests"));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method.as_str(), "GET");
    }

    #[tokio::test]
    async fn failed_required_filter_performs_no_patch() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 7,
                "title": "Unexpected title",
                "state": "open",
                "labels": [{"name": "bug"}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-github-issue-type".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-title-prefix": "[agent]"
            }),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetGithubIssueTypeResult = SetGithubIssueTypeParams {
            issue_number: GithubIssueNumber::Number(7),
            issue_type: "Bug".to_string(),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("required-title-prefix"));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method.as_str(), "GET");
    }

    #[test]
    fn config_round_trips_shared_repository_and_filter_fields() {
        let config: SetGithubIssueTypeConfig = serde_yaml::from_str(
            r#"
target-repo: octo/default
allowed-repos: [octo/other]
required-labels: [bug, triage]
required-title-prefix: "[agent]"
allowed: [Bug]
"#,
        )
        .unwrap();
        assert_eq!(config.target_repo.as_deref(), Some("octo/default"));
        assert_eq!(config.allowed_repos, vec!["octo/other".to_string()]);
        assert_eq!(config.required_labels, vec!["bug", "triage"]);
        assert_eq!(config.required_title_prefix.as_deref(), Some("[agent]"));
    }
}
