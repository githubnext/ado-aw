//! Debug-only `create-issue` safe output.
//!
//! Files a GitHub issue against an operator-configured target repository.
//! This is **not** a regular safe output — it is gated entirely by the
//! `ado-aw-debug.create-issue` front-matter section and stripped from the
//! SafeOutputs MCP server unless explicitly enabled (see
//! [`crate::safeoutputs::DEBUG_ONLY_TOOLS`]).
//!
//! Intended use: dogfood pipelines compiled from `githubnext/ado-aw` that need
//! to file failure reports back to GitHub for triage. Stage 3 authenticates
//! with a dedicated PAT exposed via the `ADO_AW_DEBUG_GITHUB_TOKEN` pipeline
//! variable and surfaced through [`ExecutionContext::github_token`].
//!
//! Notable design points:
//! * `target-repo` is operator-only — the agent never supplies it and cannot
//!   redirect issues to a different repo.
//! * Labels are merged from a static operator-configured list and an
//!   agent-supplied list. Agent labels are validated against `allowed-labels`
//!   (wildcard-aware via [`crate::safeoutputs::tag_matches_pattern`]).
//! * Assignees are merged the same way without an allowlist gate (out of
//!   scope for v1).

use anyhow::{Context, ensure};
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use regex_lite::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use super::PATH_SEGMENT;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

/// Parameters the agent supplies when calling the `create-issue` MCP tool.
#[derive(Deserialize, JsonSchema)]
pub struct CreateIssueParams {
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
}

impl Validate for CreateIssueParams {
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
            reject_pipeline_injection(label, "create-issue.label")?;
        }
        for assignee in &self.assignees {
            ensure!(!assignee.is_empty(), "assignee must not be empty");
            reject_pipeline_injection(assignee, "create-issue.assignee")?;
        }
        Ok(())
    }
}

tool_result! {
    name = "create-issue",
    write = true,
    params = CreateIssueParams,
    /// Result of filing a GitHub issue.
    pub struct CreateIssueResult {
        title: String,
        body: String,
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default)]
        assignees: Vec<String>,
    }
}

impl SanitizeContent for CreateIssueResult {
    fn sanitize_content_fields(&mut self) {
        self.title = sanitize_text(&self.title);
        self.body = sanitize_text(&self.body);
        for label in &mut self.labels {
            *label = label.chars().filter(|c| !c.is_control()).collect();
        }
        for assignee in &mut self.assignees {
            *assignee = assignee.chars().filter(|c| !c.is_control()).collect();
        }
    }
}

/// Operator-side configuration for `ado-aw-debug.create-issue`.
///
/// Lives under `ado-aw-debug:` rather than `safe-outputs:` to keep the tool
/// out of the regular safe-output surface.
#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateIssueConfig {
    /// Required: target GitHub repository in `owner/repo` form.
    #[serde(rename = "target-repo")]
    pub target_repo: String,

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

    /// Per-run budget (max number of issues filed). Read by the generic
    /// budget machinery in `crate::execute`. Stored here so
    /// `deny_unknown_fields` accepts it under `ado-aw-debug.create-issue`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

/// Compiled regex for `target-repo` validation.
///
/// GitHub repo references take the form `owner/repo`. Owner segments
/// (logins of users or organisations) admit alphanumerics and hyphens
/// and must not start or end with a hyphen. Repository segments admit
/// alphanumerics, hyphens, dots, and underscores and must not be `.`
/// or `..`. We intentionally reject underscores and dots in the owner
/// because GitHub does too.
fn target_repo_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?/[A-Za-z0-9._-]+$")
            .expect("target_repo regex is well-formed")
    })
}

/// Validate that `target-repo` is shaped like `owner/repo`.
pub(crate) fn validate_target_repo(target_repo: &str) -> anyhow::Result<()> {
    ensure!(
        !target_repo.is_empty(),
        "ado-aw-debug.create-issue.target-repo is required (expected 'owner/repo')"
    );
    ensure!(
        target_repo_regex().is_match(target_repo),
        "ado-aw-debug.create-issue.target-repo '{}' is not in 'owner/repo' format \
         (owner: alphanumerics/hyphens; repo: alphanumerics/dots/hyphens/underscores)",
        target_repo
    );
    if let Some((_owner, repo)) = target_repo.split_once('/') {
        ensure!(
            repo != "." && repo != "..",
            "ado-aw-debug.create-issue.target-repo repo segment must not be '.' or '..'"
        );
    }
    Ok(())
}

