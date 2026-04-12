//! Create pull request safe output tool

use log::{debug, info, warn};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::tools::{ExecutionContext, ExecutionResult, Executor, ToolResult, Validate};
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use anyhow::{Context, ensure};

/// Maximum allowed patch file size (5 MB)
const MAX_PATCH_SIZE_BYTES: u64 = 5 * 1024 * 1024;

/// Default maximum files allowed in a single PR
const DEFAULT_MAX_FILES: usize = 100;

/// Runtime manifest files that are protected by default.
/// These are dependency/build files that could be modified to introduce supply chain attacks.
const PROTECTED_MANIFEST_BASENAMES: &[&str] = &[
    // npm / Node.js
    "package.json",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "npm-shrinkwrap.json",
    // Go
    "go.mod",
    "go.sum",
    // Python
    "requirements.txt",
    "Pipfile",
    "Pipfile.lock",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "poetry.lock",
    // Ruby
    "Gemfile",
    "Gemfile.lock",
    // Java / Kotlin / Gradle
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "gradle.properties",
    // .NET / C#
    "Directory.Build.props",
    "Directory.Build.targets",
    "global.json",
    // Rust
    "Cargo.toml",
    "Cargo.lock",
];

/// Path prefixes that are protected by default.
/// Files under these paths are blocked unless protected-files is set to "allowed".
const PROTECTED_PATH_PREFIXES: &[&str] = &[
    ".github/",
    ".pipelines/",
    ".azure-pipelines/",
    ".agents/",
    ".claude/",
    ".codex/",
    ".copilot/",
];

/// Exact filenames (at repo root) that are protected by default.
const PROTECTED_EXACT_PATHS: &[&str] = &["CODEOWNERS"];

/// Resolve a reviewer identifier (email, display name, or ID) to an Azure DevOps identity ID.
///
/// If the input is already a GUID, returns it directly. Otherwise, uses the Azure DevOps
/// Identity Picker API to resolve the email or display name to an identity ID.
async fn resolve_reviewer_identity(
    client: &reqwest::Client,
    organization: &str,
    token: &str,
    reviewer: &str,
) -> Option<String> {
    // Check if already a GUID (36 chars with 4 hyphens)
    if reviewer.len() == 36 && reviewer.chars().filter(|c| *c == '-').count() == 4 {
        if reviewer.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            debug!("Reviewer '{}' is already a GUID", reviewer);
            return Some(reviewer.to_string());
        }
    }

    // Use Identity Picker API on vssps.dev.azure.com to resolve email or display name
    let identity_url = format!(
        "https://vssps.dev.azure.com/{}/_apis/identitypicker/identities?api-version=7.1-preview.1",
        organization
    );
    debug!("Identity lookup URL: {}", identity_url);

    let query_body = serde_json::json!({
        "query": reviewer,
        "identityTypes": ["user"],
        "operationScopes": ["ims", "source"],
        "properties": ["DisplayName", "Mail", "SubjectDescriptor"],
        "filterByAncestorEntityIds": [],
        "filterByEntityIds": [],
        "options": {
            "MinResults": 1,
            "MaxResults": 5
        }
    });

    match client
        .post(&identity_url)
        .basic_auth("", Some(token))
        .json(&query_body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(data) => {
                    // Navigate the response: results[0].identities[0].localId
                    if let Some(results) = data.get("results").and_then(|r| r.as_array()) {
                        if let Some(first_result) = results.first() {
                            if let Some(identities) =
                                first_result.get("identities").and_then(|i| i.as_array())
                            {
                                // Try to find exact match first (by email or display name)
                                let reviewer_lower = reviewer.to_lowercase();
                                for identity in identities {
                                    let display_name = identity
                                        .get("displayName")
                                        .and_then(|d| d.as_str())
                                        .unwrap_or_default()
                                        .to_lowercase();
                                    let mail = identity
                                        .get("mail")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or_default()
                                        .to_lowercase();

                                    if display_name == reviewer_lower || mail == reviewer_lower {
                                        if let Some(local_id) =
                                            identity.get("localId").and_then(|id| id.as_str())
                                        {
                                            debug!(
                                                "Resolved reviewer '{}' to ID '{}'",
                                                reviewer, local_id
                                            );
                                            return Some(local_id.to_string());
                                        }
                                    }
                                }
                                // Fall back to first result if no exact match
                                if let Some(first_identity) = identities.first() {
                                    if let Some(local_id) =
                                        first_identity.get("localId").and_then(|id| id.as_str())
                                    {
                                        debug!(
                                            "Resolved reviewer '{}' to first match ID '{}'",
                                            reviewer, local_id
                                        );
                                        return Some(local_id.to_string());
                                    }
                                }
                            }
                        }
                    }
                    warn!("No identity found for reviewer '{}'", reviewer);
                    None
                }
                Err(e) => {
                    warn!(
                        "Failed to parse identity response for '{}': {}",
                        reviewer, e
                    );
                    None
                }
            }
        }
        Ok(resp) => {
            warn!(
                "Identity lookup failed for '{}': {}",
                reviewer,
                resp.status()
            );
            None
        }
        Err(e) => {
            warn!("Identity lookup request failed for '{}': {}", reviewer, e);
            None
        }
    }
}

/// Parameters for creating a pull request
#[derive(Deserialize, JsonSchema)]
pub struct CreatePrParams {
    /// Title for the pull request; should be concise and descriptive
    pub title: String,

    /// Description of the changes in the pull request. Use markdown formatting.
    /// Explain what changes were made and why.
    pub description: String,

    /// Repository to create the PR in. Use "self" for the pipeline's own repository,
    /// or a repository alias from the checkout list for other repositories.
    /// Required when multiple repositories are checked out.
    #[serde(default)]
    pub repository: Option<String>,

    /// Labels to add to the PR for categorization.
    /// These may be subject to an operator-configured allowlist.
    #[serde(default)]
    pub labels: Vec<String>,
}

impl Validate for CreatePrParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.title.len() >= 5,
            "PR title must be at least 5 characters"
        );
        ensure!(
            self.title.len() <= 200,
            "PR title must be at most 200 characters"
        );
        ensure!(
            self.description.len() >= 10,
            "PR description must be at least 10 characters"
        );
        Ok(())
    }
}

/// Result of creating a pull request - stored as safe output
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CreatePrResult {
    /// Tool identifier
    pub name: String,
    /// Title for the pull request
    pub title: String,
    /// Description/body of the pull request (markdown)
    pub description: String,
    /// Source branch name (generated or provided)
    pub source_branch: String,
    /// Path to the patch file in the safe outputs directory
    pub patch_file: String,
    /// Repository alias ("self" or alias from checkout list)
    pub repository: String,
    /// Agent-provided labels (validated against allowed-labels at execution time)
    #[serde(default)]
    pub agent_labels: Vec<String>,
}

impl crate::tools::ToolResult for CreatePrResult {
    const NAME: &'static str = "create-pull-request";
}

impl Sanitize for CreatePrResult {
    fn sanitize_fields(&mut self) {
        self.title = sanitize_text(&self.title);
        self.description = sanitize_text(&self.description);
    }
}

impl CreatePrResult {
    /// Create a new CreatePrResult with all fields
    pub fn new(
        title: String,
        description: String,
        source_branch: String,
        patch_file: String,
        repository: String,
        agent_labels: Vec<String>,
    ) -> Self {
        Self {
            name: Self::NAME.to_string(),
            title,
            description,
            source_branch,
            patch_file,
            repository,
            agent_labels,
        }
    }
}

