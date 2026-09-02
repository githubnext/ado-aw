//! Tool parameter and result structs for MCP tools

use crate::{all_safe_output_names, tool_names};
use log::{debug, warn};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

/// Characters to percent-encode in a URL path segment.
/// Encodes the structural delimiters that would break URL parsing if left raw:
/// `#` (fragment), `?` (query), `/` (path separator), and space.
/// This hardens operator-controlled values (project names, wiki names, work item
/// types) against accidental corruption of the URL structure.
pub(crate) const PATH_SEGMENT: &AsciiSet =
    &CONTROLS.add(b'#').add(b'?').add(b'/').add(b'%').add(b' ');

/// Safe output tools that are always available regardless of filtering.
/// These are diagnostic/transparency tools that agents should always have access to.
///
/// Derived from diagnostic tool types — adding a new diagnostic tool means adding
/// its type here and the name is extracted automatically via `ToolResult::NAME`.
pub const ALWAYS_ON_TOOLS: &[&str] = tool_names![
    NoopResult,
    MissingDataResult,
    MissingToolResult,
    ReportIncompleteResult,
];

/// Non-MCP safe-output keys handled by the compiler/executor, not the MCP server.
/// These must not appear in `--enabled-tools` or they cause real MCP tools to be
/// filtered out (the router has no route for them).
pub const NON_MCP_SAFE_OUTPUT_KEYS: &[&str] = &[];

/// Global configuration keys accepted under `safe-outputs:` that are NOT tool
/// names — they configure cross-cutting Conclusion-job behaviour rather than
/// registering a tool. Unlike [`NON_MCP_SAFE_OUTPUT_KEYS`], these are
/// deliberately absent from [`ALL_KNOWN_SAFE_OUTPUTS`] (they have no tool type)
/// and must be explicitly allowed in `validate_safe_outputs_keys`.
pub const SAFE_OUTPUT_CONFIG_KEYS: &[&str] = &[
    "report-failure-as-work-item",
    "github-token",
    "github-api-url",
    "github-app",
];

/// Future tools gated behind `ado-aw-debug:` front matter.
pub const DEBUG_ONLY_TOOLS: &[&str] = &[];

/// Public tools exposed only when explicitly configured in `safe-outputs:`.
pub const CONFIGURED_ONLY_TOOLS: &[&str] = tool_names![
    AssignWorkItemResult,
    CreateGithubIssueResult,
    SetGithubIssueTypeResult,
    CommentOnGithubIssueResult,
    HideGithubIssueCommentResult,
    AddGithubIssueLabelsResult,
    RemoveGithubIssueLabelsResult,
    CloseGithubIssueResult,
    UpdateGithubIssueResult,
    SetGithubIssueFieldResult,
    AssignGithubIssueMilestoneResult,
    AssignGithubIssueToUserResult,
    UnassignGithubIssueFromUserResult,
    LinkGithubSubIssueResult,
];

/// All recognised safe-output keys accepted in front matter `safe-outputs:`.
/// This is the union of write-requiring tool types and diagnostic tool types.
///
/// Derived at compile time from tool types — no hand-maintained string lists.
///
/// Note: `memory` was removed — it is now a first-class tool configured via
/// `tools: cache-memory:` and is no longer a safe-output key.
pub const ALL_KNOWN_SAFE_OUTPUTS: &[&str] = all_safe_output_names![
    // Write-requiring MCP tools
    CreateWorkItemResult,
    AssignWorkItemResult,
    CommentOnWorkItemResult,
    UpdateWorkItemResult,
    CreatePrResult,
    CreateWikiPageResult,
    UpdateWikiPageResult,
    AddPrCommentResult,
    LinkWorkItemsResult,
    QueueBuildResult,
    CreateGitTagResult,
    AddBuildTagResult,
    CreateBranchResult,
    UpdatePrResult,
    UploadBuildAttachmentResult,
    UploadPipelineArtifactResult,
    UploadWorkitemAttachmentResult,
    SubmitPrReviewResult,
    ReplyToPrCommentResult,
    ResolvePrThreadResult,
    CreateGithubIssueResult,
    SetGithubIssueTypeResult,
    CommentOnGithubIssueResult,
    HideGithubIssueCommentResult,
    AddGithubIssueLabelsResult,
    RemoveGithubIssueLabelsResult,
    CloseGithubIssueResult,
    UpdateGithubIssueResult,
    SetGithubIssueFieldResult,
    AssignGithubIssueMilestoneResult,
    AssignGithubIssueToUserResult,
    UnassignGithubIssueFromUserResult,
    LinkGithubSubIssueResult,
    // Always-on diagnostics
    NoopResult,
    MissingDataResult,
    MissingToolResult,
    ReportIncompleteResult;
];

/// Resolve the effective branch for a wiki.
///
/// If `configured_branch` is `Some`, that value is returned directly.
/// Otherwise the wiki metadata API is queried: code wikis (type&nbsp;1) return
/// the published branch from the `versions` array; project wikis (type&nbsp;0)
/// return `Ok(None)` because the server handles branching internally.
///
/// Returns `Err` when a code wiki is detected but the branch cannot be
/// resolved — callers should surface this as a user-facing failure rather
/// than proceeding to a confusing ADO PUT error.
pub(crate) async fn resolve_wiki_branch(
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    wiki_name: &str,
    token: &str,
    configured_branch: Option<&str>,
) -> Result<Option<String>, String> {
    // Explicit configuration always wins.
    if let Some(b) = configured_branch {
        return Ok(Some(b.to_owned()));
    }

    let url = format!(
        "{}/{}/_apis/wiki/wikis/{}",
        org_url.trim_end_matches('/'),
        utf8_percent_encode(project, PATH_SEGMENT),
        utf8_percent_encode(wiki_name, PATH_SEGMENT),
    );

    let resp = match client
        .get(&url)
        .query(&[("api-version", "7.0")])
        .basic_auth("", Some(token))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!("Wiki metadata request failed: {e} — skipping branch auto-detection");
            return Ok(None);
        }
    };

    if !resp.status().is_success() {
        warn!(
            "Wiki metadata request returned HTTP {} — skipping branch auto-detection",
            resp.status()
        );
        return Ok(None);
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(e) => {
            warn!("Failed to parse wiki metadata response: {e}");
            return Ok(None);
        }
    };

    // Detect code wikis. ADO returns the type as a string enum ("codeWiki" /
    // "projectWiki") rather than a numeric value, so we check both forms.
    let is_code_wiki = match body.get("type") {
        Some(serde_json::Value::String(s)) => s.eq_ignore_ascii_case("codewiki"),
        Some(serde_json::Value::Number(n)) => n.as_u64() == Some(1),
        _ => false,
    };
    if !is_code_wiki {
        let type_val = body.get("type").cloned().unwrap_or(serde_json::Value::Null);
        debug!("Wiki is a project wiki (type {type_val}) — no branch needed");
        return Ok(None);
    }

    // Code wiki: extract the published branch from versions[0].version
    let branch = body
        .get("versions")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());

    match &branch {
        Some(b) => {
            debug!("Detected code wiki — resolved branch: {b}");
            Ok(branch)
        }
        None => Err(format!(
            "Wiki '{wiki_name}' is a code wiki but its published branch could not be \
             determined. Set 'branch' explicitly in the safe-outputs config."
        )),
    }
}

