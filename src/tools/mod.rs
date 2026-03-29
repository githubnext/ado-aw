//! Tool parameter and result structs for MCP tools

use percent_encoding::{AsciiSet, CONTROLS};

/// Characters to percent-encode in a URL path segment.
/// Encodes the structural delimiters that would break URL parsing if left raw:
/// `#` (fragment), `?` (query), `/` (path separator), and space.
/// This hardens operator-controlled values (project names, wiki names, work item
/// types) against accidental corruption of the URL structure.
pub(crate) const PATH_SEGMENT: &AsciiSet = &CONTROLS.add(b'#').add(b'?').add(b'/').add(b' ');

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
