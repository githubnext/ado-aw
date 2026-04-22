//! Stage 3 execution: Parse safe outputs and execute actions
//!
//! After the agent (Stage 1) generates safe outputs as an NDJSON file,
//! Stage 3 parses this file and executes the corresponding actions.

use anyhow::{Result, bail};
use log::{debug, error, info, warn};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

use crate::ndjson::{self, SAFE_OUTPUT_FILENAME};
use crate::sanitize::SanitizeContent;
use crate::safeoutputs::{
    AddBuildTagResult, AddPrCommentResult, CreateBranchResult, CreateGitTagResult,
    CreatePrResult, CreateWikiPageResult, CreateWorkItemResult, CommentOnWorkItemResult,
    ExecutionContext, ExecutionResult, Executor, LinkWorkItemsResult, QueueBuildResult,
    ReplyToPrCommentResult, ReportIncompleteResult, ResolvePrThreadResult, SubmitPrReviewResult,
    ToolResult, UpdatePrResult, UpdateWikiPageResult, UpdateWorkItemResult,
    UploadAttachmentResult,
};

// Re-export memory types for use by main.rs
pub use crate::tools::cache_memory::{MemoryConfig, process_agent_memory};

/// Execute all safe outputs from the NDJSON file in the specified directory
pub async fn execute_safe_outputs(
    safe_output_dir: &Path,
    ctx: &ExecutionContext,
) -> Result<Vec<ExecutionResult>> {
    let safe_output_path = safe_output_dir.join(SAFE_OUTPUT_FILENAME);

    info!("Stage 3 execution starting");
    debug!("Safe output directory: {}", safe_output_dir.display());
    debug!("Source directory: {}", ctx.source_directory.display());
    debug!(
        "ADO org: {}",
        ctx.ado_org_url.as_deref().unwrap_or("<not set>")
    );
    debug!(
        "ADO project: {}",
        ctx.ado_project.as_deref().unwrap_or("<not set>")
    );
    debug!(
        "Repository ID: {}",
        ctx.repository_id.as_deref().unwrap_or("<not set>")
    );
    debug!(
        "Repository name: {}",
        ctx.repository_name.as_deref().unwrap_or("<not set>")
    );
    if !ctx.allowed_repositories.is_empty() {
        debug!(
            "Allowed repositories: {}",
            ctx.allowed_repositories
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if !safe_output_path.exists() {
        info!(
            "No safe outputs file found at: {}",
            safe_output_path.display()
        );
        println!(
            "No safe outputs file found at: {}",
            safe_output_path.display()
        );
        return Ok(vec![]);
    }

    info!("Processing safe outputs: {}", safe_output_path.display());
    println!("Processing safe outputs: {}", safe_output_path.display());

    let entries = ndjson::read_ndjson_file(&safe_output_path).await?;

    if entries.is_empty() {
        info!("Safe outputs file is empty");
        println!("Safe outputs file is empty");
        return Ok(vec![]);
    }

    info!("Found {} safe output(s) to execute", entries.len());
    println!("Found {} safe output(s) to execute", entries.len());

    // Log summary of what we're about to execute
    for (i, entry) in entries.iter().enumerate() {
        if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
            debug!("[{}/{}] Queued: {}", i + 1, entries.len(), name);
        }
    }

    // Build budget map: tool_name → (executed_count, max_allowed).
    // Each tool declares its DEFAULT_MAX via the ToolResult trait; the operator can
    // override it with `max` in the front-matter config JSON.
    //
    // IMPORTANT: When adding a new ToolResult implementor, also register it here
    // so its budget is enforced. There is no compile-time guard for this.
    let mut budgets: HashMap<&str, (usize, usize)> = HashMap::new();
    macro_rules! register_budgets {
        ($($tool:ty),+ $(,)?) => {
            $({
                let name = <$tool>::NAME;
                let default = <$tool>::DEFAULT_MAX;
                let max = resolve_max(ctx, name, default);
                budgets.insert(name, (0, max));
            })+
        };
    }
    register_budgets!(
        CreateWorkItemResult,
        CreatePrResult,
        UpdateWorkItemResult,
        CommentOnWorkItemResult,
        CreateWikiPageResult,
        UpdateWikiPageResult,
        AddPrCommentResult,
        LinkWorkItemsResult,
        QueueBuildResult,
        CreateGitTagResult,
        AddBuildTagResult,
        CreateBranchResult,
        UpdatePrResult,
        UploadAttachmentResult,
        SubmitPrReviewResult,
        ReplyToPrCommentResult,
        ResolvePrThreadResult,
    );

    let mut results = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let entry_json = serde_json::to_string(entry).unwrap_or_else(|_| "<invalid>".to_string());
        debug!(
            "[{}/{}] Executing entry: {}",
            i + 1,
            entries.len(),
            entry_json
        );

        // Generic budget enforcement: skip excess entries rather than aborting the whole batch.
        // Budget is consumed before execution so that failed attempts (target policy rejection,
        // network errors) still count — this prevents unbounded retries against a failing endpoint.
        if let Some(tool_name) = entry.get("name").and_then(|n| n.as_str()) {
            if let Some((executed, max)) = budgets.get_mut(tool_name) {
                let context_id = extract_entry_context(entry);
                if let Some(result) = check_budget(entries.len(), i, tool_name, &context_id, *executed, *max) {
                    results.push(result);
                    continue;
                }
                *executed += 1;
            }
        }

        match execute_safe_output(entry, ctx).await {
            Ok((tool_name, result)) => {
                if result.is_warning() {
                    warn!(
                        "[{}/{}] {} warning: {}",
                        i + 1,
                        entries.len(),
                        tool_name,
                        result.message
                    );
                } else if result.success {
                    info!(
                        "[{}/{}] {} succeeded: {}",
                        i + 1,
                        entries.len(),
                        tool_name,
                        result.message
                    );
                } else {
                    warn!(
                        "[{}/{}] {} failed: {}",
                        i + 1,
                        entries.len(),
                        tool_name,
                        result.message
                    );
                }
                let symbol = if result.is_warning() { "⚠" } else if result.success { "✓" } else { "✗" };
                println!(
                    "[{}/{}] {} - {} - {}",
                    i + 1,
                    entries.len(),
                    tool_name,
                    symbol,
                    result.message
                );
                results.push(result);
            }
            Err(e) => {
                error!("[{}/{}] Execution error: {}", i + 1, entries.len(), e);
                let result = ExecutionResult::failure(format!("Failed to execute entry: {}", e));
                println!("[{}/{}] ✗ - {}", i + 1, entries.len(), result.message);
                results.push(result);
            }
        }
    }

    // Log final summary
    let success_count = results.iter().filter(|r| r.success && !r.is_warning()).count();
    let warning_count = results.iter().filter(|r| r.is_warning()).count();
    let failure_count = results.iter().filter(|r| !r.success).count();
    info!(
        "Stage 3 execution complete: {} succeeded, {} warnings, {} failed",
        success_count, warning_count, failure_count
    );

    Ok(results)
}