/// Behavior when the patch is empty or all files were excluded
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IfNoChanges {
    /// Succeed with a warning (default)
    Warn,
    /// Fail the pipeline step
    Error,
    /// Succeed silently
    Ignore,
}

/// File protection policy controlling whether manifest/CI files can be modified
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtectedFiles {
    /// Block modifications to protected files (default)
    Blocked,
    /// Allow modifications to all files
    Allowed,
}

/// Configuration for the create_pr tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   create-pull-request:
///     target-branch: main
///     draft: true
///     auto-complete: true
///     delete-source-branch: true
///     squash-merge: true
///     title-prefix: "[Bot] "
///     if-no-changes: warn
///     max-files: 100
///     protected-files: blocked
///     excluded-files:
///       - "*.lock"
///     allowed-labels:
///       - "automated"
///     reviewers:
///       - "user@example.com"
///     labels:
///       - "automated"
///       - "agent-created"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePrConfig {
    /// Target branch to merge into (default: "main")
    #[serde(default = "default_target_branch", rename = "target-branch")]
    pub target_branch: String,

    /// Whether to create the PR as a draft (default: true)
    #[serde(default = "default_true")]
    pub draft: bool,

    /// Whether to set auto-complete on the PR (default: false)
    #[serde(default, rename = "auto-complete")]
    pub auto_complete: bool,

    /// Whether to delete source branch after merge (default: true)
    #[serde(default = "default_true", rename = "delete-source-branch")]
    pub delete_source_branch: bool,

    /// Whether to squash commits on merge (default: true)
    #[serde(default = "default_true", rename = "squash-merge")]
    pub squash_merge: bool,

    /// Prefix to prepend to all PR titles
    #[serde(default, rename = "title-prefix")]
    pub title_prefix: Option<String>,

    /// Behavior when the patch is empty: "warn" (default), "error", "ignore"
    #[serde(default = "default_if_no_changes", rename = "if-no-changes")]
    pub if_no_changes: IfNoChanges,

    /// Maximum number of files allowed in a single PR (default: 100)
    #[serde(default = "default_max_files", rename = "max-files")]
    pub max_files: usize,

    /// File protection policy: "blocked" (default) or "allowed"
    /// Controls whether manifest/CI files can be modified
    #[serde(default = "default_protected_files", rename = "protected-files")]
    pub protected_files: ProtectedFiles,

    /// Glob patterns for files to exclude from the patch
    #[serde(default, rename = "excluded-files")]
    pub excluded_files: Vec<String>,

    /// Allowlist of labels the agent is permitted to use.
    /// If empty, any labels are accepted.
    #[serde(default, rename = "allowed-labels")]
    pub allowed_labels: Vec<String>,

    /// Reviewers to add to the PR (email addresses or user IDs)
    #[serde(default)]
    pub reviewers: Vec<String>,

    /// Labels to add to the PR
    #[serde(default)]
    pub labels: Vec<String>,

    /// Work item IDs to link to the PR
    #[serde(default, rename = "work-items")]
    pub work_items: Vec<i32>,

    /// Whether to record branch info in failure data when PR creation fails (default: true).
    /// When enabled, the failure response includes the pushed branch name and target branch
    /// so operators can manually create the PR. No work item is created automatically.
    #[serde(default = "default_true", rename = "record-branch-on-failure")]
    pub record_branch_on_failure: bool,
}

fn default_target_branch() -> String {
    "main".to_string()
}

fn default_true() -> bool {
    true
}

fn default_if_no_changes() -> IfNoChanges {
    IfNoChanges::Warn
}

fn default_max_files() -> usize {
    DEFAULT_MAX_FILES
}

fn default_protected_files() -> ProtectedFiles {
    ProtectedFiles::Blocked
}

impl Default for CreatePrConfig {
    fn default() -> Self {
        Self {
            target_branch: default_target_branch(),
            draft: true,
            auto_complete: false,
            delete_source_branch: true,
            squash_merge: true,
            title_prefix: None,
            if_no_changes: default_if_no_changes(),
            max_files: default_max_files(),
            protected_files: default_protected_files(),
            excluded_files: Vec::new(),
            allowed_labels: Vec::new(),
            reviewers: Vec::new(),
            labels: Vec::new(),
            work_items: Vec::new(),
            record_branch_on_failure: true,
        }
    }
}

/// Guard to ensure git worktree is cleaned up on drop
struct WorktreeGuard {
    repo_dir: std::path::PathBuf,
    worktree_path: std::path::PathBuf,
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        // Best effort cleanup - ignore errors
        let _ = std::process::Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                &self.worktree_path.to_string_lossy(),
            ])
            .current_dir(&self.repo_dir)
            .output();
    }
}