/// Build the auto-appended traceability footer.
///
/// Embeds a stable `<!-- ado-aw -->` marker so future tooling can locate
/// generated content without reflowing the body.
fn build_footer(ctx: &ExecutionContext) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("<!-- ado-aw -->".to_string());
    lines.push("---".to_string());
    if let Some(name) = ctx.definition_name.as_ref() {
        lines.push(format!("Pipeline: `{name}`"));
    }
    if let Some(build_id) = ctx.build_id {
        if let (Some(org_url), Some(project)) = (ctx.ado_org_url.as_ref(), ctx.ado_project.as_ref())
        {
            let url = format!(
                "{}/{}/_build/results?buildId={}",
                org_url.trim_end_matches('/'),
                project,
                build_id
            );
            lines.push(format!("Run: <{url}>"));
        } else {
            lines.push(format!("Build: {build_id}"));
        }
    }
    if let Some(reason) = ctx.build_reason.as_ref() {
        lines.push(format!("Trigger: `{reason}`"));
    }
    lines.join("\n")
}

/// Merge static + agent-supplied strings (case-insensitive dedupe).
fn merge_dedup_strings(static_items: &[String], agent_items: &[String]) -> Vec<String> {
    let mut all = static_items.to_vec();
    for item in agent_items {
        if !all
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(item))
        {
            all.push(item.clone());
        }
    }
    all
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
impl Executor for CreateIssueResult {
    fn dry_run_summary(&self) -> String {
        format!("create GitHub issue: '{}'", self.title)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Filing GitHub issue: '{}' ({} chars body)",
            self.title,
            self.body.len()
        );

        // SECURITY GATE: independently of the SafeOutputs MCP filter, refuse
        // to act on a `create-issue` NDJSON entry unless the operator
        // authorised it via the `ado-aw-debug.create-issue` front-matter
        // section. This closes the gap where a forged NDJSON entry (or a
        // mis-placed `safe-outputs.create-issue` config) could otherwise
        // bypass the MCP-layer default-deny.
        if !ctx
            .debug_enabled_tools
            .contains(<Self as crate::safeoutputs::ToolResult>::NAME)
        {
            return Ok(ExecutionResult::failure(
                "create-issue is a debug-only tool and is not enabled for this \
                 pipeline. Configure `ado-aw-debug.create-issue` in front matter \
                 to authorise it.",
            ));
        }

        let token = match ctx.github_token.as_ref() {
            Some(t) => t,
            None => {
                return Ok(ExecutionResult::failure(
                    "ADO_AW_DEBUG_GITHUB_TOKEN not set; required by ado-aw-debug.create-issue",
                ));
            }
        };

        let config: CreateIssueConfig = ctx.get_tool_config("create-issue");
        debug!("create-issue: target-repo={}", config.target_repo);

