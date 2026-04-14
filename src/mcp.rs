use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde_json::Value;
use std::path::PathBuf;

use crate::ndjson::{self, SAFE_OUTPUT_FILENAME};
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use crate::tools::{
    AddBuildTagParams, AddBuildTagResult,
    AddPrCommentParams, AddPrCommentResult,
    CommentOnWorkItemParams, CommentOnWorkItemResult,
    CreateBranchParams, CreateBranchResult,
    CreateGitTagParams, CreateGitTagResult,
    CreatePrParams, CreatePrResult, CreateWikiPageParams, CreateWikiPageResult,
    CreateWorkItemParams, CreateWorkItemResult,
    LinkWorkItemsParams, LinkWorkItemsResult,
    ReplyToPrCommentParams, ReplyToPrCommentResult,
    ReportIncompleteParams, ReportIncompleteResult,
    ResolvePrThreadParams, ResolvePrThreadResult,
    UpdateWikiPageParams, UpdateWikiPageResult, MissingDataParams, MissingDataResult,
    MissingToolParams, MissingToolResult, NoopParams, NoopResult, QueueBuildParams,
    QueueBuildResult, SubmitPrReviewParams, SubmitPrReviewResult, ToolResult,
    UpdatePrParams, UpdatePrResult,
    UpdateWorkItemParams, UpdateWorkItemResult,
    UploadAttachmentParams, UploadAttachmentResult,
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

/// Generate a short cryptographically random suffix for branch uniqueness
fn generate_short_id() -> String {
    use rand::RngExt;
    let value: u32 = rand::rng().random();
    format!("{:08x}", value)
}

// Re-export from tools module
use crate::tools::ALWAYS_ON_TOOLS;

// ============================================================================
// SafeOutputs MCP Server
// ============================================================================

/// SafeOutputs is safe to clone for concurrent use: it only contains immutable
/// `PathBuf` fields and a `ToolRouter`. File I/O (NDJSON append) opens files
/// fresh on each call, so no shared mutable state exists between clones.
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
        enabled_tools: Option<&[String]>,
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

        let mut tool_router = Self::tool_router();

        // Filter tools if an enabled list is provided
        if let Some(enabled) = enabled_tools {
            let all_tools: Vec<String> = tool_router.list_all().iter().map(|t| t.name.to_string()).collect();
            let total = all_tools.len();
            for tool_name in &all_tools {
                let is_always_on = ALWAYS_ON_TOOLS.contains(&tool_name.as_str());
                let is_enabled = enabled.iter().any(|e| e == tool_name);
                if !is_always_on && !is_enabled {
                    debug!("Filtering out tool: {}", tool_name);
                    tool_router.remove_route(tool_name);
                }
            }
            // Warn about enabled-tools entries that don't match any registered route
            for name in enabled {
                if !all_tools.iter().any(|t| t == name) {
                    warn!("Enabled-tools entry '{}' has no matching route (ignored)", name);
                }
            }
            let remaining: Vec<String> = tool_router.list_all().iter().map(|t| t.name.to_string()).collect();
            info!("Tool filtering applied: {} of {} tools enabled: {:?}", remaining.len(), total, remaining);
        }

        Ok(Self {
            bounding_directory: bounding_dir,
            output_directory: output_dir,
            tool_router,
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

        // Generate patch using git format-patch for proper commit metadata,
        // rename detection, and binary file handling.
        //
        // Handles both committed and uncommitted changes:
        // 1. Find the merge-base with the upstream branch (origin/HEAD or origin/main)
        // 2. If there are uncommitted changes, stage and create a temporary commit
        // 3. Generate format-patch from merge-base..HEAD to capture ALL changes
        // 4. If a temporary commit was created, reset it (preserving working tree)

        // Find the merge-base to diff against
        let merge_base = Self::find_merge_base(&git_dir).await?;
        debug!("Using merge base: {}", merge_base);

        // Check if there are uncommitted changes (staged or unstaged)
        let status_output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&git_dir)
            .output()
            .await
            .map_err(|e| {
                anyhow_to_mcp_error(anyhow::anyhow!("Failed to run git status: {}", e))
            })?;

        let has_uncommitted = !String::from_utf8_lossy(&status_output.stdout)
            .trim()
            .is_empty();
        let mut made_synthetic_commit = false;

        if has_uncommitted {
            debug!("Uncommitted changes detected, creating synthetic commit");

            // Stage all changes including untracked files
            let add_output = Command::new("git")
                .args(["add", "-A"])
                .current_dir(&git_dir)
                .output()
                .await
                .map_err(|e| {
                    anyhow_to_mcp_error(anyhow::anyhow!("Failed to run git add -A: {}", e))
                })?;

            if !add_output.status.success() {
                return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                    "git add -A failed: {}",
                    String::from_utf8_lossy(&add_output.stderr)
                )));
            }

            // Create a temporary commit with git identity flags to avoid config dependency
            let commit_output = Command::new("git")
                .args([
                    "-c", "user.email=agent@ado-aw",
                    "-c", "user.name=ADO Agent",
                    "commit", "-m", "agent changes", "--allow-empty", "--no-verify",
                ])
                .current_dir(&git_dir)
                .output()
                .await
                .map_err(|e| {
                    anyhow_to_mcp_error(anyhow::anyhow!(
                        "Failed to create temporary commit: {}",
                        e
                    ))
                })?;

            if !commit_output.status.success() {
                // Reset staging on failure
                let _ = Command::new("git")
                    .args(["reset", "HEAD", "--quiet"])
                    .current_dir(&git_dir)
                    .output()
                    .await;
                return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                    "Failed to create temporary commit: {}",
                    String::from_utf8_lossy(&commit_output.stderr)
                )));
            }
            made_synthetic_commit = true;
        } else {
            debug!("No uncommitted changes — capturing committed changes only");
        }

        // Generate format-patch from merge-base..HEAD to capture all changes
        let format_patch_result = Command::new("git")
            .args([
                "format-patch",
                &format!("{}..HEAD", merge_base),
                "--stdout",
                "-M",
            ])
            .current_dir(&git_dir)
            .output()
            .await;

        // Always undo the temporary commit before propagating errors.
        // We capture the original index state via `git stash` to restore it exactly,
        // since `git reset --mixed` would leave previously-untracked files staged.
        if made_synthetic_commit {
            // Capture the synthetic commit SHA for diagnostics
            let head_sha = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&git_dir)
                .output()
                .await
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());

            // Reset the synthetic commit, restoring changes to working tree
            let reset_output = Command::new("git")
                .args(["reset", "HEAD~1", "--mixed", "--quiet"])
                .current_dir(&git_dir)
                .output()
                .await
                .map_err(|e| {
                    anyhow_to_mcp_error(anyhow::anyhow!("Failed to run git reset: {}", e))
                })?;

            if !reset_output.status.success() {
                warn!(
                    "WARNING: synthetic commit {} was not cleaned up; \
                     run `git reset HEAD~1` to restore state",
                    head_sha
                );
                return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                    "git reset HEAD~1 failed (synthetic commit {} may remain): {}",
                    head_sha,
                    String::from_utf8_lossy(&reset_output.stderr)
                )));
            }

            // Unstage everything so the index matches the pre-generate_patch state.
            // `git reset --mixed` leaves previously-untracked files as staged new files;
            // this reset restores them to untracked.
            let _ = Command::new("git")
                .args(["reset", "HEAD", "--quiet"])
                .current_dir(&git_dir)
                .output()
                .await;
        }

        // Now check the format-patch result after cleanup
        let format_patch_output = format_patch_result.map_err(|e| {
            anyhow_to_mcp_error(anyhow::anyhow!("Failed to run git format-patch: {}", e))
        })?;

        if !format_patch_output.status.success() {
            return Err(anyhow_to_mcp_error(anyhow::anyhow!(
                "git format-patch failed: {}",
                String::from_utf8_lossy(&format_patch_output.stderr)
            )));
        }

        let patch = String::from_utf8_lossy(&format_patch_output.stdout).to_string();

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

    /// Find the merge-base commit to diff against.
    ///
    /// Tries (in order):
    /// 1. Detect actual default branch via `git symbolic-ref refs/remotes/origin/HEAD`
    /// 2. Common default branch names: `origin/main`, `origin/master`
    /// 3. Root commit via `git rev-list --max-parents=0 HEAD` (handles single-commit repos)
    async fn find_merge_base(git_dir: &std::path::Path) -> Result<String, McpError> {
        use tokio::process::Command;

        // First, try to discover the actual default branch from origin/HEAD
        let symbolic_output = Command::new("git")
            .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
            .current_dir(git_dir)
            .output()
            .await
            .ok();

        let mut candidates: Vec<String> = Vec::new();

        if let Some(out) = symbolic_output.filter(|o| o.status.success()) {
            let refname = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // e.g. "refs/remotes/origin/main" → "origin/main"
            if let Some(branch) = refname.strip_prefix("refs/remotes/") {
                candidates.push(branch.to_string());
            }
        }

        // Always try common defaults as fallbacks
        for name in &["origin/main", "origin/master"] {
            if !candidates.iter().any(|c| c == *name) {
                candidates.push(name.to_string());
            }
        }

        for remote_ref in &candidates {
            let output = Command::new("git")
                .args(["merge-base", "HEAD", remote_ref])
                .current_dir(git_dir)
                .output()
                .await
                .ok();

            if let Some(out) = output {
                if out.status.success() {
                    let base = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !base.is_empty() {
                        return Ok(base);
                    }
                }
            }
        }

        // Fallback: find the root commit. Only valid for single-commit repos where HEAD~1
        // doesn't exist. For repos with longer history this would produce enormous patches.
        let root_output = Command::new("git")
            .args(["rev-list", "--max-parents=0", "HEAD"])
            .current_dir(git_dir)
            .output()
            .await
            .ok();

        if let Some(out) = root_output.filter(|o| o.status.success()) {
            let output_str = String::from_utf8_lossy(&out.stdout).to_string();
            let roots: Vec<&str> = output_str.trim().lines().collect();

            // Only use root commit fallback for repos with a single commit
            if roots.len() == 1 {
                let sha = roots[0].to_string();
                warn!(
                    "Could not find merge-base with origin; using root commit {} \
                     (single-commit repository)",
                    sha
                );
                return Ok(sha);
            }
        }

        Err(anyhow_to_mcp_error(anyhow::anyhow!(
            "Cannot determine diff base: no remote tracking branch found. \
             Push a tracking branch or ensure origin/HEAD is configured."
        )))
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
        name = "comment-on-work-item",
        description = "Add a comment to an existing Azure DevOps work item. \
Provide the work item ID and the comment body in markdown. The comment will be \
posted during safe output processing. Target restrictions may apply based on \
pipeline configuration."
    )]
    async fn comment_on_work_item(
        &self,
        params: Parameters<CommentOnWorkItemParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: comment-on-work-item - work item #{}",
            params.0.work_item_id
        );
        debug!("Body length: {} chars", params.0.body.len());
        // Sanitize untrusted agent-provided text fields (IS-01)
        let mut sanitized = params.0;
        sanitized.body = sanitize_text(&sanitized.body);
        let result: CommentOnWorkItemResult = sanitized.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        info!("Comment queued for work item #{}", result.work_item_id);
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Comment queued for work item #{}. The comment will be posted during safe output processing.",
            result.work_item_id
        ))]))
    }

    #[tool(
        name = "update-work-item",
        description = "Update an existing Azure DevOps work item. Only fields explicitly enabled \
in the pipeline configuration (safe-outputs.update-work-item) may be changed. Updates may be \
further restricted by target (only a specific work item ID) or title-prefix (only work items \
whose current title starts with a configured prefix). Provide the work item ID and only the \
fields you want to update."
    )]
    async fn update_work_item(
        &self,
        params: Parameters<UpdateWorkItemParams>,
    ) -> Result<CallToolResult, McpError> {
        info!("Tool called: update-work-item - id={}", params.0.id);
        let mut result: UpdateWorkItemResult = params.0.try_into()?;
        // Sanitize before persisting to NDJSON (defense-in-depth; Stage 2 sanitizes again)
        result.sanitize_fields();
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        info!("Work item update queued for #{}", result.id);
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Work item #{} update queued. Changes will be applied during safe output processing.",
            result.id
        ))]))
    }

    #[tool(
        name = "create-pull-request",
        description = "Create a new pull request to propose code changes. Before calling this tool, \
stage and commit your changes using git add and git commit — each logical change should be its \
own commit with a descriptive message. The PR will be created from your committed changes. \
Use 'self' for the pipeline's own repository, or a repository alias from the checkout list."
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
            sanitized.labels,
        );

        // Write to safe outputs
        let _ = self.write_safe_output_file(&result).await;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "PR request saved for repository '{}'. Patch file: {}. Changes will be pushed and PR created during safe output processing.",
            repository, result.patch_file
        ))]))
    }

    #[tool(
        name = "update-wiki-page",
        description = "Create or update an Azure DevOps wiki page with the provided markdown content. \
The page path (e.g. '/Overview/Architecture') and the wiki to write to are determined by the \
pipeline configuration. Use this to publish findings, summaries, documentation, or any other \
structured output that should be visible in the project wiki."
    )]
    async fn update_wiki_page(
        &self,
        params: Parameters<UpdateWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        info!("Tool called: update-wiki-page - '{}'", params.0.path);
        debug!("Content length: {} chars", params.0.content.len());

        // Sanitize untrusted agent-provided text fields (IS-01).
        // Path: strip control characters to prevent injection into the NDJSON record.
        // Content and comment: apply the full sanitization pipeline.
        let mut sanitized = params.0;
        sanitized.path = sanitized
            .path
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect();
        sanitized.content = sanitize_text(&sanitized.content);
        sanitized.comment = sanitized.comment.map(|c| sanitize_text(&c));

        let result: UpdateWikiPageResult = sanitized.try_into()?;
        let _ = self.write_safe_output_file(&result).await;

        info!("Wiki page edit queued: '{}'", result.path);
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Wiki page edit queued for '{}'. The page will be created or updated during safe output processing.",
            result.path
        ))]))
    }

    #[tool(
        name = "create-wiki-page",
        description = "Create a new Azure DevOps wiki page with the provided markdown content. \
The page path (e.g. '/Overview/NewPage') and the wiki to write to are determined by the \
pipeline configuration. The page must not already exist — use update-wiki-page to update \
existing pages. Use this to publish findings, summaries, documentation, or any other \
structured output that should be visible in the project wiki."
    )]
    async fn create_wiki_page(
        &self,
        params: Parameters<CreateWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        info!("Tool called: create-wiki-page - '{}'", params.0.path);
        debug!("Content length: {} chars", params.0.content.len());

        // Sanitize untrusted agent-provided text fields (IS-01).
        // Path: strip control characters to prevent injection into the NDJSON record.
        // Content and comment: apply the full sanitization pipeline.
        let mut sanitized = params.0;
        sanitized.path = sanitized
            .path
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect();
        sanitized.content = sanitize_text(&sanitized.content);
        sanitized.comment = sanitized.comment.map(|c| sanitize_text(&c));

        let result: CreateWikiPageResult = sanitized.try_into()?;
        let _ = self.write_safe_output_file(&result).await;

        info!("Wiki page creation queued: '{}'", result.path);
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Wiki page creation queued for '{}'. The page will be created during safe output processing.",
            result.path
        ))]))
    }

    #[tool(
        name = "add-pr-comment",
        description = "Add a comment thread to an Azure DevOps pull request. Supports both \
general comments and file-specific inline comments with optional line positioning. \
The comment will be posted during safe output processing."
    )]
    async fn add_pr_comment(
        &self,
        params: Parameters<AddPrCommentParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: add-pr-comment - PR #{}",
            params.0.pull_request_id
        );
        debug!("Content length: {} chars", params.0.content.len());
        let mut sanitized = params.0;
        sanitized.content = sanitize_text(&sanitized.content);
        let result: AddPrCommentResult = sanitized.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        info!("PR comment queued for PR #{}", result.pull_request_id);
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Comment queued for PR #{}. The comment will be posted during safe output processing.",
            result.pull_request_id
        ))]))
    }

    #[tool(
        name = "link-work-items",
        description = "Create a relationship link between two Azure DevOps work items. \
Supported link types: parent, child, related, predecessor, successor, duplicate, duplicate-of. \
The link will be created during safe output processing."
    )]
    async fn link_work_items(
        &self,
        params: Parameters<LinkWorkItemsParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: link-work-items - {} -> {} ({})",
            params.0.source_id, params.0.target_id, params.0.link_type
        );
        let mut sanitized = params.0;
        sanitized.comment = sanitized.comment.map(|c| sanitize_text(&c));
        let result: LinkWorkItemsResult = sanitized.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Link queued: work item #{} → #{} ({}). The link will be created during safe output processing.",
            result.source_id, result.target_id, result.link_type
        ))]))
    }

    #[tool(
        name = "queue-build",
        description = "Trigger an Azure DevOps pipeline/build run. The pipeline must be in the \
allowed-pipelines list configured in the pipeline definition. Optionally specify a branch \
and template parameters."
    )]
    async fn queue_build(
        &self,
        params: Parameters<QueueBuildParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: queue-build - pipeline {}",
            params.0.pipeline_id
        );
        let mut sanitized = params.0;
        sanitized.reason = sanitized.reason.map(|r| sanitize_text(&r));
        let result: QueueBuildResult = sanitized.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Build queued for pipeline {}. The build will be triggered during safe output processing.",
            result.pipeline_id
        ))]))
    }

    #[tool(
        name = "create-git-tag",
        description = "Create an annotated git tag on a commit in an Azure DevOps repository. \
The tag will be created during safe output processing."
    )]
    async fn create_git_tag(
        &self,
        params: Parameters<CreateGitTagParams>,
    ) -> Result<CallToolResult, McpError> {
        info!("Tool called: create-git-tag - '{}'", params.0.tag_name);
        let mut sanitized = params.0;
        sanitized.message = sanitized.message.map(|m| sanitize_text(&m));
        let result: CreateGitTagResult = sanitized.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Git tag '{}' queued. The tag will be created during safe output processing.",
            result.tag_name
        ))]))
    }

    #[tool(
        name = "add-build-tag",
        description = "Add a tag to an Azure DevOps build for classification and filtering. \
The tag will be added during safe output processing."
    )]
    async fn add_build_tag(
        &self,
        params: Parameters<AddBuildTagParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: add-build-tag - build {} tag '{}'",
            params.0.build_id, params.0.tag
        );
        let result: AddBuildTagResult = params.0.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Build tag '{}' queued for build #{}. The tag will be added during safe output processing.",
            result.tag, result.build_id
        ))]))
    }

    #[tool(
        name = "create-branch",
        description = "Create a new branch in an Azure DevOps repository without creating a \
pull request. The branch will be created during safe output processing."
    )]
    async fn create_branch(
        &self,
        params: Parameters<CreateBranchParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: create-branch - '{}'",
            params.0.branch_name
        );
        let result: CreateBranchResult = params.0.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Branch '{}' queued for creation. The branch will be created during safe output processing.",
            result.branch_name
        ))]))
    }

    #[tool(
        name = "update-pr",
        description = "Update pull request metadata in Azure DevOps. Supports operations: \
add-reviewers, add-labels, set-auto-complete, vote, update-description. \
Changes will be applied during safe output processing."
    )]
    async fn update_pr(
        &self,
        params: Parameters<UpdatePrParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: update-pr - PR #{} operation '{}'",
            params.0.pull_request_id, params.0.operation
        );
        let mut sanitized = params.0;
        sanitized.description = sanitized.description.map(|d| sanitize_text(&d));
        let result: UpdatePrResult = sanitized.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "PR #{} '{}' operation queued. Changes will be applied during safe output processing.",
            result.pull_request_id, result.operation
        ))]))
    }

    #[tool(
        name = "upload-attachment",
        description = "Upload a file attachment to an Azure DevOps work item. The file will be \
uploaded and linked during safe output processing. File size and type restrictions may apply."
    )]
    async fn upload_attachment(
        &self,
        params: Parameters<UploadAttachmentParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: upload-attachment - work item #{} file '{}'",
            params.0.work_item_id, params.0.file_path
        );
        let mut sanitized = params.0;
        sanitized.comment = sanitized.comment.map(|c| sanitize_text(&c));
        let result: UploadAttachmentResult = sanitized.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Attachment '{}' queued for work item #{}. The file will be uploaded during safe output processing.",
            result.file_path, result.work_item_id
        ))]))
    }

    #[tool(
        name = "submit-pr-review",
        description = "Submit a pull request review with a decision (approve, request-changes, \
or comment-only) and an optional body explaining the rationale. The review will be \
submitted during safe output processing. Requires 'allowed-events' to be configured."
    )]
    async fn submit_pr_review(
        &self,
        params: Parameters<SubmitPrReviewParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: submit-pr-review - PR #{} event '{}'",
            params.0.pull_request_id, params.0.event
        );
        let mut sanitized = params.0;
        sanitized.body = sanitized.body.map(|b| sanitize_text(&b));
        let result: SubmitPrReviewResult = sanitized.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "PR review '{}' queued for PR #{}. The review will be submitted during safe output processing.",
            result.event, result.pull_request_id
        ))]))
    }

    #[tool(
        name = "reply-to-pr-review-comment",
        description = "Reply to an existing review comment thread on an Azure DevOps pull request. \
Provide the PR ID, thread ID, and reply content. The reply will be posted during safe output processing."
    )]
    async fn reply_to_pr_review_comment(
        &self,
        params: Parameters<ReplyToPrCommentParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: reply-to-pr-review-comment - PR #{} thread #{}",
            params.0.pull_request_id, params.0.thread_id
        );
        let mut sanitized = params.0;
        sanitized.content = sanitize_text(&sanitized.content);
        let result: ReplyToPrCommentResult = sanitized.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Reply queued for thread #{} on PR #{}. The reply will be posted during safe output processing.",
            result.thread_id, result.pull_request_id
        ))]))
    }

    #[tool(
        name = "resolve-pr-review-thread",
        description = "Resolve or change the status of a review thread on an Azure DevOps pull request. \
Valid statuses: fixed, wont-fix, closed, by-design, active. \
The status change will be applied during safe output processing."
    )]
    async fn resolve_pr_review_thread(
        &self,
        params: Parameters<ResolvePrThreadParams>,
    ) -> Result<CallToolResult, McpError> {
        info!(
            "Tool called: resolve-pr-review-thread - PR #{} thread #{} → '{}'",
            params.0.pull_request_id, params.0.thread_id, params.0.status
        );
        let result: ResolvePrThreadResult = params.0.try_into()?;
        self.write_safe_output_file(&result).await
            .map_err(|e| anyhow_to_mcp_error(anyhow::anyhow!("Failed to write safe output: {}", e)))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Thread #{} status change to '{}' queued for PR #{}. The change will be applied during safe output processing.",
            result.thread_id, result.status, result.pull_request_id
        ))]))
    }

    #[tool(
        name = "report-incomplete",
        description = "Signal that the task could not be completed due to infrastructure failure, \
tool errors, or other environmental issues beyond the agent's control. Use this when the \
agent attempted work but couldn't finish (e.g., API timeouts, build failures, resource limits)."
    )]
    async fn report_incomplete(
        &self,
        params: Parameters<ReportIncompleteParams>,
    ) -> Result<CallToolResult, McpError> {
        warn!("Tool called: report-incomplete - '{}'", params.0.reason);
        let mut sanitized = params.0;
        sanitized.reason = sanitize_text(&sanitized.reason);
        sanitized.context = sanitized.context.map(|c| sanitize_text(&c));
        let result: ReportIncompleteResult = sanitized.try_into()?;
        if let Err(e) = self.write_safe_output_file(&result).await {
            warn!("Failed to write report-incomplete safe output: {}", e);
        }
        Ok(CallToolResult::success(vec![]))
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

pub async fn run(output_directory: &str, bounding_directory: &str, enabled_tools: Option<&[String]>) -> Result<()> {
    // Create and run the server with STDIO transport
    let service = SafeOutputs::new(bounding_directory, output_directory, enabled_tools)
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

/// Run SafeOutputs MCP server over HTTP using the Streamable HTTP protocol.
///
/// This is used for MCPG integration: the gateway connects to this server as an
/// HTTP backend and proxies tool calls from the agent.
pub async fn run_http(
    output_directory: &str,
    bounding_directory: &str,
    port: u16,
    api_key: Option<&str>,
    enabled_tools: Option<&[String]>,
) -> Result<()> {
    use axum::Router;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
        session::local::LocalSessionManager,
    };
    use std::sync::Arc;

    let bounding = bounding_directory.to_string();
    let output = output_directory.to_string();

    // Generate or use provided API key.
    // In production the pipeline always passes --api-key with a cryptographically
    // random value; this fallback covers dev/test invocations.
    let api_key = match api_key {
        Some(k) => k.to_string(),
        None => {
            let mut buf = [0u8; 32];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut f| {
                    use std::io::Read;
                    f.read_exact(&mut buf)
                })
                .context(
                    "Cannot generate secure API key: /dev/urandom unavailable. \
                    Pass --api-key explicitly.",
                )?;
            buf.iter().map(|b| format!("{:02x}", b)).collect()
        }
    };

    info!("Starting SafeOutputs HTTP server on port {}", port);

    let config = StreamableHttpServerConfig {
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
        stateful_mode: true,
    };

    let session_manager = Arc::new(LocalSessionManager::default());

    // Pre-initialize SafeOutputs once and share via clone.
    // The factory closure runs on a Tokio worker thread, so we cannot
    // use block_on() inside it — that would panic with "Cannot start
    // a runtime from within a runtime".
    let safe_outputs_template = SafeOutputs::new(&bounding, &output, enabled_tools).await?;
    let mcp_service = StreamableHttpService::new(
        move || Ok(safe_outputs_template.clone()),
        session_manager,
        config,
    );

    // Wrap with API key auth middleware (constant-time comparison to
    // prevent timing side-channels from a compromised AWF container).
    let expected_key = api_key.clone();
    let app = Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route(
            "/mcp",
            axum::routing::post(axum::routing::any_service(mcp_service.clone()))
                .get(axum::routing::any_service(mcp_service.clone()))
                .delete(axum::routing::any_service(mcp_service)),
        )
        .layer(axum::middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| {
            let expected = expected_key.clone();
            async move {
                // Skip auth for health endpoint
                if req.uri().path() == "/health" {
                    return next.run(req).await;
                }

                // Constant-time comparison to prevent timing side-channels.
                // Length check is non-constant-time but leaking length doesn't
                // help brute-force a high-entropy token.
                if let Some(auth) = req.headers().get("authorization") {
                    if let Ok(auth_str) = auth.to_str() {
                        let expected_header = format!("Bearer {}", expected);
                        use subtle::ConstantTimeEq;
                        let expected_bytes = expected_header.as_bytes();
                        let provided_bytes = auth_str.as_bytes();
                        if expected_bytes.len() == provided_bytes.len()
                            && expected_bytes.ct_eq(provided_bytes).into()
                        {
                            return next.run(req).await;
                        }
                    }
                }

                use axum::response::IntoResponse;
                (axum::http::StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
            }
        }));

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("SafeOutputs HTTP server listening on {}", addr);

    // Print port for pipeline capture (key is already known by the caller)
    println!("SAFE_OUTPUTS_PORT={}", port);
    log::debug!("SafeOutputs API key configured (not printed for security)");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            info!("SafeOutputs HTTP server shutting down");
        })
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn create_test_safe_outputs() -> (SafeOutputs, tempfile::TempDir) {
        let temp_dir = tempdir().unwrap();
        let safe_outputs = SafeOutputs::new(temp_dir.path(), temp_dir.path(), None)
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
        assert_eq!(id.len(), 8);
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
        let result = SafeOutputs::new("/nonexistent/path", temp_dir.path(), None).await;

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
        let result = SafeOutputs::new(temp_dir.path(), "/nonexistent/path", None).await;

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

    // ─── Tool filtering tests ───────────────────────────────────────────

    #[tokio::test]
    async fn test_tool_filtering_none_exposes_all() {
        let temp_dir = tempfile::tempdir().unwrap();
        let so = SafeOutputs::new(temp_dir.path(), temp_dir.path(), None)
            .await
            .unwrap();
        let tools = so.tool_router.list_all();
        // Should have many tools (all registered)
        assert!(tools.len() > 10, "Expected all tools, got {}", tools.len());
    }

    #[tokio::test]
    async fn test_tool_filtering_specific_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        let enabled = vec!["create-pull-request".to_string()];
        let so = SafeOutputs::new(temp_dir.path(), temp_dir.path(), Some(&enabled))
            .await
            .unwrap();
        let tools = so.tool_router.list_all();
        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

        // Should have create-pull-request + always-on tools
        assert!(tool_names.contains(&"create-pull-request".to_string()));
        assert!(tool_names.contains(&"noop".to_string()));
        assert!(tool_names.contains(&"missing-data".to_string()));
        assert!(tool_names.contains(&"missing-tool".to_string()));
        assert!(tool_names.contains(&"report-incomplete".to_string()));

        // Should NOT have other tools
        assert!(!tool_names.contains(&"create-work-item".to_string()));
        assert!(!tool_names.contains(&"update-wiki-page".to_string()));
    }

    #[tokio::test]
    async fn test_tool_filtering_always_on_never_removed() {
        let temp_dir = tempfile::tempdir().unwrap();
        // Enable only a tool that doesn't exist — should still have always-on tools
        let enabled = vec!["nonexistent-tool".to_string()];
        let so = SafeOutputs::new(temp_dir.path(), temp_dir.path(), Some(&enabled))
            .await
            .unwrap();
        let tools = so.tool_router.list_all();
        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

        for always_on in ALWAYS_ON_TOOLS {
            assert!(
                tool_names.contains(&always_on.to_string()),
                "Always-on tool '{}' should be present",
                always_on
            );
        }
    }

    #[tokio::test]
    async fn test_tool_filtering_multiple_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        let enabled = vec![
            "create-pull-request".to_string(),
            "create-work-item".to_string(),
            "comment-on-work-item".to_string(),
        ];
        let so = SafeOutputs::new(temp_dir.path(), temp_dir.path(), Some(&enabled))
            .await
            .unwrap();
        let tools = so.tool_router.list_all();
        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

        // All enabled tools should be present
        for tool in &enabled {
            assert!(
                tool_names.contains(tool),
                "Enabled tool '{}' should be present, got {:?}",
                tool,
                tool_names
            );
        }
        // Always-on tools should be present
        for tool in ALWAYS_ON_TOOLS {
            assert!(
                tool_names.contains(&tool.to_string()),
                "Always-on tool '{}' should be present, got {:?}",
                tool,
                tool_names
            );
        }
        // Non-enabled tools should be absent
        assert!(!tool_names.contains(&"update-wiki-page".to_string()),
            "Non-enabled tool should be filtered out");
    }

    /// Asserts that ALL_KNOWN_SAFE_OUTPUTS contains every tool registered in the
    /// router (plus the non-MCP keys like "memory"). This prevents the list from
    /// drifting when new tools are added to the router but not to the constant.
    #[tokio::test]
    async fn test_all_known_safe_outputs_covers_router() {
        use crate::tools::ALL_KNOWN_SAFE_OUTPUTS;

        let temp_dir = tempfile::tempdir().unwrap();
        let so = SafeOutputs::new(temp_dir.path(), temp_dir.path(), None)
            .await
            .unwrap();
        let router_tools: Vec<String> = so
            .tool_router
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();

        for tool_name in &router_tools {
            assert!(
                ALL_KNOWN_SAFE_OUTPUTS.contains(&tool_name.as_str()),
                "Tool '{}' is registered in the router but missing from ALL_KNOWN_SAFE_OUTPUTS in src/tools/mod.rs",
                tool_name
            );
        }
    }
}
