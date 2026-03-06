use anyhow::Result;
use log::{debug, error, info, warn};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde_json::Value;
use std::path::PathBuf;

use crate::ndjson::{self, SAFE_OUTPUT_FILENAME};
use crate::sanitize::sanitize as sanitize_text;
use crate::tools::{
    CreatePrParams, CreatePrResult, CreateWorkItemParams, CreateWorkItemResult, MissingDataParams,
    MissingDataResult, MissingToolParams, MissingToolResult, NoopParams, NoopResult, ToolResult,
    anyhow_to_mcp_error,
};

/// Sanitize a title into a safe branch name slug.
/// Only allows alphanumeric characters and dashes, collapses multiple dashes,
/// and limits length to prevent injection attacks.
fn slugify_title(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse multiple dashes and trim leading/trailing dashes
    let collapsed: String = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    // Limit length to 50 chars for reasonable branch names
    collapsed.chars().take(50).collect()
}

/// Generate a short random suffix for branch uniqueness
fn generate_short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // Take last 6 hex digits of timestamp for short unique suffix
    format!("{:06x}", (timestamp & 0xFFFFFF) as u32)
}

// ============================================================================
// SafeOutputs MCP Server
// ============================================================================

#[derive(Clone, Debug)]
pub struct SafeOutputs {
    bounding_directory: PathBuf,
    output_directory: PathBuf,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SafeOutputs {
    /// Get the full path to the safe output file
    fn safe_output_path(&self) -> PathBuf {
        self.output_directory.join(SAFE_OUTPUT_FILENAME)
    }

    /// Read the current contents of the safe output file as NDJSON
    async fn read_safe_output_file(&self) -> Result<Vec<Value>> {
        ndjson::read_ndjson_file(&self.safe_output_path()).await
    }

    /// Append a value to the safe output file (NDJSON - just append a line)
    async fn write_safe_output_file<T: ToolResult>(&self, value: &T) -> Result<()> {
        ndjson::append_to_ndjson_file(&self.safe_output_path(), value).await
    }

    /// Append a value, but only if we haven't reached the maximum entries for this tool
    async fn write_safe_output_file_with_maximum<T: ToolResult>(
        &self,
        value: &T,
        maximum: usize,
    ) -> Result<bool> {
        let array = self.read_safe_output_file().await?;

        // Count existing entries for this specific tool using T::NAME
        let tool_count = array
            .iter()
            .filter(|v| v.get("name").and_then(|n| n.as_str()) == Some(T::NAME))
            .count();

        if tool_count >= maximum {
            return Ok(false);
        }

        self.write_safe_output_file(value).await?;
        Ok(true)
    }

    async fn new(
        bounding_directory: impl Into<PathBuf>,
        output_directory: impl Into<PathBuf>,
    ) -> Result<Self> {
        let bounding_dir = bounding_directory.into();
        let output_dir = output_directory.into();
        info!(
            "Initializing SafeOutputs MCP server: bounding={}, output={}",
            bounding_dir.display(),
            output_dir.display()
        );
        anyhow::ensure!(
            bounding_dir.exists() && bounding_dir.is_dir(),
            "bounding_directory: {:?} is not a valid path or directory",
            bounding_dir
        );
        anyhow::ensure!(
            output_dir.exists() && output_dir.is_dir(),
            "output_directory: {:?} is not a valid path or directory",
            output_dir
        );

        // Initialize the safe output file
        debug!("Initializing safe output file");
        ndjson::init_ndjson_file(&output_dir.join(SAFE_OUTPUT_FILENAME)).await?;

        Ok(Self {
            bounding_directory: bounding_dir,
            output_directory: output_dir,
            tool_router: Self::tool_router(),
        })
    }

    /// Generate a git diff patch from a specific directory
    /// If `repository` is Some, it's treated as a subdirectory of bounding_directory
    /// If `repository` is None or "self", use bounding_directory directly
    async fn generate_patch(&self, repository: Option<&str>) -> Result<String, McpError> {
        use tokio::process::Command;

        // Determine the git directory based on repository
        let git_dir = match repository {
            Some("self") | None => self.bounding_directory.clone(),
            Some(repo_alias) => {
                if repo_alias.contains('/')
                    || repo_alias.contains('\\')
                    || repo_alias.contains("..")
                {
                    return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                        "Invalid repository alias: {}. Path traversal is not allowed.",
                        repo_alias
                    )));
                }
                let repo_path = self.bounding_directory.join(repo_alias);
                let canonical_repo_path = repo_path.canonicalize().map_err(|e| {
                    anyhow_to_mcp_error(anyhow::anyhow!(
                        "Failed to canonicalize repository path: {}",
                        e
                    ))
                })?;
                let canonical_bounding_dir =
                    self.bounding_directory.canonicalize().map_err(|e| {
                        anyhow_to_mcp_error(anyhow::anyhow!(
                            "Failed to canonicalize bounding directory: {}",
                            e
                        ))
                    })?;
                if !canonical_repo_path.starts_with(&canonical_bounding_dir) {
                    return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                        "Repository path escapes bounding directory: {}",
                        repo_path.display()
                    )));
                }
                if !repo_path.exists() {
                    return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                        "Repository directory not found: {}",
                        repo_path.display()
                    )));
                }
                repo_path
            }
        };

        // Run git diff against the target branch to capture all changes
        // Try origin/main first (remote tracking), then main, then HEAD as fallback
        let diff_targets = ["origin/main", "main", "HEAD"];
        let mut last_error = String::new();
        let mut diff_output = None;

        for target in &diff_targets {
            let output = Command::new("git")
                .args(["diff", target])
                .current_dir(&git_dir)
                .output()
                .await
                .map_err(|e| {
                    anyhow_to_mcp_error(anyhow::anyhow!("Failed to run git diff: {}", e))
                })?;

            if output.status.success() {
                diff_output = Some(output);
                break;
            }
            last_error = String::from_utf8_lossy(&output.stderr).to_string();
        }

        let mut patch = if let Some(output) = diff_output {
            String::from_utf8_lossy(&output.stdout).to_string()
        } else {
            return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                "git diff failed against all targets (origin/main, main, HEAD): {}",
                last_error
            )));
        };

        // Also include untracked files that have been added
        let status_output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&git_dir)
            .output()
            .await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to run git status: {}", e)))?;

        if !status_output.status.success() {
            return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                "git status failed: {}",
                String::from_utf8_lossy(&status_output.stderr)
            )));
        }

        let status = String::from_utf8_lossy(&status_output.stdout);
        for line in status.lines() {
            if line.starts_with("?? ") {
                // Untracked file - generate a diff for it
                let file_path = line[3..].trim();
                let file_full_path = git_dir.join(file_path);

                if file_full_path.is_file() {
                    if let Ok(content) = tokio::fs::read_to_string(&file_full_path).await {
                        patch.push_str(&format!("diff --git a/{} b/{}\n", file_path, file_path));
                        patch.push_str("new file mode 100644\n");
                        patch.push_str("--- /dev/null\n");
                        patch.push_str(&format!("+++ b/{}\n", file_path));

                        let line_count = content.lines().count();
                        patch.push_str(&format!("@@ -0,0 +1,{} @@\n", line_count));

                        for line in content.lines() {
                            patch.push('+');
                            patch.push_str(line);
                            patch.push('\n');
                        }
                    }
                }
            }
        }

        Ok(patch)
    }

    /// Generate a unique patch filename
    fn generate_patch_filename(&self, repository: &str) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        // Sanitize repository name for filename
        let safe_repo = repository.replace(['/', '\\'], "-");
        format!("pr-{}-{}.patch", safe_repo, timestamp)
    }

    #[tool(
        description = "Log a transparency message when no significant actions are needed. Use this to confirm workflow completion and provide visibility when analysis is complete but no changes or outputs are required (e.g., 'No issues found', 'All checks passed'). This ensures the workflow produces human-visible output even when no other actions are taken."
    )]
    async fn noop(&self, params: Parameters<NoopParams>) -> Result<CallToolResult, McpError> {
        debug!("Tool called: noop - {:?}", params.0.context);
        let mut sanitized = params.0;
        sanitized.context = sanitized.context.map(|c| sanitize_text(&c));
        let result: NoopResult = sanitized.try_into()?;
        let _ = self.write_safe_output_file_with_maximum(&result, 1).await;
        Ok(CallToolResult::success(vec![]))
    }

    #[tool(
        name = "missing-tool",
        description = "Report that a tool or capability needed to complete the task is not available, or share any information you deem important about missing functionality or limitations. Use this when you cannot accomplish what was requested because the required functionality is missing or access is restricted."
    )]
    async fn missing_tool(
        &self,
        params: Parameters<MissingToolParams>,
    ) -> Result<CallToolResult, McpError> {
        warn!("Tool called: missing-tool - '{}'", params.0.tool_name);
        debug!("Context: {:?}", params.0.context);
        let mut sanitized = params.0;
        sanitized.tool_name = sanitize_text(&sanitized.tool_name);
        sanitized.context = sanitized.context.map(|c| sanitize_text(&c));
        let result: MissingToolResult = sanitized.try_into()?;
        let _ = self.write_safe_output_file(&result).await;
        Ok(CallToolResult::success(vec![]))
    }
    #[tool(
        name = "missing-data",
        description = "Report that data or information needed to complete the task is not available. Use this when you cannot accomplish what was requested because required data, context, or information is missing."
    )]
    async fn missing_data(
        &self,
        params: Parameters<MissingDataParams>,
    ) -> Result<CallToolResult, McpError> {
        debug!("Tool called: missing-data - {:?}", params.0.context);
        let mut sanitized = params.0;
        sanitized.data_type = sanitize_text(&sanitized.data_type);
        sanitized.reason = sanitize_text(&sanitized.reason);
        sanitized.context = sanitized.context.map(|c| sanitize_text(&c));
        let result: MissingDataResult = sanitized.try_into()?;
        let _ = self.write_safe_output_file(&result).await;
        Ok(CallToolResult::success(vec![]))
    }

    #[tool(name = "create-work-item", description = "Create an azure devops work item")]
    async fn create_work_item(
        &self,
        params: Parameters<CreateWorkItemParams>,
    ) -> Result<CallToolResult, McpError> {
        info!("Tool called: create-work-item - '{}'", params.0.title);
        debug!("Description length: {} chars", params.0.description.len());
        // Sanitize untrusted agent-provided text fields (IS-01)
        let mut sanitized = params.0;
        sanitized.title = sanitize_text(&sanitized.title);
        sanitized.description = sanitize_text(&sanitized.description);
        let result: CreateWorkItemResult = sanitized.try_into()?;
        let _ = self.write_safe_output_file(&result).await;
        info!("Work item queued for creation");
        Ok(CallToolResult::success(vec![]))
    }

    #[tool(
        name = "create-pull-request",
        description = "Create a new pull request to propose code changes. Use this after making file edits to submit them for review and merging. The PR will be created from the current branch with your committed changes. Use 'self' for the pipeline's own repository, or a repository alias from the checkout list."
    )]
    async fn create_pr(
        &self,
        params: Parameters<CreatePrParams>,
    ) -> Result<CallToolResult, McpError> {
        info!("Tool called: create_pr - '{}'", params.0.title);
        // Sanitize untrusted agent-provided text fields (IS-01)
        let mut sanitized = params.0;
        sanitized.title = sanitize_text(&sanitized.title);
        sanitized.description = sanitize_text(&sanitized.description);

        // Determine repository(default to "self" if not provided)
        let repository = sanitized.repository.as_deref().unwrap_or("self");
        debug!("Repository: {}", repository);

        // Generate the patch from current git changes in the specified repository
        debug!("Generating patch for repository: {}", repository);
        let patch_content = self.generate_patch(Some(repository)).await?;

        if patch_content.trim().is_empty() {
            warn!("No changes detected in repository '{}'", repository);
            return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                "No changes detected in repository '{}'. Make code changes before creating a PR.",
                repository
            )));
        }
        debug!("Patch size: {} bytes", patch_content.len());

        // Generate a unique filename for the patch (include repo for clarity)
        let patch_filename = self.generate_patch_filename(repository);
        let patch_path = self.output_directory.join(&patch_filename);
        debug!("Patch filename: {}", patch_filename);

        // Write the patch file
        tokio::fs::write(&patch_path, &patch_content)
            .await
            .map_err(|e| {
                anyhow_to_mcp_error(anyhow::anyhow!("Failed to write patch file: {}", e))
            })?;

        // Generate source branch name from sanitized title + short unique suffix
        let title_slug = slugify_title(&sanitized.title);
        let short_id = generate_short_id();
        let source_branch = if title_slug.is_empty() {
            format!("agent/pr-{}", short_id)
        } else {
            format!("agent/{}-{}", title_slug, short_id)
        };

        // Create the result with patch file reference
        let result = CreatePrResult::new(
            sanitized.title.clone(),
            sanitized.description.clone(),
            source_branch,
            patch_filename,
            repository.to_string(),
        );

        // Write to safe outputs
        let _ = self.write_safe_output_file(&result).await;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "PR request saved for repository '{}'. Patch file: {}. Changes will be pushed and PR created during safe output processing.",
            repository, result.patch_file
        ))]))
    }
}

