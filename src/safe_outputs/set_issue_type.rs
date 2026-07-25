//! `set-issue-type` safe output.

use anyhow::Context;
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use super::create_issue::{resolve_target_repo, validate_target_repo};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::secure::GithubTemporaryId;
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum GithubIssueNumber {
    Number(u64),
    Temporary(GithubTemporaryId),
}

impl<'de> Deserialize<'de> for GithubIssueNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct IssueNumberVisitor;

        impl serde::de::Visitor<'_> for IssueNumberVisitor {
            type Value = GithubIssueNumber;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a positive issue number or #aw_ temporary issue ID")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(GithubIssueNumber::Number(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                u64::try_from(value)
                    .map(GithubIssueNumber::Number)
                    .map_err(|_| E::custom("issue_number must be positive"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value.chars().all(|c| c.is_ascii_digit()) {
                    return value
                        .parse::<u64>()
                        .map(GithubIssueNumber::Number)
                        .map_err(|_| E::custom("quoted issue_number is outside the u64 range"));
                }
                GithubTemporaryId::parse(value)
                    .map(GithubIssueNumber::Temporary)
                    .map_err(E::custom)
            }
        }

        deserializer.deserialize_any(IssueNumberVisitor)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct SetIssueTypeParams {
    /// Positive GitHub issue number or a temporary ID from create_issue.
    pub issue_number: GithubIssueNumber,
    /// Native issue type name. An empty string clears the type.
    pub issue_type: String,
}

impl Validate for SetIssueTypeParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let GithubIssueNumber::Number(number) = self.issue_number {
            anyhow::ensure!(number > 0, "issue_number must be positive");
        }
        anyhow::ensure!(
            self.issue_type.len() <= 128,
            "issue_type must be 128 characters or fewer"
        );
        reject_pipeline_injection(&self.issue_type, "set-issue-type.issue_type")?;
        Ok(())
    }
}

tool_result! {
    name = "set-issue-type",
    write = true,
    params = SetIssueTypeParams,
    default_max = 5,
    /// Result of setting or clearing a GitHub issue's native type.
    pub struct SetIssueTypeResult {
        issue_number: GithubIssueNumber,
        issue_type: String,
    }
}

impl SanitizeContent for SetIssueTypeResult {
    fn sanitize_content_fields(&mut self) {
        self.issue_type = sanitize_config(&self.issue_type);
    }
}

#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetIssueTypeConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