#[async_trait::async_trait]
impl Executor for CreatePrResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Creating PR: '{}' in repository '{}'",
            self.title, self.repository
        );
        debug!("PR description length: {} chars", self.description.len());
        debug!("Source branch: {}", self.source_branch);
        debug!("Patch file: {}", self.patch_file);

        let config: CreatePrConfig = ctx.get_tool_config("create-pull-request");
        debug!("Target branch from config: {}", config.target_branch);
        debug!("Draft: {}", config.draft);
        debug!("Auto-complete: {}", config.auto_complete);
        debug!("Squash merge: {}", config.squash_merge);

        // Apply title prefix if configured
        let effective_title = if let Some(ref prefix) = config.title_prefix {
            format!("{}{}", prefix, self.title)
        } else {
            self.title.clone()
        };

        // Validate repository against allowed list
        debug!(
            "Validating repository '{}' against allowed list",
            self.repository
        );
        let repo_id = if self.repository == "self" {
            // "self" uses the pipeline's own repository
            debug!("Using 'self' repository");
            ctx.repository_id
                .as_ref()
                .or(ctx.repository_name.as_ref())
                .context("Repository ID not configured for 'self'")?
                .clone()
        } else if let Some(ado_repo_name) = ctx.allowed_repositories.get(&self.repository) {
            // Alias found in allowed list - use the mapped ADO repo name
            debug!(
                "Repository alias '{}' maps to '{}'",
                self.repository, ado_repo_name
            );
            ado_repo_name.clone()
        } else if ctx.allowed_repositories.is_empty() {
            // No allowed_repositories configured - fall back to default repo (backward compat)
            debug!("No allowed_repositories configured, using default repo");
            ctx.repository_id
                .as_ref()
                .or(ctx.repository_name.as_ref())
                .context("Repository ID not configured")?
                .clone()
        } else {
            // Repository not in allowed list
            warn!(
                "Repository '{}' not in allowed list: {:?}",
                self.repository,
                ctx.allowed_repositories.keys().collect::<Vec<_>>()
            );
            return Ok(ExecutionResult::failure(format!(
                "Repository '{}' is not in the allowed list. Allowed: self, {}",
                self.repository,
                ctx.allowed_repositories
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        debug!("Resolved repository ID: {}", repo_id);

        // Get ADO configuration
        let org_url = ctx
            .ado_org_url
            .as_ref()
            .context("Azure DevOps organization URL not configured")?;
        let organization = ctx
            .ado_organization
            .as_ref()
            .context("Azure DevOps organization name not configured")?;
        let project = ctx
            .ado_project
            .as_ref()
            .context("Azure DevOps project not configured")?;
        let token = ctx
            .access_token
            .as_ref()
            .context("Access token not configured")?;
        debug!(
            "ADO org: {}, organization: {}, project: {}",
            org_url, organization, project
        );

        // Validate and read the patch file
        let patch_path = ctx.working_directory.join(&self.patch_file);
        if !patch_path.exists() {
            return Ok(ExecutionResult::failure(format!(
                "Patch file not found: {}",
                self.patch_file
            )));
        }

        // Security: Enforce patch file size limit
        let metadata = tokio::fs::metadata(&patch_path)
            .await
            .context("Failed to get patch file metadata")?;
        if metadata.len() > MAX_PATCH_SIZE_BYTES {
            return Ok(ExecutionResult::failure(format!(
                "Patch file exceeds maximum size of {} bytes (got {} bytes)",
                MAX_PATCH_SIZE_BYTES,
                metadata.len()
            )));
        }

        // Read patch content for validation
        debug!("Reading patch file content");
        let patch_content = tokio::fs::read_to_string(&patch_path)
            .await
            .context("Failed to read patch file")?;
        debug!("Patch content size: {} bytes", patch_content.len());

        // Filter excluded files from patch content if configured
        let patch_content = if !config.excluded_files.is_empty() {
            debug!("Filtering {} excluded-files patterns from patch", config.excluded_files.len());
            let filtered = filter_excluded_files_from_patch(&patch_content, &config.excluded_files);
            debug!("Patch size after exclusion: {} bytes (was {} bytes)", filtered.len(), patch_content.len());
            if filtered.trim().is_empty() {
                // All files were excluded
                match config.if_no_changes {
                    IfNoChanges::Error => {
                        return Ok(ExecutionResult::failure(
                            "All files in patch were excluded by excluded-files patterns".to_string(),
                        ));
                    }
                    IfNoChanges::Ignore => {
                        return Ok(ExecutionResult::success(
                            "All files in patch were excluded — nothing to do".to_string(),
                        ));
                    }
                    IfNoChanges::Warn => {
                        warn!("All files in patch were excluded by excluded-files patterns (if-no-changes: warn)");
                        return Ok(ExecutionResult::warning(
                            "All files in patch were excluded by excluded-files patterns".to_string(),
                        ));
                    }
                }
            }
            // Rewrite the filtered patch to the patch file
            tokio::fs::write(&patch_path, &filtered)
                .await
                .context("Failed to write filtered patch file")?;
            filtered
        } else {
            patch_content
        };

        // Security: Validate patch paths before applying
        debug!("Validating patch paths for security");
        if let Err(e) = validate_patch_paths(&patch_content) {
            warn!("Patch path validation failed: {}", e);
            return Ok(ExecutionResult::failure(format!(
                "Patch validation failed: {}",
                e
            )));
        }
        debug!("Patch path validation passed");

        // Extract file paths from patch for validation
        let patch_paths = extract_paths_from_patch(&patch_content);

        // Security: File protection check
        if config.protected_files != ProtectedFiles::Allowed {
            let protected = find_protected_files(&patch_paths);
            if !protected.is_empty() {
                warn!(
                    "Patch modifies {} protected file(s): {:?}",
                    protected.len(),
                    protected
                );
                return Ok(ExecutionResult::failure(format!(
                    "Patch modifies protected files (set protected-files: allowed to override): {}",
                    protected.join(", ")
                )));
            }
        }

        // Security: Max files per PR check (count diff blocks, not paths, to avoid
        // double-counting renames which appear in both --- and +++ lines)
        let file_count = count_patch_files(&patch_content);
        if file_count > config.max_files {
            warn!(
                "Patch contains {} files, exceeding max of {}",
                file_count,
                config.max_files
            );
            return Ok(ExecutionResult::failure(format!(
                "Patch contains {} files, exceeding maximum of {} files per PR",
                file_count,
                config.max_files
            )));
        }

        // Use target branch from config
        let target_branch = &config.target_branch;
        let source_ref = format!("refs/heads/{}", self.source_branch);
        let target_ref = format!("refs/heads/{}", target_branch);
        debug!("Source ref: {}, Target ref: {}", source_ref, target_ref);

        // Determine the git repository directory from the source checkout
        // For "self", use the source directory; for other repos, use the subdirectory
        let repo_git_dir = if self.repository == "self" {
            ctx.source_directory.clone()
        } else {
            ctx.source_directory.join(&self.repository)
        };
        debug!("Git repository directory: {}", repo_git_dir.display());

        // Verify this is a git repository
        debug!("Verifying git repository");
        let git_check = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&repo_git_dir)
            .output()
            .await
            .context("Failed to verify git repository")?;

        if !git_check.status.success() {
            warn!("Not a git repository: {}", repo_git_dir.display());
            return Ok(ExecutionResult::failure(format!(
                "Not a git repository: {}",
                repo_git_dir.display()
            )));
        }
        debug!("Git repository verified");

        // Create a temporary directory for the worktree
        let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
        let worktree_path = temp_dir.path().join("worktree");
        debug!("Creating worktree at: {}", worktree_path.display());

        // Create a worktree at the target branch
        let worktree_output = Command::new("git")
            .args([
                "worktree",
                "add",
                &worktree_path.to_string_lossy(),
                &format!("origin/{}", target_branch),
            ])
            .current_dir(&repo_git_dir)
            .output()
            .await
            .context("Failed to create git worktree")?;

        if !worktree_output.status.success() {
            debug!(
                "Worktree creation with origin/ prefix failed, trying without: {}",
                String::from_utf8_lossy(&worktree_output.stderr)
            );
            // Try with just the branch name if origin/ prefix fails
            let worktree_output = Command::new("git")
                .args([
                    "worktree",
                    "add",
                    &worktree_path.to_string_lossy(),
                    target_branch,
                ])
                .current_dir(&repo_git_dir)
                .output()
                .await
                .context("Failed to create git worktree")?;

            if !worktree_output.status.success() {
                warn!(
                    "Failed to create worktree: {}",
                    String::from_utf8_lossy(&worktree_output.stderr)
                );
                return Ok(ExecutionResult::failure(format!(
                    "Failed to create worktree: {}",
                    String::from_utf8_lossy(&worktree_output.stderr)
                )));
            }
        }
        debug!("Worktree created successfully");

        // Ensure worktree cleanup on exit
        let _worktree_guard = WorktreeGuard {
            repo_dir: repo_git_dir.clone(),
            worktree_path: worktree_path.clone(),
        };

        // Create and checkout the source branch in the worktree
        debug!("Creating source branch: {}", self.source_branch);
        let checkout_output = Command::new("git")
            .args(["checkout", "-b", &self.source_branch])
            .current_dir(&worktree_path)
            .output()
            .await
            .context("Failed to create source branch")?;

        if !checkout_output.status.success() {
            warn!(
                "Failed to create source branch: {}",
                String::from_utf8_lossy(&checkout_output.stderr)
            );
            return Ok(ExecutionResult::failure(format!(
                "Failed to create source branch: {}",
                String::from_utf8_lossy(&checkout_output.stderr)
            )));
        }
        debug!("Source branch created");

        // Apply the format-patch using git am --3way for proper conflict handling.
        // git am handles the email-style patch format from git format-patch and
        // --3way enables three-way merge for better conflict resolution.
        debug!("Applying patch with git am --3way");
        let am_output = Command::new("git")
            .args(["am", "--3way", &patch_path.to_string_lossy()])
            .current_dir(&worktree_path)
            .output()
            .await
            .context("Failed to run git am")?;

        // Track whether git am created a commit (affects how we collect changes)
        let patch_committed = am_output.status.success();

        if !patch_committed {
            let stderr = String::from_utf8_lossy(&am_output.stderr);
            debug!("git am --3way failed: {}", stderr);

            // Abort the failed am to leave worktree clean
            let _ = Command::new("git")
                .args(["am", "--abort"])
                .current_dir(&worktree_path)
                .output()
                .await;

            // Fallback: try git apply (handles plain diff format for backward compatibility)
            debug!("Falling back to git apply --3way");
            let apply_output = Command::new("git")
                .args(["apply", "--3way", &patch_path.to_string_lossy()])
                .current_dir(&worktree_path)
                .output()
                .await
                .context("Failed to run git apply --3way")?;

            if !apply_output.status.success() {
                let err_msg = format!(
                    "Patch could not be applied (conflicts): {}",
                    String::from_utf8_lossy(&apply_output.stderr)
                );
                warn!("{}", err_msg);
                return Ok(ExecutionResult::failure(err_msg));
            }
            debug!("Patch applied with git apply --3way fallback");
        } else {
            debug!("Patch applied successfully with git am");
        }

        // Collect changed files. The method depends on how the patch was applied:
        // - git am: changes are committed → use git diff-tree to compare with parent
        // - git apply: changes are in working tree → use git status --porcelain
        debug!("Getting list of changed files");
        let (status_str, use_diff_tree) = if patch_committed {
            let diff_tree_output = Command::new("git")
                .args(["diff-tree", "--no-commit-id", "-r", "--name-status", "HEAD"])
                .current_dir(&worktree_path)
                .output()
                .await
                .context("Failed to run git diff-tree")?;

            if !diff_tree_output.status.success() {
                warn!(
                    "Failed to get diff-tree: {}",
                    String::from_utf8_lossy(&diff_tree_output.stderr)
                );
                return Ok(ExecutionResult::failure(format!(
                    "Failed to get diff-tree: {}",
                    String::from_utf8_lossy(&diff_tree_output.stderr)
                )));
            }
            (String::from_utf8_lossy(&diff_tree_output.stdout).to_string(), true)
        } else {
            let status_output = Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(&worktree_path)
                .output()
                .await
                .context("Failed to run git status")?;

            if !status_output.status.success() {
                warn!(
                    "Failed to get git status: {}",
                    String::from_utf8_lossy(&status_output.stderr)
                );
                return Ok(ExecutionResult::failure(format!(
                    "Failed to get git status: {}",
                    String::from_utf8_lossy(&status_output.stderr)
                )));
            }
            (String::from_utf8_lossy(&status_output.stdout).to_string(), false)
        };

        debug!("Change detection output:\n{}", status_str);
        let changes = if use_diff_tree {
            collect_changes_from_diff_tree(&worktree_path, &status_str).await?
        } else {
            collect_changes_from_worktree(&worktree_path, &status_str).await?
        };
        debug!("Collected {} file changes for push", changes.len());

        if changes.is_empty() {
            // Handle no-changes based on config
            match config.if_no_changes {
                IfNoChanges::Error => {
                    warn!("No changes detected after applying patch (if-no-changes: error)");
                    return Ok(ExecutionResult::failure(
                        "No changes detected after applying patch".to_string(),
                    ));
                }
                IfNoChanges::Ignore => {
                    info!("No changes detected after applying patch (if-no-changes: ignore)");
                    return Ok(ExecutionResult::success(
                        "No changes detected — nothing to do".to_string(),
                    ));
                }
                IfNoChanges::Warn => {
                    warn!("No changes detected after applying patch (if-no-changes: warn)");
                    return Ok(ExecutionResult::warning(
                        "No changes detected after applying patch".to_string(),
                    ));
                }
            }
        }

        // Use ADO REST API to create branch and push changes
        let client = reqwest::Client::new();

        // Get the target branch ref to find the base commit
        debug!("Getting target branch ref from ADO");
        let refs_url = format!(
            "{}{}/_apis/git/repositories/{}/refs?filter=heads/{}&api-version=7.1",
            org_url, project, repo_id, target_branch
        );
        debug!("Refs URL: {}", refs_url);

        let refs_response = client
            .get(&refs_url)
            .basic_auth("", Some(token))
            .send()
            .await
            .context("Failed to get target branch ref")?;

        if !refs_response.status().is_success() {
            let status = refs_response.status();
            let body = refs_response.text().await.unwrap_or_default();
            warn!("Failed to get target branch ref: {} - {}", status, body);
            return Ok(ExecutionResult::failure(format!(
                "Failed to get target branch ref: {} - {}",
                status, body
            )));
        }

        let refs_data: serde_json::Value = refs_response.json().await?;
        let base_commit = refs_data["value"][0]["objectId"]
            .as_str()
            .context("Could not find target branch commit")?;
        debug!("Base commit: {}", base_commit);

        info!(
            "Base commit for target branch '{}': {}",
            target_branch, base_commit
        );

        // Push changes via ADO API (this creates the branch and commits in one call)
        info!("Pushing changes to ADO");
        let push_url = format!(
            "{}{}/_apis/git/repositories/{}/pushes?api-version=7.1",
            org_url, project, repo_id
        );
        debug!("Push URL: {}", push_url);

        // For creating a new branch with a commit:
        // - refUpdates.oldObjectId = zeros (new ref)
        // - commits[0].parents = [base_commit] (parent of new commit)
        // ADO will compute the newObjectId from the commit
        let push_body = serde_json::json!({
            "refUpdates": [{
                "name": source_ref,
                "oldObjectId": "0000000000000000000000000000000000000000"
            }],
            "commits": [{
                "comment": effective_title,
                "changes": changes,
                "parents": [base_commit]
            }]
        });

        debug!(
            "Push request body: {}",
            serde_json::to_string_pretty(&push_body).unwrap_or_default()
        );

        let push_response = client
            .post(&push_url)
            .basic_auth("", Some(token))
            .json(&push_body)
            .send()
            .await
            .context("Failed to push changes")?;

        if !push_response.status().is_success() {
            let status = push_response.status();
            let body = push_response.text().await.unwrap_or_default();
            warn!("Failed to push changes: {} - {}", status, body);
            return Ok(ExecutionResult::failure(format!(
                "Failed to push changes: {} - {}",
                status, body
            )));
        }
        debug!("Changes pushed successfully");

        // Append provenance footer to description
        let description_with_footer = format!("{}{}", self.description, generate_pr_footer());

        // Create the pull request via REST API
        info!("Creating pull request");
        let pr_url = format!(
            "{}{}/_apis/git/repositories/{}/pullrequests?api-version=7.1",
            org_url, project, repo_id
        );
        debug!("PR URL: {}", pr_url);

        let mut pr_body = serde_json::json!({
            "sourceRefName": source_ref,
            "targetRefName": target_ref,
            "title": effective_title,
            "description": description_with_footer,
            "isDraft": config.draft,
        });

        // Add work item links if configured
        if !config.work_items.is_empty() {
            debug!("Linking {} work items", config.work_items.len());
            pr_body["workItemRefs"] = serde_json::json!(
                config
                    .work_items
                    .iter()
                    .map(|id| serde_json::json!({"id": id}))
                    .collect::<Vec<_>>()
            );
        }

        // Validate and add labels (merge operator labels + validated agent labels)
        let mut all_labels = config.labels.clone();

        // Validate agent-provided labels against allowed-labels
        if !self.agent_labels.is_empty() {
            if !config.allowed_labels.is_empty() {
                let disallowed: Vec<_> = self
                    .agent_labels
                    .iter()
                    .filter(|l| !config.allowed_labels.contains(l))
                    .collect();
                if !disallowed.is_empty() {
                    warn!(
                        "Agent labels not in allowed-labels: {:?}",
                        disallowed
                    );
                    return Ok(ExecutionResult::failure(format!(
                        "Agent-provided labels not in allowed-labels: {}",
                        disallowed.iter().map(|l| l.as_str()).collect::<Vec<_>>().join(", ")
                    )));
                }
            }
            // Merge agent labels (dedup)
            for label in &self.agent_labels {
                if !all_labels.contains(label) {
                    all_labels.push(label.clone());
                }
            }
        }

        if !all_labels.is_empty() {
            debug!("Adding {} labels", all_labels.len());
            pr_body["labels"] = serde_json::json!(
                all_labels
                    .iter()
                    .map(|l| serde_json::json!({"name": l}))
                    .collect::<Vec<_>>()
            );
        }

        let pr_response = client
            .post(&pr_url)
            .basic_auth("", Some(token))
            .json(&pr_body)
            .send()
            .await
            .context("Failed to create pull request")?;

        if !pr_response.status().is_success() {
            let status = pr_response.status();
            let body = pr_response.text().await.unwrap_or_default();
            warn!("Failed to create pull request: {} - {}", status, body);

            // Record branch info for manual recovery if enabled
            if config.record_branch_on_failure {
                info!("PR creation failed, recording branch info for manual recovery");
                let fallback_description = format!(
                    "## Pull Request Creation Failed\n\n\
                    A pull request could not be created automatically.\n\n\
                    **Branch:** `{}`\n\
                    **Target:** `{}`\n\
                    **Repository:** `{}`\n\n\
                    **Error:** {} - {}\n\n\
                    ### Original PR Description\n\n\
                    {}\n\n\
                    ---\n\
                    *To create the PR manually, merge branch `{}` into `{}`.*",
                    self.source_branch, target_branch, self.repository,
                    status, body,
                    self.description,
                    self.source_branch, target_branch
                );
                return Ok(ExecutionResult::failure_with_data(
                    format!(
                        "Failed to create pull request: {} - {}. Branch '{}' was pushed — create the PR manually.",
                        status, body, self.source_branch,
                    ),
                    serde_json::json!({
                        "fallback": "branch-recorded",
                        "branch": self.source_branch,
                        "target_branch": target_branch,
                        "repository": self.repository,
                        "description": fallback_description
                    }),
                ));
            }

            return Ok(ExecutionResult::failure(format!(
                "Failed to create pull request: {} - {}",
                status, body
            )));
        }

        let pr_data: serde_json::Value = pr_response.json().await?;
        let pr_id = pr_data["pullRequestId"].as_i64().unwrap_or(0);
        let pr_web_url = pr_data["url"].as_str().unwrap_or("");
        info!("Pull request created: #{} - {}", pr_id, pr_web_url);

        // Set completion options (delete source branch, squash merge) and optionally auto-complete
        // completionOptions apply when the PR is completed by anyone, auto_complete makes it complete automatically
        {
            debug!(
                "Setting PR completion options: delete_source_branch={}, squash_merge={}, auto_complete={}",
                config.delete_source_branch, config.squash_merge, config.auto_complete
            );
            let pr_update_url = format!(
                "{}{}/_apis/git/repositories/{}/pullrequests/{}?api-version=7.1",
                org_url, project, repo_id, pr_id
            );

            let mut update_body = serde_json::json!({
                "completionOptions": {
                    "deleteSourceBranch": config.delete_source_branch,
                    "squashMerge": config.squash_merge
                }
            });

            // Only set autoCompleteSetBy if auto_complete is enabled
            if config.auto_complete {
                update_body["autoCompleteSetBy"] = serde_json::json!({
                    "id": pr_data["createdBy"]["id"]
                });
            }

            match client
                .patch(&pr_update_url)
                .basic_auth("", Some(token))
                .json(&update_body)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    debug!("PR completion options set successfully");
                }
                Ok(resp) => {
                    warn!("Failed to set PR completion options: {}", resp.status());
                }
                Err(e) => {
                    warn!("Failed to set PR completion options: {}", e);
                }
            }
        }

        // Add reviewers if configured
        if !config.reviewers.is_empty() {
            debug!("Adding {} reviewers", config.reviewers.len());
            for reviewer in &config.reviewers {
                debug!("Adding reviewer: {}", reviewer);

                // Resolve reviewer identity (email/name -> ID)
                let reviewer_id =
                    match resolve_reviewer_identity(&client, organization, token, reviewer).await {
                        Some(id) => id,
                        None => {
                            warn!(
                                "Could not resolve reviewer '{}' to an identity ID, skipping",
                                reviewer
                            );
                            continue;
                        }
                    };

                let reviewer_url = format!(
                    "{}{}/_apis/git/repositories/{}/pullrequests/{}/reviewers/{}?api-version=7.1",
                    org_url, project, repo_id, pr_id, reviewer_id
                );

                let reviewer_body = serde_json::json!({
                    "vote": 0,
                    "isRequired": false
                });

                match client
                    .put(&reviewer_url)
                    .basic_auth("", Some(token))
                    .json(&reviewer_body)
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        debug!(
                            "Reviewer '{}' (ID: {}) added successfully",
                            reviewer, reviewer_id
                        );
                    }
                    Ok(resp) => {
                        warn!(
                            "Failed to add reviewer '{}' (ID: {}): {}",
                            reviewer,
                            reviewer_id,
                            resp.status()
                        );
                    }
                    Err(e) => {
                        warn!("Failed to add reviewer '{}': {}", reviewer, e);
                    }
                }
            }
        }

        info!(
            "PR #{} created successfully: {} -> {}{}",
            pr_id, self.source_branch, target_branch,
            if config.draft { " (draft)" } else { "" }
        );

        Ok(ExecutionResult::success_with_data(
            format!("Created pull request #{}: {}", pr_id, effective_title),
            serde_json::json!({
                "pull_request_id": pr_id,
                "url": pr_web_url,
                "source_branch": self.source_branch,
                "target_branch": target_branch,
                "draft": config.draft
            }),
        ))
    }
}

