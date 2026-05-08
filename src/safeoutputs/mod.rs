//! Tool parameter and result structs for MCP tools

use crate::{all_safe_output_names, tool_names};
use log::{debug, warn};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

/// Characters to percent-encode in a URL path segment.
/// Encodes the structural delimiters that would break URL parsing if left raw:
/// `#` (fragment), `?` (query), `/` (path separator), and space.
/// This hardens operator-controlled values (project names, wiki names, work item
/// types) against accidental corruption of the URL structure.
pub(crate) const PATH_SEGMENT: &AsciiSet = &CONTROLS.add(b'#').add(b'?').add(b'/').add(b' ');

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

/// Safe-output tools that require write access to ADO.
/// Compile-time derived from tool types via `ToolResult::NAME`.
///
/// Adding a new write-requiring tool: create the struct with `tool_result!{ write = true, ... }`,
/// then add its type to this list.
pub const WRITE_REQUIRING_SAFE_OUTPUTS: &[&str] = tool_names![
    CreateWorkItemResult,
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
];

/// Non-MCP safe-output keys handled by the compiler/executor, not the MCP server.
/// These must not appear in `--enabled-tools` or they cause real MCP tools to be
/// filtered out (the router has no route for them).
pub const NON_MCP_SAFE_OUTPUT_KEYS: &[&str] = &[];

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
/// Azure DevOps repository names are case-insensitive, so the trailing-name fallback
/// matches case-insensitively. Returns the resolved ADO repo name (the map value) on
/// success, or `None` if no entry matches.
pub(crate) fn lookup_allowed_repository<'a>(
    input: &str,
    allowed_repositories: &'a std::collections::HashMap<String, String>,
) -> Option<&'a String> {
    // 1. Exact alias key match
    if let Some(name) = allowed_repositories.get(input) {
        return Some(name);
    }
    // 2. Case-insensitive value match (full "project/repo" or just "repo").
    // ADO repo names are case-insensitive, so accept any case for the full path.
    if let Some((_, name)) = allowed_repositories
        .iter()
        .find(|(_, v)| v.eq_ignore_ascii_case(input))
    {
        return Some(name);
    }
    // 3. Trailing repo-name part match (case-insensitive)
    allowed_repositories.iter().find_map(|(_, v)| {
        let trailing = v.rsplit('/').next().unwrap_or(v.as_str());
        if trailing.eq_ignore_ascii_case(input) {
            Some(v)
        } else {
            None
        }
    })
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
    if input_refers_to_self(alias, ctx) {
        return ctx
            .repository_name
            .clone()
            .ok_or_else(|| ExecutionResult::failure("BUILD_REPOSITORY_NAME not set"));
    }
    lookup_allowed_repository(alias, &ctx.allowed_repositories)
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
    wildcard_match(
        &pattern.to_ascii_lowercase(),
        &tag.to_ascii_lowercase(),
    )
}

/// Return `true` if `name` is matched by `pattern` (**case-sensitive**).
///
/// Uses [`wildcard_match`] for artifact-name allow-lists where case matters.
pub(crate) fn name_matches_pattern(name: &str, pattern: &str) -> bool {
    wildcard_match(pattern, name)
}

/// Validate a string against `git check-ref-format` rules.
///
/// Returns `Ok(())` if the name is valid, or an `Err` describing the violation.
/// This covers the structural rules that Azure DevOps also enforces — catching
/// them early gives clearer error messages than letting the API fail.
pub(crate) fn validate_git_ref_name(name: &str, label: &str) -> anyhow::Result<()> {
    use anyhow::ensure;

    ensure!(!name.is_empty(), "{label} must not be empty");
    ensure!(!name.contains(".."), "{label} must not contain '..'");
    ensure!(!name.contains("@{"), "{label} must not contain '@{{'");
    ensure!(!name.ends_with('.'), "{label} must not end with '.'");
    ensure!(!name.ends_with(".lock"), "{label} must not end with '.lock'");
    ensure!(
        !name.contains('\\'),
        "{label} must not contain backslash"
    );
    ensure!(
        !name.contains("//"),
        "{label} must not contain consecutive slashes"
    );
    for ch in ['~', '^', ':', '?', '*', '['] {
        ensure!(
            !name.contains(ch),
            "{label} must not contain '{ch}'"
        );
    }
    for component in name.split('/') {
        ensure!(
            !component.starts_with('.'),
            "{label} path component must not start with '.'"
        );
    }
    Ok(())
}

mod add_build_tag;
mod add_pr_comment;
mod comment_on_work_item;
mod create_branch;
mod create_git_tag;
mod create_pr;
mod create_wiki_page;
mod create_work_item;
mod link_work_items;
mod missing_data;
mod missing_tool;
mod noop;
mod queue_build;
mod reply_to_pr_comment;
mod report_incomplete;
mod resolve_pr_thread;
mod result;
mod submit_pr_review;
mod update_pr;
mod update_wiki_page;
mod update_work_item;
mod upload_build_attachment;
mod upload_pipeline_artifact;
mod upload_workitem_attachment;

