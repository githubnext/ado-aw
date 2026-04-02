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
/// return `None` because the server handles branching internally.
pub(crate) async fn resolve_wiki_branch(
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    wiki_name: &str,
    token: &str,
    configured_branch: Option<&str>,
) -> Option<String> {
    // Explicit configuration always wins.
    if let Some(b) = configured_branch {
        return Some(b.to_owned());
    }

    let url = format!(
        "{}/{}/_apis/wiki/wikis/{}",
        org_url.trim_end_matches('/'),
        utf8_percent_encode(project, PATH_SEGMENT),
        utf8_percent_encode(wiki_name, PATH_SEGMENT),
    );

    let resp = client
        .get(&url)
        .query(&[("api-version", "7.0")])
        .basic_auth("", Some(token))
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        warn!(
            "Wiki metadata request returned HTTP {} — skipping branch auto-detection",
            resp.status()
        );
        return None;
    }

    let body: serde_json::Value = resp.json().await.ok()?;

    // type 0 = project wiki, type 1 = code wiki
    let wiki_type = body.get("type").and_then(|v| v.as_u64()).unwrap_or(0);
    if wiki_type != 1 {
        debug!("Wiki is a project wiki (type {wiki_type}) — no branch needed");
        return None;
    }

    // Code wiki: extract the published branch from versions[0].version
    let branch = body
        .get("versions")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());

    if branch.is_none() {
        warn!("Code wiki detected but versions array is empty — branch auto-detection failed");
    } else {
        debug!("Detected code wiki — resolved branch: {branch:?}");
    }
    branch
}

mod comment_on_work_item;
mod create_pr;
mod create_wiki_page;
mod create_work_item;
mod update_wiki_page;
pub mod memory;
mod missing_data;
mod missing_tool;
mod noop;
mod result;
mod update_work_item;

pub use comment_on_work_item::*;
pub use create_pr::*;
pub use create_wiki_page::*;
pub use create_work_item::*;
pub use update_wiki_page::*;
pub use missing_data::*;
pub use missing_tool::*;
pub use noop::*;
pub use result::{
    ExecutionContext, ExecutionResult, Executor, ToolResult, Validate, anyhow_to_mcp_error,
};
pub use update_work_item::*;
