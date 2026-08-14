//! Shared policy and target resolution for GitHub issue safe outputs.
#![allow(dead_code)] // The remaining GitHub issue tools consume this shared surface in later slices.

use anyhow::ensure;
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::sync::OnceLock;

use crate::safe_outputs::{ExecutionContext, ExecutionResult};
use crate::secure::GithubTemporaryId;
use crate::validate::reject_pipeline_injection;

/// Positive GitHub issue number or a same-run temporary issue ID.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum GithubIssueNumber {
    Number(u64),
    Temporary(GithubTemporaryId),
}

impl GithubIssueNumber {
    pub fn validate(&self, field: &str) -> anyhow::Result<()> {
        if let Self::Number(number) = self {
            ensure!(*number > 0, "{field} must be positive");
        }
        Ok(())
    }
}

impl fmt::Display for GithubIssueNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(number) => write!(formatter, "{number}"),
            Self::Temporary(temporary_id) => formatter.write_str(&temporary_id.canonical()),
        }
    }
}

impl_temporary_reference_deserialize!(
    GithubIssueNumber,
    GithubTemporaryId,
    expecting = "a positive issue number or #aw_ temporary issue ID",
    negative = "issue number must be positive",
    quoted_out_of_range = "quoted issue number is outside the u64 range",
);

/// Borrowed repository policy shared by creation and mutation tools.
#[derive(Debug, Clone, Copy)]
pub struct GithubRepositoryPolicy<'a> {
    pub target_repo: Option<&'a str>,
    pub allowed_repos: &'a [String],
}

impl<'a> GithubRepositoryPolicy<'a> {
    pub const fn new(target_repo: Option<&'a str>, allowed_repos: &'a [String]) -> Self {
        Self {
            target_repo,
            allowed_repos,
        }
    }
}

/// Borrowed filters that must pass against the live issue or pull request.
#[derive(Debug, Clone, Copy, Default)]
pub struct GithubMutationFilters<'a> {
    pub required_labels: &'a [String],
    pub required_title_prefix: Option<&'a str>,
}

impl GithubMutationFilters<'_> {
    pub fn is_empty(&self) -> bool {
        self.required_labels.is_empty() && self.required_title_prefix.is_none()
    }
}

/// Validate shared mutation-filter configuration before fetching a target.
pub fn validate_github_mutation_filter_config(
    filters: GithubMutationFilters<'_>,
) -> anyhow::Result<()> {
    for label in filters.required_labels {
        ensure!(
            !label.is_empty(),
            "required-labels entries must not be empty"
        );
        reject_pipeline_injection(label, "required-labels")?;
    }
    if let Some(prefix) = filters.required_title_prefix {
        ensure!(
            !prefix.is_empty(),
            "required-title-prefix must not be empty"
        );
        reject_pipeline_injection(prefix, "required-title-prefix")?;
    }
    Ok(())
}

/// Whether a mutation may target issues, pull requests, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GithubTargetCapabilities {
    pub issues: bool,
    pub pull_requests: bool,
}

impl GithubTargetCapabilities {
    pub const ISSUES_ONLY: Self = Self {
        issues: true,
        pull_requests: false,
    };
    pub const ISSUES_AND_PULL_REQUESTS: Self = Self {
        issues: true,
        pull_requests: true,
    };
}

/// Live GitHub target type returned by the shared API client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubTargetKind {
    Issue,
    PullRequest,
}

/// Issue/PR metadata used for policy checks before the first write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubTargetMetadata {
    pub number: u64,
    pub node_id: Option<String>,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub kind: GithubTargetKind,
    pub html_url: Option<String>,
}

/// Fully resolved mutation target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGithubIssueTarget {
    pub repository: String,
    pub number: u64,
    pub url: Option<String>,
}

fn target_repo_regex() -> &'static regex_lite::Regex {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex_lite::Regex::new(r"^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?/[A-Za-z0-9._-]+$")
            .expect("GitHub repository regex is well-formed")
    })
}

