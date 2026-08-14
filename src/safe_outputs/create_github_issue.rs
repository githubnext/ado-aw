//! `create-github-issue` safe output.
//!
//! Files a GitHub issue against an operator-configured target repository.
//! Stage 3 authenticates with the credential exposed through
//! [`ExecutionContext::github_token`]; Agent and Detection never see it.
//!
//! Notable design points:
//! * Agent repository selection is bounded by exact operator-configured
//!   `target-repo` and `allowed-repos` entries.
//! * Labels are merged from a static operator-configured list and an
//!   agent-supplied list. Agent labels are validated against `allowed-labels`
//!   (wildcard-aware via [`crate::safe_outputs::tag_matches_pattern`]).
//! * Assignees are merged the same way without an allowlist gate (out of
//!   scope for v1).

use anyhow::ensure;
use log::{debug, info};
use reqwest::Method;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, GithubClient, GithubRepositoryPolicy, Validate,
    build_github_trace_footer, merge_github_values, resolve_github_repository,
    validate_github_repository,
};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::secure::GithubTemporaryId;
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

/// Parameters the agent supplies when calling the `create-github-issue` MCP tool.
#[derive(Deserialize, JsonSchema)]
pub struct CreateGithubIssueParams {
    /// Concise issue title summarizing the bug, feature, or task.
    pub title: String,

    /// Detailed issue description in Markdown.
    pub body: String,

    /// Labels to apply to the issue. Subject to the operator-configured
    /// `allowed-labels` allowlist.
    #[serde(default)]
    pub labels: Vec<String>,

    /// GitHub usernames to assign to the issue.
    #[serde(default)]
    pub assignees: Vec<String>,

    /// Optional target repository. Must exactly match `target-repo` or an
    /// `allowed-repos` entry.
    #[serde(default)]
    pub repository: Option<String>,

    /// Temporary identifier used by later safe outputs in the same run.
    #[serde(default)]
    pub temporary_id: Option<GithubTemporaryId>,
}

impl Validate for CreateGithubIssueParams {
    fn validate(&self) -> anyhow::Result<()> {
        // Note: length checks are byte-based (`str::len()`), which is acceptable
        // here because limits are defensive bounds rather than user-facing quotas.
        ensure!(self.title.len() >= 5, "title must be at least 5 characters");
        ensure!(self.body.len() >= 30, "body must be at least 30 characters");
        ensure!(
            self.title.len() <= 256,
            "title must be 256 characters or fewer"
        );
        for label in &self.labels {
            ensure!(!label.is_empty(), "label must not be empty");
            reject_pipeline_injection(label, "create-github-issue.label")?;
        }
        for assignee in &self.assignees {
            ensure!(!assignee.is_empty(), "assignee must not be empty");
            reject_pipeline_injection(assignee, "create-github-issue.assignee")?;
        }
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        Ok(())
    }
}

tool_result! {
    name = "create-github-issue",
    write = true,
    params = CreateGithubIssueParams,
    /// Result of filing a GitHub issue.
    pub struct CreateGithubIssueResult {
        title: String,
        body: String,
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default)]
        assignees: Vec<String>,
        #[serde(default)]
        repository: Option<String>,
        #[serde(default)]
        temporary_id: Option<GithubTemporaryId>,
    }
}

impl SanitizeContent for CreateGithubIssueResult {
    fn sanitize_content_fields(&mut self) {
        self.title = sanitize_text(&self.title);
        self.body = sanitize_text(&self.body);
        for label in &mut self.labels {
            *label = label.chars().filter(|c| !c.is_control()).collect();
        }
        for assignee in &mut self.assignees {
            *assignee = assignee.chars().filter(|c| !c.is_control()).collect();
        }
        self.repository = self
            .repository
            .as_deref()
            .map(crate::sanitize::sanitize_config);
    }
}