pub use add_build_tag::*;
pub use add_pr_comment::*;
pub use comment_on_work_item::*;
pub use create_branch::*;
pub use create_git_tag::*;
pub use create_pr::*;
pub use create_wiki_page::*;
pub use create_work_item::*;
pub use link_work_items::*;
pub use missing_data::*;
pub use missing_tool::*;
pub use noop::*;
pub use queue_build::*;
pub use reply_to_pr_comment::*;
pub use report_incomplete::*;
pub use resolve_pr_thread::*;
pub use result::{
    ExecutionContext, ExecutionResult, Executor, ToolResult, Validate, anyhow_to_mcp_error,
    org_from_url,
};
pub use submit_pr_review::*;
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
    fn test_write_requiring_subset_of_all_known() {
        for name in WRITE_REQUIRING_SAFE_OUTPUTS {
            assert!(
                ALL_KNOWN_SAFE_OUTPUTS.contains(name),
                "WRITE_REQUIRING_SAFE_OUTPUTS entry '{}' is missing from ALL_KNOWN_SAFE_OUTPUTS",
                name
            );
        }
    }

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
        assert!(CreateWorkItemResult::REQUIRES_WRITE);
        assert!(CommentOnWorkItemResult::REQUIRES_WRITE);
        assert!(UpdateWorkItemResult::REQUIRES_WRITE);
        assert!(CreatePrResult::REQUIRES_WRITE);
        assert!(CreateWikiPageResult::REQUIRES_WRITE);
        assert!(UpdateWikiPageResult::REQUIRES_WRITE);
        assert!(AddPrCommentResult::REQUIRES_WRITE);
        assert!(LinkWorkItemsResult::REQUIRES_WRITE);
        assert!(QueueBuildResult::REQUIRES_WRITE);
        assert!(CreateGitTagResult::REQUIRES_WRITE);
        assert!(AddBuildTagResult::REQUIRES_WRITE);
        assert!(CreateBranchResult::REQUIRES_WRITE);
        assert!(UpdatePrResult::REQUIRES_WRITE);
        assert!(UploadBuildAttachmentResult::REQUIRES_WRITE);
        assert!(UploadPipelineArtifactResult::REQUIRES_WRITE);
        assert!(UploadWorkitemAttachmentResult::REQUIRES_WRITE);
        assert!(SubmitPrReviewResult::REQUIRES_WRITE);
        assert!(ReplyToPrCommentResult::REQUIRES_WRITE);
        assert!(ResolvePrThreadResult::REQUIRES_WRITE);

        // Diagnostic tools (should NOT require write)
        assert!(!NoopResult::REQUIRES_WRITE);
        assert!(!MissingDataResult::REQUIRES_WRITE);
        assert!(!MissingToolResult::REQUIRES_WRITE);
        assert!(!ReportIncompleteResult::REQUIRES_WRITE);
    }

    /// Verify ALL_KNOWN_SAFE_OUTPUTS has exactly the right count:
    /// write tools + diagnostics + non-MCP keys.
    #[test]
    fn test_all_known_completeness() {
        // The three sub-lists must be disjoint — a tool in multiple lists would
        // be duplicated in ALL_KNOWN and the count would mismatch.
        for name in WRITE_REQUIRING_SAFE_OUTPUTS {
            assert!(
                !ALWAYS_ON_TOOLS.contains(name),
                "Tool '{}' appears in both WRITE_REQUIRING and ALWAYS_ON — lists must be disjoint",
                name
            );
            assert!(
                !NON_MCP_SAFE_OUTPUT_KEYS.contains(name),
                "Tool '{}' appears in both WRITE_REQUIRING and NON_MCP — lists must be disjoint",
                name
            );
        }
        for name in ALWAYS_ON_TOOLS {
            assert!(
                !NON_MCP_SAFE_OUTPUT_KEYS.contains(name),
                "Tool '{}' appears in both ALWAYS_ON and NON_MCP — lists must be disjoint",
                name
            );
        }

        let expected = WRITE_REQUIRING_SAFE_OUTPUTS.len()
            + ALWAYS_ON_TOOLS.len()
            + NON_MCP_SAFE_OUTPUT_KEYS.len();
        assert_eq!(
            ALL_KNOWN_SAFE_OUTPUTS.len(),
            expected,
            "ALL_KNOWN_SAFE_OUTPUTS should be the union of write + diagnostic + non-MCP lists"
        );
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

    // ─── resolve_repo_name ──────────────────────────────────────────────

    fn ctx_with(
        repository_name: Option<&str>,
        allowed: std::collections::HashMap<String, String>,
    ) -> ExecutionContext {
        let mut ctx = ExecutionContext::default();
        ctx.repository_name = repository_name.map(|s| s.to_string());
        ctx.allowed_repositories = allowed;
        ctx
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
    fn test_resolve_repo_name_self_by_repository_name() {
        let ctx = ctx_with(Some("4x4/sdk-FtdiDeviceControl"), sample_allowed());
        // Trailing-name match on ctx.repository_name (case-insensitive)
        assert_eq!(
            resolve_repo_name(Some("sdk-FtdiDeviceControl"), &ctx).unwrap(),
            "4x4/sdk-FtdiDeviceControl"
        );
        assert_eq!(
            resolve_repo_name(Some("sdk-ftdidevicecontrol"), &ctx).unwrap(),
            "4x4/sdk-FtdiDeviceControl"
        );
        // Full-value match on ctx.repository_name (case-insensitive)
        assert_eq!(
            resolve_repo_name(Some("4X4/sdk-ftdidevicecontrol"), &ctx).unwrap(),
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