/// Validate an exact GitHub repository slug.
pub fn validate_github_repository(repository: &str) -> anyhow::Result<()> {
    ensure!(
        !repository.is_empty(),
        "target-repo is required (expected 'owner/repo')"
    );
    reject_pipeline_injection(repository, "GitHub repository")?;
    ensure!(
        target_repo_regex().is_match(repository),
        "target-repo '{}' is not in 'owner/repo' format \
         (owner: alphanumerics/hyphens; repo: alphanumerics/dots/hyphens/underscores)",
        repository
    );
    let (_, name) = repository
        .split_once('/')
        .expect("validated GitHub repository contains a slash");
    ensure!(
        name != "." && name != "..",
        "target-repo repo segment must not be '.' or '..'"
    );
    Ok(())
}

/// Backward-compatible name used by the existing GitHub issue tools.
pub fn validate_target_repo(target_repo: &str) -> anyhow::Result<()> {
    validate_github_repository(target_repo)
}

/// Return validated configured repositories, deduplicated case-insensitively.
pub fn configured_github_repositories(
    policy: GithubRepositoryPolicy<'_>,
) -> anyhow::Result<Vec<String>> {
    let mut repositories = Vec::new();
    if let Some(target_repo) = policy.target_repo {
        validate_github_repository(target_repo)?;
        repositories.push(target_repo.to_string());
    }
    for repository in policy.allowed_repos {
        validate_github_repository(repository)?;
        if !repositories
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(repository))
        {
            repositories.push(repository.clone());
        }
    }
    Ok(repositories)
}

fn current_github_repository(ctx: &ExecutionContext) -> Result<String, ExecutionResult> {
    let provider = ctx.repository_provider.as_deref().unwrap_or_default();
    if !provider.eq_ignore_ascii_case("github")
        && !provider.eq_ignore_ascii_case("githubenterprise")
    {
        return Err(ExecutionResult::failure(
            "target-repo is required when the Azure DevOps pipeline source is not GitHub",
        ));
    }
    if provider.eq_ignore_ascii_case("githubenterprise")
        && ctx
            .github_api_url
            .eq_ignore_ascii_case("https://api.github.com")
    {
        return Err(ExecutionResult::failure(
            "safe-outputs.github-api-url or GitHub App api-url is required for a \
             GitHub Enterprise source",
        ));
    }
    let repository = ctx.repository_name.clone().ok_or_else(|| {
        ExecutionResult::failure(
            "BUILD_REPOSITORY_NAME is not set; configure target-repo explicitly",
        )
    })?;
    validate_github_repository(&repository)
        .map_err(|error| ExecutionResult::failure(error.to_string()))?;
    Ok(repository)
}

/// Select an effective repository using agent selection, fixed target, then source fallback.
pub fn resolve_github_repository(
    requested_repository: Option<&str>,
    policy: GithubRepositoryPolicy<'_>,
    ctx: &ExecutionContext,
) -> Result<String, ExecutionResult> {
    let configured = configured_github_repositories(policy)
        .map_err(|error| ExecutionResult::failure(error.to_string()))?;
    let default_repository = match policy.target_repo {
        Some(repository) => repository.to_string(),
        None => current_github_repository(ctx)?,
    };

    let Some(requested) = requested_repository else {
        return Ok(default_repository);
    };
    validate_github_repository(requested)
        .map_err(|error| ExecutionResult::failure(error.to_string()))?;

    if requested.eq_ignore_ascii_case(&default_repository) {
        return Ok(default_repository);
    }
    if let Some(allowed) = configured
        .iter()
        .find(|repository| repository.eq_ignore_ascii_case(requested))
    {
        return Ok(allowed.clone());
    }

    let mut allowed = vec![default_repository];
    for repository in configured {
        if !allowed
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&repository))
        {
            allowed.push(repository);
        }
    }
    Err(ExecutionResult::failure(format!(
        "repository '{}' is not an exact target-repo or allowed-repos entry: {}",
        crate::sanitize::neutralize_pipeline_commands(requested),
        allowed.join(", ")
    )))
}

/// Backward-compatible fixed/default target resolver.
pub fn resolve_target_repo(
    configured: Option<&str>,
    ctx: &ExecutionContext,
) -> Result<String, ExecutionResult> {
    resolve_github_repository(None, GithubRepositoryPolicy::new(configured, &[]), ctx)
}