/// Operator-side configuration for `safe-outputs.create-github-issue`.
#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGithubIssueConfig {
    /// Target GitHub repository in `owner/repo` form. When omitted, Stage 3
    /// resolves the current repository only for GitHub-backed ADO builds.
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,

    /// Additional exact repositories the agent may select.
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,

    /// Optional prefix prepended to every agent-supplied title (e.g.
    /// `"[pipeline-failure] "`).
    #[serde(default, rename = "title-prefix")]
    pub title_prefix: Option<String>,

    /// Static labels always applied to the issue regardless of agent input.
    #[serde(default)]
    pub labels: Vec<String>,

    /// Allowlist for agent-supplied labels.
    ///
    /// **Default-deny semantics**: an empty/absent list means **no
    /// agent-supplied labels are accepted**. To accept any agent label,
    /// set `allowed-labels: ["*"]` explicitly. Patterns may include `*`
    /// wildcards (e.g. `"agent-*"`).
    #[serde(default, rename = "allowed-labels")]
    pub allowed_labels: Vec<String>,

    /// Static assignees always added regardless of agent input.
    #[serde(default)]
    pub assignees: Vec<String>,

    /// Require every proposal to include a temporary ID.
    #[serde(default, rename = "require-temporary-id")]
    #[sanitize_config(skip)]
    pub require_temporary_id: bool,

    /// Per-run budget (max number of issues filed). Read by the generic
    /// budget machinery in `crate::execute`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

/// Sentinel pattern in `allowed-labels` that opts out of the default-deny
/// behaviour and admits any agent-supplied label.
const ALLOWED_LABELS_ANY: &str = "*";

/// Maximum length of the **final** issue title, after `title-prefix` is
/// applied. GitHub itself accepts up to 256 characters; we mirror the
/// agent-side `Validate` limit so a long prefix can't trick us into
/// hitting the API with an over-long string.
const MAX_FINAL_TITLE_LEN: usize = 256;

#[async_trait::async_trait]
impl Executor for CreateGithubIssueResult {
    fn dry_run_summary(&self) -> String {
        format!("create GitHub issue: '{}'", self.title)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Filing GitHub issue: '{}' ({} chars body)",
            self.title,
            self.body.len()
        );

        if !ctx.tool_configs.contains_key("create-github-issue") {
            return Ok(ExecutionResult::failure(
                "create-github-issue is not configured for this workflow",
            ));
        }

        let token = match ctx.github_token.as_ref() {
            Some(t) => t,
            None => {
                return Ok(ExecutionResult::failure(
                    "ADO_AW_GITHUB_TOKEN is not set; configure safe-outputs.github-token \
                     or safe-outputs.github-app",
                ));
            }
        };