/// Collect file changes from a worktree based on git status output
///
/// Parses git status --porcelain output and reads file contents to build
/// ADO Push API change objects with full file content.
async fn collect_changes_from_worktree(
    worktree_path: &std::path::Path,
    status_output: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut changes = Vec::new();

    for line in status_output.lines() {
        if line.len() < 3 {
            continue;
        }

        let status_code = &line[0..2];
        let file_path = line[3..].trim();

        // Skip empty paths
        if file_path.is_empty() {
            continue;
        }

        // Validate path for security
        validate_single_path(file_path)?;

        let full_path = worktree_path.join(file_path);

        match status_code {
            // Deleted files
            " D" | "D " | "DD" => {
                changes.push(serde_json::json!({
                    "changeType": "delete",
                    "item": {
                        "path": format!("/{}", file_path)
                    }
                }));
            }
            // New/untracked files
            "??" | "A " | " A" | "AM" => {
                if full_path.is_file() {
                    changes.push(read_file_change("add", file_path, &full_path).await?);
                }
            }
            // Modified files
            " M" | "M " | "MM" => {
                if full_path.is_file() {
                    changes.push(read_file_change("edit", file_path, &full_path).await?);
                }
            }
            // Renamed files - format is "R  old_path -> new_path"
            // For "RM" (renamed + modified), we emit both a rename and an edit change.
            // The ADO Pushes API processes changes sequentially within a single push,
            // so the rename establishes the file at the new path, then the edit updates
            // its content — this is the correct way to express rename-with-modification.
            "R " | " R" | "RM" => {
                if let Some((old_path, new_path)) = file_path.split_once(" -> ") {
                    validate_single_path(old_path.trim())?;
                    validate_single_path(new_path.trim())?;

                    changes.push(serde_json::json!({
                        "changeType": "rename",
                        "sourceServerItem": format!("/{}", old_path.trim()),
                        "item": {
                            "path": format!("/{}", new_path.trim())
                        }
                    }));

                    // If status is "RM" (renamed + modified), also emit content
                    if status_code == "RM" {
                        let new_full_path = worktree_path.join(new_path.trim());
                        if new_full_path.is_file() {
                            changes.push(read_file_change("edit", new_path.trim(), &new_full_path).await?);
                        }
                    }
                }
            }
            // Other statuses - try to handle as edit if file exists
            _ => {
                if full_path.is_file() {
                    changes.push(read_file_change("edit", file_path, &full_path).await?);
                }
            }
        }
    }

    Ok(changes)
}