#[async_trait::async_trait]
impl Executor for SetIssueTypeResult {
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
        if !ctx.tool_configs.contains_key("set-issue-type") {
            return Ok(ExecutionResult::failure(
                "set-issue-type is not configured for this workflow",
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
        let config: SetIssueTypeConfig = ctx.get_tool_config("set-issue-type");

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

        let (target_repo, issue_number) = match &self.issue_number {
            GithubIssueNumber::Number(number) => {
                let target = match resolve_target_repo(config.target_repo.as_deref(), ctx) {
                    Ok(target) => target,
                    Err(result) => return Ok(result),
                };
                (target, *number)
            }
            GithubIssueNumber::Temporary(temporary_id) => {
                let Some(issue) = ctx.resolve_github_issue(temporary_id)? else {
                    return Ok(ExecutionResult::failure(format!(
                        "temporary issue ID '{}' has not been resolved; create-issue must \
                         succeed earlier in the same SafeOutputs job",
                        temporary_id.canonical()
                    )));
                };
                if let Some(configured) = config.target_repo.as_deref() {
                    if let Err(error) = validate_target_repo(configured) {
                        return Ok(ExecutionResult::failure(error.to_string()));
                    }
                    if !configured.eq_ignore_ascii_case(&issue.repository) {
                        return Ok(ExecutionResult::failure(format!(
                            "temporary issue ID '{}' resolved to repository '{}', which does \
                             not match set-issue-type.target-repo '{}'",
                            temporary_id.canonical(),
                            issue.repository,
                            configured
                        )));
                    }
                }
                (issue.repository, issue.number)
            }
        };

        let (owner, repo) = target_repo
            .split_once('/')
            .context("target-repo must be 'owner/repo'")?;
        let url = format!(
            "{}/repos/{}/{}/issues/{}",
            ctx.github_api_url.trim_end_matches('/'),
            utf8_percent_encode(owner, PATH_SEGMENT),
            utf8_percent_encode(repo, PATH_SEGMENT),
            issue_number
        );
        debug!("PATCHing GitHub issue type at {url}");

        // gh-aw's set-issue-type contract uses an empty string to clear the
        // native type; preserve that wire behavior for front-matter parity.
        let response = reqwest::Client::new()
            .patch(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header(
                "User-Agent",
                format!("ado-aw/{}", env!("CARGO_PKG_VERSION")),
            )
            .bearer_auth(token)
            .json(&serde_json::json!({ "type": resolved_type }))
            .send()
            .await
            .context("Failed to send request to GitHub API")?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read response body>".to_string());
            return Ok(ExecutionResult::failure(format!(
                "Failed to set GitHub issue type (HTTP {}): {}",
                status,
                crate::sanitize::neutralize_pipeline_commands(&body)
            )));
        }

        let action = if resolved_type.is_empty() {
            "Cleared".to_string()
        } else {
            format!("Set to '{resolved_type}'")
        };
        info!(
            "{} native type for GitHub issue {}#{}",
            action, target_repo, issue_number
        );
        Ok(ExecutionResult::success_with_data(
            format!("{} issue type for {}#{}", action, target_repo, issue_number),
            serde_json::json!({
                "number": issue_number,
                "target_repo": target_repo,
                "issue_type": resolved_type,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::{CreateIssueParams, ToolResult};
    use std::collections::HashMap;

    #[test]
    fn result_name_and_default_budget_match_contract() {
        assert_eq!(SetIssueTypeResult::NAME, "set-issue-type");
        assert_eq!(SetIssueTypeResult::DEFAULT_MAX, 5);
    }

    #[test]
    fn validates_numeric_and_temporary_targets() {
        assert!(
            SetIssueTypeParams {
                issue_number: GithubIssueNumber::Number(1),
                issue_type: "Bug".to_string(),
            }
            .validate()
            .is_ok()
        );
        assert!(
            SetIssueTypeParams {
                issue_number: GithubIssueNumber::Temporary(
                    GithubTemporaryId::parse("#aw_bug1").unwrap()
                ),
                issue_type: String::new(),
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn rejects_zero_issue_number() {
        let result = SetIssueTypeParams {
            issue_number: GithubIssueNumber::Number(0),
            issue_type: "Bug".to_string(),
        }
        .validate();
        assert!(result.is_err());
    }

    #[test]
    fn quoted_numeric_issue_number_deserializes_as_number() {
        let params: SetIssueTypeParams = serde_json::from_value(serde_json::json!({
            "issue_number": "42",
            "issue_type": "Bug"
        }))
        .unwrap();
        assert!(matches!(
            params.issue_number,
            GithubIssueNumber::Number(42)
        ));
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
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/42"))
            .and(body_json(serde_json::json!({ "type": "Bug" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "create-issue".to_string(),
            serde_json::json!({
                "target-repo": "octo/repo",
                "require-temporary-id": true
            }),
        );
        tool_configs.insert(
            "set-issue-type".to_string(),
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

        let mut create: crate::safe_outputs::CreateIssueResult = CreateIssueParams {
            title: "A real issue title".to_string(),
            body: "A detailed issue body that is long enough for validation.".to_string(),
            labels: vec![],
            assignees: vec![],
            temporary_id: Some(GithubTemporaryId::parse("#aw_bug1").unwrap()),
        }
        .try_into()
        .unwrap();
        let created = create.execute_sanitized(&ctx).await.unwrap();
        assert!(created.success, "create failed: {}", created.message);

        let mut duplicate: crate::safe_outputs::CreateIssueResult = CreateIssueParams {
            title: "A second real issue title".to_string(),
            body: "Another detailed issue body that is long enough for validation.".to_string(),
            labels: vec![],
            assignees: vec![],
            temporary_id: Some(GithubTemporaryId::parse("#aw_bug1").unwrap()),
        }
        .try_into()
        .unwrap();
        let duplicate_result = duplicate.execute_sanitized(&ctx).await.unwrap();
        assert!(!duplicate_result.success);
        assert!(duplicate_result.message.contains("already used"));

        let mut set_type: SetIssueTypeResult = SetIssueTypeParams {
            issue_number: GithubIssueNumber::Temporary(
                GithubTemporaryId::parse("aw_bug1").unwrap(),
            ),
            issue_type: "bug".to_string(),
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
            "set-issue-type".to_string(),
            serde_json::json!({"target-repo": "octo/repo"}),
        );
        let ctx = ExecutionContext {
            github_token: Some("token".to_string()),
            tool_configs,
            ..Default::default()
        };
        let mut result: SetIssueTypeResult = SetIssueTypeParams {
            issue_number: GithubIssueNumber::Temporary(
                GithubTemporaryId::parse("#aw_missing").unwrap(),
            ),
            issue_type: "Bug".to_string(),
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
        let mut result: SetIssueTypeResult = SetIssueTypeParams {
            issue_number: GithubIssueNumber::Number(1),
            issue_type: "Bug".to_string(),
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
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({ "type": "" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "set-issue-type".to_string(),
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
        let mut result: SetIssueTypeResult = SetIssueTypeParams {
            issue_number: GithubIssueNumber::Number(7),
            issue_type: String::new(),
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "clear failed: {}", execution.message);
    }
}