/// Look up an ADO repo name in `allowed_repositories`, accepting either:
/// 1. an exact alias key (e.g. `repo-sdk-ftdidevicecontrol`),
/// 2. an exact value match against the configured `name` (e.g. `4x4/sdk-FtdiDeviceControl`), or
/// 3. a case-insensitive match against the trailing repo-name part of the value
///    (e.g. `sdk-FtdiDeviceControl` for `4x4/sdk-FtdiDeviceControl`).
///
/// Azure DevOps repository names are case-insensitive, so the name-based
/// fallbacks match case-insensitively. Returns the resolved alias key only when
/// the match is unique; ambiguous names are rejected.
pub(crate) fn lookup_allowed_repository_alias<'a>(
    input: &str,
    allowed_repositories: &'a std::collections::HashMap<String, String>,
) -> Option<&'a String> {
    fn unique_alias<'a>(mut matches: impl Iterator<Item = &'a String>) -> Option<&'a String> {
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(first)
    }

    // 1. Exact alias key match
    if let Some((alias, _)) = allowed_repositories.get_key_value(input) {
        return Some(alias);
    }
    // 2. Unique case-insensitive full-value match ("project/repo").
    // ADO repo names are case-insensitive, so accept any case for the full path.
    if let Some(alias) = unique_alias(
        allowed_repositories
            .iter()
            .filter(|(_, value)| value.eq_ignore_ascii_case(input))
            .map(|(alias, _)| alias),
    ) {
        return Some(alias);
    }
    // 3. Unique trailing repo-name match (case-insensitive).
    unique_alias(
        allowed_repositories
            .iter()
            .filter(|(_, value)| {
                value
                    .rsplit('/')
                    .next()
                    .unwrap_or(value.as_str())
                    .eq_ignore_ascii_case(input)
            })
            .map(|(alias, _)| alias),
    )
}

/// Look up an ADO repo name in `allowed_repositories`, accepting either:
/// 1. an exact alias key (e.g. `repo-sdk-ftdidevicecontrol`),
/// 2. an exact value match against the configured `name` (e.g. `4x4/sdk-FtdiDeviceControl`), or
/// 3. a case-insensitive match against the trailing repo-name part of the value
///    (e.g. `sdk-FtdiDeviceControl` for `4x4/sdk-FtdiDeviceControl`).
///
/// Azure DevOps repository names are case-insensitive, so the trailing-name fallback
/// matches case-insensitively. Returns the resolved ADO repo name (the map value) on
/// success, or `None` if no entry matches.
pub(crate) fn lookup_allowed_repository<'a>(
    input: &str,
    allowed_repositories: &'a std::collections::HashMap<String, String>,
) -> Option<&'a String> {
    lookup_allowed_repository_alias(input, allowed_repositories)
        .and_then(|alias| allowed_repositories.get(alias))
}

/// Return `true` if `input` refers to the pipeline's own repository — either the
/// literal string `"self"`, the empty string, or a case-insensitive match against
/// `ctx.repository_name` (full value or trailing repo-name part).
pub(crate) fn input_refers_to_self(input: &str, ctx: &ExecutionContext) -> bool {
    if input == "self" || input.is_empty() {
        if input.is_empty() {
            debug!("Empty repository alias treated as 'self'");
        }
        return true;
    }
    if let Some(name) = ctx.repository_name.as_deref() {
        if name.eq_ignore_ascii_case(input) {
            return true;
        }
        let trailing = name.rsplit('/').next().unwrap_or(name);
        if trailing.eq_ignore_ascii_case(input) {
            return true;
        }
    }
    false
}

/// Normalize a repository selector to the compiler/runtime alias key.
///
/// Accepts a raw agent-supplied selector (`"self"`, `""`, an alias key, a full
/// `project/repo` value, or a bare repo name) and returns the canonical alias
/// key — `"self"` or a key of `ctx.allowed_repositories`.
///
/// **Idempotent**: passing an already-canonical alias returns it unchanged
/// (`"self"` short-circuits on [`input_refers_to_self`]; an alias key hits the
/// exact-key arm of [`lookup_allowed_repository_alias`]), so callers may
/// canonicalize defensively without changing the result.
///
/// **Precedence**: literal `"self"`/empty selects self; an exact checkout alias
/// selects that alias; name-based matches are accepted only when they identify
/// exactly one of self or the configured repositories.
pub(crate) fn canonical_repository_alias(
    repository: &str,
    ctx: &ExecutionContext,
) -> Option<String> {
    if repository == "self" || repository.is_empty() {
        return Some("self".to_string());
    }
    if ctx.allowed_repositories.contains_key(repository) {
        return Some(repository.to_string());
    }

    let mut matches = Vec::new();
    if input_refers_to_self(repository, ctx) {
        matches.push("self".to_string());
    }
    for (alias, value) in &ctx.allowed_repositories {
        let trailing = value.rsplit('/').next().unwrap_or(value);
        if value.eq_ignore_ascii_case(repository) || trailing.eq_ignore_ascii_case(repository) {
            matches.push(alias.clone());
        }
    }
    matches.sort();
    matches.dedup();
    (matches.len() == 1).then(|| matches.remove(0))
}

#[derive(Debug, Clone)]
pub(crate) struct RepositoryTargetSpec {
    pub alias: String,
    pub repo_type: String,
    pub name: String,
    pub organization: Option<String>,
    pub endpoint: Option<String>,
}

pub(crate) fn repository_write_scope_key(
    organization: &str,
    project: &str,
    repository: &str,
) -> String {
    format!("{organization}/{project}/{repository}").to_ascii_lowercase()
}

pub(crate) fn authenticate_ado_request(
    request: reqwest::RequestBuilder,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
) -> reqwest::RequestBuilder {
    if connection_type == Some(crate::compile::types::WriteConnectionType::AzureDevOps) {
        request.bearer_auth(token)
    } else {
        request.basic_auth("", Some(token))
    }
}

