//! `close-github-issue` safe output.

use anyhow::ensure;
use log::{debug, info, warn};
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
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::secure::GithubTemporaryId;
use crate::tool_result;
use crate::validate::reject_pipeline_injection;
use ado_aw_derive::SanitizeConfig;

const MAX_COMMENT_LEN: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GithubIssueStateReason {
    Completed,
    NotPlanned,
    Duplicate,
}

impl GithubIssueStateReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NotPlanned => "not_planned",
            Self::Duplicate => "duplicate",
        }
    }
}

impl std::fmt::Display for GithubIssueStateReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A canonical issue reference accepted by `duplicate_of`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum GithubDuplicateReference {
    Number(u64),
    Reference(String),
}

impl GithubDuplicateReference {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Number(number) => ensure!(*number > 0, "duplicate_of must be positive"),
            Self::Reference(reference) => {
                ensure!(
                    !reference.trim().is_empty(),
                    "duplicate_of must not be empty"
                );
                ensure!(
                    reference.len() <= 512,
                    "duplicate_of must be 512 characters or fewer"
                );
                reject_pipeline_injection(reference, "close-github-issue.duplicate_of")?;
                parse_duplicate_reference(reference, "owner/repo")
                    .map(|_| ())
                    .map_err(anyhow::Error::msg)?;
            }
        }
        Ok(())
    }

    fn display_sanitized(&self) -> String {
        match self {
            Self::Number(number) => number.to_string(),
            Self::Reference(reference) => crate::sanitize::neutralize_pipeline_commands(reference),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedDuplicateReference {
    SameRepository(u64),
    Temporary(GithubTemporaryId),
    ExplicitRepository { repository: String, number: u64 },
}

fn parse_positive_number(value: &str) -> Option<u64> {
    value.parse::<u64>().ok().filter(|number| *number > 0)
}

fn parse_duplicate_reference(
    reference: &str,
    default_repository: &str,
) -> Result<ParsedDuplicateReference, String> {
    let reference = reference.trim();
    if let Some(number) = reference
        .strip_prefix('#')
        .unwrap_or(reference)
        .chars()
        .all(|character| character.is_ascii_digit())
        .then(|| reference.strip_prefix('#').unwrap_or(reference))
        .and_then(parse_positive_number)
    {
        return Ok(ParsedDuplicateReference::SameRepository(number));
    }
    if let Ok(temporary_id) = GithubTemporaryId::parse(reference) {
        return Ok(ParsedDuplicateReference::Temporary(temporary_id));
    }
    if let Some((repository, number)) = reference.rsplit_once('#')
        && let Some(number) = parse_positive_number(number)
    {
        validate_github_repository(repository).map_err(|error| error.to_string())?;
        return Ok(ParsedDuplicateReference::ExplicitRepository {
            repository: repository.to_string(),
            number,
        });
    }
    if let Ok(url) = url::Url::parse(reference)
        && matches!(url.scheme(), "https" | "http")
        && url.query().is_none()
        && url.fragment().is_none()
    {
        let segments: Vec<&str> = url
            .path_segments()
            .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
            .unwrap_or_default();
        if let [owner, repository, "issues", number] = segments.as_slice()
            && let Some(number) = parse_positive_number(number)
        {
            let repository = format!("{owner}/{repository}");
            validate_github_repository(&repository).map_err(|error| error.to_string())?;
            return Ok(ParsedDuplicateReference::ExplicitRepository { repository, number });
        }
    }
    Err(format!(
        "duplicate_of '{}' must be a positive issue number, temporary ID, \
         owner/repo#number, or full issue URL (default repository: {default_repository})",
        crate::sanitize::neutralize_pipeline_commands(reference)
    ))
}

#[derive(Deserialize, JsonSchema)]
pub struct CloseGithubIssueParams {
    /// Positive GitHub issue number or a temporary ID from create-github-issue.
    pub issue_number: GithubIssueNumber,
    /// Optional closing comment.
    #[serde(default)]
    pub body: Option<String>,
    /// Reason for closing. Defaults to `completed`.
    #[serde(default)]
    pub state_reason: Option<GithubIssueStateReason>,
    /// Canonical issue when closing as a duplicate.
    #[serde(default)]
    pub duplicate_of: Option<GithubDuplicateReference>,
    /// Optional target repository.
    #[serde(default)]
    pub repository: Option<String>,
}

impl Validate for CloseGithubIssueParams {
    fn validate(&self) -> anyhow::Result<()> {
        self.issue_number.validate("issue_number")?;
        if let Some(body) = self.body.as_deref() {
            ensure!(!body.trim().is_empty(), "body must not be empty");
            ensure!(
                body.len() <= MAX_COMMENT_LEN,
                "body must be {MAX_COMMENT_LEN} characters or fewer"
            );
        }
        if let Some(repository) = self.repository.as_deref() {
            validate_github_repository(repository)?;
        }
        if let Some(duplicate_of) = &self.duplicate_of {
            duplicate_of.validate()?;
            if let Some(state_reason) = self.state_reason {
                ensure!(
                    state_reason == GithubIssueStateReason::Duplicate,
                    "duplicate_of requires state_reason 'duplicate'"
                );
            }
        }
        Ok(())
    }
}

tool_result! {
    name = "close-github-issue",
    write = true,
    params = CloseGithubIssueParams,
    default_max = 1,
    /// Result of closing a GitHub issue.
    pub struct CloseGithubIssueResult {
        issue_number: GithubIssueNumber,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        state_reason: Option<GithubIssueStateReason>,
        #[serde(default)]
        duplicate_of: Option<GithubDuplicateReference>,
        #[serde(default)]
        repository: Option<String>,
    }
}

impl SanitizeContent for CloseGithubIssueResult {
    fn sanitize_content_fields(&mut self) {
        self.body = self.body.as_deref().map(sanitize_text);
        self.repository = self.repository.as_deref().map(sanitize_config);
        if let Some(GithubDuplicateReference::Reference(reference)) = &mut self.duplicate_of {
            *reference = sanitize_config(reference);
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseGithubIssueConfig {
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repos")]
    pub allowed_repos: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    /// Fixed state reason. When set, the agent cannot override it.
    #[serde(default, rename = "state-reason")]
    pub state_reason: Option<GithubIssueStateReason>,
    /// Agent-selectable state reasons. Empty means all supported reasons.
    #[serde(default, rename = "allowed-state-reason")]
    pub allowed_state_reason: Vec<GithubIssueStateReason>,
    /// Whether an agent-supplied closing comment may be posted.
    #[serde(default = "default_true", rename = "allow-body")]
    #[sanitize_config(skip)]
    pub allow_body: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

impl Default for CloseGithubIssueConfig {
    fn default() -> Self {
        Self {
            target_repo: None,
            allowed_repos: Vec::new(),
            required_labels: Vec::new(),
            required_title_prefix: None,
            state_reason: None,
            allowed_state_reason: Vec::new(),
            allow_body: true,
            max: None,
        }
    }
}

pub(crate) fn validate_close_github_issue_config(
    config: &CloseGithubIssueConfig,
) -> anyhow::Result<()> {
    validate_github_mutation_filter_config(GithubMutationFilters {
        required_labels: &config.required_labels,
        required_title_prefix: config.required_title_prefix.as_deref(),
    })?;
    ensure!(
        config.state_reason.is_none() || config.allowed_state_reason.is_empty(),
        "close-github-issue config cannot set both state-reason and allowed-state-reason"
    );
    Ok(())
}

impl CloseGithubIssueResult {
    fn effective_state_reason(
        &self,
        config: &CloseGithubIssueConfig,
    ) -> Result<GithubIssueStateReason, ExecutionResult> {
        if config.state_reason.is_some() && !config.allowed_state_reason.is_empty() {
            return Err(ExecutionResult::failure(
                "close-github-issue config cannot set both state-reason and \
                 allowed-state-reason",
            ));
        }
        let reason = config
            .state_reason
            .or(self.state_reason)
            .or_else(|| config.allowed_state_reason.first().copied())
            .unwrap_or(GithubIssueStateReason::Completed);
        if config.state_reason.is_none()
            && !config.allowed_state_reason.is_empty()
            && !config.allowed_state_reason.contains(&reason)
        {
            return Err(ExecutionResult::failure(format!(
                "state_reason '{}' is not permitted by allowed-state-reason: {}",
                reason,
                config
                    .allowed_state_reason
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        if reason == GithubIssueStateReason::Duplicate && self.duplicate_of.is_none() {
            return Err(ExecutionResult::failure(
                "state_reason 'duplicate' requires duplicate_of",
            ));
        }
        if reason != GithubIssueStateReason::Duplicate && self.duplicate_of.is_some() {
            return Err(ExecutionResult::failure(
                "duplicate_of requires effective state_reason 'duplicate'",
            ));
        }
        Ok(reason)
    }

    fn resolve_duplicate(
        &self,
        duplicate_of: &GithubDuplicateReference,
        target_repository: &str,
        policy: GithubRepositoryPolicy<'_>,
        ctx: &ExecutionContext,
    ) -> anyhow::Result<Result<crate::safe_outputs::ResolvedGithubIssueTarget, ExecutionResult>>
    {
        let parsed = match duplicate_of {
            GithubDuplicateReference::Number(number) if *number > 0 => {
                ParsedDuplicateReference::SameRepository(*number)
            }
            GithubDuplicateReference::Number(_) => {
                return Ok(Err(ExecutionResult::failure(
                    "duplicate_of must be positive",
                )));
            }
            GithubDuplicateReference::Reference(reference) => {
                match parse_duplicate_reference(reference, target_repository) {
                    Ok(parsed) => parsed,
                    Err(error) => return Ok(Err(ExecutionResult::failure(error))),
                }
            }
        };

        let (issue_number, repository) = match parsed {
            ParsedDuplicateReference::SameRepository(number) => (
                GithubIssueNumber::Number(number),
                Some(target_repository.to_string()),
            ),
            ParsedDuplicateReference::Temporary(temporary_id) => {
                (GithubIssueNumber::Temporary(temporary_id), None)
            }
            ParsedDuplicateReference::ExplicitRepository { repository, number } => {
                (GithubIssueNumber::Number(number), Some(repository))
            }
        };
        resolve_github_issue_target(&issue_number, repository.as_deref(), policy, ctx)
    }
}

#[async_trait::async_trait]
impl Executor for CloseGithubIssueResult {
    fn dry_run_summary(&self) -> String {
        let target = match &self.issue_number {
            GithubIssueNumber::Number(number) => format!("#{number}"),
            GithubIssueNumber::Temporary(id) => id.canonical(),
        };
        format!("close GitHub issue {target}")
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if !ctx.tool_configs.contains_key("close-github-issue") {
            return Ok(ExecutionResult::failure(
                "close-github-issue is not configured for this workflow",
            ));
        }
        let Some(token) = ctx.github_token.as_ref() else {
            return Ok(ExecutionResult::failure(
                "ADO_AW_GITHUB_TOKEN is not set; configure safe-outputs.github-token \
                 or safe-outputs.github-app",
            ));
        };
        let config: CloseGithubIssueConfig = ctx.get_tool_config("close-github-issue")?;
        validate_close_github_issue_config(&config)?;
        let state_reason = match self.effective_state_reason(&config) {
            Ok(reason) => reason,
            Err(result) => return Ok(result),
        };
        let comment_omitted = self.body.is_some() && !config.allow_body;
        if comment_omitted {
            warn!("Omitting GitHub issue closing comment because allow-body is false");
        }

        let filters = GithubMutationFilters {
            required_labels: &config.required_labels,
            required_title_prefix: config.required_title_prefix.as_deref(),
        };
        if let Err(error) = validate_github_mutation_filter_config(filters) {
            return Ok(ExecutionResult::failure(error.to_string()));
        }
        let policy =
            GithubRepositoryPolicy::new(config.target_repo.as_deref(), &config.allowed_repos);
        let target = match resolve_github_issue_target(
            &self.issue_number,
            self.repository.as_deref(),
            policy,
            ctx,
        )? {
            Ok(target) => target,
            Err(result) => return Ok(result),
        };
        let client = GithubClient::new(&ctx.github_api_url, token)?;
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

        let mut canonical = None;
        if let Some(duplicate_of) = &self.duplicate_of {
            let duplicate_target =
                match self.resolve_duplicate(duplicate_of, &target.repository, policy, ctx)? {
                    Ok(target) => target,
                    Err(result) => return Ok(result),
                };
            if duplicate_target
                .repository
                .eq_ignore_ascii_case(&target.repository)
                && duplicate_target.number == target.number
            {
                return Ok(ExecutionResult::failure(
                    "duplicate_of cannot reference the issue being closed",
                ));
            }
            let duplicate_metadata = match client
                .get_issue(&duplicate_target.repository, duplicate_target.number)
                .await?
            {
                Ok(metadata) => metadata,
                Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
            };
            if let Err(result) = validate_github_target_capability(
                &duplicate_metadata,
                GithubTargetCapabilities::ISSUES_ONLY,
            ) {
                return Ok(result);
            }
            let Some(duplicate_node_id) = metadata.node_id.clone() else {
                return Ok(ExecutionResult::failure(format!(
                    "GitHub issue {}#{} has no GraphQL node ID",
                    target.repository, target.number
                )));
            };
            let Some(canonical_node_id) = duplicate_metadata.node_id else {
                return Ok(ExecutionResult::failure(format!(
                    "Canonical GitHub issue {}#{} has no GraphQL node ID",
                    duplicate_target.repository, duplicate_target.number
                )));
            };
            canonical = Some((
                duplicate_target.repository,
                duplicate_target.number,
                duplicate_node_id,
                canonical_node_id,
            ));
        }

        let already_closed = metadata.state.eq_ignore_ascii_case("closed");
        if already_closed && canonical.is_none() {
            return Ok(ExecutionResult::success_with_data(
                format!(
                    "GitHub issue {}#{} is already closed",
                    target.repository, target.number
                ),
                serde_json::json!({
                    "number": target.number,
                    "target_repo": target.repository,
                    "already_closed": true,
                    "state_reason": state_reason.as_str(),
                }),
            ));
        }

        if !already_closed
            && !comment_omitted
            && let Some(body) = self.body.as_deref()
        {
            let response = client
                .send(
                    Method::POST,
                    client.issue_comments_url(&target.repository, target.number)?,
                    Some(&serde_json::json!({ "body": body })),
                )
                .await?;
            if !response.is_success() {
                let error = response
                    .require_success("Failed to add GitHub issue closing comment")
                    .expect_err("non-success response must produce an API error");
                return Ok(ExecutionResult::failure(error.to_string()));
            }
        }

        if !already_closed {
            debug!(
                "Closing GitHub issue {}#{} as {}",
                target.repository, target.number, state_reason
            );
            let response = client
                .send(
                    Method::PATCH,
                    client.issue_url(&target.repository, target.number)?,
                    Some(&serde_json::json!({
                        "state": "closed",
                        "state_reason": state_reason.as_str(),
                    })),
                )
                .await?;
            if !response.is_success() {
                let error = response
                    .require_success("Failed to close GitHub issue")
                    .expect_err("non-success response must produce an API error");
                return Ok(ExecutionResult::failure(error.to_string()));
            }
        }

        if let Some((canonical_repo, canonical_number, duplicate_id, canonical_id)) = canonical {
            let data = match client
                .graphql(
                    "Failed to mark GitHub issue as duplicate",
                    r#"mutation MarkAsDuplicate($duplicateId: ID!, $canonicalId: ID!) {
  markAsDuplicate(input: { duplicateId: $duplicateId, canonicalId: $canonicalId }) {
    duplicate { ... on Issue { id number } }
  }
}"#,
                    serde_json::json!({
                        "duplicateId": duplicate_id,
                        "canonicalId": canonical_id,
                    }),
                )
                .await?
            {
                Ok(data) => data,
                Err(error) => {
                    return Ok(ExecutionResult::failure_with_data(
                        format!(
                            "Closed GitHub issue {}#{} but failed to create duplicate \
                             relationship: {}",
                            target.repository, target.number, error
                        ),
                        serde_json::json!({
                            "number": target.number,
                            "target_repo": target.repository,
                            "closed": true,
                            "already_closed": already_closed,
                            "duplicate_of": format!("{canonical_repo}#{canonical_number}"),
                        }),
                    ));
                }
            };
            if data
                .pointer("/markAsDuplicate/duplicate/id")
                .and_then(serde_json::Value::as_str)
                .is_none()
            {
                return Ok(ExecutionResult::failure_with_data(
                    format!(
                        "Closed GitHub issue {}#{} but GitHub returned no duplicate relationship",
                        target.repository, target.number
                    ),
                    serde_json::json!({
                        "number": target.number,
                        "target_repo": target.repository,
                        "closed": true,
                        "already_closed": already_closed,
                        "duplicate_of": format!("{canonical_repo}#{canonical_number}"),
                    }),
                ));
            }
        }

        let action = if already_closed {
            "Confirmed duplicate relationship for already-closed"
        } else {
            "Closed"
        };
        info!(
            "{} GitHub issue {}#{} as {}",
            action, target.repository, target.number, state_reason
        );
        let message = format!(
            "{} GitHub issue {}#{} as {}",
            action, target.repository, target.number, state_reason
        );
        let message = if comment_omitted {
            format!("{message}; closing comment omitted because allow-body is false")
        } else {
            message
        };
        Ok(ExecutionResult::success_with_data(
            message,
            serde_json::json!({
                "number": target.number,
                "target_repo": target.repository,
                "already_closed": already_closed,
                "state_reason": state_reason.as_str(),
                "comment_omitted": comment_omitted,
                "duplicate_of": self.duplicate_of.as_ref().map(
                    GithubDuplicateReference::display_sanitized
                ),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ToolResult;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn context(server: &MockServer, config: serde_json::Value) -> ExecutionContext {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("close-github-issue".to_string(), config);
        ExecutionContext {
            github_token: Some("token".to_string()),
            github_api_url: server.uri(),
            tool_configs,
            ..Default::default()
        }
    }

    fn open_issue(number: u64) -> serde_json::Value {
        serde_json::json!({
            "number": number,
            "node_id": format!("I_{number}"),
            "title": "[agent] Issue",
            "state": "open",
            "labels": [{"name": "automation"}],
            "html_url": format!("https://github.example/octo/repo/issues/{number}")
        })
    }

    fn result(
        issue_number: GithubIssueNumber,
        state_reason: Option<GithubIssueStateReason>,
    ) -> CloseGithubIssueResult {
        CloseGithubIssueParams {
            issue_number,
            body: None,
            state_reason,
            duplicate_of: None,
            repository: None,
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn contract_name_and_budget() {
        assert_eq!(CloseGithubIssueResult::NAME, "close-github-issue");
        assert_eq!(CloseGithubIssueResult::DEFAULT_MAX, 1);
    }

    #[test]
    fn validates_numeric_and_temporary_targets() {
        assert!(
            CloseGithubIssueParams {
                issue_number: GithubIssueNumber::Number(1),
                body: None,
                state_reason: None,
                duplicate_of: None,
                repository: Some("octo/repo".to_string()),
            }
            .validate()
            .is_ok()
        );
        assert!(
            CloseGithubIssueParams {
                issue_number: GithubIssueNumber::Temporary(
                    GithubTemporaryId::parse("#aw_issue1").unwrap()
                ),
                body: Some("Resolved by the latest deployment.".to_string()),
                state_reason: Some(GithubIssueStateReason::Completed),
                duplicate_of: None,
                repository: None,
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn duplicate_reference_formats_and_invariants() {
        for value in [
            "42",
            "#42",
            "#aw_issue1",
            "octo/other#42",
            "https://github.com/octo/other/issues/42",
        ] {
            assert!(
                GithubDuplicateReference::Reference(value.to_string())
                    .validate()
                    .is_ok(),
                "{value}"
            );
        }
        assert!(
            GithubDuplicateReference::Reference("octo/other#0".to_string())
                .validate()
                .is_err()
        );
        assert!(
            CloseGithubIssueParams {
                issue_number: GithubIssueNumber::Number(1),
                body: None,
                state_reason: Some(GithubIssueStateReason::Completed),
                duplicate_of: Some(GithubDuplicateReference::Number(2)),
                repository: None,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn config_is_strict_and_allow_body_defaults_true() {
        assert!(
            serde_json::from_value::<CloseGithubIssueConfig>(serde_json::json!({
                "unknown": true
            }))
            .is_err()
        );
        let config: CloseGithubIssueConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(config.allow_body);
    }

    #[test]
    fn omitted_reason_uses_first_allowed_reason_then_completed_fallback() {
        let result = result(GithubIssueNumber::Number(7), None);
        let configured = CloseGithubIssueConfig {
            allowed_state_reason: vec![
                GithubIssueStateReason::NotPlanned,
                GithubIssueStateReason::Completed,
            ],
            ..Default::default()
        };
        assert_eq!(
            result.effective_state_reason(&configured).unwrap(),
            GithubIssueStateReason::NotPlanned
        );
        assert_eq!(
            result
                .effective_state_reason(&CloseGithubIssueConfig::default())
                .unwrap(),
            GithubIssueStateReason::Completed
        );
    }

    #[tokio::test]
    async fn closes_with_optional_comment_and_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(open_issue(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .and(body_json(serde_json::json!({"body": "Resolved safely."})))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({
                "state": "closed",
                "state_reason": "not_planned"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "required-labels": ["AUTOMATION"],
                "required-title-prefix": "[agent]",
                "allowed-state-reason": ["completed", "not_planned"]
            }),
        );
        let mut result: CloseGithubIssueResult = CloseGithubIssueParams {
            issue_number: GithubIssueNumber::Number(7),
            body: Some("Resolved safely.".to_string()),
            state_reason: Some(GithubIssueStateReason::NotPlanned),
            duplicate_of: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
    }

    #[tokio::test]
    async fn fixed_reason_overrides_agent_reason() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(open_issue(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({
                "state": "closed",
                "state_reason": "not_planned"
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "state-reason": "not_planned"
            }),
        );
        let mut result = result(
            GithubIssueNumber::Number(7),
            Some(GithubIssueStateReason::Completed),
        );
        assert!(result.execute_sanitized(&ctx).await.unwrap().success);
    }

    #[tokio::test]
    async fn disallowed_reason_fails_before_http() {
        let server = MockServer::start().await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed-state-reason": ["completed"]
            }),
        );
        let mut denied_reason = result(
            GithubIssueNumber::Number(7),
            Some(GithubIssueStateReason::NotPlanned),
        );
        assert!(!denied_reason.execute_sanitized(&ctx).await.unwrap().success);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn disallowed_body_is_omitted_but_issue_is_closed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(open_issue(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/issues/7/comments"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .and(body_json(serde_json::json!({
                "state": "closed",
                "state_reason": "completed"
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allow-body": false
            }),
        );
        let mut result: CloseGithubIssueResult = CloseGithubIssueParams {
            issue_number: GithubIssueNumber::Number(7),
            body: Some("Must not be posted.".to_string()),
            state_reason: None,
            duplicate_of: None,
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();

        assert!(execution.success, "{}", execution.message);
        assert!(execution.message.contains("closing comment omitted"));
        assert_eq!(
            execution.data.as_ref().unwrap()["comment_omitted"],
            serde_json::json!(true)
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn already_closed_is_idempotent_without_writes() {
        let server = MockServer::start().await;
        let mut closed = open_issue(7);
        closed["state"] = serde_json::json!("closed");
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(closed))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        let mut result = result(GithubIssueNumber::Number(7), None);
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success);
        assert!(execution.message.contains("already closed"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn duplicate_is_preflighted_then_marked_natively() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(open_issue(7)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/canonical/issues/9"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 9,
                "node_id": "I_9",
                "title": "Canonical issue",
                "state": "open",
                "labels": [],
                "html_url": "https://github.example/octo/canonical/issues/9"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": r#"mutation MarkAsDuplicate($duplicateId: ID!, $canonicalId: ID!) {
  markAsDuplicate(input: { duplicateId: $duplicateId, canonicalId: $canonicalId }) {
    duplicate { ... on Issue { id number } }
  }
}"#,
                "variables": {
                    "duplicateId": "I_7",
                    "canonicalId": "I_9"
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "markAsDuplicate": {
                        "duplicate": {"id": "I_7", "number": 7}
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed-repos": ["octo/canonical"],
                "allowed-state-reason": ["duplicate"]
            }),
        );
        let mut result: CloseGithubIssueResult = CloseGithubIssueParams {
            issue_number: GithubIssueNumber::Number(7),
            body: None,
            state_reason: Some(GithubIssueStateReason::Duplicate),
            duplicate_of: Some(GithubDuplicateReference::Reference(
                "octo/canonical#9".to_string(),
            )),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
    }

    #[tokio::test]
    async fn already_closed_duplicate_retries_native_relationship() {
        let server = MockServer::start().await;
        let mut closed = open_issue(7);
        closed["state"] = serde_json::json!("closed");
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(closed))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/canonical/issues/9"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 9,
                "node_id": "I_9",
                "title": "Canonical issue",
                "state": "open",
                "labels": [],
                "html_url": "https://github.example/octo/canonical/issues/9"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(serde_json::json!({
                "query": r#"mutation MarkAsDuplicate($duplicateId: ID!, $canonicalId: ID!) {
  markAsDuplicate(input: { duplicateId: $duplicateId, canonicalId: $canonicalId }) {
    duplicate { ... on Issue { id number } }
  }
}"#,
                "variables": {
                    "duplicateId": "I_7",
                    "canonicalId": "I_9"
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "markAsDuplicate": {
                        "duplicate": {"id": "I_7", "number": 7}
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed-repos": ["octo/canonical"],
                "allowed-state-reason": ["duplicate"]
            }),
        );
        let mut result: CloseGithubIssueResult = CloseGithubIssueParams {
            issue_number: GithubIssueNumber::Number(7),
            body: None,
            state_reason: None,
            duplicate_of: Some(GithubDuplicateReference::Reference(
                "octo/canonical#9".to_string(),
            )),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "{}", execution.message);
        assert_eq!(
            execution.data.as_ref().unwrap()["already_closed"],
            serde_json::json!(true)
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            !requests
                .iter()
                .any(|request| request.method.as_str() == "PATCH")
        );
    }

    #[tokio::test]
    async fn duplicate_repository_policy_fails_before_writes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/issues/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(open_issue(7)))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context(
            &server,
            serde_json::json!({
                "target-repo": "octo/repo",
                "allowed-state-reason": ["duplicate"]
            }),
        );
        let mut result: CloseGithubIssueResult = CloseGithubIssueParams {
            issue_number: GithubIssueNumber::Number(7),
            body: None,
            state_reason: Some(GithubIssueStateReason::Duplicate),
            duplicate_of: Some(GithubDuplicateReference::Reference(
                "other/repo#9".to_string(),
            )),
            repository: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("not an exact"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dry_run_makes_no_requests() {
        let server = MockServer::start().await;
        let mut ctx = context(&server, serde_json::json!({"target-repo": "octo/repo"}));
        ctx.dry_run = true;
        let mut result = result(GithubIssueNumber::Number(7), None);
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success);
        assert!(execution.message.contains("[DRY-RUN]"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