/// Resolve a numeric or temporary issue reference under the consumer's repository policy.
pub fn resolve_github_issue_target(
    issue_number: &GithubIssueNumber,
    requested_repository: Option<&str>,
    policy: GithubRepositoryPolicy<'_>,
    ctx: &ExecutionContext,
) -> anyhow::Result<Result<ResolvedGithubIssueTarget, ExecutionResult>> {
    if let Err(error) = configured_github_repositories(policy) {
        return Ok(Err(ExecutionResult::failure(error.to_string())));
    }

    match issue_number {
        GithubIssueNumber::Number(number) => {
            if *number == 0 {
                return Ok(Err(ExecutionResult::failure(
                    "issue_number must be positive",
                )));
            }
            let repository = match resolve_github_repository(requested_repository, policy, ctx) {
                Ok(repository) => repository,
                Err(error) => return Ok(Err(error)),
            };
            Ok(Ok(ResolvedGithubIssueTarget {
                repository,
                number: *number,
                url: None,
            }))
        }
        GithubIssueNumber::Temporary(temporary_id) => {
            let Some(issue) = ctx.resolve_github_issue(temporary_id)? else {
                return Ok(Err(ExecutionResult::failure(format!(
                    "temporary issue ID '{}' has not been resolved; create-github-issue must \
                     succeed earlier in the same SafeOutputs job",
                    temporary_id.canonical()
                ))));
            };
            if let Some(requested) = requested_repository
                && !requested.eq_ignore_ascii_case(&issue.repository)
            {
                return Ok(Err(ExecutionResult::failure(format!(
                    "temporary issue ID '{}' resolved to repository '{}', which does not match \
                     requested repository '{}'",
                    temporary_id.canonical(),
                    issue.repository,
                    crate::sanitize::neutralize_pipeline_commands(requested)
                ))));
            }
            let repository = match resolve_github_repository(Some(&issue.repository), policy, ctx) {
                Ok(repository) => repository,
                Err(error) => return Ok(Err(error)),
            };
            Ok(Ok(ResolvedGithubIssueTarget {
                repository,
                number: issue.number,
                url: Some(issue.url),
            }))
        }
    }
}

/// Check all required labels and the required title prefix against live metadata.
pub fn validate_github_mutation_filters(
    metadata: &GithubTargetMetadata,
    filters: GithubMutationFilters<'_>,
) -> Result<(), ExecutionResult> {
    let missing: Vec<&str> = filters
        .required_labels
        .iter()
        .map(String::as_str)
        .filter(|required| {
            !metadata
                .labels
                .iter()
                .any(|label| label.eq_ignore_ascii_case(required))
        })
        .collect();
    if !missing.is_empty() {
        return Err(ExecutionResult::failure(format!(
            "GitHub target #{} is missing required labels: {}",
            metadata.number,
            missing.join(", ")
        )));
    }
    if let Some(prefix) = filters.required_title_prefix
        && !metadata.title.starts_with(prefix)
    {
        return Err(ExecutionResult::failure(format!(
            "GitHub target #{} title does not start with required-title-prefix '{}'",
            metadata.number,
            crate::sanitize::neutralize_pipeline_commands(prefix)
        )));
    }
    Ok(())
}

/// Check whether the live target kind is enabled for a tool.
pub fn validate_github_target_capability(
    metadata: &GithubTargetMetadata,
    capabilities: GithubTargetCapabilities,
) -> Result<(), ExecutionResult> {
    let allowed = match metadata.kind {
        GithubTargetKind::Issue => capabilities.issues,
        GithubTargetKind::PullRequest => capabilities.pull_requests,
    };
    if allowed {
        return Ok(());
    }
    let kind = match metadata.kind {
        GithubTargetKind::Issue => "issues",
        GithubTargetKind::PullRequest => "pull requests",
    };
    Err(ExecutionResult::failure(format!(
        "GitHub target #{} is a {kind} target, but this tool is not configured to mutate {kind}",
        metadata.number
    )))
}