pub(crate) fn configure_repository_write_context(
    ctx: &mut ExecutionContext,
    checkout: &[String],
    repositories: Vec<RepositoryTargetSpec>,
    write_connection_type: Option<crate::compile::types::WriteConnectionType>,
    write_allow: &[crate::compile::types::AdoOrganizationScope],
) {
    ctx.allowed_repositories.clear();
    ctx.repository_targets.clear();
    ctx.cross_organization_repositories.clear();
    for alias in checkout {
        let Some(repository) = repositories
            .iter()
            .find(|repository| &repository.alias == alias)
        else {
            continue;
        };
        ctx.allowed_repositories
            .insert(alias.clone(), repository.name.clone());
        ctx.repository_targets.insert(
            alias.clone(),
            crate::safe_outputs::result::AdoRepositoryTargetConfig {
                name: repository.name.clone(),
                organization: repository.organization.clone(),
                endpoint: repository.endpoint.clone(),
            },
        );
        if repository.repo_type.eq_ignore_ascii_case("git")
            && repository.endpoint.is_some()
            && repository.organization.is_none()
        {
            ctx.cross_organization_repositories.insert(alias.clone());
        }
    }

    ctx.write_connection_type = write_connection_type;
    ctx.write_allowed_repositories = write_allow
        .iter()
        .flat_map(|organization| {
            organization.projects.iter().flat_map(move |project| {
                project.repositories.iter().map(move |repository| {
                    repository_write_scope_key(
                        organization.organization.as_str(),
                        project.project.as_str(),
                        repository.as_str(),
                    )
                })
            })
        })
        .collect();
}

fn split_repository_target_name(
    name: &str,
    current_project: &str,
) -> Result<(String, String), ExecutionResult> {
    match name.split_once('/') {
        Some((project, repository)) if !repository.contains('/') => {
            Ok((project.to_string(), repository.to_string()))
        }
        None => Ok((current_project.to_string(), name.to_string())),
        _ => Err(ExecutionResult::failure(format!(
            "Repository '{name}' must be a repository name or project/repository"
        ))),
    }
}

/// Resolve a repository selector to an exact organization/project/repository
/// destination and enforce the additional cross-organization write policy.
pub(crate) fn resolve_repository_write_target(
    repository: Option<&str>,
    ctx: &ExecutionContext,
) -> Result<crate::safe_outputs::result::AdoRepositoryTarget, ExecutionResult> {
    let selector = repository.unwrap_or("self");
    let Some(alias) = canonical_repository_alias(selector, ctx) else {
        return Err(ExecutionResult::failure(format!(
            "Repository '{selector}' is not in the allowed repository list"
        )));
    };
    let current_org_url = ctx.ado_org_url.as_deref().ok_or_else(|| {
        ExecutionResult::failure("Azure DevOps organization URL not configured")
    })?;
    let current_organization = ctx.ado_organization.as_deref().ok_or_else(|| {
        ExecutionResult::failure("Azure DevOps organization name not configured")
    })?;
    let current_project = ctx
        .ado_project
        .as_deref()
        .ok_or_else(|| ExecutionResult::failure("Azure DevOps project not configured"))?;

    if alias == "self" {
        let name = ctx
            .repository_name
            .as_deref()
            .ok_or_else(|| ExecutionResult::failure("BUILD_REPOSITORY_NAME not set"))?;
        let (_, repository) = split_repository_target_name(name, current_project)?;
        return Ok(crate::safe_outputs::result::AdoRepositoryTarget {
            alias,
            organization: current_organization.to_string(),
            organization_url: current_org_url.trim_end_matches('/').to_string(),
            project: current_project.to_string(),
            repository,
            repository_id: ctx.repository_id.clone(),
            cross_organization: false,
        });
    }

    let config = ctx.repository_targets.get(&alias).cloned().or_else(|| {
        ctx.allowed_repositories.get(&alias).map(|name| {
            crate::safe_outputs::result::AdoRepositoryTargetConfig {
                name: name.clone(),
                organization: None,
                endpoint: None,
            }
        })
    });
    let Some(config) = config else {
        return Err(ExecutionResult::failure(format!(
            "Repository alias '{alias}' has no configured target metadata"
        )));
    };
    if config.organization.is_none()
        && (config.endpoint.is_some() || ctx.cross_organization_repositories.contains(&alias))
    {
        return Err(ExecutionResult::failure(format!(
            "Repository '{selector}' (checkout alias '{alias}') uses an endpoint-backed \
             Azure Repos checkout but has no `repos.organization`; the target organization \
             cannot be resolved safely."
        )));
    }
    if config.endpoint.is_some()
        && config
            .organization
            .as_deref()
            .is_some_and(|organization| organization.eq_ignore_ascii_case(current_organization))
    {
        return Err(ExecutionResult::failure(format!(
            "Repository '{selector}' (checkout alias '{alias}') uses an endpoint-backed \
             Azure Repos checkout but declares the pipeline's current organization \
             '{current_organization}'. Remove the unnecessary endpoint for a same-organization \
             repository or set `repos.organization` to the actual target organization."
        )));
    }

    let (project, repository_name) =
        split_repository_target_name(&config.name, current_project)?;
    let organization = config
        .organization
        .as_deref()
        .unwrap_or(current_organization);
    let cross_organization = !organization.eq_ignore_ascii_case(current_organization);
    if cross_organization {
        if ctx.write_connection_type
            != Some(crate::compile::types::WriteConnectionType::AzureDevOps)
        {
            return Err(ExecutionResult::failure(format!(
                "Repository '{selector}' resolves to cross-organization target \
                 '{organization}/{project}/{repository_name}', but permissions.write must use \
                 `connection-type: azureDevOps`."
            )));
        }
        let scope = repository_write_scope_key(organization, &project, &repository_name);
        if !ctx.write_allowed_repositories.contains(&scope) {
            return Err(ExecutionResult::failure(format!(
                "Repository '{selector}' resolves to cross-organization target \
                 '{organization}/{project}/{repository_name}', which is not listed in \
                 permissions.write.allow."
            )));
        }
    }

    Ok(crate::safe_outputs::result::AdoRepositoryTarget {
        alias,
        organization: organization.to_string(),
        organization_url: if cross_organization {
            format!("https://dev.azure.com/{organization}")
        } else {
            current_org_url.trim_end_matches('/').to_string()
        },
        project,
        repository: repository_name,
        repository_id: None,
        cross_organization,
    })
}

/// Resolve a repository selector to its checkout directory.
///
/// The checkout root and `self` directory differ in multi-checkout jobs.
/// Named repositories are resolved through the configured alias map rather
/// than appended from untrusted selector text.
///
/// `repository` may be **either** a raw agent-supplied selector or an alias
/// already canonicalized by [`canonical_repository_alias`]; both are supported
/// because that helper is idempotent. `add-pr-comment` passes the raw value
/// straight from the agent, while `create-pull-request` canonicalizes first so
/// it can reuse the alias for target-branch resolution. Callers must not build
/// the path themselves — routing every selector through here is what keeps
/// untrusted text out of the path join.
pub(crate) fn resolve_repository_checkout_dir(
    repository: &str,
    ctx: &ExecutionContext,
) -> anyhow::Result<std::path::PathBuf> {
    let Some(alias) = canonical_repository_alias(repository, ctx) else {
        anyhow::bail!(
            "Repository '{}' is not in the allowed repository list",
            repository
        );
    };
    if alias == "self" {
        return Ok(ctx.self_repository_directory.clone());
    }

    Ok(ctx.source_directory.join(alias))
}