/// Collect file changes from a git diff-tree --name-status output.
///
/// Used when git am has already committed the changes. Parses the output format:
/// `M\tpath`, `A\tpath`, `D\tpath`, `R100\told_path\tnew_path`
async fn collect_changes_from_diff_tree(
    worktree_path: &std::path::Path,
    diff_tree_output: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut changes = Vec::new();

    for line in diff_tree_output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }

        let status_code = parts[0];
        let file_path = parts[1];

        // Validate path for security
        validate_single_path(file_path)?;

        let full_path = worktree_path.join(file_path);

        if status_code == "D" {
            // Deleted file
            changes.push(serde_json::json!({
                "changeType": "delete",
                "item": {
                    "path": format!("/{}", file_path)
                }
            }));
        } else if status_code == "A" {
            // Added file
            if full_path.is_file() {
                changes.push(read_file_change("add", file_path, &full_path).await?);
            }
        } else if status_code.starts_with('R') && parts.len() >= 3 {
            // Renamed file: R100\told_path\tnew_path
            let old_path = file_path;
            let new_path = parts[2];
            validate_single_path(old_path)?;
            validate_single_path(new_path)?;

            // Emit the rename
            changes.push(serde_json::json!({
                "changeType": "rename",
                "sourceServerItem": format!("/{}", old_path),
                "item": {
                    "path": format!("/{}", new_path)
                }
            }));

            // If the file was also modified (similarity < 100), emit an edit with content
            let new_full_path = worktree_path.join(new_path);
            if status_code != "R100" && new_full_path.is_file() {
                changes.push(read_file_change("edit", new_path, &new_full_path).await?);
            }
        } else {
            // Modified or other — read current content
            if full_path.is_file() {
                changes.push(read_file_change("edit", file_path, &full_path).await?);
            }
        }
    }

    Ok(changes)
}