/// Apply gh-aw-compatible case-insensitive `*` glob policy with blocked-first semantics.
pub fn validate_blocked_first_globs(
    values: &[String],
    allowed: &[String],
    blocked: &[String],
    field: &str,
) -> Result<(), ExecutionResult> {
    for value in values {
        if blocked
            .iter()
            .any(|pattern| simple_github_glob_matches(value, pattern))
        {
            return Err(ExecutionResult::failure(format!(
                "{field} '{}' is blocked by policy",
                crate::sanitize::neutralize_pipeline_commands(value)
            )));
        }
    }
    if allowed.is_empty() {
        return Ok(());
    }
    for value in values {
        if !allowed
            .iter()
            .any(|pattern| simple_github_glob_matches(value, pattern))
        {
            return Err(ExecutionResult::failure(format!(
                "{field} '{}' is not allowed by policy",
                crate::sanitize::neutralize_pipeline_commands(value)
            )));
        }
    }
    Ok(())
}

/// Match gh-aw's simple glob contract: case-insensitive, with only `*` special.
pub fn simple_github_glob_matches(value: &str, pattern: &str) -> bool {
    !value.is_empty()
        && !pattern.is_empty()
        && super::wildcard_match(&pattern.to_ascii_lowercase(), &value.to_ascii_lowercase())
}

/// Extract the owner from a validated repository slug.
pub fn github_repository_owner(repository: &str) -> anyhow::Result<&str> {
    validate_github_repository(repository)?;
    Ok(repository
        .split_once('/')
        .expect("validated GitHub repository contains slash")
        .0)
}

/// Return the single owner suitable for GitHub App repository scoping.
pub fn github_app_owner_for_repositories<'a>(
    repositories: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<Option<String>> {
    let mut owner: Option<String> = None;
    for repository in repositories {
        let candidate = github_repository_owner(repository)?;
        if let Some(existing) = owner.as_deref() {
            ensure!(
                existing.eq_ignore_ascii_case(candidate),
                "GitHub App authentication requires all target-repo and allowed-repos values \
                 to have the same owner; found '{}' and '{}'",
                existing,
                candidate
            );
        } else {
            owner = Some(candidate.to_string());
        }
    }
    Ok(owner)
}