/// Parse a JSON entry as `T` and run it through `execute_sanitized`.
///
/// This is the common path for all tools that implement `Executor`. The tool name
/// is used only for the error message so callers don't have to repeat it.
async fn dispatch_tool<T>(
    tool_name: &str,
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<ExecutionResult>
where
    T: DeserializeOwned + Executor,
{
    debug!("Parsing {} payload", tool_name);
    let mut output: T = serde_json::from_value(entry.clone())
        .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", tool_name, e))?;
    output.execute_sanitized(ctx).await
}

/// Execute a single safe output entry, returning the tool name and result
pub async fn execute_safe_output(
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<(String, ExecutionResult)> {
    // First check the name field to dispatch correctly
    let tool_name = entry
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("Safe output missing 'name' field"))?;

    debug!("Dispatching tool: {}", tool_name);

    // Dispatch based on tool name. All standard tools go through `dispatch_tool` which
    // handles deserialization and sanitized execution uniformly. Special cases (informational
    // outputs and report-incomplete) are handled inline.
    let result = match tool_name {
        "create-work-item" => dispatch_tool::<CreateWorkItemResult>(tool_name, entry, ctx).await?,
        "comment-on-work-item" => dispatch_tool::<CommentOnWorkItemResult>(tool_name, entry, ctx).await?,
        "update-work-item" => dispatch_tool::<UpdateWorkItemResult>(tool_name, entry, ctx).await?,
        "create-pull-request" => dispatch_tool::<CreatePrResult>(tool_name, entry, ctx).await?,
        "update-wiki-page" => dispatch_tool::<UpdateWikiPageResult>(tool_name, entry, ctx).await?,
        "create-wiki-page" => dispatch_tool::<CreateWikiPageResult>(tool_name, entry, ctx).await?,
        "add-pr-comment" => dispatch_tool::<AddPrCommentResult>(tool_name, entry, ctx).await?,
        "link-work-items" => dispatch_tool::<LinkWorkItemsResult>(tool_name, entry, ctx).await?,
        "queue-build" => dispatch_tool::<QueueBuildResult>(tool_name, entry, ctx).await?,
        "create-git-tag" => dispatch_tool::<CreateGitTagResult>(tool_name, entry, ctx).await?,
        "add-build-tag" => dispatch_tool::<AddBuildTagResult>(tool_name, entry, ctx).await?,
        "create-branch" => dispatch_tool::<CreateBranchResult>(tool_name, entry, ctx).await?,
        "update-pr" => dispatch_tool::<UpdatePrResult>(tool_name, entry, ctx).await?,
        "upload-attachment" => dispatch_tool::<UploadAttachmentResult>(tool_name, entry, ctx).await?,
        "submit-pr-review" => dispatch_tool::<SubmitPrReviewResult>(tool_name, entry, ctx).await?,
        "reply-to-pr-review-comment" => dispatch_tool::<ReplyToPrCommentResult>(tool_name, entry, ctx).await?,
        "resolve-pr-thread" => dispatch_tool::<ResolvePrThreadResult>(tool_name, entry, ctx).await?,
        // Informational outputs — no side effects, always succeed
        "noop" | "missing-tool" | "missing-data" => {
            debug!("Skipping informational entry: {}", tool_name);
            ExecutionResult::success(format!("Skipped informational output: {}", tool_name))
        }
        // report-incomplete does not implement Executor; Stage 3 surfaces its reason as a failure
        "report-incomplete" => {
            let mut output: ReportIncompleteResult = serde_json::from_value(entry.clone())
                .map_err(|e| anyhow::anyhow!("Failed to parse report-incomplete: {}", e))?;
            output.sanitize_content_fields();
            debug!("report-incomplete: {}", output.reason);
            ExecutionResult::failure(format!("Agent reported task incomplete: {}", output.reason))
        }
        other => {
            error!("Unknown tool type: {}", other);
            bail!("Unknown tool type: {}. No executor registered.", other)
        }
    };

    Ok((tool_name.to_string(), result))
}

/// Read the operator's `max` override from the tool's config JSON, falling back to the
/// tool's `DEFAULT_MAX` (declared on the `ToolResult` trait) when not configured.
fn resolve_max(ctx: &ExecutionContext, tool_name: &str, default_max: u32) -> usize {
    ctx.tool_configs
        .get(tool_name)
        .and_then(|v| v.get("max"))
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(default_max as usize)
}

/// Extract a human-readable context identifier from a safe-output entry for log messages.
/// Called before sanitization, so all string values are stripped of control characters
/// to prevent log injection.
fn extract_entry_context(entry: &Value) -> String {
    if let Some(id) = entry.get("id").and_then(|v| v.as_u64()) {
        return format!(" (work item #{})", id);
    }
    if let Some(id) = entry.get("work_item_id").and_then(|v| v.as_i64()) {
        return format!(" (work item #{})", id);
    }
    if let Some(title) = entry.get("title").and_then(|v| v.as_str()) {
        let clean: String = title.chars().filter(|c| !c.is_control()).collect();
        let truncated: &str = if clean.chars().count() > 40 {
            &clean[..clean.char_indices().nth(40).map(|(i, _)| i).unwrap_or(clean.len())]
        } else {
            &clean
        };
        return format!(" (\"{}\")", truncated);
    }
    if let Some(path) = entry.get("path").and_then(|v| v.as_str()) {
        let clean: String = path.chars().filter(|c| !c.is_control()).collect();
        return format!(" (path: {})", clean);
    }
    String::new()
}

/// Returns `Some(result)` when the budget for `tool_name` is exhausted so the caller can push the
/// result and `continue` to the next entry. Returns `None` when a budget slot is still available
/// and the caller should proceed with execution.
///
/// `total` is the total number of entries (for the `[i/total]` log prefix), `i` is the
/// zero-based index of the current entry, `wi_id` is a pre-formatted context string like
/// `" (work item #42)"` or `""`.
fn check_budget(
    total: usize,
    i: usize,
    tool_name: &str,
    wi_id: &str,
    executed: usize,
    max: usize,
) -> Option<ExecutionResult> {
    if executed < max {
        return None;
    }
    warn!(
        "[{}/{}] Skipping {}{} entry: max ({}) already reached for this run",
        i + 1,
        total,
        tool_name,
        wi_id,
        max
    );
    let result = ExecutionResult::failure(format!(
        "Skipped{}: maximum {} count ({}) already reached. \
         Increase 'max' in safe-outputs.{} to allow more.",
        wi_id, tool_name, max, tool_name
    ));
    println!(
        "[{}/{}] {} - ✗ - {}",
        i + 1,
        total,
        tool_name,
        result.message
    );
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_execute_unknown_tool_fails() {
        let entry = serde_json::json!({"name": "unknown_tool", "foo": "bar"});
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: false,
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown tool type")
        );
    }

    #[tokio::test]
    async fn test_execute_create_work_item_missing_context() {
        let entry = serde_json::json!({
            "name": "create-work-item",
            "title": "Test work item",
            "description": "A description that is definitely longer than thirty characters."
        });

        // Context without required fields
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: false,
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    #[tokio::test]
    async fn test_execute_missing_name_fails() {
        let entry = serde_json::json!({"foo": "bar"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name"));
    }

    #[tokio::test]
    async fn test_execute_noop_succeeds() {
        let entry = serde_json::json!({"name": "noop", "context": "test"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok());
        let (tool_name, result) = result.unwrap();
        assert_eq!(tool_name, "noop");
        assert!(result.success);
        assert!(result.message.contains("Skipped"));
    }

    #[tokio::test]
    async fn test_execute_missing_tool_succeeds() {
        let entry = serde_json::json!({"name": "missing-tool", "tool_name": "some_tool"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok());
        let (tool_name, result) = result.unwrap();
        assert_eq!(tool_name, "missing-tool");
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_empty_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ExecutionContext::default();

        let results = execute_safe_outputs(temp_dir.path(), &ctx).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_from_ndjson() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        // Write test NDJSON
        let ndjson = r#"{"name":"noop","context":"test1"}
{"name":"missing-tool","tool_name":"my_tool"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let ctx = ExecutionContext::default();
        let results = execute_safe_outputs(temp_dir.path(), &ctx).await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert!(results[1].success);
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_empty_file_returns_empty() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);
        tokio::fs::write(&safe_output_path, "").await.unwrap();

        let ctx = ExecutionContext::default();
        let results = execute_safe_outputs(temp_dir.path(), &ctx).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_execute_missing_data_succeeds() {
        let entry = serde_json::json!({"name": "missing-data", "data_type": "schema", "reason": "not available"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok());
        let (tool_name, result) = result.unwrap();
        assert_eq!(tool_name, "missing-data");
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_safe_output_malformed_work_item_returns_err() {
        // Missing required fields (title and description)
        let entry = serde_json::json!({"name": "create-work-item"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_unknown_tool_error_contains_tool_name() {
        let entry = serde_json::json!({"name": "evil-backdoor"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("evil-backdoor"));
    }

    #[tokio::test]
    async fn test_execute_malformed_update_wiki_page_returns_err() {
        // Missing required fields (path and content)
        let entry = serde_json::json!({"name": "update-wiki-page"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_update_wiki_page_missing_context() {
        let entry = serde_json::json!({
            "name": "update-wiki-page",
            "path": "/Overview",
            "content": "This is some valid wiki content."
        });

        // Context without required fields (ado_org_url, etc.)
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: false,
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    #[tokio::test]
    async fn test_execute_malformed_create_wiki_page_returns_err() {
        // Missing required fields (path and content)
        let entry = serde_json::json!({"name": "create-wiki-page"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_create_wiki_page_missing_context() {
        let entry = serde_json::json!({
            "name": "create-wiki-page",
            "path": "/NewPage",
            "content": "This is some valid wiki content."
        });

        // Context without required fields (ado_org_url, etc.)
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: false,
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    #[tokio::test]
    async fn test_execute_malformed_comment_on_work_item_returns_err() {
        // Missing required fields (work_item_id and body)
        let entry = serde_json::json!({"name": "comment-on-work-item"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_comment_on_work_item_missing_context() {
        let entry = serde_json::json!({
            "name": "comment-on-work-item",
            "work_item_id": 12345,
            "body": "This is a comment on the work item."
        });

        // Context without required fields (ado_org_url, etc.)
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: false,
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    /// Excess update-work-item entries beyond `max` are skipped (failure result added) rather than
    /// aborting the entire batch. Other tool entries must still execute.
    #[tokio::test]
    async fn test_execute_update_work_item_max_skips_excess_not_abort() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        // Write 3 update-work-item entries + 1 noop; max defaults to 1
        let ndjson = r#"{"name":"update-work-item","id":1,"title":"First update"}
{"name":"update-work-item","id":2,"title":"Second update"}
{"name":"update-work-item","id":3,"title":"Third update"}
{"name":"noop","context":"still runs"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        // Config: update-work-item with max=1 (default), title=true so the field check passes
        let update_cfg = serde_json::json!({
            "title": true,
            "max": 1,
            "target": "*"
        });
        let mut tool_configs = HashMap::new();
        tool_configs.insert("update-work-item".to_string(), update_cfg);

        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/org".to_string()),
            ado_organization: Some("org".to_string()),
            ado_project: Some("Proj".to_string()),
            access_token: Some("token".to_string()),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs,
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: false,
        };

        let results = execute_safe_outputs(temp_dir.path(), &ctx).await;
        // The batch must NOT abort — execute_safe_outputs should return Ok
        assert!(
            results.is_ok(),
            "Batch should not abort when max is exceeded; got: {:?}",
            results
        );
        let results = results.unwrap();
        // 4 entries total: 3 update-work-item + 1 noop
        assert_eq!(results.len(), 4, "Expected 4 results (3 uwi + 1 noop)");

        // The first update-work-item fails with HTTP error (no real ADO) but was attempted
        // The 2nd and 3rd are skipped due to max
        let skipped: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("maximum update-work-item count"))
            .collect();
        assert_eq!(skipped.len(), 2, "Expected 2 skipped entries, got: {:?}", skipped);

        // The noop still executes successfully
        let noop_result = &results[3];
        assert!(
            noop_result.success,
            "noop should still succeed even when prior entries are skipped"
        );
    }

    // --- check_budget unit tests ---

    #[test]
    fn test_check_budget_returns_none_when_under_limit() {
        let result = check_budget(5, 0, "update-work-item", "", 0, 3);
        assert!(result.is_none());
    }

    #[test]
    fn test_check_budget_returns_none_at_exactly_one_below_limit() {
        let result = check_budget(5, 1, "update-work-item", "", 2, 3);
        assert!(result.is_none());
    }

    #[test]
    fn test_check_budget_returns_failure_when_at_limit() {
        let result = check_budget(5, 2, "update-work-item", "", 3, 3);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(!r.success);
        assert!(r.message.contains("maximum update-work-item count (3)"));
        assert!(r.message.contains("safe-outputs.update-work-item"));
    }

    #[test]
    fn test_check_budget_returns_failure_when_over_limit() {
        let result = check_budget(5, 3, "comment-on-work-item", " (work item #99)", 5, 2);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(!r.success);
        assert!(r.message.contains("(work item #99)"));
        assert!(r.message.contains("maximum comment-on-work-item count (2)"));
    }

    #[test]
    fn test_check_budget_zero_max_always_skips() {
        let result = check_budget(3, 0, "update-work-item", "", 0, 0);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(!r.success);
        assert!(r.message.contains("maximum update-work-item count (0)"));
    }

    #[test]
    fn test_check_budget_wi_id_included_in_message() {
        let result = check_budget(4, 1, "update-work-item", " (work item #42)", 1, 1);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.message.contains("(work item #42)"));
    }

    // --- extract_entry_context unit tests ---

    #[test]
    fn test_extract_entry_context_with_id() {
        let entry = serde_json::json!({"name": "update-work-item", "id": 42});
        assert_eq!(extract_entry_context(&entry), " (work item #42)");
    }

    #[test]
    fn test_extract_entry_context_with_work_item_id() {
        let entry = serde_json::json!({"name": "comment-on-work-item", "work_item_id": 99});
        assert_eq!(extract_entry_context(&entry), " (work item #99)");
    }

    #[test]
    fn test_extract_entry_context_with_title() {
        let entry = serde_json::json!({"name": "create-work-item", "title": "Fix the bug"});
        assert_eq!(extract_entry_context(&entry), " (\"Fix the bug\")");
    }

    #[test]
    fn test_extract_entry_context_with_path() {
        let entry = serde_json::json!({"name": "create-wiki-page", "path": "/Overview/NewPage"});
        assert_eq!(extract_entry_context(&entry), " (path: /Overview/NewPage)");
    }

    #[test]
    fn test_extract_entry_context_truncates_long_title_utf8_safe() {
        // 41 emoji characters — each is 4 bytes, so naive &title[..40] would panic
        let title = "🔥".repeat(41);
        let entry = serde_json::json!({"name": "create-work-item", "title": title});
        let ctx = extract_entry_context(&entry);
        assert!(ctx.starts_with(" (\""));
        assert!(ctx.ends_with("\")"));
        // Should contain exactly 40 emoji chars (not panic)
        let inner = &ctx[3..ctx.len() - 2];
        assert_eq!(inner.chars().count(), 40);
    }

    #[test]
    fn test_extract_entry_context_empty() {
        let entry = serde_json::json!({"name": "noop"});
        assert_eq!(extract_entry_context(&entry), "");
    }

    #[test]
    fn test_extract_entry_context_strips_control_chars() {
        let entry = serde_json::json!({"name": "create-work-item", "title": "Good\ntitle\r\nhere"});
        assert_eq!(extract_entry_context(&entry), " (\"Goodtitlehere\")");
    }

    #[test]
    fn test_extract_entry_context_strips_control_chars_from_path() {
        let entry = serde_json::json!({"name": "create-wiki-page", "path": "/Page\n/Injected"});
        assert_eq!(extract_entry_context(&entry), " (path: /Page/Injected)");
    }

    // --- resolve_max and DEFAULT_MAX unit tests ---

    #[test]
    fn test_default_max_trait_constant() {
        assert_eq!(CreateWorkItemResult::DEFAULT_MAX, 1);
        assert_eq!(CreatePrResult::DEFAULT_MAX, 1);
        assert_eq!(UpdateWorkItemResult::DEFAULT_MAX, 1);
        assert_eq!(CommentOnWorkItemResult::DEFAULT_MAX, 1);
        assert_eq!(CreateWikiPageResult::DEFAULT_MAX, 1);
        assert_eq!(UpdateWikiPageResult::DEFAULT_MAX, 1);
    }

    #[test]
    fn test_resolve_max_uses_config_override() {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("test-tool".to_string(), serde_json::json!({"max": 5}));
        let ctx = ExecutionContext {
            tool_configs,
            dry_run: false,
            ..ExecutionContext::default()
        };
        assert_eq!(resolve_max(&ctx, "test-tool", 1), 5);
    }

    #[test]
    fn test_resolve_max_falls_back_to_default() {
        let ctx = ExecutionContext::default();
        assert_eq!(resolve_max(&ctx, "nonexistent-tool", 3), 3);
    }

    #[test]
    fn test_resolve_max_uses_default_when_no_max_in_config() {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("test-tool".to_string(), serde_json::json!({"other": true}));
        let ctx = ExecutionContext {
            tool_configs,
            dry_run: false,
            ..ExecutionContext::default()
        };
        assert_eq!(resolve_max(&ctx, "test-tool", 7), 7);
    }

    // --- Generic budget enforcement for all tool types ---

    #[tokio::test]
    async fn test_budget_enforcement_create_work_item_max() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        // Write 3 create-work-item entries + 1 noop; max set to 2
        let ndjson = r#"{"name":"create-work-item","title":"First item","description":"A description that is definitely longer than thirty characters."}
{"name":"create-work-item","title":"Second item","description":"A description that is definitely longer than thirty characters."}
{"name":"create-work-item","title":"Third item","description":"A description that is definitely longer than thirty characters."}
{"name":"noop","context":"still runs"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let mut tool_configs = HashMap::new();
        tool_configs.insert("create-work-item".to_string(), serde_json::json!({"max": 2}));

        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/org".to_string()),
            ado_organization: Some("org".to_string()),
            ado_project: Some("Proj".to_string()),
            access_token: Some("token".to_string()),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs,
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: false,
        };

        let results = execute_safe_outputs(temp_dir.path(), &ctx).await;
        assert!(results.is_ok(), "Batch should not abort when max is exceeded");
        let results = results.unwrap();
        assert_eq!(results.len(), 4, "Expected 4 results");

        // Only 1 should be skipped (max=2 allows first 2, third is skipped)
        let skipped: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("maximum create-work-item count"))
            .collect();
        assert_eq!(skipped.len(), 1, "Expected 1 skipped entry, got: {:?}", skipped);

        // noop still runs
        assert!(results[3].success, "noop should still succeed");
    }

    #[tokio::test]
    async fn test_budget_enforcement_mixed_tools_independent_budgets() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        // Mix of tools: each has max=1 (default), so only the first of each type should pass budget
        let ndjson = r#"{"name":"create-work-item","title":"WI 1","description":"A description that is definitely longer than thirty characters."}
{"name":"create-work-item","title":"WI 2","description":"A description that is definitely longer than thirty characters."}
{"name":"create-wiki-page","path":"/Page1","content":"Some valid wiki content here."}
{"name":"create-wiki-page","path":"/Page2","content":"Some valid wiki content here."}
{"name":"noop","context":"always runs"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/org".to_string()),
            ado_organization: Some("org".to_string()),
            ado_project: Some("Proj".to_string()),
            access_token: Some("token".to_string()),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(), // defaults: max=1 for all
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: false,
        };

        let results = execute_safe_outputs(temp_dir.path(), &ctx).await.unwrap();
        assert_eq!(results.len(), 5);

        // Second create-work-item should be skipped
        let cwi_skipped: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("maximum create-work-item count"))
            .collect();
        assert_eq!(cwi_skipped.len(), 1, "Expected 1 skipped create-work-item");

        // Second create-wiki-page should be skipped
        let cwp_skipped: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("maximum create-wiki-page count"))
            .collect();
        assert_eq!(cwp_skipped.len(), 1, "Expected 1 skipped create-wiki-page");

        // noop always runs
        assert!(results[4].success, "noop should still succeed");
    }

    // ─── dry-run tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_dry_run_create_work_item_succeeds() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        let ndjson = r#"{"name":"create-work-item","title":"Test work item title","description":"This is a test description that is long enough to pass validation checks"}"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let mut ctx = ExecutionContext::default();
        ctx.dry_run = true;

        let results = execute_safe_outputs(temp_dir.path(), &ctx).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success, "dry-run should succeed");
        assert!(
            results[0].message.contains("[DRY-RUN]"),
            "message should contain [DRY-RUN], got: {}",
            results[0].message
        );
        assert!(
            results[0].message.contains("create work item"),
            "message should contain tool summary, got: {}",
            results[0].message
        );
    }

    #[tokio::test]
    async fn test_dry_run_multiple_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        let ndjson = [
            r#"{"name":"create-work-item","title":"Test work item title","description":"This is a test description that is long enough to pass validation checks"}"#,
            r#"{"name":"noop","context":"nothing to do"}"#,
        ]
        .join("\n");
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let mut ctx = ExecutionContext::default();
        ctx.dry_run = true;

        let results = execute_safe_outputs(temp_dir.path(), &ctx).await.unwrap();
        assert_eq!(results.len(), 2);
        // create-work-item goes through Executor trait → dry-run intercepted
        assert!(results[0].message.contains("[DRY-RUN]"));
        // noop is handled inline, not through Executor → runs normally
        assert!(results[1].success);
        assert!(results[1].message.contains("noop"));
    }

    #[tokio::test]
    async fn test_dry_run_default_is_false() {
        let ctx = ExecutionContext::default();
        assert!(!ctx.dry_run, "dry_run should default to false");
    }

    #[tokio::test]
    async fn test_non_dry_run_still_fails_without_ado_config() {
        // Verify dry_run=false (default) still behaves normally: missing ADO config causes error
        let entry = serde_json::json!({
            "name": "create-work-item",
            "title": "Test work item",
            "description": "A description that is definitely longer than thirty characters."
        });

        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: false,
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err(), "should fail without ADO config when not in dry-run mode");
    }

    #[tokio::test]
    async fn test_dry_run_succeeds_without_ado_config() {
        // With dry_run=true, missing ADO config should NOT cause failure.
        // Input validation (title length, etc.) still runs — only ADO API calls are skipped.
        let entry = serde_json::json!({
            "name": "create-work-item",
            "title": "Test work item",
            "description": "A description that is definitely longer than thirty characters."
        });

        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            agent_stats: None,
            dry_run: true,
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok(), "dry-run should succeed without ADO config");
        let (tool_name, exec_result) = result.unwrap();
        assert_eq!(tool_name, "create-work-item");
        assert!(exec_result.success);
        assert!(exec_result.message.contains("[DRY-RUN]"));
    }

    #[tokio::test]
    async fn test_dry_run_report_incomplete_still_fails() {
        // report-incomplete is dispatched inline (not through Executor trait),
        // so it still returns ExecutionResult::failure even in dry-run mode.
        // This is correct: the agent declared it couldn't complete the task.
        let entry = serde_json::json!({
            "name": "report-incomplete",
            "reason": "Could not find the required data to complete the analysis"
        });

        let mut ctx = ExecutionContext::default();
        ctx.dry_run = true;

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok(), "dispatch should succeed");
        let (tool_name, exec_result) = result.unwrap();
        assert_eq!(tool_name, "report-incomplete");
        assert!(!exec_result.success, "report-incomplete should still be a failure in dry-run mode");
        assert!(
            exec_result.message.contains("incomplete"),
            "message should mention incomplete, got: {}",
            exec_result.message
        );
    }
}
