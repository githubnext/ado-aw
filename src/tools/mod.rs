//! Tool parameter and result structs for MCP tools

use log::{debug, warn};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

/// Characters to percent-encode in a URL path segment.
/// Encodes the structural delimiters that would break URL parsing if left raw:
/// `#` (fragment), `?` (query), `/` (path separator), and space.
/// This hardens operator-controlled values (project names, wiki names, work item
/// types) against accidental corruption of the URL structure.
pub(crate) const PATH_SEGMENT: &AsciiSet = &CONTROLS.add(b'#').add(b'?').add(b'/').add(b' ');

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

/// Resolve a repository alias to its ADO repo name.
///
/// "self" (or None) → `ctx.repository_name`, otherwise look up in `ctx.allowed_repositories`.
pub(crate) fn resolve_repo_name(
    repo_alias: Option<&str>,
    ctx: &ExecutionContext,
) -> Result<String, ExecutionResult> {
    let alias = repo_alias.unwrap_or("self");
    if alias == "self" {
        ctx.repository_name
            .clone()
            .ok_or_else(|| ExecutionResult::failure("BUILD_REPOSITORY_NAME not set"))
    } else {
        ctx.allowed_repositories
            .get(alias)
            .cloned()
            .ok_or_else(|| {
                ExecutionResult::failure(format!(
                    "Repository '{}' is not in the allowed repository list",
                    alias
                ))
            })
    }
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
    ensure!(!name.contains("@{{"), "{label} must not contain '@{{'");
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
pub mod memory;
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
mod upload_attachment;

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
};
pub use submit_pr_review::*;
pub use update_pr::*;
pub use update_wiki_page::*;
pub use update_work_item::*;
pub use upload_attachment::*;