/// Resolve a repository alias to its ADO repo name.
///
/// Accepts `"self"` (or `None`) → `ctx.repository_name`, an alias key from
/// `ctx.allowed_repositories`, an exact value match, or a case-insensitive match
/// against the trailing repo-name part of either `ctx.repository_name` or any
/// configured allowed repository. See [`lookup_allowed_repository`] for the
/// matching rules used against `ctx.allowed_repositories`.
pub(crate) fn resolve_repo_name(
    repo_alias: Option<&str>,
    ctx: &ExecutionContext,
) -> Result<String, ExecutionResult> {
    let alias = repo_alias.unwrap_or("self");
    let Some(alias) = canonical_repository_alias(alias, ctx) else {
        return Err(ExecutionResult::failure(format!(
            "Repository '{}' is not in the allowed repository list",
            alias
        )));
    };
    if alias == "self" {
        return ctx
            .repository_name
            .clone()
            .ok_or_else(|| ExecutionResult::failure("BUILD_REPOSITORY_NAME not set"));
    }
    ctx.allowed_repositories
        .get(&alias)
        .cloned()
        .ok_or_else(|| {
            ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed repository list",
                alias
            ))
        })
}

/// Match a `value` against a `pattern` where `*` matches zero or more of **any**
/// character (including `/`).
///
/// Unlike file-path glob matching, `/` is **not** treated as a segment separator —
/// these patterns are used for tags, artifact names, and similar non-path strings.
///
/// Only the `*` wildcard is supported; there is no `?`, `[…]`, or `**` syntax.
/// Literal `*` characters cannot be escaped — this is intentional since the values
/// being matched (ADO tags, artifact names) cannot contain `*`.
pub(crate) fn wildcard_match(pattern: &str, value: &str) -> bool {
    let p = pattern.as_bytes();
    let v = value.as_bytes();
    let (pn, vn) = (p.len(), v.len());

    let mut pi = 0;
    let mut vi = 0;
    // Saved positions for backtracking on `*`
    let mut star_p = usize::MAX;
    let mut star_v: usize = 0;

    while vi < vn {
        if pi < pn && p[pi] == b'*' {
            star_p = pi;
            star_v = vi;
            pi += 1;
        } else if pi < pn && p[pi] == v[vi] {
            pi += 1;
            vi += 1;
        } else if star_p != usize::MAX {
            // Backtrack: let the last `*` consume one more character
            pi = star_p + 1;
            star_v += 1;
            vi = star_v;
        } else {
            return false;
        }
    }

    // Consume any trailing `*`s in the pattern
    while pi < pn && p[pi] == b'*' {
        pi += 1;
    }

    pi == pn
}

/// Return `true` if `tag` is matched by `pattern`.
///
/// Uses [`wildcard_match`] with **case-insensitive** comparison. `*` in the
/// pattern matches zero or more of any character (including `/`), so
/// `copilot:repo=org/project/*@main` correctly matches
/// `copilot:repo=org/project/MyRepo@main`.
///
/// This is the shared matcher for `allowed-tags` in `create-work-item`,
/// `update-work-item`, and `add-build-tag`.
pub(crate) fn tag_matches_pattern(tag: &str, pattern: &str) -> bool {
    wildcard_match(&pattern.to_ascii_lowercase(), &tag.to_ascii_lowercase())
}

const NON_ASSIGNABLE_WORK_ITEM_IDENTITIES: &[&str] = &["Agency", "GitHub Copilot"];

/// Normalize and validate an identity before assigning an Azure DevOps work item.
pub(crate) fn normalize_work_item_assignee(
    assignee: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    let assignee = assignee.trim();
    anyhow::ensure!(!assignee.is_empty(), "{field_name} must not be empty");
    crate::validate::reject_pipeline_injection(assignee, field_name)?;
    if NON_ASSIGNABLE_WORK_ITEM_IDENTITIES
        .iter()
        .any(|blocked| blocked.eq_ignore_ascii_case(assignee))
    {
        anyhow::bail!("{field_name} cannot assign the reserved identity '{assignee}'");
    }
    Ok(assignee.to_string())
}

/// Return `true` if `name` is matched by `pattern` (**case-sensitive**).
///
/// Uses [`wildcard_match`] for artifact-name allow-lists where case matters.
pub(crate) fn name_matches_pattern(name: &str, pattern: &str) -> bool {
    wildcard_match(pattern, name)
}

/// Re-export of the canonical git ref-name validator (now in [`crate::validate`]).
pub(crate) use crate::validate::validate_git_ref_name;

macro_rules! impl_temporary_reference_deserialize {
    (
        $reference:ident,
        $temporary:ty,
        expecting = $expecting:literal,
        negative = $negative:literal,
        quoted_out_of_range = $quoted_out_of_range:literal $(,)?
    ) => {
        impl<'de> serde::Deserialize<'de> for $reference {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct TemporaryReferenceVisitor;

                impl serde::de::Visitor<'_> for TemporaryReferenceVisitor {
                    type Value = $reference;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str($expecting)
                    }

                    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                        Ok($reference::Number(value))
                    }

                    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        u64::try_from(value)
                            .map($reference::Number)
                            .map_err(|_| E::custom($negative))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        if value.chars().all(|character| character.is_ascii_digit()) {
                            return value
                                .parse::<u64>()
                                .map($reference::Number)
                                .map_err(|_| E::custom($quoted_out_of_range));
                        }
                        <$temporary>::parse(value)
                            .map($reference::Temporary)
                            .map_err(E::custom)
                    }
                }

                deserializer.deserialize_any(TemporaryReferenceVisitor)
            }
        }
    };
}

mod add_build_tag;
mod add_github_issue_labels;
mod add_pr_comment;
mod assign_work_item;
mod assign_github_issue_milestone;
mod assign_github_issue_to_user;
mod close_github_issue;
mod comment_on_github_issue;
mod comment_on_work_item;
mod create_branch;
mod create_git_tag;
mod create_github_issue;
mod create_pull_request;
mod create_wiki_page;
mod create_work_item;
mod github_api;
mod github_issue_common;
mod hide_github_issue_comment;
mod link_github_sub_issue;
mod link_work_items;
mod missing_data;
mod missing_tool;
mod noop;
mod queue_build;
mod remove_github_issue_labels;
mod reply_to_pr_comment;
mod report_incomplete;
mod resolve_pr_thread;
mod result;
mod set_github_issue_field;
mod set_github_issue_type;
mod submit_pr_review;
mod unassign_github_issue_from_user;
mod update_github_issue;
mod update_pr;
mod update_wiki_page;
mod update_work_item;
mod upload_build_attachment;
mod upload_pipeline_artifact;
mod upload_workitem_attachment;