/// Read a file and produce an ADO push change entry.
/// Handles both text (rawtext) and binary (base64encoded) content.
async fn read_file_change(
    change_type: &str,
    file_path: &str,
    full_path: &std::path::Path,
) -> anyhow::Result<serde_json::Value> {
    let bytes = tokio::fs::read(full_path)
        .await
        .with_context(|| format!("Failed to read file: {}", file_path))?;

    // Try UTF-8 first; fall back to base64 for binary files
    match String::from_utf8(bytes.clone()) {
        Ok(content) => Ok(serde_json::json!({
            "changeType": change_type,
            "item": {
                "path": format!("/{}", file_path)
            },
            "newContent": {
                "content": content,
                "contentType": "rawtext"
            }
        })),
        Err(_) => {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Ok(serde_json::json!({
                "changeType": change_type,
                "item": {
                    "path": format!("/{}", file_path)
                },
                "newContent": {
                    "content": encoded,
                    "contentType": "base64encoded"
                }
            }))
        }
    }
}
///
/// Security checks:
/// - No path traversal (..)
/// - No .git directory modifications
/// - No absolute paths
/// - No null bytes
fn validate_patch_paths(patch_content: &str) -> anyhow::Result<()> {
    for line in patch_content.lines() {
        // Check diff headers for file paths
        if line.starts_with("diff --git") {
            // Extract paths from "diff --git a/path b/path"
            let parts: Vec<&str> = line.split_whitespace().collect();
            for part in parts.iter().skip(2) {
                let path = part.trim_start_matches("a/").trim_start_matches("b/");
                validate_single_path(path)?;
            }
        } else if line.starts_with("---") || line.starts_with("+++") {
            // Extract path from "--- a/path" or "+++ b/path"
            if let Some(path) = line.split_whitespace().nth(1) {
                if path != "/dev/null" {
                    let clean_path = path.trim_start_matches("a/").trim_start_matches("b/");
                    validate_single_path(clean_path)?;
                }
            }
        } else if line.starts_with("rename from ") || line.starts_with("rename to ") {
            let path = line.split_whitespace().last().unwrap_or("");
            validate_single_path(path)?;
        } else if line.starts_with("copy from ") || line.starts_with("copy to ") {
            let path = line.split_whitespace().last().unwrap_or("");
            validate_single_path(path)?;
        }
    }
    Ok(())
}

/// Validate a single file path for security issues
fn validate_single_path(path: &str) -> anyhow::Result<()> {
    // Check for null bytes
    ensure!(!path.contains('\0'), "Path contains null byte: {:?}", path);

    // Check for absolute paths
    ensure!(
        !path.starts_with('/') && !path.starts_with('\\'),
        "Absolute paths not allowed: {}",
        path
    );

    // Check for Windows absolute paths (C:\, D:\, etc.)
    ensure!(
        !(path.len() >= 2 && path.chars().nth(1) == Some(':')),
        "Windows absolute paths not allowed: {}",
        path
    );

    // Check for path traversal
    for component in path.split(['/', '\\']) {
        ensure!(component != "..", "Path traversal not allowed: {}", path);
    }

    // Check for .git directory modifications
    let lower_path = path.to_lowercase();
    ensure!(
        !lower_path.starts_with(".git/")
            && !lower_path.starts_with(".git\\")
            && lower_path != ".git",
        ".git directory modifications not allowed: {}",
        path
    );

    // Check for git hooks specifically
    ensure!(
        !lower_path.contains(".git/hooks") && !lower_path.contains(".git\\hooks"),
        "Git hooks modifications not allowed: {}",
        path
    );

    Ok(())
}