// Implement the server handler
#[tool_handler]
impl ServerHandler for SafeOutputs {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "A set of tools that generate SafeOutput compatible results.".into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

pub async fn run(output_directory: &str, bounding_directory: &str) -> Result<()> {
    // Create and run the server with STDIO transport
    let service = SafeOutputs::new(bounding_directory, output_directory)
        .await?
        .serve(stdio())
        .await
        .inspect_err(|e| {
            error!("Error starting MCP server: {}", e);
        })?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP exited with error: {:?}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn create_test_safe_outputs() -> (SafeOutputs, tempfile::TempDir) {
        let temp_dir = tempdir().unwrap();
        let safe_outputs = SafeOutputs::new(temp_dir.path(), temp_dir.path())
            .await
            .unwrap();
        (safe_outputs, temp_dir)
    }

    #[test]
    fn test_slugify_title_basic() {
        assert_eq!(slugify_title("Fix bug in parser"), "fix-bug-in-parser");
    }

    #[test]
    fn test_slugify_title_special_chars() {
        assert_eq!(slugify_title("Fix: parser (v2)"), "fix-parser-v2");
        assert_eq!(slugify_title("Update README.md"), "update-readme-md");
    }

    #[test]
    fn test_slugify_title_injection_attempts() {
        // Path traversal
        assert_eq!(slugify_title("fix/../../../etc/passwd"), "fix-etc-passwd");
        // Shell injection
        assert_eq!(slugify_title("fix; rm -rf /"), "fix-rm-rf");
        assert_eq!(slugify_title("fix$(curl evil.com)"), "fix-curl-evil-com");
        assert_eq!(slugify_title("fix`whoami`"), "fix-whoami");
        // Null bytes and encoding
        assert_eq!(slugify_title("fix\x00bug"), "fix-bug");
    }

    #[test]
    fn test_slugify_title_length_limit() {
        let long_title = "a".repeat(100);
        let result = slugify_title(&long_title);
        assert_eq!(result.len(), 50);
    }

    #[test]
    fn test_slugify_title_empty_and_special_only() {
        assert_eq!(slugify_title(""), "");
        assert_eq!(slugify_title("!@#$%"), "");
        assert_eq!(slugify_title("---"), "");
    }

    #[test]
    fn test_generate_short_id_format() {
        let id = generate_short_id();
        assert_eq!(id.len(), 6);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn test_new_creates_empty_safe_output_file() {
        let (safe_outputs, _temp_dir) = create_test_safe_outputs().await;

        let contents = safe_outputs.read_safe_output_file().await.unwrap();
        assert!(contents.is_empty());
    }

    #[tokio::test]
    async fn test_new_fails_with_invalid_bounding_directory() {
        let temp_dir = tempdir().unwrap();
        let result = SafeOutputs::new("/nonexistent/path", temp_dir.path()).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("bounding_directory")
        );
    }

    #[tokio::test]
    async fn test_new_fails_with_invalid_output_directory() {
        let temp_dir = tempdir().unwrap();
        let result = SafeOutputs::new(temp_dir.path(), "/nonexistent/path").await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("output_directory"));
    }