/// Repository names for a GitHub App installation owner, deduplicated case-insensitively.
pub fn github_app_repository_names<'a>(
    owner: &str,
    repositories: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    for repository in repositories {
        validate_github_repository(repository)?;
        let (candidate_owner, name) = repository
            .split_once('/')
            .expect("validated GitHub repository contains slash");
        ensure!(
            candidate_owner.eq_ignore_ascii_case(owner),
            "GitHub repository '{}' does not belong to App owner '{}'",
            repository,
            owner
        );
        if !names
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(name))
        {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Stable hidden marker used to identify comments from one ADO pipeline definition.
pub fn github_pipeline_comment_marker(ctx: &ExecutionContext) -> anyhow::Result<String> {
    let definition_id = ctx.definition_id.ok_or_else(|| {
        anyhow::anyhow!("SYSTEM_DEFINITIONID is required when hide-older-comments is enabled")
    })?;
    Ok(format!(
        "<!-- ado-aw:github-comment:pipeline-definition-id={definition_id} -->"
    ))
}

/// Generic hidden marker for comments that do not use definition-scoped
/// hide-older-comments behavior.
pub const GITHUB_COMMENT_MARKER: &str = "<!-- ado-aw:github-comment -->";

/// Build the traceability footer used by GitHub issue content.
pub fn build_github_trace_footer(ctx: &ExecutionContext) -> String {
    let mut lines = vec!["<!-- ado-aw -->".to_string(), "---".to_string()];
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

/// Merge operator and agent strings with case-insensitive deduplication.
pub fn merge_github_values(operator: &[String], agent: &[String]) -> Vec<String> {
    let mut merged = operator.to_vec();
    for value in agent {
        if !merged
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(value))
        {
            merged.push(value.clone());
        }
    }
    merged
}

/// Validate and deduplicate values intended to become exact repository policy.
pub fn dedupe_github_repositories(repositories: &[String]) -> anyhow::Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for repository in repositories {
        validate_github_repository(repository)?;
        if seen.insert(repository.to_ascii_lowercase()) {
            deduped.push(repository.clone());
        }
    }
    Ok(deduped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::ResolvedGithubIssue;
    use std::collections::HashMap;

    fn github_ctx() -> ExecutionContext {
        ExecutionContext {
            repository_provider: Some("GitHub".to_string()),
            repository_name: Some("octo/current".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn issue_number_accepts_numbers_quoted_numbers_and_temporary_ids() {
        let numeric: GithubIssueNumber = serde_json::from_str("42").unwrap();
        let quoted: GithubIssueNumber = serde_json::from_str("\"42\"").unwrap();
        let temporary: GithubIssueNumber = serde_json::from_str("\"#aw_bug1\"").unwrap();
        assert!(matches!(numeric, GithubIssueNumber::Number(42)));
        assert!(matches!(quoted, GithubIssueNumber::Number(42)));
        assert!(matches!(temporary, GithubIssueNumber::Temporary(_)));
    }

    #[test]
    fn issue_number_rejects_negative_and_malformed_temporary_values() {
        assert!(serde_json::from_str::<GithubIssueNumber>("-1").is_err());
        assert!(serde_json::from_str::<GithubIssueNumber>("\"not-an-id\"").is_err());
        assert!(
            GithubIssueNumber::Number(0)
                .validate("issue_number")
                .is_err()
        );
    }

    #[test]
    fn validates_repository_shapes_and_injection() {
        assert!(validate_github_repository("githubnext/ado-aw").is_ok());
        assert!(validate_github_repository("user/.github").is_ok());
        assert!(validate_github_repository("owner/repo_with.dot-dash").is_ok());
        for invalid in [
            "",
            "owner",
            "owner/repo/extra",
            "-owner/repo",
            "owner-/repo",
            "under_score/repo",
            "owner/..",
            "owner/$(TOKEN)",
        ] {
            assert!(
                validate_github_repository(invalid).is_err(),
                "{invalid} should fail"
            );
        }
    }

    #[test]
    fn repository_policy_uses_target_then_current_fallback() {
        let ctx = github_ctx();
        let allowed = vec![];
        assert_eq!(
            resolve_github_repository(
                None,
                GithubRepositoryPolicy::new(Some("octo/fixed"), &allowed),
                &ctx
            )
            .unwrap(),
            "octo/fixed"
        );
        assert_eq!(
            resolve_github_repository(None, GithubRepositoryPolicy::new(None, &allowed), &ctx)
                .unwrap(),
            "octo/current"
        );
    }

    #[test]
    fn repository_selection_is_exact_and_case_insensitive() {
        let ctx = github_ctx();
        let allowed = vec!["Octo/Other".to_string()];
        let selected = resolve_github_repository(
            Some("octo/other"),
            GithubRepositoryPolicy::new(Some("octo/fixed"), &allowed),
            &ctx,
        )
        .unwrap();
        assert_eq!(selected, "Octo/Other");

        let denied = resolve_github_repository(
            Some("octo/not-allowed"),
            GithubRepositoryPolicy::new(Some("octo/fixed"), &allowed),
            &ctx,
        )
        .unwrap_err();
        assert!(denied.message.contains("not an exact"));
    }

    #[test]
    fn configured_repositories_dedupe_case_insensitively() {
        let allowed = vec![
            "OCTO/REPO".to_string(),
            "octo/other".to_string(),
            "Octo/Other".to_string(),
        ];
        let repositories = configured_github_repositories(GithubRepositoryPolicy::new(
            Some("octo/repo"),
            &allowed,
        ))
        .unwrap();
        assert_eq!(repositories, vec!["octo/repo", "octo/other"]);
    }

    #[test]
    fn implicit_repository_rejects_non_github_and_unconfigured_ghes() {
        let ado = ExecutionContext {
            repository_provider: Some("TfsGit".to_string()),
            repository_name: Some("repo".to_string()),
            ..Default::default()
        };
        assert!(
            resolve_github_repository(None, GithubRepositoryPolicy::new(None, &[]), &ado)
                .unwrap_err()
                .message
                .contains("target-repo is required")
        );

        let ghes = ExecutionContext {
            repository_provider: Some("GitHubEnterprise".to_string()),
            repository_name: Some("octo/repo".to_string()),
            ..Default::default()
        };
        assert!(
            resolve_github_repository(None, GithubRepositoryPolicy::new(None, &[]), &ghes)
                .unwrap_err()
                .message
                .contains("GitHub Enterprise source")
        );
    }

    #[test]
    fn temporary_target_enforces_repository_policy_and_explicit_match() {
        let temporary_id = GithubTemporaryId::parse("#aw_bug1").unwrap();
        let ctx = github_ctx();
        ctx.register_resolved_github_issue(
            &temporary_id,
            ResolvedGithubIssue {
                repository: "octo/created".to_string(),
                number: 17,
                url: "https://github.com/octo/created/issues/17".to_string(),
            },
        )
        .unwrap();
        let allowed = vec!["octo/created".to_string()];
        let target = resolve_github_issue_target(
            &GithubIssueNumber::Temporary(temporary_id.clone()),
            None,
            GithubRepositoryPolicy::new(Some("octo/default"), &allowed),
            &ctx,
        )
        .unwrap()
        .unwrap();
        assert_eq!(target.repository, "octo/created");
        assert_eq!(target.number, 17);

        let mismatch = resolve_github_issue_target(
            &GithubIssueNumber::Temporary(temporary_id),
            Some("octo/default"),
            GithubRepositoryPolicy::new(Some("octo/default"), &allowed),
            &ctx,
        )
        .unwrap()
        .unwrap_err();
        assert!(mismatch.message.contains("does not match"));
    }

    fn issue_metadata() -> GithubTargetMetadata {
        GithubTargetMetadata {
            number: 7,
            node_id: Some("I_1".to_string()),
            title: "[agent] Fix it".to_string(),
            state: "open".to_string(),
            labels: vec!["Bug".to_string(), "triage".to_string()],
            kind: GithubTargetKind::Issue,
            html_url: None,
        }
    }

    #[test]
    fn required_labels_and_title_prefix_are_all_required() {
        let metadata = issue_metadata();
        assert!(
            validate_github_mutation_filters(
                &metadata,
                GithubMutationFilters {
                    required_labels: &["bug".to_string(), "TRIAGE".to_string()],
                    required_title_prefix: Some("[agent]"),
                }
            )
            .is_ok()
        );
        assert!(
            validate_github_mutation_filters(
                &metadata,
                GithubMutationFilters {
                    required_labels: &["missing".to_string()],
                    required_title_prefix: None,
                }
            )
            .unwrap_err()
            .message
            .contains("missing required labels")
        );
        assert!(
            validate_github_mutation_filters(
                &metadata,
                GithubMutationFilters {
                    required_labels: &[],
                    required_title_prefix: Some("[other]"),
                }
            )
            .unwrap_err()
            .message
            .contains("required-title-prefix")
        );
    }

    #[test]
    fn mutation_filter_config_rejects_empty_values_and_injection() {
        assert!(
            validate_github_mutation_filter_config(GithubMutationFilters {
                required_labels: &["".to_string()],
                required_title_prefix: None,
            })
            .is_err()
        );
        assert!(
            validate_github_mutation_filter_config(GithubMutationFilters {
                required_labels: &[],
                required_title_prefix: Some("$(TOKEN)"),
            })
            .is_err()
        );
    }

    #[test]
    fn target_capability_distinguishes_issues_and_pull_requests() {
        let issue = issue_metadata();
        assert!(
            validate_github_target_capability(&issue, GithubTargetCapabilities::ISSUES_ONLY)
                .is_ok()
        );
        let pull_request = GithubTargetMetadata {
            kind: GithubTargetKind::PullRequest,
            ..issue
        };
        assert!(
            validate_github_target_capability(&pull_request, GithubTargetCapabilities::ISSUES_ONLY)
                .is_err()
        );
    }

    #[test]
    fn blocked_globs_win_and_empty_allowlist_is_unrestricted() {
        let values = vec!["dependabot[bot]".to_string()];
        assert!(validate_blocked_first_globs(&values, &[], &[], "assignee").is_ok());
        assert!(
            validate_blocked_first_globs(
                &values,
                &["*".to_string()],
                &["*[bot]".to_string()],
                "assignee"
            )
            .unwrap_err()
            .message
            .contains("blocked")
        );
        assert!(
            validate_blocked_first_globs(
                &["alice".to_string()],
                &["octo-*".to_string()],
                &[],
                "assignee"
            )
            .is_err()
        );
    }

    #[test]
    fn pat_targets_may_span_owners_but_app_helper_rejects_them() {
        let repositories = vec!["octo/one".to_string(), "hubot/two".to_string()];
        assert_eq!(
            dedupe_github_repositories(&repositories).unwrap(),
            repositories
        );
        assert!(
            github_app_owner_for_repositories(repositories.iter().map(String::as_str)).is_err()
        );
    }

    #[test]
    fn app_owner_and_repository_names_are_case_insensitive() {
        let repositories = ["Octo/One", "octo/two", "OCTO/ONE"];
        assert_eq!(
            github_app_owner_for_repositories(repositories).unwrap(),
            Some("Octo".to_string())
        );
        assert_eq!(
            github_app_repository_names("octo", repositories).unwrap(),
            vec!["One".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn stable_comment_marker_requires_definition_id() {
        let ctx = ExecutionContext {
            definition_id: Some(123),
            ..Default::default()
        };
        assert_eq!(
            github_pipeline_comment_marker(&ctx).unwrap(),
            "<!-- ado-aw:github-comment:pipeline-definition-id=123 -->"
        );
        assert!(github_pipeline_comment_marker(&ExecutionContext::default()).is_err());
    }

    #[test]
    fn trace_footer_and_merge_preserve_existing_contract() {
        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/octo".to_string()),
            ado_project: Some("project".to_string()),
            build_id: Some(42),
            definition_name: Some("pipeline".to_string()),
            build_reason: Some("Manual".to_string()),
            ..Default::default()
        };
        let footer = build_github_trace_footer(&ctx);
        assert!(footer.contains("<!-- ado-aw -->"));
        assert!(footer.contains("buildId=42"));
        assert_eq!(
            merge_github_values(
                &["bug".to_string(), "Triage".to_string()],
                &["BUG".to_string(), "fresh".to_string()]
            ),
            vec!["bug".to_string(), "Triage".to_string(), "fresh".to_string()]
        );
    }

    #[test]
    fn unresolved_temporary_target_fails_cleanly() {
        let result = resolve_github_issue_target(
            &GithubIssueNumber::Temporary(GithubTemporaryId::parse("#aw_none").unwrap()),
            None,
            GithubRepositoryPolicy::new(Some("octo/repo"), &[]),
            &github_ctx(),
        )
        .unwrap()
        .unwrap_err();
        assert!(result.message.contains("has not been resolved"));
    }

    #[test]
    fn repository_error_does_not_expose_pipeline_commands() {
        let result = resolve_github_repository(
            Some("octo/##vso[task.complete]"),
            GithubRepositoryPolicy::new(Some("octo/repo"), &[]),
            &github_ctx(),
        )
        .unwrap_err();
        assert!(
            !result
                .message
                .lines()
                .any(|line| line.starts_with("##vso["))
        );
    }

    #[test]
    fn registered_issue_map_is_shared_across_context_clones() {
        let ctx = github_ctx();
        let cloned = ctx.clone();
        let temporary_id = GithubTemporaryId::parse("#aw_map1").unwrap();
        ctx.register_resolved_github_issue(
            &temporary_id,
            ResolvedGithubIssue {
                repository: "octo/repo".to_string(),
                number: 1,
                url: String::new(),
            },
        )
        .unwrap();
        let issues: HashMap<_, _> = cloned.resolved_github_issues.lock().unwrap().clone();
        assert!(issues.contains_key("#aw_map1"));
    }
}
