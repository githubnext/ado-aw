//! Stage 2 execution: Parse safe outputs and execute actions
//!
//! After the agent (Stage 1) generates safe outputs as an NDJSON file,
//! Stage 2 parses this file and executes the corresponding actions.

use anyhow::{Result, bail};
use log::{debug, error, info, warn};
use serde_json::Value;
use std::path::Path;

use crate::ndjson::{self, SAFE_OUTPUT_FILENAME};
use crate::tools::{
    CreatePrResult, CreateWikiPageResult, CreateWorkItemResult, EditWikiPageResult,
    ExecutionContext, ExecutionResult, Executor,
};

// Re-export memory types for use by main.rs
pub use crate::tools::memory::{MemoryConfig, process_agent_memory};

/// Execute all safe outputs from the NDJSON file in the specified directory
pub async fn execute_safe_outputs(
    safe_output_dir: &Path,
    ctx: &ExecutionContext,
) -> Result<Vec<ExecutionResult>> {
    let safe_output_path = safe_output_dir.join(SAFE_OUTPUT_FILENAME);

    info!("Stage 2 execution starting");
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

    let mut results = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let entry_json = serde_json::to_string(entry).unwrap_or_else(|_| "<invalid>".to_string());
        debug!(
            "[{}/{}] Executing entry: {}",
            i + 1,
            entries.len(),
            entry_json
        );

        match execute_safe_output(entry, ctx).await {
            Ok((tool_name, result)) => {
                if result.success {
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
                println!(
                    "[{}/{}] {} - {} - {}",
                    i + 1,
                    entries.len(),
                    tool_name,
                    if result.success { "✓" } else { "✗" },
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
    let success_count = results.iter().filter(|r| r.success).count();
    let failure_count = results.len() - success_count;
    info!(
        "Stage 2 execution complete: {} succeeded, {} failed",
        success_count, failure_count
    );

    Ok(results)
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

    // Dispatch based on tool name
    let result = match tool_name {
        "create-work-item" => {
            debug!("Parsing create-work-item payload");
            let mut output: CreateWorkItemResult = serde_json::from_value(entry.clone())
                .map_err(|e| anyhow::anyhow!("Failed to parse create-work-item: {}", e))?;
            debug!(
                "create-work-item: title='{}', description length={}",
                output.title,
                output.description.len()
            );
            output.execute_sanitized(ctx).await?
        }
        "create-pull-request" => {
            debug!("Parsing create-pull-request payload");
            let mut output: CreatePrResult = serde_json::from_value(entry.clone())
                .map_err(|e| anyhow::anyhow!("Failed to parse create-pull-request: {}", e))?;
            debug!(
                "create-pull-request: title='{}', repo='{}', branch='{}', patch='{}'",
                output.title, output.repository, output.source_branch, output.patch_file
            );
            output.execute_sanitized(ctx).await?
        }
        "edit-wiki-page" => {
            debug!("Parsing edit-wiki-page payload");
            let mut output: EditWikiPageResult = serde_json::from_value(entry.clone())
                .map_err(|e| anyhow::anyhow!("Failed to parse edit-wiki-page: {}", e))?;
            debug!(
                "edit-wiki-page: path='{}', content length={}",
                output.path,
                output.content.len()
            );
            output.execute_sanitized(ctx).await?
        }
        "create-wiki-page" => {
            debug!("Parsing create-wiki-page payload");
            let mut output: CreateWikiPageResult = serde_json::from_value(entry.clone())
                .map_err(|e| anyhow::anyhow!("Failed to parse create-wiki-page: {}", e))?;
            debug!(
                "create-wiki-page: path='{}', content length={}",
                output.path,
                output.content.len()
            );
            output.execute_sanitized(ctx).await?
        }
        "noop" => {
            debug!("Skipping noop entry");
            ExecutionResult::success("Skipped informational output: noop")
        }
        "missing-tool" => {
            debug!("Skipping missing-tool entry");
            ExecutionResult::success("Skipped informational output: missing-tool")
        }
        "missing-data" => {
            debug!("Skipping missing-data entry");
            ExecutionResult::success("Skipped informational output: missing-data")
        }
        other => {
            error!("Unknown tool type: {}", other);
            bail!("Unknown tool type: {}. No executor registered.", other)
        }
    };

    Ok((tool_name.to_string(), result))
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
    async fn test_execute_malformed_edit_wiki_page_returns_err() {
        // Missing required fields (path and content)
        let entry = serde_json::json!({"name": "edit-wiki-page"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_edit_wiki_page_missing_context() {
        let entry = serde_json::json!({
            "name": "edit-wiki-page",
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
}