    #[tokio::test]
    async fn test_write_safe_output_file_appends_ndjson() {
        let (safe_outputs, _temp_dir) = create_test_safe_outputs().await;

        let result1: NoopResult = NoopParams {
            context: Some("first".to_string()),
        }
        .try_into()
        .unwrap();
        let result2: NoopResult = NoopParams {
            context: Some("second".to_string()),
        }
        .try_into()
        .unwrap();

        safe_outputs.write_safe_output_file(&result1).await.unwrap();
        safe_outputs.write_safe_output_file(&result2).await.unwrap();

        let contents = safe_outputs.read_safe_output_file().await.unwrap();
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["context"], "first");
        assert_eq!(contents[1]["context"], "second");
    }

    #[tokio::test]
    async fn test_write_safe_output_file_with_maximum_respects_limit() {
        let (safe_outputs, _temp_dir) = create_test_safe_outputs().await;

        let result1: NoopResult = NoopParams {
            context: Some("first".to_string()),
        }
        .try_into()
        .unwrap();
        let result2: NoopResult = NoopParams {
            context: Some("second".to_string()),
        }
        .try_into()
        .unwrap();

        // First write should succeed
        let written1 = safe_outputs
            .write_safe_output_file_with_maximum(&result1, 1)
            .await
            .unwrap();
        assert!(written1);

        // Second write should be rejected (max 1)
        let written2 = safe_outputs
            .write_safe_output_file_with_maximum(&result2, 1)
            .await
            .unwrap();
        assert!(!written2);

        // Only one entry should exist
        let contents = safe_outputs.read_safe_output_file().await.unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["context"], "first");
    }

    #[tokio::test]
    async fn test_write_safe_output_file_with_maximum_counts_per_tool() {
        let (safe_outputs, _temp_dir) = create_test_safe_outputs().await;

        let noop1: NoopResult = NoopParams {
            context: Some("noop1".to_string()),
        }
        .try_into()
        .unwrap();
        let missing1: MissingToolResult = MissingToolParams {
            tool_name: "tool1".to_string(),
            context: None,
        }
        .try_into()
        .unwrap();
        let missing2: MissingToolResult = MissingToolParams {
            tool_name: "tool2".to_string(),
            context: None,
        }
        .try_into()
        .unwrap();

        // Write one noop (max 1)
        let written = safe_outputs
            .write_safe_output_file_with_maximum(&noop1, 1)
            .await
            .unwrap();
        assert!(written);

        // Write missing tools (different type, should not count against noop max)
        let written = safe_outputs
            .write_safe_output_file_with_maximum(&missing1, 2)
            .await
            .unwrap();
        assert!(written);

        let written = safe_outputs
            .write_safe_output_file_with_maximum(&missing2, 2)
            .await
            .unwrap();
        assert!(written);

        // Third missing tool should be rejected (max 2)
        let missing3: MissingToolResult = MissingToolParams {
            tool_name: "tool3".to_string(),
            context: None,
        }
        .try_into()
        .unwrap();
        let written = safe_outputs
            .write_safe_output_file_with_maximum(&missing3, 2)
            .await
            .unwrap();
        assert!(!written);

        let contents = safe_outputs.read_safe_output_file().await.unwrap();
        assert_eq!(contents.len(), 3); // 1 noop + 2 missing tools
    }

    #[tokio::test]
    async fn test_read_safe_output_file_parses_ndjson() {
        let (safe_outputs, temp_dir) = create_test_safe_outputs().await;

        // Manually write NDJSON
        let ndjson = r#"{"name":"noop","context":"test1"}
{"name":"noop","context":"test2"}
"#;
        tokio::fs::write(temp_dir.path().join(SAFE_OUTPUT_FILENAME), ndjson)
            .await
            .unwrap();

        let contents = safe_outputs.read_safe_output_file().await.unwrap();
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["name"], "noop");
        assert_eq!(contents[1]["context"], "test2");
    }

    #[tokio::test]
    async fn test_safe_output_path_returns_correct_path() {
        let (safe_outputs, temp_dir) = create_test_safe_outputs().await;

        let expected = temp_dir.path().join(SAFE_OUTPUT_FILENAME);
        assert_eq!(safe_outputs.safe_output_path(), expected);
    }

    #[tokio::test]
    async fn test_tool_results_serialize_with_name_field() {
        let noop: NoopResult = NoopParams {
            context: Some("context".to_string()),
        }
        .try_into()
        .unwrap();
        let json = serde_json::to_value(&noop).unwrap();

        assert_eq!(json["name"], "noop");
        assert_eq!(json["context"], "context");

        let missing: MissingToolResult = MissingToolParams {
            tool_name: "my_tool".to_string(),
            context: Some("ctx".to_string()),
        }
        .try_into()
        .unwrap();
        let json = serde_json::to_value(&missing).unwrap();

        assert_eq!(json["name"], "missing-tool");
        assert_eq!(json["tool_name"], "my_tool");
        assert_eq!(json["context"], "ctx");
    }
}