pub use add_build_tag::*;
pub use add_github_issue_labels::*;
pub use add_pr_comment::*;
pub use assign_work_item::*;
pub use assign_github_issue_milestone::*;
pub use assign_github_issue_to_user::*;
pub use close_github_issue::*;
pub use comment_on_github_issue::*;
pub use comment_on_work_item::*;
pub use create_branch::*;
pub use create_git_tag::*;
pub use create_github_issue::*;
pub use create_pull_request::*;
pub use create_wiki_page::*;
pub use create_work_item::*;
pub use github_api::*;
pub use github_issue_common::*;
pub use hide_github_issue_comment::*;
pub use link_github_sub_issue::*;
pub use link_work_items::*;
pub use missing_data::*;
pub use missing_tool::*;
pub use noop::*;
pub use queue_build::*;
pub use remove_github_issue_labels::*;
pub use reply_to_pr_comment::*;
pub use report_incomplete::*;
pub use resolve_pr_thread::*;
pub use result::{
    ExecutionContext, ExecutionResult, Executor, ResolvedGithubIssue, ResolvedWorkItem, ToolResult,
    Validate, anyhow_to_mcp_error, org_from_url,
};
pub use set_github_issue_field::*;
pub use set_github_issue_type::*;
pub use submit_pr_review::*;
pub use unassign_github_issue_from_user::*;
pub use update_github_issue::*;
pub use update_pr::*;
pub use update_wiki_page::*;
pub use update_work_item::*;
pub use upload_build_attachment::*;
pub use upload_pipeline_artifact::*;
pub use upload_workitem_attachment::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_always_on_subset_of_all_known() {
        for name in ALWAYS_ON_TOOLS {
            assert!(
                ALL_KNOWN_SAFE_OUTPUTS.contains(name),
                "ALWAYS_ON_TOOLS entry '{}' is missing from ALL_KNOWN_SAFE_OUTPUTS",
                name
            );
        }
    }

    #[test]
    fn test_non_mcp_keys_subset_of_all_known() {
        for name in NON_MCP_SAFE_OUTPUT_KEYS {
            assert!(
                ALL_KNOWN_SAFE_OUTPUTS.contains(name),
                "NON_MCP_SAFE_OUTPUT_KEYS entry '{}' is missing from ALL_KNOWN_SAFE_OUTPUTS",
                name
            );
        }
    }

    /// Verify that every type in the write-requiring list actually has
    /// `REQUIRES_WRITE == true`, and every diagnostic type has `false`.
    #[test]
    fn test_requires_write_consistency() {
        // Write-requiring tools
        const {
            assert!(CreateGithubIssueResult::REQUIRES_WRITE);
        }
        const {
            assert!(SetGithubIssueTypeResult::REQUIRES_WRITE);
        }
        const {
            assert!(CreateWorkItemResult::REQUIRES_WRITE);
        }
        const {
            assert!(AssignWorkItemResult::REQUIRES_WRITE);
        }
        const {
            assert!(CommentOnWorkItemResult::REQUIRES_WRITE);
        }
        const {
            assert!(UpdateWorkItemResult::REQUIRES_WRITE);
        }
        const {
            assert!(CreatePrResult::REQUIRES_WRITE);
        }
        const {
            assert!(CreateWikiPageResult::REQUIRES_WRITE);
        }
        const {
            assert!(UpdateWikiPageResult::REQUIRES_WRITE);
        }
        const {
            assert!(AddPrCommentResult::REQUIRES_WRITE);
        }
        const {
            assert!(LinkWorkItemsResult::REQUIRES_WRITE);
        }
        const {
            assert!(QueueBuildResult::REQUIRES_WRITE);
        }
        const {
            assert!(CreateGitTagResult::REQUIRES_WRITE);
        }
        const {
            assert!(AddBuildTagResult::REQUIRES_WRITE);
        }
        const {
            assert!(CreateBranchResult::REQUIRES_WRITE);
        }
        const {
            assert!(UpdatePrResult::REQUIRES_WRITE);
        }
        const {
            assert!(UploadBuildAttachmentResult::REQUIRES_WRITE);
        }
        const {
            assert!(UploadPipelineArtifactResult::REQUIRES_WRITE);
        }
        const {
            assert!(UploadWorkitemAttachmentResult::REQUIRES_WRITE);
        }
        const {
            assert!(SubmitPrReviewResult::REQUIRES_WRITE);
        }
        const {
            assert!(ReplyToPrCommentResult::REQUIRES_WRITE);
        }
        const {
            assert!(ResolvePrThreadResult::REQUIRES_WRITE);
        }

        // Diagnostic tools (should NOT require write)
        const {
            assert!(!NoopResult::REQUIRES_WRITE);
        }
        const {
            assert!(!MissingDataResult::REQUIRES_WRITE);
        }
        const {
            assert!(!MissingToolResult::REQUIRES_WRITE);
        }
        const {
            assert!(!ReportIncompleteResult::REQUIRES_WRITE);
        }
    }

    /// Verify ALL_KNOWN_SAFE_OUTPUTS contains no duplicate entries, and
    /// that the always-on and non-MCP sub-lists are disjoint.
    #[test]
    fn test_all_known_completeness() {
        // No duplicates: a tool name appearing twice in ALL_KNOWN would
        // mean `all_safe_output_names!` was given the same type twice
        // and would silently break tool routing.
        let mut seen = std::collections::HashSet::new();
        for name in ALL_KNOWN_SAFE_OUTPUTS {
            assert!(
                seen.insert(*name),
                "ALL_KNOWN_SAFE_OUTPUTS contains duplicate entry '{}'",
                name
            );
        }

        // ALWAYS_ON and NON_MCP must be disjoint — a diagnostic tool
        // that also appears as a non-MCP key would be both routed and
        // intercepted, giving inconsistent behaviour.
        for name in ALWAYS_ON_TOOLS {
            assert!(
                !NON_MCP_SAFE_OUTPUT_KEYS.contains(name),
                "Tool '{}' appears in both ALWAYS_ON and NON_MCP — lists must be disjoint",
                name
            );
        }
    }

    // ─── validate_git_ref_name ──────────────────────────────────────────────

    #[test]
    fn test_validate_git_ref_name_rejects_at_brace() {
        assert!(validate_git_ref_name("branch@{0}", "b").is_err());
    }

    #[test]
    fn test_validate_git_ref_name_rejects_dotlock_suffix() {
        assert!(validate_git_ref_name("my-branch.lock", "b").is_err());
    }

    #[test]
    fn test_validate_git_ref_name_rejects_consecutive_slashes() {
        assert!(validate_git_ref_name("feat//thing", "b").is_err());
    }

    #[test]
    fn test_validate_git_ref_name_rejects_backslash() {
        assert!(validate_git_ref_name("feat\\evil", "b").is_err());
    }

    #[test]
    fn test_validate_git_ref_name_rejects_special_chars() {
        for ch in ['~', '^', ':', '?', '*', '['] {
            let name = format!("feat{ch}bad");
            assert!(
                validate_git_ref_name(&name, "b").is_err(),
                "should reject '{ch}'"
            );
        }
    }

    #[test]
    fn test_validate_git_ref_name_rejects_component_starting_with_dot() {
        assert!(validate_git_ref_name("feat/.hidden", "b").is_err());
    }

    #[test]
    fn test_validate_git_ref_name_rejects_trailing_dot() {
        assert!(validate_git_ref_name("my-branch.", "b").is_err());
    }

    #[test]
    fn test_validate_git_ref_name_rejects_double_dot() {
        assert!(validate_git_ref_name("foo..bar", "b").is_err());
    }

    #[test]
    fn test_validate_git_ref_name_rejects_empty() {
        assert!(validate_git_ref_name("", "b").is_err());
    }

    #[test]
    fn test_validate_git_ref_name_accepts_valid_refs() {
        assert!(validate_git_ref_name("feature/add-login", "b").is_ok());
        assert!(validate_git_ref_name("v1.2.3", "b").is_ok());
        assert!(validate_git_ref_name("release/2026-04-17", "b").is_ok());
    }

    // ─── wildcard_match ─────────────────────────────────────────────────

    #[test]
    fn test_wildcard_match_exact() {
        assert!(wildcard_match("hello", "hello"));
        assert!(!wildcard_match("hello", "world"));
    }

    #[test]
    fn test_wildcard_match_star_any() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("*", "a/b/c"));
    }

    #[test]
    fn test_wildcard_match_trailing_star() {
        assert!(wildcard_match("agent-*", "agent-created"));
        assert!(wildcard_match("agent-*", "agent-"));
        assert!(!wildcard_match("agent-*", "bot-created"));
    }

    #[test]
    fn test_wildcard_match_middle_star() {
        assert!(wildcard_match("a*z", "az"));
        assert!(wildcard_match("a*z", "abcz"));
        assert!(!wildcard_match("a*z", "abcy"));
    }

    #[test]
    fn test_wildcard_match_star_crosses_slash() {
        // Unlike file-path globs, * matches across /
        assert!(wildcard_match("team/*", "team/sub/item"));
        assert!(wildcard_match("prefix/*@main", "prefix/a/b/c@main"));
    }

    #[test]
    fn test_wildcard_match_multiple_stars() {
        assert!(wildcard_match("*-*", "a-b"));
        assert!(wildcard_match("*-*", "abc-def"));
        assert!(!wildcard_match("*-*", "abc"));
    }

    #[test]
    fn test_wildcard_match_case_sensitive() {
        // wildcard_match itself is case-sensitive
        assert!(!wildcard_match("Hello", "hello"));
    }

    // ─── tag_matches_pattern ───────────────────────────────────────────────

    #[test]
    fn test_tag_matches_pattern_exact_case_insensitive() {
        assert!(tag_matches_pattern("Review", "review"));
        assert!(tag_matches_pattern("AUTOMATED", "Automated"));
        assert!(tag_matches_pattern("automated", "automated"));
    }

    #[test]
    fn test_tag_matches_pattern_exact_mismatch() {
        assert!(!tag_matches_pattern("other", "review"));
    }

    #[test]
    fn test_tag_matches_pattern_prefix_wildcard_case_insensitive() {
        // Uppercase pattern prefix must match lowercase tag
        assert!(tag_matches_pattern("agent-created", "Agent-*"));
        // Lowercase pattern prefix must match mixed-case tag
        assert!(tag_matches_pattern("Agent-Review", "agent-*"));
        // Exact prefix boundary
        assert!(tag_matches_pattern("agent-", "agent-*"));
    }

    #[test]
    fn test_tag_matches_pattern_prefix_wildcard_mismatch() {
        assert!(!tag_matches_pattern("bot-created", "agent-*"));
    }

    #[test]
    fn test_tag_matches_pattern_star_only_matches_everything() {
        assert!(tag_matches_pattern("anything", "*"));
        assert!(tag_matches_pattern("", "*"));
    }

    #[test]
    fn test_tag_matches_pattern_middle_wildcard() {
        // Glob wildcard in the middle of the pattern
        assert!(tag_matches_pattern(
            "copilot:repo=msazuresphere/4x4/VsCodeExtension@main",
            "copilot:repo=msazuresphere/4x4/*@main"
        ));
        assert!(tag_matches_pattern(
            "copilot:repo=msazuresphere/4x4/DevTools@main",
            "copilot:repo=msazuresphere/4x4/*@main"
        ));
        // Wrong suffix should not match
        assert!(!tag_matches_pattern(
            "copilot:repo=msazuresphere/4x4/DevTools@dev",
            "copilot:repo=msazuresphere/4x4/*@main"
        ));
    }

    #[test]
    fn test_tag_matches_pattern_middle_wildcard_case_insensitive() {
        assert!(tag_matches_pattern(
            "Copilot:Repo=MSAzureSphere/4x4/Tools@Main",
            "copilot:repo=msazuresphere/4x4/*@main"
        ));
    }

    #[test]
    fn test_tag_matches_pattern_star_crosses_slash() {
        // Hierarchical tags: * must match across /
        assert!(tag_matches_pattern("team/subgroup/item", "team/*"));
    }

    // ─── name_matches_pattern ───────────────────────────────────────────────

    #[test]
    fn test_name_matches_pattern_case_sensitive() {
        assert!(name_matches_pattern("report", "report"));
        assert!(!name_matches_pattern("Report", "report"));
    }

    #[test]
    fn test_name_matches_pattern_wildcard() {
        assert!(name_matches_pattern("agent-report-123", "agent-*"));
        assert!(name_matches_pattern("agent-report", "agent-*"));
        assert!(!name_matches_pattern("bot-report", "agent-*"));
    }

    // ─── lookup_allowed_repository ──────────────────────────────────────

    fn sample_allowed() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "repo-sdk-ftdidevicecontrol".to_string(),
            "4x4/sdk-FtdiDeviceControl".to_string(),
        );
        m.insert(
            "repo-sdk-devicecommunication".to_string(),
            "4x4/sdk-DeviceCommunication".to_string(),
        );
        m
    }

    #[test]
    fn test_lookup_allowed_repository_by_alias() {
        let m = sample_allowed();
        assert_eq!(
            lookup_allowed_repository("repo-sdk-ftdidevicecontrol", &m),
            Some(&"4x4/sdk-FtdiDeviceControl".to_string())
        );
    }

    #[test]
    fn test_lookup_allowed_repository_by_full_value() {
        let m = sample_allowed();
        assert_eq!(
            lookup_allowed_repository("4x4/sdk-FtdiDeviceControl", &m),
            Some(&"4x4/sdk-FtdiDeviceControl".to_string())
        );
    }

    #[test]
    fn test_lookup_allowed_repository_by_trailing_name() {
        let m = sample_allowed();
        // Exact case
        assert_eq!(
            lookup_allowed_repository("sdk-FtdiDeviceControl", &m),
            Some(&"4x4/sdk-FtdiDeviceControl".to_string())
        );
        // Case-insensitive (ADO repo names are case-insensitive)
        assert_eq!(
            lookup_allowed_repository("sdk-ftdidevicecontrol", &m),
            Some(&"4x4/sdk-FtdiDeviceControl".to_string())
        );
        assert_eq!(
            lookup_allowed_repository("SDK-DEVICECOMMUNICATION", &m),
            Some(&"4x4/sdk-DeviceCommunication".to_string())
        );
    }

    #[test]
    fn test_lookup_allowed_repository_no_match() {
        let m = sample_allowed();
        assert_eq!(lookup_allowed_repository("does-not-exist", &m), None);
        // Partial name should not match
        assert_eq!(lookup_allowed_repository("sdk", &m), None);
    }

    #[test]
    fn test_lookup_allowed_repository_no_slash_value() {
        let mut m = std::collections::HashMap::new();
        m.insert("alias".to_string(), "PlainName".to_string());
        // Full value
        assert_eq!(
            lookup_allowed_repository("PlainName", &m),
            Some(&"PlainName".to_string())
        );
        // Case-insensitive trailing match
        assert_eq!(
            lookup_allowed_repository("plainname", &m),
            Some(&"PlainName".to_string())
        );
    }

    #[test]
    fn test_lookup_allowed_repository_case_insensitive_full_value() {
        let m = sample_allowed();
        // Case-insensitive on the full "project/repo" value
        assert_eq!(
            lookup_allowed_repository("4x4/SDK-FTDIDEVICECONTROL", &m),
            Some(&"4x4/sdk-FtdiDeviceControl".to_string())
        );
        assert_eq!(
            lookup_allowed_repository("4X4/sdk-ftdidevicecontrol", &m),
            Some(&"4x4/sdk-FtdiDeviceControl".to_string())
        );
    }

    #[test]
    fn test_lookup_allowed_repository_rejects_ambiguous_bare_name() {
        let allowed = std::collections::HashMap::from([
            ("tools-a".to_string(), "ProjectA/tools".to_string()),
            ("tools-b".to_string(), "ProjectB/tools".to_string()),
        ]);

        assert_eq!(lookup_allowed_repository_alias("tools", &allowed), None);
        assert_eq!(lookup_allowed_repository("tools", &allowed), None);
        assert_eq!(
            lookup_allowed_repository_alias("ProjectA/tools", &allowed),
            Some(&"tools-a".to_string())
        );
    }

    // ─── resolve_repo_name ──────────────────────────────────────────────

    fn ctx_with(
        repository_name: Option<&str>,
        allowed: std::collections::HashMap<String, String>,
    ) -> ExecutionContext {
        ExecutionContext {
            repository_name: repository_name.map(|s| s.to_string()),
            allowed_repositories: allowed,
            repo_refs: std::collections::HashMap::new(),
            ..Default::default()
        }
    }

    #[test]
    fn split_repository_target_name_rejects_too_many_segments() {
        let error =
            split_repository_target_name("Project/team/repository", "Current Project").unwrap_err();

        assert!(
            error
                .message
                .contains("must be a repository name or project/repository"),
            "{}",
            error.message
        );
    }

    fn repository_target_ctx() -> ExecutionContext {
        ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/current-org/".to_string()),
            ado_organization: Some("current-org".to_string()),
            ado_project: Some("Current Project".to_string()),
            repository_id: Some("self-id".to_string()),
            repository_name: Some("Current Project/self-repo".to_string()),
            ..Default::default()
        }
    }

    fn cross_org_allow() -> Vec<crate::compile::types::AdoOrganizationScope> {
        vec![crate::compile::types::AdoOrganizationScope {
            organization: crate::secure::AdoOrganization::parse("other-org").unwrap(),
            projects: vec![crate::compile::types::AdoProjectScope {
                project: crate::secure::AdoProject::parse("Other Project").unwrap(),
                project_id: None,
                repositories: vec![crate::secure::AdoRepository::parse("target-repo").unwrap()],
            }],
        }]
    }

    #[test]
    fn repository_write_target_resolves_self_with_repository_id() {
        let ctx = repository_target_ctx();

        let target = resolve_repository_write_target(Some("self"), &ctx).unwrap();

        assert_eq!(target.organization, "current-org");
        assert_eq!(target.project, "Current Project");
        assert_eq!(target.repository, "self-repo");
        assert_eq!(target.repository_locator(), "self-id");
        assert!(!target.cross_organization);
    }

    #[test]
    fn repository_write_target_resolves_same_org_checkout_project() {
        let mut ctx = repository_target_ctx();
        configure_repository_write_context(
            &mut ctx,
            &["tools".to_string()],
            vec![RepositoryTargetSpec {
                alias: "tools".to_string(),
                repo_type: "git".to_string(),
                name: "Tools Project/tooling".to_string(),
                organization: None,
                endpoint: None,
            }],
            None,
            &[],
        );

        let target = resolve_repository_write_target(Some("tooling"), &ctx).unwrap();

        assert_eq!(target.organization, "current-org");
        assert_eq!(target.project, "Tools Project");
        assert_eq!(target.repository, "tooling");
        assert!(!target.cross_organization);
    }

    #[test]
    fn repository_write_target_resolves_allowed_cross_org_checkout() {
        let mut ctx = repository_target_ctx();
        configure_repository_write_context(
            &mut ctx,
            &["target".to_string()],
            vec![RepositoryTargetSpec {
                alias: "target".to_string(),
                repo_type: "git".to_string(),
                name: "Other Project/target-repo".to_string(),
                organization: Some("other-org".to_string()),
                endpoint: Some("cross-org-checkout".to_string()),
            }],
            Some(crate::compile::types::WriteConnectionType::AzureDevOps),
            &cross_org_allow(),
        );

        let target = resolve_repository_write_target(Some("target"), &ctx).unwrap();

        assert_eq!(target.organization_url, "https://dev.azure.com/other-org");
        assert_eq!(target.project, "Other Project");
        assert_eq!(target.repository, "target-repo");
        assert!(target.cross_organization);
    }

    #[test]
    fn exact_cross_org_alias_wins_over_self_repository_name() {
        let mut ctx = repository_target_ctx();
        ctx.repository_name = Some("Current Project/target".to_string());
        configure_repository_write_context(
            &mut ctx,
            &["target".to_string()],
            vec![RepositoryTargetSpec {
                alias: "target".to_string(),
                repo_type: "git".to_string(),
                name: "Other Project/target-repo".to_string(),
                organization: Some("other-org".to_string()),
                endpoint: Some("cross-org-checkout".to_string()),
            }],
            Some(crate::compile::types::WriteConnectionType::AzureDevOps),
            &cross_org_allow(),
        );

        let target = resolve_repository_write_target(Some("target"), &ctx).unwrap();

        assert_eq!(target.organization, "other-org");
        assert_eq!(target.repository, "target-repo");
    }

    #[test]
    fn repository_name_collision_between_self_and_alias_is_ambiguous() {
        let mut ctx = repository_target_ctx();
        ctx.repository_name = Some("Current Project/shared".to_string());
        configure_repository_write_context(
            &mut ctx,
            &["other".to_string()],
            vec![RepositoryTargetSpec {
                alias: "other".to_string(),
                repo_type: "git".to_string(),
                name: "Other Project/shared".to_string(),
                organization: None,
                endpoint: None,
            }],
            None,
            &[],
        );

        assert!(canonical_repository_alias("shared", &ctx).is_none());
    }

    #[test]
    fn repository_write_target_rejects_incomplete_or_unauthorized_cross_org() {
        for (organization, connection_type, allow, expected) in [
            (
                None,
                Some(crate::compile::types::WriteConnectionType::AzureDevOps),
                cross_org_allow(),
                "no `repos.organization`",
            ),
            (
                Some("other-org".to_string()),
                Some(crate::compile::types::WriteConnectionType::AzureRm),
                cross_org_allow(),
                "connection-type: azureDevOps",
            ),
            (
                Some("other-org".to_string()),
                Some(crate::compile::types::WriteConnectionType::AzureDevOps),
                Vec::new(),
                "not listed in permissions.write.allow",
            ),
        ] {
            let mut ctx = repository_target_ctx();
            configure_repository_write_context(
                &mut ctx,
                &["target".to_string()],
                vec![RepositoryTargetSpec {
                    alias: "target".to_string(),
                    repo_type: "git".to_string(),
                    name: "Other Project/target-repo".to_string(),
                    organization,
                    endpoint: Some("cross-org-checkout".to_string()),
                }],
                connection_type,
                &allow,
            );

            let error = resolve_repository_write_target(Some("target"), &ctx).unwrap_err();
            assert!(error.message.contains(expected), "{}", error.message);
        }
    }

    #[test]
    fn repository_write_target_rejects_endpoint_declared_as_current_org() {
        let mut ctx = repository_target_ctx();
        configure_repository_write_context(
            &mut ctx,
            &["target".to_string()],
            vec![RepositoryTargetSpec {
                alias: "target".to_string(),
                repo_type: "git".to_string(),
                name: "Other Project/target-repo".to_string(),
                organization: Some("current-org".to_string()),
                endpoint: Some("cross-org-checkout".to_string()),
            }],
            Some(crate::compile::types::WriteConnectionType::AzureDevOps),
            &cross_org_allow(),
        );

        let error = resolve_repository_write_target(Some("target"), &ctx).unwrap_err();

        assert!(error.message.contains("declares the pipeline's current organization"));
    }

    #[test]
    fn path_segment_encodes_literal_percent_sequences() {
        assert_eq!(
            utf8_percent_encode("Project%2FArchive", PATH_SEGMENT).to_string(),
            "Project%252FArchive"
        );
        assert_eq!(
            utf8_percent_encode("Repo%23Name%3F", PATH_SEGMENT).to_string(),
            "Repo%2523Name%253F"
        );
    }

    #[test]
    fn ado_request_auth_matches_connection_type() {
        let client = reqwest::Client::new();
        let bearer = authenticate_ado_request(
            client.get("https://example.test"),
            "entra-token",
            Some(crate::compile::types::WriteConnectionType::AzureDevOps),
        )
        .build()
        .unwrap();
        assert_eq!(
            bearer
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer entra-token")
        );

        let basic = authenticate_ado_request(
            client.get("https://example.test"),
            "pat-token",
            Some(crate::compile::types::WriteConnectionType::AzureRm),
        )
        .build()
        .unwrap();
        assert_eq!(
            basic
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Basic OnBhdC10b2tlbg==")
        );

        let default_auth =
            authenticate_ado_request(client.get("https://example.test"), "pat-token", None)
                .build()
                .unwrap();
        assert_eq!(
            default_auth
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Basic OnBhdC10b2tlbg==")
        );
    }

    #[test]
    fn test_resolve_repository_checkout_dir_distinguishes_root_and_self() {
        let mut ctx = ctx_with(Some("4x4/current-repo"), sample_allowed());
        ctx.source_directory = std::path::PathBuf::from("checkout-root");
        ctx.self_repository_directory =
            std::path::PathBuf::from("checkout-root").join("current-repo");

        assert_eq!(
            resolve_repository_checkout_dir("self", &ctx).unwrap(),
            ctx.self_repository_directory
        );
        assert_eq!(
            resolve_repository_checkout_dir("4X4/CURRENT-REPO", &ctx).unwrap(),
            ctx.self_repository_directory
        );
        assert_eq!(
            resolve_repository_checkout_dir("sdk-ftdidevicecontrol", &ctx).unwrap(),
            ctx.source_directory.join("repo-sdk-ftdidevicecontrol")
        );
        assert_eq!(
            resolve_repository_checkout_dir("4x4/sdk-DeviceCommunication", &ctx).unwrap(),
            ctx.source_directory.join("repo-sdk-devicecommunication")
        );
    }

    #[test]
    fn test_resolve_repository_checkout_dir_rejects_unknown_selector() {
        let ctx = ctx_with(Some("4x4/current-repo"), sample_allowed());
        let err = resolve_repository_checkout_dir("../outside", &ctx).unwrap_err();
        assert!(
            err.to_string()
                .contains("not in the allowed repository list"),
            "got: {err}"
        );
    }

    #[test]
    fn test_resolve_repo_name_self_literal() {
        let ctx = ctx_with(Some("4x4/sdk-FtdiDeviceControl"), sample_allowed());
        assert_eq!(
            resolve_repo_name(Some("self"), &ctx).unwrap(),
            "4x4/sdk-FtdiDeviceControl"
        );
        assert_eq!(
            resolve_repo_name(None, &ctx).unwrap(),
            "4x4/sdk-FtdiDeviceControl"
        );
    }

    #[test]
    fn test_resolve_repo_name_rejects_self_alias_name_collision() {
        let ctx = ctx_with(Some("4x4/sdk-FtdiDeviceControl"), sample_allowed());
        for selector in [
            "sdk-FtdiDeviceControl",
            "sdk-ftdidevicecontrol",
            "4X4/sdk-ftdidevicecontrol",
        ] {
            let error = resolve_repo_name(Some(selector), &ctx).unwrap_err();
            assert!(
                error.message.contains("not in the allowed repository list"),
                "{}",
                error.message
            );
        }
        // Exact checkout aliases still win over name-based ambiguity.
        assert_eq!(
            resolve_repo_name(Some("repo-sdk-ftdidevicecontrol"), &ctx).unwrap(),
            "4x4/sdk-FtdiDeviceControl"
        );
    }

    #[test]
    fn test_resolve_repo_name_alias() {
        let ctx = ctx_with(Some("4x4/some-other-repo"), sample_allowed());
        assert_eq!(
            resolve_repo_name(Some("repo-sdk-devicecommunication"), &ctx).unwrap(),
            "4x4/sdk-DeviceCommunication"
        );
        // Trailing-name match against allowed list
        assert_eq!(
            resolve_repo_name(Some("sdk-DeviceCommunication"), &ctx).unwrap(),
            "4x4/sdk-DeviceCommunication"
        );
    }

    #[test]
    fn test_resolve_repo_name_unknown() {
        let ctx = ctx_with(Some("4x4/some-other-repo"), sample_allowed());
        let err = resolve_repo_name(Some("does-not-exist"), &ctx).unwrap_err();
        assert!(
            err.message.contains("not in the allowed repository list"),
            "got: {:?}",
            err.message
        );
    }
}