        let config: CreateGithubIssueConfig = ctx.get_tool_config("create-github-issue")?;
        if config.require_temporary_id && self.temporary_id.is_none() {
            return Ok(ExecutionResult::failure(
                "create-github-issue requires temporary_id because \
                 safe-outputs.create-github-issue.require-temporary-id is true",
            ));
        }
        let target_repo = match resolve_github_repository(
            self.repository.as_deref(),
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos),
            ctx,
        ) {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        debug!("create-github-issue: target-repo={target_repo}");
        if let Some(temporary_id) = &self.temporary_id
            && ctx.has_resolved_github_issue(temporary_id)?
        {
            return Ok(ExecutionResult::failure(format!(
                "temporary_id '{}' was already used in this run",
                temporary_id.canonical()
            )));
        }

        // Validate agent-supplied labels against allowed-labels.
        // Default-deny semantics: an empty list means NO agent labels are
        // accepted. Operators must opt in to unrestricted by setting
        // `allowed-labels: ["*"]`. Static labels under `labels:` are always
        // applied regardless.
        if !self.labels.is_empty() {
            let allow_any = config
                .allowed_labels
                .iter()
                .any(|p| p == ALLOWED_LABELS_ANY);
            if !allow_any {
                let disallowed: Vec<String> = self
                    .labels
                    .iter()
                    .filter(|label| {
                        !config
                            .allowed_labels
                            .iter()
                            .any(|pattern| super::tag_matches_pattern(label, pattern))
                    })
                    .map(|label| {
                        // Neutralise pipeline-command sequences before we
                        // echo agent-supplied content into our own log line
                        // and the failure message.
                        crate::sanitize::neutralize_pipeline_commands(label)
                    })
                    .collect();
                if !disallowed.is_empty() {
                    let msg = if config.allowed_labels.is_empty() {
                        format!(
                            "Agent-supplied labels rejected (no `allowed-labels` configured; \
                             set `allowed-labels: [\"*\"]` to permit any): {}",
                            disallowed.join(", ")
                        )
                    } else {
                        format!(
                            "Agent-supplied labels not in allowed-labels: {}",
                            disallowed.join(", ")
                        )
                    };
                    return Ok(ExecutionResult::failure(msg));
                }
            }
        }

        let final_title = match &config.title_prefix {
            Some(prefix) => format!("{}{}", prefix, self.title),
            None => self.title.clone(),
        };
        if final_title.len() > MAX_FINAL_TITLE_LEN {
            return Ok(ExecutionResult::failure(format!(
                "Final issue title exceeds {MAX_FINAL_TITLE_LEN} characters \
                 ({} chars after applying title-prefix). Shorten title-prefix \
                 or the agent title.",
                final_title.len()
            )));
        }
        let body_with_footer = format!("{}\n\n{}", self.body, build_github_trace_footer(ctx));
        let all_labels = merge_github_values(&config.labels, &self.labels);
        let all_assignees = merge_github_values(&config.assignees, &self.assignees);

        let client = GithubClient::new(&ctx.github_api_url, token)?;
        let url = client.issues_url(&target_repo)?;
        debug!("POSTing to {}", url);

        let payload = serde_json::json!({
            "title": final_title,
            "body": body_with_footer,
            "labels": all_labels,
            "assignees": all_assignees,
        });

        let response = client.send(Method::POST, url, Some(&payload)).await?;

        let status = response.status;
        if status.is_success() {
            let body: serde_json::Value = response
                .json("Failed to parse GitHub API response")
                .map_err(anyhow::Error::new)?;
            let Some(number) = body
                .get("number")
                .and_then(|v| v.as_u64())
                .filter(|number| *number > 0)
            else {
                return Ok(ExecutionResult::failure(
                    "GitHub create-github-issue response contained no positive issue number",
                ));
            };
            let html_url = body
                .get("html_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            info!(
                "Filed GitHub issue {}#{}: {}",
                target_repo, number, html_url
            );
            if let Some(temporary_id) = &self.temporary_id
                && let Err(error) = ctx.register_resolved_github_issue(
                    temporary_id,
                    crate::safe_outputs::ResolvedGithubIssue {
                        repository: target_repo.clone(),
                        number,
                        url: html_url.clone(),
                    },
                )
            {
                return Ok(ExecutionResult::failure_with_data(
                    format!(
                        "Filed issue {}#{} but failed to register temporary_id '{}': {}",
                        target_repo,
                        number,
                        temporary_id.canonical(),
                        crate::sanitize::neutralize_pipeline_commands(&error.to_string())
                    ),
                    serde_json::json!({
                        "number": number,
                        "url": html_url,
                        "target_repo": target_repo,
                        "temporary_id": temporary_id.canonical(),
                    }),
                ));
            }
            Ok(ExecutionResult::success_with_data(
                format!("Filed issue {}#{}: {}", target_repo, number, html_url),
                serde_json::json!({
                    "number": number,
                    "url": html_url,
                    "target_repo": target_repo,
                    "temporary_id": self.temporary_id.as_ref().map(GithubTemporaryId::canonical),
                }),
            ))
        } else {
            let error = response
                .require_success("Failed to file GitHub issue")
                .expect_err("non-success response must produce an API error");
            Ok(ExecutionResult::failure(error.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::{ToolResult, resolve_target_repo, validate_target_repo};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn ctx_with_config(
        config: serde_json::Value,
        github_token: Option<String>,
    ) -> ExecutionContext {
        let mut tool_configs: HashMap<String, serde_json::Value> = HashMap::new();
        tool_configs.insert("create-github-issue".to_string(), config);
        ExecutionContext {
            github_token,
            tool_configs,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            ..Default::default()
        }
    }

    fn valid_params() -> CreateGithubIssueParams {
        CreateGithubIssueParams {
            title: "Pipeline failure on main".to_string(),
            body: "The agent step failed during stage 1 with a network timeout.".to_string(),
            labels: vec![],
            assignees: vec![],
            repository: None,
            temporary_id: None,
        }
    }

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(CreateGithubIssueResult::NAME, "create-github-issue");
    }

    #[test]
    fn test_validate_rejects_short_title() {
        let params = CreateGithubIssueParams {
            title: "Hi".to_string(),
            ..valid_params()
        };
        assert!(<CreateGithubIssueParams as Validate>::validate(&params).is_err());
    }

    #[test]
    fn test_validate_rejects_short_body() {
        let params = CreateGithubIssueParams {
            body: "too short".to_string(),
            ..valid_params()
        };
        assert!(<CreateGithubIssueParams as Validate>::validate(&params).is_err());
    }

    #[test]
    fn test_validate_rejects_pipeline_injection_in_label() {
        let params = CreateGithubIssueParams {
            labels: vec!["##vso[task.complete]".to_string()],
            ..valid_params()
        };
        assert!(<CreateGithubIssueParams as Validate>::validate(&params).is_err());
    }

    #[test]
    fn test_validate_rejects_pipeline_injection_in_assignee() {
        let params = CreateGithubIssueParams {
            assignees: vec!["$(SYSTEM_ACCESSTOKEN)".to_string()],
            ..valid_params()
        };
        assert!(<CreateGithubIssueParams as Validate>::validate(&params).is_err());
    }

    #[test]
    fn test_validate_rejects_malformed_repository() {
        let params = CreateGithubIssueParams {
            repository: Some("octo/$(TOKEN)".to_string()),
            ..valid_params()
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_sanitize_strips_control_chars() {
        let mut result = CreateGithubIssueResult {
            name: "create-github-issue".to_string(),
            title: "ok\u{0007}title".to_string(),
            body: "body\u{0008}with\u{0001}ctl chars (more than 30 characters total)".to_string(),
            labels: vec!["la\u{0007}bel".to_string()],
            assignees: vec!["jo\u{0008}hn".to_string()],
            repository: Some("octo/repo".to_string()),
            temporary_id: None,
        };
        result.sanitize_content_fields();
        assert!(!result.title.contains('\u{0007}'));
        assert!(!result.body.contains('\u{0008}'));
        assert!(!result.body.contains('\u{0001}'));
        assert!(!result.labels[0].contains('\u{0007}'));
        assert!(!result.assignees[0].contains('\u{0008}'));
    }

    #[test]
    fn test_dry_run_summary_format() {
        let result = CreateGithubIssueResult {
            name: "create-github-issue".to_string(),
            title: "Fix the build".to_string(),
            body: "anything".to_string(),
            labels: vec![],
            assignees: vec![],
            repository: None,
            temporary_id: None,
        };
        assert_eq!(
            result.dry_run_summary(),
            "create GitHub issue: 'Fix the build'"
        );
    }

    #[test]
    fn test_target_repo_regex_accepts_canonical_forms() {
        assert!(validate_target_repo("githubnext/ado-aw").is_ok());
        assert!(validate_target_repo("a/b").is_ok());
        assert!(validate_target_repo("My-Org/some.repo-here").is_ok());
        // Repo segment may include dots/underscores; owner segment may not.
        assert!(validate_target_repo("user/repo_with_underscore").is_ok());
        assert!(validate_target_repo("user/.github").is_ok());
    }

    #[test]
    fn test_target_repo_regex_rejects_bad_forms() {
        assert!(validate_target_repo("").is_err());
        assert!(validate_target_repo("bare-name").is_err());
        assert!(validate_target_repo("a/b/c").is_err());
        assert!(validate_target_repo("/repo").is_err());
        assert!(validate_target_repo("owner/").is_err());
        assert!(validate_target_repo("-leading/repo").is_err());
        assert!(validate_target_repo("trailing-/repo").is_err());
        // GitHub does not admit dots or underscores in owner logins.
        assert!(validate_target_repo("Acme.Inc/repo").is_err());
        assert!(validate_target_repo("under_score/repo").is_err());
        // Repo segment alone may not be `.` or `..`.
        assert!(validate_target_repo("owner/.").is_err());
        assert!(validate_target_repo("owner/..").is_err());
    }

    #[test]
    fn resolve_target_repo_uses_current_github_source() {
        let ctx = ExecutionContext {
            repository_provider: Some("GitHub".to_string()),
            repository_name: Some("octo/repo".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_target_repo(None, &ctx).unwrap(), "octo/repo");
    }

    #[test]
    fn resolve_target_repo_rejects_azure_repos_default() {
        let ctx = ExecutionContext {
            repository_provider: Some("TfsGit".to_string()),
            repository_name: Some("repo".to_string()),
            ..Default::default()
        };
        let error = resolve_target_repo(None, &ctx).unwrap_err();
        assert!(error.message.contains("target-repo is required"));
    }

    #[test]
    fn resolve_target_repo_rejects_ghe_source_left_on_dotcom_api_url() {
        // A GitHub Enterprise pipeline source whose API URL was never overridden
        // still points at api.github.com, so implicitly resolving the current
        // repository would file the issue on the wrong host entirely.
        let ctx = ExecutionContext {
            repository_provider: Some("GitHubEnterprise".to_string()),
            repository_name: Some("octo/repo".to_string()),
            github_api_url: "https://api.github.com".to_string(),
            ..Default::default()
        };
        let error = resolve_target_repo(None, &ctx).unwrap_err();
        assert!(
            error.message.contains("GitHub Enterprise source"),
            "unexpected message: {}",
            error.message
        );
    }

    #[test]
    fn resolve_target_repo_accepts_ghe_source_with_explicit_api_url() {
        // The positive counterpart: once the GHE API URL is set, implicit
        // resolution of the current repository is allowed again.
        let ctx = ExecutionContext {
            repository_provider: Some("GitHubEnterprise".to_string()),
            repository_name: Some("octo/repo".to_string()),
            github_api_url: "https://ghe.example.com/api/v3".to_string(),
            ..Default::default()
        };
        assert_eq!(resolve_target_repo(None, &ctx).unwrap(), "octo/repo");
    }

    #[test]
    fn resolve_target_repo_rejects_missing_build_repository_name() {
        // GitHub-backed build, but ADO did not surface BUILD_REPOSITORY_NAME.
        // There is nothing safe to fall back to, so this must fail rather than
        // resolve to an empty or guessed slug.
        let ctx = ExecutionContext {
            repository_provider: Some("GitHub".to_string()),
            repository_name: None,
            ..Default::default()
        };
        let error = resolve_target_repo(None, &ctx).unwrap_err();
        assert!(
            error.message.contains("BUILD_REPOSITORY_NAME is not set"),
            "unexpected message: {}",
            error.message
        );
    }

    /// Config shape errors fail during strict deserialization, while the
    /// repository slug itself is validated before any request is sent.
    /// `test_execute_fails_when_target_repo_invalid` covers the plain
    /// rejection; this pins the *no redirect* half, with a usable current
    /// repository deliberately present in the context.
    #[tokio::test]
    async fn malformed_target_repo_does_not_redirect_to_current_repository() {
        let mut ctx = ctx_with_config(
            serde_json::json!({ "target-repo": "not a valid slug" }),
            Some("fake-pat".to_string()),
        );
        ctx.repository_provider = Some("GitHub".to_string());
        ctx.repository_name = Some("octo/current".to_string());

        let mut result: CreateGithubIssueResult = valid_params().try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(
            execution.message.contains("target-repo"),
            "expected an explicit target-repo rejection, got: {}",
            execution.message
        );
        assert!(
            !execution.message.contains("octo/current"),
            "must not silently fall back to the current repository: {}",
            execution.message
        );
    }

    #[test]
    fn test_merge_dedup_strings_dedupes_case_insensitively() {
        let merged = merge_github_values(
            &["bug".into(), "Triage".into()],
            &["BUG".into(), "fresh".into()],
        );
        assert_eq!(
            merged,
            vec!["bug".to_string(), "Triage".to_string(), "fresh".to_string()]
        );
    }

    #[tokio::test]
    async fn test_execute_fails_when_github_token_missing() {
        let params = valid_params();
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({"target-repo": "githubnext/ado-aw"}),
            None,
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(
            exec.message.contains("ADO_AW_GITHUB_TOKEN"),
            "expected ADO_AW_GITHUB_TOKEN message, got: {}",
            exec.message
        );
    }

    #[tokio::test]
    async fn test_execute_rejects_when_tool_not_configured() {
        let params = valid_params();
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ExecutionContext {
            github_token: Some("token-that-must-not-be-used".to_string()),
            ..Default::default()
        };
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(exec.message.contains("not configured"));
    }

    #[tokio::test]
    async fn test_execute_fails_when_target_repo_invalid() {
        let params = valid_params();
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({"target-repo": "not-a-valid-repo"}),
            Some("fake-pat".to_string()),
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(
            exec.message.contains("target-repo"),
            "expected target-repo error, got: {}",
            exec.message
        );
    }

    #[tokio::test]
    async fn test_execute_requires_temporary_id_when_configured() {
        let params = valid_params();
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({
                "target-repo": "githubnext/ado-aw",
                "require-temporary-id": true
            }),
            Some("fake-pat".to_string()),
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(exec.message.contains("requires temporary_id"));
    }

    #[tokio::test]
    async fn target_configuration_error_precedes_duplicate_temporary_id() {
        let temporary_id = GithubTemporaryId::parse("#aw_dup1").unwrap();
        let mut params = valid_params();
        params.temporary_id = Some(temporary_id.clone());
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({"require-temporary-id": true}),
            Some("token".to_string()),
        );
        ctx.register_resolved_github_issue(
            &temporary_id,
            crate::safe_outputs::ResolvedGithubIssue {
                repository: "octo/repo".to_string(),
                number: 1,
                url: "https://github.com/octo/repo/issues/1".to_string(),
            },
        )
        .unwrap();

        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(exec.message.contains("target-repo is required"));
        assert!(!exec.message.contains("already used"));
    }

    #[tokio::test]
    async fn test_execute_rejects_disallowed_label() {
        let params = CreateGithubIssueParams {
            labels: vec!["manual".to_string()],
            ..valid_params()
        };
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({
                "target-repo": "githubnext/ado-aw",
                "allowed-labels": ["agent-*", "automated"]
            }),
            Some("fake-pat".to_string()),
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(
            exec.message.contains("not in allowed-labels"),
            "expected allowed-labels error, got: {}",
            exec.message
        );
    }

    #[tokio::test]
    async fn test_execute_accepts_label_matching_wildcard() {
        let params = CreateGithubIssueParams {
            labels: vec!["agent-created".to_string()],
            ..valid_params()
        };
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({
                "target-repo": "githubnext/ado-aw",
                "allowed-labels": ["agent-*"]
            }),
            Some("fake-pat".to_string()),
        );
        // The HTTP call will fail (no real network in CI), but we assert that
        // failure is NOT the policy-rejection message — i.e., wildcard match
        // passed.
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        if !exec.success {
            assert!(
                !exec.message.contains("not in allowed-labels"),
                "expected wildcard match to pass, got policy rejection: {}",
                exec.message
            );
        }
    }

    #[tokio::test]
    async fn test_execute_rejects_agent_label_when_allowed_labels_empty() {
        // Default-deny: empty allowed-labels means no agent labels allowed.
        let params = CreateGithubIssueParams {
            labels: vec!["bug".to_string()],
            ..valid_params()
        };
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({"target-repo": "githubnext/ado-aw"}),
            Some("fake-pat".to_string()),
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(
            exec.message.contains("no `allowed-labels` configured"),
            "expected default-deny message, got: {}",
            exec.message
        );
    }

    #[tokio::test]
    async fn test_execute_accepts_any_agent_label_with_star_allowlist() {
        let params = CreateGithubIssueParams {
            labels: vec!["arbitrary-label".to_string()],
            ..valid_params()
        };
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({
                "target-repo": "githubnext/ado-aw",
                "allowed-labels": ["*"]
            }),
            Some("fake-pat".to_string()),
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        // Network call will fail; ensure that failure is NOT the policy
        // rejection — the `*` allowlist must let arbitrary labels through.
        if !exec.success {
            assert!(
                !exec.message.contains("allowed-labels"),
                "expected `*` to bypass the allowlist, got policy rejection: {}",
                exec.message
            );
        }
    }

    #[tokio::test]
    async fn test_execute_rejects_overlong_final_title_after_prefix() {
        let long_prefix = "X".repeat(250);
        let params = CreateGithubIssueParams {
            title: "valid title here".to_string(),
            ..valid_params()
        };
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({
                "target-repo": "githubnext/ado-aw",
                "title-prefix": long_prefix,
            }),
            Some("fake-pat".to_string()),
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(
            exec.message.contains("Final issue title"),
            "expected length error, got: {}",
            exec.message
        );
    }

    #[tokio::test]
    async fn agent_repository_must_be_an_exact_allowed_repo() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/allowed/issues"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "number": 12,
                "html_url": "https://github.example/octo/allowed/issues/12"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = ctx_with_config(
            serde_json::json!({
                "target-repo": "octo/default",
                "allowed-repos": ["octo/allowed"]
            }),
            Some("token".to_string()),
        );
        ctx.github_api_url = server.uri();
        let mut params = valid_params();
        params.repository = Some("OCTO/ALLOWED".to_string());
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "create failed: {}", execution.message);
        assert_eq!(
            execution
                .data
                .as_ref()
                .and_then(|data| data["target_repo"].as_str()),
            Some("octo/allowed")
        );
    }

    #[tokio::test]
    async fn denied_agent_repository_fails_before_http() {
        use wiremock::MockServer;

        let server = MockServer::start().await;
        let mut ctx = ctx_with_config(
            serde_json::json!({
                "target-repo": "octo/default",
                "allowed-repos": ["octo/allowed"]
            }),
            Some("token".to_string()),
        );
        ctx.github_api_url = server.uri();
        let mut params = valid_params();
        params.repository = Some("octo/denied".to_string());
        let mut result: CreateGithubIssueResult = params.try_into().unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("not an exact"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_execute_neutralizes_pipeline_command_in_label_error() {
        // Even though Validate would reject this label up front, Stage 3
        // deserialises directly from NDJSON — so a forged payload could
        // contain ##vso[...] in labels. The error message must neutralise
        // these sequences so they can't act as live ADO pipeline commands
        // when the message is echoed to stdout.
        let mut result = CreateGithubIssueResult {
            name: "create-github-issue".to_string(),
            title: "Pipeline failure on main".to_string(),
            body: "This is a sufficiently long body for the issue parameters.".to_string(),
            labels: vec!["##vso[task.complete]".to_string()],
            assignees: vec![],
            repository: None,
            temporary_id: None,
        };
        let ctx = ctx_with_config(
            serde_json::json!({
                "target-repo": "githubnext/ado-aw",
                "allowed-labels": ["agent-*"]
            }),
            Some("fake-pat".to_string()),
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        // The neutraliser wraps `##vso[` in backticks so ADO's line-prefix
        // parser ignores it. A live command would appear at the start of a
        // line; after neutralisation, every `##vso[` instance must be
        // preceded by a backtick.
        for line in exec.message.lines() {
            assert!(
                !line.starts_with("##vso["),
                "live pipeline command at start of line: {}",
                line
            );
        }
        // And every occurrence of `##vso[` should be wrapped in backticks
        // (the neutraliser's signature).
        if exec.message.contains("##vso[") {
            assert!(
                exec.message.contains("`##vso[`"),
                "expected neutralised `##vso[` form, got: {}",
                exec.message
            );
        }
    }

    #[test]
    fn test_config_round_trips_kebab_case() {
        let yaml = r#"
target-repo: githubnext/ado-aw
allowed-repos: [githubnext/other]
title-prefix: "[bug] "
labels: [a]
allowed-labels: ["agent-*"]
assignees: [u1]
"#;
        let cfg: CreateGithubIssueConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.target_repo.as_deref(), Some("githubnext/ado-aw"));
        assert_eq!(cfg.allowed_repos, vec!["githubnext/other".to_string()]);
        assert_eq!(cfg.title_prefix.as_deref(), Some("[bug] "));
        assert_eq!(cfg.labels, vec!["a".to_string()]);
        assert_eq!(cfg.allowed_labels, vec!["agent-*".to_string()]);
        assert_eq!(cfg.assignees, vec!["u1".to_string()]);
    }

    #[test]
    fn test_config_rejects_unknown_fields() {
        let yaml = r#"
target-repo: githubnext/ado-aw
unexpected: oops
"#;
        let result: Result<CreateGithubIssueConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            result.is_err(),
            "deny_unknown_fields should reject unexpected key"
        );
    }

    #[test]
    fn test_footer_includes_marker() {
        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/myorg".to_string()),
            ado_project: Some("MyProject".to_string()),
            build_id: Some(42),
            definition_name: Some("dogfood".to_string()),
            build_reason: Some("Manual".to_string()),
            ..Default::default()
        };
        let footer = build_github_trace_footer(&ctx);
        assert!(footer.contains("<!-- ado-aw -->"));
        assert!(footer.contains("buildId=42"));
        assert!(footer.contains("dogfood"));
    }
}