/// Extract all file paths from a patch/diff content.
/// Returns deduplicated list of file paths referenced in the patch (both source and destination).
/// Uses `--- a/` and `+++ b/` lines for robust parsing (handles quoted paths
/// with spaces that break `diff --git` header parsing via split_whitespace).
fn extract_paths_from_patch(patch_content: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in patch_content.lines() {
        if let Some(rest) = line.strip_prefix("--- a/") {
            let path = rest.trim().trim_matches('"');
            if !path.is_empty() {
                paths.push(path.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("+++ b/") {
            let path = rest.trim().trim_matches('"');
            if !path.is_empty() {
                paths.push(path.to_string());
            }
        } else if line.starts_with("rename from ") || line.starts_with("rename to ") ||
                  line.starts_with("copy from ") || line.starts_with("copy to ") {
            // "rename from <path>" / "rename to <path>"
            let path = line.splitn(3, ' ').nth(2).unwrap_or("").trim_matches('"');
            if !path.is_empty() {
                paths.push(path.to_string());
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

/// Count the number of distinct file changes in a patch.
/// Uses `diff --git` headers as the canonical count — each header represents exactly
/// one file change (add, edit, delete, or rename). This avoids double-counting renames,
/// which produce both `--- a/old` and `+++ b/new` lines.
fn count_patch_files(patch_content: &str) -> usize {
    patch_content
        .lines()
        .filter(|line| line.starts_with("diff --git"))
        .count()
}

/// Check if any file paths in the patch are protected.
///
/// Protected files include:
/// - Runtime manifests (package.json, go.mod, Cargo.toml, etc.)
/// - CI/pipeline configurations (.github/, .pipelines/, etc.)
/// - Agent instruction files (.agents/, .claude/, .codex/, .copilot/)
/// - Access control files (CODEOWNERS)
///
/// Returns a list of protected file paths found, or empty vec if none.
fn find_protected_files(paths: &[String]) -> Vec<String> {
    let mut protected = Vec::new();
    for path in paths {
        let lower_path = path.to_lowercase();

        // Check basename against manifest list
        let basename = path.rsplit('/').next().unwrap_or(path);
        let lower_basename = basename.to_lowercase();
        for manifest in PROTECTED_MANIFEST_BASENAMES {
            if lower_basename == *manifest {
                protected.push(path.clone());
                break;
            }
        }

        // Check path prefixes
        for prefix in PROTECTED_PATH_PREFIXES {
            if lower_path.starts_with(prefix) || lower_path.starts_with(&prefix.replace('/', "\\")) {
                if !protected.contains(path) {
                    protected.push(path.clone());
                }
                break;
            }
        }

        // Check exact paths
        for exact in PROTECTED_EXACT_PATHS {
            if lower_path == exact.to_lowercase() {
                if !protected.contains(path) {
                    protected.push(path.clone());
                }
                break;
            }
        }
    }
    protected
}

/// Generate a provenance footer for the PR body
fn generate_pr_footer() -> String {
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    format!(
        "\n\n---\n\
        > 🤖 *This pull request was created by an automated agent.*\n\
        > Generated at: {}\n\
        > Compiler: ado-aw v{}",
        timestamp,
        env!("CARGO_PKG_VERSION")
    )
}

/// Filter out diff entries for files matching excluded-files glob patterns.
/// Splits the patch into per-file diff blocks and removes those matching any pattern.
/// Uses `+++ b/` lines for path extraction (robust with quoted/space-containing paths).
///
/// Patterns without a `/` are automatically prefixed with `**/` so that e.g. `*.lock`
/// matches `subdir/Cargo.lock` (not just root-level lockfiles).
///
/// Note: When using format-patch output with multiple commits, excluding all diffs within
/// a commit leaves the `From <SHA> ...` / `Subject:` envelope intact with no diff hunks.
/// `git am` treats these as empty commits, which is harmless but may leave no-op commits
/// on the branch.
fn filter_excluded_files_from_patch(patch_content: &str, excluded_patterns: &[String]) -> String {
    if excluded_patterns.is_empty() {
        return patch_content.to_string();
    }

    let normalized: Vec<String> = excluded_patterns
        .iter()
        .map(|p| normalize_glob_pattern(p))
        .collect();

    let mut result = String::with_capacity(patch_content.len());
    let mut current_block = String::new();
    let mut current_path: Option<String> = None;

    for line in patch_content.lines() {
        if line.starts_with("diff --git") {
            // Flush previous block if it's not excluded
            if let Some(ref path) = current_path {
                if !normalized.iter().any(|p| glob_match::glob_match(p, path)) {
                    result.push_str(&current_block);
                } else {
                    debug!("Excluding file from patch: {}", path);
                }
            } else if !current_block.is_empty() {
                result.push_str(&current_block);
            }

            // Start new block, path will be set when we see +++ b/
            current_block = String::new();
            current_block.push_str(line);
            current_block.push('\n');
            current_path = None;
        } else {
            // Extract path from "+++ b/path" line (handles quoted paths)
            if let Some(rest) = line.strip_prefix("+++ b/") {
                current_path = Some(rest.trim().trim_matches('"').to_string());
            }
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    // Flush last block
    if let Some(ref path) = current_path {
        if !normalized.iter().any(|p| glob_match::glob_match(p, path)) {
            result.push_str(&current_block);
        } else {
            debug!("Excluding file from patch: {}", path);
        }
    } else if !current_block.is_empty() {
        result.push_str(&current_block);
    }

    result
}

/// Normalize a glob pattern for cross-directory matching.
/// Patterns without a `/` are prefixed with `**/` so that e.g. `*.lock` matches
/// `subdir/Cargo.lock`, not just root-level files.
fn normalize_glob_pattern(pattern: &str) -> String {
    if pattern.contains('/') || pattern.starts_with("**/") {
        pattern.to_string()
    } else {
        format!("**/{}", pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_params_valid() {
        let params = CreatePrParams {
            title: "Fix bug in parser".to_string(),
            description: "This PR fixes a critical bug in the parser module.".to_string(),
            repository: None,
            labels: vec![],
        };
        assert!(params.validate().is_ok());
    }

    #[test]
    fn test_validate_params_short_title() {
        let params = CreatePrParams {
            title: "Fix".to_string(),
            description: "This PR fixes a critical bug in the parser module.".to_string(),
            repository: None,
            labels: vec![],
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_validate_params_short_description() {
        let params = CreatePrParams {
            title: "Fix bug in parser".to_string(),
            description: "Fix bug".to_string(),
            repository: None,
            labels: vec![],
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_config_default_target_branch() {
        let config = CreatePrConfig::default();
        assert_eq!(config.target_branch, "main");
        assert!(config.draft);
        assert!(!config.auto_complete);
        assert!(config.delete_source_branch);
        assert!(config.squash_merge);
        assert_eq!(config.if_no_changes, IfNoChanges::Warn);
        assert_eq!(config.max_files, 100);
        assert_eq!(config.protected_files, ProtectedFiles::Blocked);
        assert!(config.excluded_files.is_empty());
        assert!(config.allowed_labels.is_empty());
        assert!(config.record_branch_on_failure);
    }

    #[test]
    fn test_validate_patch_paths_valid() {
        let patch = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
     println!("World");
 }
"#;
        assert!(validate_patch_paths(patch).is_ok());
    }

    #[test]
    fn test_validate_patch_paths_new_file() {
        let patch = r#"diff --git a/new_file.txt b/new_file.txt
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1,2 @@
+Hello
+World
"#;
        assert!(validate_patch_paths(patch).is_ok());
    }

    #[test]
    fn test_validate_patch_paths_traversal_rejected() {
        let patch = r#"diff --git a/../../../etc/passwd b/../../../etc/passwd
--- a/../../../etc/passwd
+++ b/../../../etc/passwd
"#;
        assert!(validate_patch_paths(patch).is_err());
    }

    #[test]
    fn test_validate_patch_paths_git_dir_rejected() {
        let patch = r#"diff --git a/.git/hooks/pre-commit b/.git/hooks/pre-commit
new file mode 100755
--- /dev/null
+++ b/.git/hooks/pre-commit
@@ -0,0 +1 @@
+#!/bin/bash
"#;
        assert!(validate_patch_paths(patch).is_err());
    }

    #[test]
    fn test_validate_patch_paths_absolute_rejected() {
        let patch = r#"diff --git a//etc/passwd b//etc/passwd
--- a//etc/passwd
+++ b//etc/passwd
"#;
        assert!(validate_patch_paths(patch).is_err());
    }

    #[test]
    fn test_validate_single_path_valid() {
        assert!(validate_single_path("src/main.rs").is_ok());
        assert!(validate_single_path("deeply/nested/path/file.txt").is_ok());
        assert!(validate_single_path("file.txt").is_ok());
    }

    #[test]
    fn test_validate_single_path_traversal() {
        assert!(validate_single_path("../secret.txt").is_err());
        assert!(validate_single_path("foo/../bar").is_err());
        assert!(validate_single_path("foo/bar/../../baz").is_err());
    }

    #[test]
    fn test_validate_single_path_git_dir() {
        assert!(validate_single_path(".git").is_err());
        assert!(validate_single_path(".git/config").is_err());
        assert!(validate_single_path(".git/hooks/pre-commit").is_err());
        assert!(validate_single_path(".GIT/config").is_err()); // case insensitive
    }

    #[test]
    fn test_validate_single_path_absolute() {
        assert!(validate_single_path("/etc/passwd").is_err());
        assert!(validate_single_path("\\Windows\\System32").is_err());
        assert!(validate_single_path("C:\\Windows").is_err());
    }

    #[test]
    fn test_validate_single_path_null_byte() {
        assert!(validate_single_path("file\0.txt").is_err());
    }

    // ─── Protected files detection ──────────────────────────────────────────

    #[test]
    fn test_find_protected_files_manifests() {
        let paths = vec![
            "src/main.rs".to_string(),
            "package.json".to_string(),
            "go.mod".to_string(),
        ];
        let protected = find_protected_files(&paths);
        assert_eq!(protected, vec!["package.json", "go.mod"]);
    }

    #[test]
    fn test_find_protected_files_ci_configs() {
        let paths = vec![
            "src/lib.rs".to_string(),
            ".github/workflows/ci.yml".to_string(),
            ".pipelines/build.yml".to_string(),
        ];
        let protected = find_protected_files(&paths);
        assert!(protected.contains(&".github/workflows/ci.yml".to_string()));
        assert!(protected.contains(&".pipelines/build.yml".to_string()));
    }

    #[test]
    fn test_find_protected_files_agent_configs() {
        let paths = vec![
            ".agents/config.md".to_string(),
            ".claude/settings.json".to_string(),
            ".copilot/instructions.md".to_string(),
        ];
        let protected = find_protected_files(&paths);
        assert_eq!(protected.len(), 3);
    }

    #[test]
    fn test_find_protected_files_codeowners() {
        let paths = vec!["CODEOWNERS".to_string(), "README.md".to_string()];
        let protected = find_protected_files(&paths);
        assert_eq!(protected, vec!["CODEOWNERS"]);
    }

    #[test]
    fn test_find_protected_files_none() {
        let paths = vec![
            "src/main.rs".to_string(),
            "docs/README.md".to_string(),
        ];
        let protected = find_protected_files(&paths);
        assert!(protected.is_empty());
    }

    #[test]
    fn test_find_protected_files_nested_manifest() {
        let paths = vec!["services/api/package.json".to_string()];
        let protected = find_protected_files(&paths);
        assert_eq!(protected, vec!["services/api/package.json"]);
    }

    // ─── Excluded files filtering ───────────────────────────────────────────

    #[test]
    fn test_filter_excluded_files_basic() {
        let patch = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/Cargo.lock b/Cargo.lock\n--- a/Cargo.lock\n+++ b/Cargo.lock\n@@ -1 +1 @@\n-old\n+new\n";
        let result = filter_excluded_files_from_patch(patch, &["*.lock".to_string()]);
        assert!(result.contains("src/main.rs"));
        assert!(!result.contains("Cargo.lock"));
    }

    #[test]
    fn test_filter_excluded_files_nested_glob() {
        // *.lock should match subdir/Cargo.lock thanks to auto-prepended **/
        let patch = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/services/api/package-lock.json b/services/api/package-lock.json\n--- a/services/api/package-lock.json\n+++ b/services/api/package-lock.json\n@@ -1 +1 @@\n-old\n+new\n";
        let result =
            filter_excluded_files_from_patch(patch, &["package-lock.json".to_string()]);
        assert!(result.contains("src/main.rs"));
        assert!(!result.contains("package-lock.json"));
    }

    #[test]
    fn test_normalize_glob_pattern() {
        // Patterns without / get **/ prefix
        assert_eq!(normalize_glob_pattern("*.lock"), "**/*.lock");
        assert_eq!(normalize_glob_pattern("Cargo.toml"), "**/Cargo.toml");
        // Patterns with / stay as-is
        assert_eq!(normalize_glob_pattern("src/*.rs"), "src/*.rs");
        // Already-prefixed patterns stay as-is
        assert_eq!(normalize_glob_pattern("**/*.lock"), "**/*.lock");
    }

    #[test]
    fn test_filter_excluded_files_empty_patterns() {
        let patch = "diff --git a/file.txt b/file.txt\n+content\n";
        let result = filter_excluded_files_from_patch(patch, &[]);
        assert_eq!(result, patch);
    }

    // ─── Extract paths from patch ───────────────────────────────────────────

    #[test]
    fn test_extract_paths_from_patch() {
        let patch = "diff --git a/src/main.rs b/src/main.rs\nindex abc..def\n--- a/src/main.rs\n+++ b/src/main.rs\ndiff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n";
        let paths = extract_paths_from_patch(patch);
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(paths.contains(&"README.md".to_string()));
    }

    #[test]
    fn test_extract_paths_from_patch_with_spaces() {
        let patch = "diff --git \"a/path with spaces/file.txt\" \"b/path with spaces/file.txt\"\n--- a/path with spaces/file.txt\n+++ b/path with spaces/file.txt\n";
        let paths = extract_paths_from_patch(patch);
        assert!(paths.contains(&"path with spaces/file.txt".to_string()));
    }

    #[test]
    fn test_extract_paths_new_file() {
        let patch = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n";
        let paths = extract_paths_from_patch(patch);
        assert!(paths.contains(&"new.txt".to_string()));
        // /dev/null from --- should not be included (no a/ prefix)
        assert!(!paths.contains(&"/dev/null".to_string()));
    }

}