        if let Err(e) = validate_target_repo(&config.target_repo) {
            return Ok(ExecutionResult::failure(e.to_string()));
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
        let body_with_footer = format!("{}\n\n{}", self.body, build_footer(ctx));
        let all_labels = merge_dedup_strings(&config.labels, &self.labels);
        let all_assignees = merge_dedup_strings(&config.assignees, &self.assignees);

        // Split target-repo only after validation.
        let (owner, repo) = config
            .target_repo
            .split_once('/')
            .context("target-repo must be 'owner/repo'")?;

        let url = format!(
            "https://api.github.com/repos/{}/{}/issues",
            utf8_percent_encode(owner, PATH_SEGMENT),
            utf8_percent_encode(repo, PATH_SEGMENT),
        );
        debug!("POSTing to {}", url);

        let payload = serde_json::json!({
            "title": final_title,
            "body": body_with_footer,
            "labels": all_labels,
            "assignees": all_assignees,
        });

        let user_agent = format!("ado-aw/{}", env!("CARGO_PKG_VERSION"));
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", user_agent)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .context("Failed to send request to GitHub API")?;

        let status = response.status();
        if status.is_success() {
            let body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse GitHub API response")?;
            let number = body.get("number").and_then(|v| v.as_i64()).unwrap_or(0);
            let html_url = body
                .get("html_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            info!(
                "Filed GitHub issue {}#{}: {}",
                config.target_repo, number, html_url
            );
            Ok(ExecutionResult::success_with_data(
                format!(
                    "Filed issue {}#{}: {}",
                    config.target_repo, number, html_url
                ),
                serde_json::json!({
                    "number": number,
                    "url": html_url,
                    "target_repo": config.target_repo,
                }),
            ))
        } else {
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read response body>".to_string());
            Ok(ExecutionResult::failure(format!(
                "Failed to file GitHub issue (HTTP {}): {}",
                status, body_text
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn ctx_with_config(
        config: serde_json::Value,
        github_token: Option<String>,
    ) -> ExecutionContext {
        let mut tool_configs: HashMap<String, serde_json::Value> = HashMap::new();
        tool_configs.insert("create-issue".to_string(), config);
        let mut debug_enabled_tools = std::collections::HashSet::new();
        debug_enabled_tools.insert("create-issue".to_string());
        ExecutionContext {
            github_token,
            tool_configs,
            debug_enabled_tools,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            ..Default::default()
        }
    }

    /// Build a context that mirrors a forged-NDJSON scenario: tool config is
    /// present, but the operator never authorised the debug tool via
    /// `ado-aw-debug.create-issue`, so `debug_enabled_tools` is empty.
    fn ctx_unauthorized(
        config: serde_json::Value,
        github_token: Option<String>,
    ) -> ExecutionContext {
        let mut tool_configs: HashMap<String, serde_json::Value> = HashMap::new();
        tool_configs.insert("create-issue".to_string(), config);
        ExecutionContext {
            github_token,
            tool_configs,
            debug_enabled_tools: std::collections::HashSet::new(),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            ..Default::default()
        }
    }

    fn valid_params() -> CreateIssueParams {
        CreateIssueParams {
            title: "Pipeline failure on main".to_string(),
            body: "The agent step failed during stage 1 with a network timeout.".to_string(),
            labels: vec![],
            assignees: vec![],
        }
    }

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(CreateIssueResult::NAME, "create-issue");
    }

    #[test]
    fn test_validate_rejects_short_title() {
        let params = CreateIssueParams {
            title: "Hi".to_string(),
            ..valid_params()
        };
        assert!(<CreateIssueParams as Validate>::validate(&params).is_err());
    }

    #[test]
    fn test_validate_rejects_short_body() {
        let params = CreateIssueParams {
            body: "too short".to_string(),
            ..valid_params()
        };
        assert!(<CreateIssueParams as Validate>::validate(&params).is_err());
    }

    #[test]
    fn test_validate_rejects_pipeline_injection_in_label() {
        let params = CreateIssueParams {
            labels: vec!["##vso[task.complete]".to_string()],
            ..valid_params()
        };
        assert!(<CreateIssueParams as Validate>::validate(&params).is_err());
    }

    #[test]
    fn test_validate_rejects_pipeline_injection_in_assignee() {
        let params = CreateIssueParams {
            assignees: vec!["$(SYSTEM_ACCESSTOKEN)".to_string()],
            ..valid_params()
        };
        assert!(<CreateIssueParams as Validate>::validate(&params).is_err());
    }

    #[test]
    fn test_sanitize_strips_control_chars() {
        let mut result = CreateIssueResult {
            name: "create-issue".to_string(),
            title: "ok\u{0007}title".to_string(),
            body: "body\u{0008}with\u{0001}ctl chars (more than 30 characters total)".to_string(),
            labels: vec!["la\u{0007}bel".to_string()],
            assignees: vec!["jo\u{0008}hn".to_string()],
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
        let result = CreateIssueResult {
            name: "create-issue".to_string(),
            title: "Fix the build".to_string(),
            body: "anything".to_string(),
            labels: vec![],
            assignees: vec![],
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
    fn test_merge_dedup_strings_dedupes_case_insensitively() {
        let merged = merge_dedup_strings(
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
        let mut result: CreateIssueResult = params.try_into().unwrap();
        let ctx = ctx_with_config(
            serde_json::json!({"target-repo": "githubnext/ado-aw"}),
            None,
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(
            exec.message.contains("ADO_AW_DEBUG_GITHUB_TOKEN"),
            "expected ADO_AW_DEBUG_GITHUB_TOKEN message, got: {}",
            exec.message
        );
    }

    #[tokio::test]
    async fn test_execute_fails_when_target_repo_invalid() {
        let params = valid_params();
        let mut result: CreateIssueResult = params.try_into().unwrap();
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
    async fn test_execute_rejects_disallowed_label() {
        let params = CreateIssueParams {
            labels: vec!["manual".to_string()],
            ..valid_params()
        };
        let mut result: CreateIssueResult = params.try_into().unwrap();
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
        let params = CreateIssueParams {
            labels: vec!["agent-created".to_string()],
            ..valid_params()
        };
        let mut result: CreateIssueResult = params.try_into().unwrap();
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
    async fn test_execute_rejects_when_debug_tool_not_authorized() {
        // Forged-NDJSON scenario: payload contains a `create-issue` entry
        // and ctx.tool_configs has a config under "create-issue" — but the
        // operator never set `ado-aw-debug.create-issue`, so
        // `debug_enabled_tools` is empty. The executor MUST refuse before
        // touching the token.
        let params = valid_params();
        let mut result: CreateIssueResult = params.try_into().unwrap();
        let ctx = ctx_unauthorized(
            serde_json::json!({"target-repo": "githubnext/ado-aw"}),
            Some("token-that-must-not-be-used".to_string()),
        );
        let exec = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec.success);
        assert!(
            exec.message.contains("debug-only tool"),
            "expected debug-only refusal, got: {}",
            exec.message
        );
        // Also confirm the message does NOT mention the token — a token
        // mention would imply we made it past the gate.
        assert!(
            !exec.message.contains("GITHUB_TOKEN"),
            "executor must refuse before checking token: {}",
            exec.message
        );
    }

    #[tokio::test]
    async fn test_execute_rejects_agent_label_when_allowed_labels_empty() {
        // Default-deny: empty allowed-labels means no agent labels allowed.
        let params = CreateIssueParams {
            labels: vec!["bug".to_string()],
            ..valid_params()
        };
        let mut result: CreateIssueResult = params.try_into().unwrap();
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
        let params = CreateIssueParams {
            labels: vec!["arbitrary-label".to_string()],
            ..valid_params()
        };
        let mut result: CreateIssueResult = params.try_into().unwrap();
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
        let params = CreateIssueParams {
            title: "valid title here".to_string(),
            ..valid_params()
        };
        let mut result: CreateIssueResult = params.try_into().unwrap();
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
    async fn test_execute_neutralizes_pipeline_command_in_label_error() {
        // Even though Validate would reject this label up front, Stage 3
        // deserialises directly from NDJSON — so a forged payload could
        // contain ##vso[...] in labels. The error message must neutralise
        // these sequences so they can't act as live ADO pipeline commands
        // when the message is echoed to stdout.
        let mut result = CreateIssueResult {
            name: "create-issue".to_string(),
            title: "Pipeline failure on main".to_string(),
            body: "This is a sufficiently long body for the issue parameters.".to_string(),
            labels: vec!["##vso[task.complete]".to_string()],
            assignees: vec![],
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
title-prefix: "[bug] "
labels: [a]
allowed-labels: ["agent-*"]
assignees: [u1]
"#;
        let cfg: CreateIssueConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.target_repo, "githubnext/ado-aw");
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
        let result: Result<CreateIssueConfig, _> = serde_yaml::from_str(yaml);
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
        let footer = build_footer(&ctx);
        assert!(footer.contains("<!-- ado-aw -->"));
        assert!(footer.contains("buildId=42"));
        assert!(footer.contains("dogfood"));
    }
}
