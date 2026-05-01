//! Create pull request safe output tool

use log::{debug, info, warn};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use ado_aw_derive::SanitizeConfig;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, ToolResult, Validate};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use anyhow::{Context, ensure};

/// Maximum allowed patch file size (5 MB)
const MAX_PATCH_SIZE_BYTES: u64 = 5 * 1024 * 1024;

/// Default maximum files allowed in a single PR
const DEFAULT_MAX_FILES: usize = 100;

/// Runtime manifest files that are protected by default (all lowercase for
/// case-insensitive comparison).
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
    "pipfile",
    "pipfile.lock",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "poetry.lock",
    // Ruby
    "gemfile",
    "gemfile.lock",
    // Java / Kotlin / Gradle
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "gradle.properties",
    // .NET / C#
    "directory.build.props",
    "directory.build.targets",
    "global.json",
    // Rust
    "cargo.toml",
    "cargo.lock",
    // Bun
    "bun.lockb",
    "bunfig.toml",
    // Deno
    "deno.json",
    "deno.jsonc",
    "deno.lock",
    // Elixir
    "mix.exs",
    "mix.lock",
    // Haskell
    "stack.yaml",
    "stack.yaml.lock",
    // Python (uv)
    "uv.lock",
    // .NET (additional)
    "nuget.config",
    "directory.packages.props",
    // Docker / container
    "dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
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
const PROTECTED_EXACT_PATHS: &[&str] = &["CODEOWNERS", "docs/CODEOWNERS"];

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

/// Internal params struct mirroring CreatePrResult fields for the tool_result! macro.
/// The actual MCP parameters come from CreatePrParams; this struct enables the macro's
/// TryFrom generation while CreatePrResult is constructed via CreatePrResult::new().
#[derive(Deserialize, JsonSchema)]
struct CreatePrResultFields {
    title: String,
    description: String,
    source_branch: String,
    patch_file: String,
    repository: String,
    #[serde(default)]
    agent_labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_commit: Option<String>,
    /// SHA-256 hex digest of the patch file, recorded at staging time.
    patch_sha256: String,
}

impl Validate for CreatePrResultFields {}

tool_result! {
    name = "create-pull-request",
    write = true,
    params = CreatePrResultFields,
    /// Result of creating a pull request - stored as safe output
    pub struct CreatePrResult {
        /// Title for the pull request
        title: String,
        /// Description/body of the pull request (markdown)
        description: String,
        /// Source branch name (generated or provided)
        source_branch: String,
        /// Path to the patch file in the safe outputs directory
        patch_file: String,
        /// Repository alias ("self" or alias from checkout list)
        repository: String,
        /// Agent-provided labels (validated against allowed-labels at execution time)
        #[serde(default)]
        agent_labels: Vec<String>,
        /// Base commit SHA recorded at patch generation time (merge-base of HEAD and
        /// the upstream branch). When present, Stage 3 uses this as the parent commit
        /// for the ADO Push API, ensuring the patch applies cleanly even if the target
        /// branch has advanced since the agent ran. Falls back to resolving the live
        /// target branch HEAD via the ADO refs API when absent (backward compatibility).
        ///
        /// Note: this is the merge-base, not the target branch HEAD. The PR diff in ADO
        /// compares file states and displays correctly regardless; however, the branch
        /// history shows a parent older than current main. This is normal for topic
        /// branches and is resolved when the PR is merged.
        #[serde(skip_serializing_if = "Option::is_none")]
        base_commit: Option<String>,
        /// SHA-256 hex digest of the patch file recorded at Stage 1.
        /// Stage 3 re-hashes the file and rejects mismatches — catches
        /// patch file tampering between stages.
        patch_sha256: String,
    }
}

impl SanitizeContent for CreatePrResult {
    fn sanitize_content_fields(&mut self) {
        self.title = sanitize_text(&self.title);
        self.description = sanitize_text(&self.description);
        for label in &mut self.agent_labels {
            *label = sanitize_text(label);
        }
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
        base_commit: Option<String>,
        patch_sha256: String,
    ) -> Self {
        Self {
            name: Self::NAME.to_string(),
            title,
            description,
            source_branch,
            patch_file,
            repository,
            agent_labels,
            base_commit,
            patch_sha256,
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
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct CreatePrConfig {
    /// Target branch to merge into (default: "main")
    #[serde(default = "default_target_branch", rename = "target-branch")]
    pub target_branch: String,

    /// Whether to create the PR as a draft (default: true)
    #[serde(default = "default_draft")]
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
    #[serde(default = "default_true", rename = "fallback-record-branch")]
    pub fallback_record_branch: bool,

    /// Whether to include agent execution stats in the PR description (default: true).
    #[serde(default = "default_true", rename = "include-stats")]
    pub include_stats: bool,
}

fn default_target_branch() -> String {
    "main".to_string()
}

fn default_draft() -> bool {
    true
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
            fallback_record_branch: true,
            include_stats: true,
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
    fn dry_run_summary(&self) -> String {
        format!("create PR: '{}' in repo '{}'", self.title, self.repository)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Creating PR: '{}' in repository '{}'",
            self.title, self.repository
        );
        debug!(
            "create-pull-request: title='{}', repo='{}', branch='{}', patch='{}'",
            self.title, self.repository, self.source_branch, self.patch_file
        );
        debug!("PR description length: {} chars", self.description.len());
        debug!("Source branch: {}", self.source_branch);
        debug!("Patch file: {}", self.patch_file);

        let config: CreatePrConfig = ctx.get_tool_config("create-pull-request");
        debug!("Target branch from config: {}", config.target_branch);
        debug!("Draft: {}", config.draft);
        debug!("Auto-complete: {}", config.auto_complete);
        debug!("Squash merge: {}", config.squash_merge);

        if config.draft && config.auto_complete {
            warn!(
                "auto-complete cannot be set on a draft PR; set draft: false to enable auto-complete"
            );
        }

        // Apply title prefix if configured
        let effective_title = if let Some(ref prefix) = config.title_prefix {
            format!("{}{}", prefix, self.title)
        } else {
            self.title.clone()
        };

        // ADO PR titles have a 400-character limit
        let title_char_count = effective_title.chars().count();
        if title_char_count > 400 {
            return Ok(ExecutionResult::failure(format!(
                "PR title too long after applying title-prefix ({} chars, max 400)",
                title_char_count
            )));
        }

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

        // SHA-256 integrity check: verify the patch file hasn't been tampered
        // with between Stage 1 and Stage 3.
        let live_hash =
            crate::hash::sha256_hex(patch_content.as_bytes());
        if live_hash != self.patch_sha256 {
            return Ok(ExecutionResult::failure(format!(
                "Patch file SHA-256 mismatch: expected {}, got {} — \
                 the file may have been tampered with between stages",
                self.patch_sha256, live_hash
            )));
        }
        debug!("Patch file SHA-256 verified: {}", live_hash);

        // Excluded files are handled via --exclude flags on git am / git apply,
        // which filters them at the git level rather than post-processing patch content.
        // This is the same approach used by gh-aw (via :(exclude) pathspecs).
        // Note: Exclusion happens during patch application (before the protection check).
        // If a protected file matches an excluded-files pattern, it is silently dropped
        // from the patch rather than triggering a protection error.
        let exclude_args: Vec<String> = config
            .excluded_files
            .iter()
            .map(|p| format!("--exclude={}", p))
            .collect();
        if !exclude_args.is_empty() {
            debug!(
                "Will apply {} excluded-files patterns via --exclude flags",
                exclude_args.len()
            );
        }

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

        // Extract file paths from patch for validation.
        // Filter out excluded files before the protection check — if a protected file
        // matches an excluded-files pattern, it will be excluded from the patch by
        // git am/apply --exclude and should not trigger a protection error.
        let patch_paths: Vec<String> = extract_paths_from_patch(&patch_content)
            .into_iter()
            .filter(|p| {
                !config.excluded_files.iter().any(|pat| glob_match_simple(pat, p))
            })
            .collect();

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
        let mut source_branch = self.source_branch.clone();
        let mut source_ref = format!("refs/heads/{}", source_branch);
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

        // Create and checkout a local branch in the worktree for patch application.
        // Note: this local branch name may differ from the final remote branch name
        // if a collision is detected later — the ADO push is REST-only, so the local
        // branch name is not used for the remote ref.
        debug!("Creating source branch: {}", source_branch);
        let checkout_output = Command::new("git")
            .args(["checkout", "-b", &source_branch])
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

        // Record the worktree HEAD before applying the patch so we can diff against
        // it later. For multi-commit patches, git am creates N commits and diff-tree HEAD
        // alone only shows the last commit's changes — we need base_sha..HEAD.
        let base_sha_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&worktree_path)
            .output()
            .await
            .context("Failed to get worktree HEAD SHA")?;
        let base_sha = String::from_utf8_lossy(&base_sha_output.stdout).trim().to_string();
        debug!("Worktree base SHA before patch: {}", base_sha);

        // Apply the patch. Strategy depends on whether excluded-files are configured:
        // - Without exclusions: prefer git am --3way (preserves commit metadata)
        //   with git apply --3way as fallback
        // - With exclusions: use git apply --3way directly (git am does not support
        //   --exclude flags; git apply does)
        let patch_committed = match apply_patch_to_worktree(&worktree_path, &patch_path, &exclude_args).await? {
            Ok(committed) => committed,
            Err(result) => return Ok(result),
        };

        // Collect changed files. The method depends on how the patch was applied:
        // - git am: changes are committed → use git diff-tree to compare base_sha..HEAD
        //   (covers all commits in multi-commit patches, not just the last one)
        // - git apply: changes are in working tree → use git status --porcelain
        debug!("Getting list of changed files");
        let (status_str, use_diff_tree) = if patch_committed {
            let diff_tree_output = Command::new("git")
                .args(["diff-tree", "-r", "--name-status", &base_sha, "HEAD"])
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

        // Resolve the base commit for the push.
        // Prefer the merge-base SHA recorded at patch generation time (Stage 1) so the
        // patch is applied against the exact commit it was created from.  Fall back to
        // querying the ADO refs API when the field is absent (backward compat with old
        // NDJSON entries).
        let base_commit: String = if let Some(ref recorded) = self.base_commit {
            // Validate SHA format before trusting Stage 1 data
            if recorded.len() != 40 || !recorded.chars().all(|c| c.is_ascii_hexdigit()) {
                anyhow::bail!(
                    "Invalid base_commit SHA from Stage 1 NDJSON: {:?}",
                    recorded
                );
            }
            info!(
                "Using recorded base_commit from Stage 1: {}",
                recorded
            );
            recorded.clone()
        } else {
            debug!("No recorded base_commit — resolving from ADO refs API");
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
            let resolved = refs_data["value"][0]["objectId"]
                .as_str()
                .context("Could not find target branch commit")?;
            resolved.to_string()
        };
        debug!("Base commit: {}", base_commit);

        info!(
            "Base commit for target branch '{}': {}",
            target_branch, base_commit
        );

        // Check if the source branch already exists (e.g. from a retry or previous run).
        // Retry with new random suffixes up to 3 times.
        for attempt in 0..3 {
            let check_ref_url = format!(
                "{}{}/_apis/git/repositories/{}/refs?filter=heads/{}&api-version=7.1",
                org_url, project, repo_id, source_branch
            );
            debug!("Checking if source branch exists (attempt {}): {}", attempt + 1, check_ref_url);

            let check_ref_response = client
                .get(&check_ref_url)
                .basic_auth("", Some(token))
                .send()
                .await
                .context("Failed to check source branch existence")?;

            if check_ref_response.status().is_success() {
                let check_data: serde_json::Value = check_ref_response.json().await?;
                let refs = check_data["value"].as_array();
                if refs.is_some_and(|r| !r.is_empty()) {
                    warn!(
                        "Branch '{}' already exists, generating new suffix (attempt {})",
                        source_branch, attempt + 1
                    );
                    use rand::RngExt;
                    let new_suffix: u32 = rand::rng().random();
                    let new_hex = format!("{:08x}", new_suffix);
                    source_branch = if let Some(pos) = source_branch.rfind('-') {
                        format!("{}-{}", &source_branch[..pos], new_hex)
                    } else {
                        format!("{}-{}", source_branch, new_hex)
                    };
                    source_ref = format!("refs/heads/{}", source_branch);
                    info!("Renamed source branch to '{}'", source_branch);
                    continue;
                }
            }
            break;
        }

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

            // Handle TOCTOU branch collision: if the push fails because the branch
            // was created between our check and the push, retry with a new suffix
            if status.as_u16() == 409
                || (status.as_u16() == 400 && body.contains("already exists"))
            {
                warn!(
                    "Push failed due to branch collision, retrying with new suffix: {}",
                    body
                );
                use rand::RngExt;
                let new_suffix: u32 = rand::rng().random();
                let new_hex = format!("{:08x}", new_suffix);
                source_branch = if let Some(pos) = source_branch.rfind('-') {
                    format!("{}-{}", &source_branch[..pos], new_hex)
                } else {
                    format!("{}-{}", source_branch, new_hex)
                };
                source_ref = format!("refs/heads/{}", source_branch);
                info!("Retrying push with branch '{}'", source_branch);

                let retry_body = serde_json::json!({
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

                let retry_response = client
                    .post(&push_url)
                    .basic_auth("", Some(token))
                    .json(&retry_body)
                    .send()
                    .await
                    .context("Failed to push changes (retry)")?;

                if !retry_response.status().is_success() {
                    let retry_status = retry_response.status();
                    let retry_body_text = retry_response.text().await.unwrap_or_default();
                    warn!("Retry push also failed: {} - {}", retry_status, retry_body_text);
                    return Ok(ExecutionResult::failure(format!(
                        "Failed to push changes after retry: {} - {}",
                        retry_status, retry_body_text
                    )));
                }
            } else {
                warn!("Failed to push changes: {} - {}", status, body);
                return Ok(ExecutionResult::failure(format!(
                    "Failed to push changes: {} - {}",
                    status, body
                )));
            }
        }
        debug!("Changes pushed successfully");

        // Append agent stats then provenance footer to description.
        // Footer goes last as the final unambiguous provenance marker.
        let description_with_stats = crate::agent_stats::append_stats_to_body(
            &self.description,
            ctx,
            config.include_stats,
        );
        let description_final = format!("{}{}", description_with_stats, generate_pr_footer());

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
            "description": description_final,
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
                    .filter(|l| {
                        !config
                            .allowed_labels
                            .iter()
                            .any(|al| al.eq_ignore_ascii_case(l))
                    })
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
            // Merge agent labels (case-insensitive dedup to match allowlist comparison)
            for label in &self.agent_labels {
                if !all_labels.iter().any(|l| l.eq_ignore_ascii_case(label)) {
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
            if config.fallback_record_branch {
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
                    source_branch, target_branch, self.repository,
                    status, sanitize_text(truncate_error_body(&body, 500)),
                    sanitize_text(&self.description),
                    source_branch, target_branch
                );
                return Ok(ExecutionResult::failure_with_data(
                    format!(
                        "Failed to create pull request: {} - {}. Branch '{}' was pushed — create the PR manually.",
                        status, sanitize_text(truncate_error_body(&body, 500)), source_branch,
                    ),
                    serde_json::json!({
                        "fallback": "branch-recorded",
                        "branch": source_branch,
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

        // Set completion options (delete source branch, squash merge, auto-complete)
        // and add reviewers.
        set_pr_completion_options(
            &client, &config, org_url, project, &repo_id, pr_id,
            pr_data["createdBy"]["id"].as_str(), token,
        ).await;
        add_reviewers_to_pr(
            &client, &config, org_url, project, &repo_id, pr_id, organization, token,
        ).await;

        info!(
            "PR #{} created successfully: {} -> {}{}",
            pr_id, source_branch, target_branch,
            if config.draft { " (draft)" } else { "" }
        );

        Ok(ExecutionResult::success_with_data(
            format!("Created pull request #{}: {}", pr_id, effective_title),
            serde_json::json!({
                "pull_request_id": pr_id,
                "url": pr_web_url,
                "source_branch": source_branch,
                "target_branch": target_branch,
                "draft": config.draft
            }),
        ))
    }
}

/// Check for unresolved conflict markers in the working tree.
///
/// Uses `git grep` to look for `<<<<<<<` or `>>>>>>>` markers (with trailing
/// space to avoid false positives from reStructuredText heading underlines).
/// Returns `Some(failure)` if conflicts are found, `None` if the tree is clean.
async fn check_for_conflict_markers(worktree_path: &std::path::Path) -> anyhow::Result<Option<ExecutionResult>> {
    let conflict_check = Command::new("git")
        .args(["grep", "-l", "-E", r"^(<<<<<<<\s|>>>>>>>\s)"])
        .current_dir(worktree_path)
        .output()
        .await
        .context("Failed to run git grep for conflict markers")?;

    if conflict_check.status.success() {
        let conflicting_files = String::from_utf8_lossy(&conflict_check.stdout).trim().to_string();
        let err_msg = format!(
            "Patch applied with unresolved conflict markers in: {}",
            conflicting_files
        );
        warn!("{}", err_msg);
        return Ok(Some(ExecutionResult::failure(err_msg)));
    }
    Ok(None)
}

/// Apply a patch to a git worktree using `git apply --3way` with `--exclude` flags.
///
/// Used when `excluded-files` are configured (git am does not support `--exclude`).
/// Returns `Ok(false)` on success (`false` = changes are staged, not committed).
async fn apply_patch_with_exclusions(
    worktree_path: &std::path::Path,
    patch_path: &std::path::Path,
    exclude_args: &[String],
) -> anyhow::Result<Result<bool, ExecutionResult>> {
    debug!("Using git apply --3way (excluded-files configured)");
    let mut apply_args: Vec<String> = vec!["apply".into(), "--3way".into()];
    apply_args.extend(exclude_args.iter().cloned());
    apply_args.push(patch_path.to_string_lossy().into_owned());

    let apply_output = Command::new("git")
        .args(&apply_args)
        .current_dir(worktree_path)
        .output()
        .await
        .context("Failed to run git apply --3way")?;

    if !apply_output.status.success() {
        let err_msg = format!(
            "Patch could not be applied (conflicts): {}",
            String::from_utf8_lossy(&apply_output.stderr)
        );
        warn!("{}", err_msg);
        return Ok(Err(ExecutionResult::failure(err_msg)));
    }
    debug!("Patch applied with git apply --3way");

    if let Some(conflict_result) = check_for_conflict_markers(worktree_path).await? {
        return Ok(Err(conflict_result));
    }
    Ok(Ok(false))
}

/// Apply a patch to a git worktree using `git am --3way`, with a `git apply` fallback.
///
/// Used when no `excluded-files` are configured. Prefers `git am` because it
/// preserves original commit metadata. Falls back to `git apply --3way` if `git am`
/// fails. Returns `Ok(true)` when `git am` committed the changes, `Ok(false)` when
/// `git apply` left them staged.
async fn apply_patch_without_exclusions(
    worktree_path: &std::path::Path,
    patch_path: &std::path::Path,
) -> anyhow::Result<Result<bool, ExecutionResult>> {
    // No exclusions — use git am --3way for proper commit metadata preservation
    debug!("Applying patch with git am --3way");
    let am_output = Command::new("git")
        .args(["am", "--3way", &patch_path.to_string_lossy()])
        .current_dir(worktree_path)
        .output()
        .await
        .context("Failed to run git am")?;

    if am_output.status.success() {
        debug!("Patch applied successfully with git am");
        return Ok(Ok(true));
    }

    let stderr = String::from_utf8_lossy(&am_output.stderr);
    debug!("git am --3way failed: {}", stderr);

    // Abort the failed am to leave worktree clean
    let _ = Command::new("git")
        .args(["am", "--abort"])
        .current_dir(worktree_path)
        .output()
        .await;

    // Fallback: try git apply --3way
    debug!("Falling back to git apply --3way");
    let apply_output = Command::new("git")
        .args(["apply", "--3way", &patch_path.to_string_lossy()])
        .current_dir(worktree_path)
        .output()
        .await
        .context("Failed to run git apply --3way")?;

    if !apply_output.status.success() {
        let err_msg = format!(
            "Patch could not be applied (conflicts): {}",
            String::from_utf8_lossy(&apply_output.stderr)
        );
        warn!("{}", err_msg);
        return Ok(Err(ExecutionResult::failure(err_msg)));
    }
    debug!("Patch applied with git apply --3way fallback");

    if let Some(conflict_result) = check_for_conflict_markers(worktree_path).await? {
        return Ok(Err(conflict_result));
    }
    Ok(Ok(false))
}

/// Apply a patch to a git worktree, choosing the right strategy automatically.
///
/// Delegates to [`apply_patch_with_exclusions`] when `exclude_args` is non-empty
/// (because `git am` doesn't support `--exclude`), otherwise delegates to
/// [`apply_patch_without_exclusions`] which prefers `git am` for commit metadata.
///
/// Returns `Ok(patch_committed)` on success (`true` = changes are committed via
/// `git am`, `false` = changes are staged via `git apply`).
/// Returns `Err(ExecutionResult)` on expected failures (conflicts, patch errors).
async fn apply_patch_to_worktree(
    worktree_path: &std::path::Path,
    patch_path: &std::path::Path,
    exclude_args: &[String],
) -> anyhow::Result<Result<bool, ExecutionResult>> {
    if !exclude_args.is_empty() {
        apply_patch_with_exclusions(worktree_path, patch_path, exclude_args).await
    } else {
        apply_patch_without_exclusions(worktree_path, patch_path).await
    }
}

/// Set PR completion options (delete-source-branch, squash-merge) and optionally
/// enable auto-complete. Logs a warning on failure but does not propagate the error
/// because these are best-effort settings that do not affect the PR's existence.
async fn set_pr_completion_options(
    client: &reqwest::Client,
    config: &CreatePrConfig,
    org_url: &str,
    project: &str,
    repo_id: &str,
    pr_id: i64,
    pr_created_by_id: Option<&str>,
    token: &str,
) {
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

    // Only set autoCompleteSetBy if auto_complete is enabled and PR is not a draft
    // (ADO silently ignores auto-complete on draft PRs, so skip the API call)
    if config.auto_complete && !config.draft {
        if let Some(creator_id) = pr_created_by_id {
            update_body["autoCompleteSetBy"] = serde_json::json!({ "id": creator_id });
        } else {
            warn!("auto_complete requested but creator ID is unavailable; skipping autoCompleteSetBy");
        }
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

/// Add configured reviewers to a pull request.
///
/// Resolves each reviewer's identity (email/display-name → ADO identity ID) and
/// issues a `PUT` for each one. Logs a warning if a reviewer cannot be resolved or
/// if the API call fails; does not abort the overall PR creation.
async fn add_reviewers_to_pr(
    client: &reqwest::Client,
    config: &CreatePrConfig,
    org_url: &str,
    project: &str,
    repo_id: &str,
    pr_id: i64,
    organization: &str,
    token: &str,
) {
    if config.reviewers.is_empty() {
        return;
    }
    debug!("Adding {} reviewers", config.reviewers.len());
    for reviewer in &config.reviewers {
        debug!("Adding reviewer: {}", reviewer);

        // Resolve reviewer identity (email/name -> ID)
        let reviewer_id = match resolve_reviewer_identity(client, organization, token, reviewer).await {
            Some(id) => id,
            None => {
                warn!("Could not resolve reviewer '{}' to an identity ID, skipping", reviewer);
                continue;
            }
        };

        let reviewer_url = format!(
            "{}{}/_apis/git/repositories/{}/pullrequests/{}/reviewers/{}?api-version=7.1",
            org_url, project, repo_id, pr_id, reviewer_id
        );
        let reviewer_body = serde_json::json!({ "vote": 0, "isRequired": false });

        match client
            .put(&reviewer_url)
            .basic_auth("", Some(token))
            .json(&reviewer_body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                debug!("Reviewer '{}' (ID: {}) added successfully", reviewer, reviewer_id);
            }
            Ok(resp) => {
                warn!(
                    "Failed to add reviewer '{}' (ID: {}): {}",
                    reviewer, reviewer_id, resp.status()
                );
            }
            Err(e) => {
                warn!("Failed to add reviewer '{}': {}", reviewer, e);
            }
        }
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
            // old_path (= file_path) is already validated above
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
        } else if status_code.starts_with('C') && parts.len() >= 3 {
            // Copied file: C100\tsrc_path\tdest_path
            let dest_path = parts[2];
            validate_single_path(dest_path)?;

            let dest_full_path = worktree_path.join(dest_path);
            if dest_full_path.is_file() {
                changes.push(read_file_change("add", dest_path, &dest_full_path).await?);
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
    let mut in_diff = false;
    for line in patch_content.lines() {
        // Only validate paths within diff blocks, not commit message bodies.
        // format-patch output includes commit messages before each diff section.
        if line.starts_with("diff --git") {
            in_diff = true;
            // Extract paths using strip_prefix for correct handling of spaces
            if let Some(rest) = line.strip_prefix("diff --git a/") {
                // The b/ path starts after the last " b/" — but for simple validation,
                // validate the a/ path (everything before " b/") and the b/ path
                if let Some((a_path, b_path)) = rest.rsplit_once(" b/") {
                    validate_single_path(a_path.trim_matches('"'))?;
                    validate_single_path(b_path.trim_matches('"'))?;
                }
            }
            continue;
        }
        // Reset on commit envelope boundaries
        if line.starts_with("From ") && in_diff {
            in_diff = false;
            continue;
        }
        if !in_diff {
            continue;
        }
        if let Some(rest) = line.strip_prefix("--- a/") {
            let path = rest.trim().trim_matches('"');
            validate_single_path(path)?;
        } else if let Some(rest) = line.strip_prefix("+++ b/") {
            let path = rest.trim().trim_matches('"');
            validate_single_path(path)?;
        } else if line.starts_with("--- /dev/null") || line.starts_with("+++ /dev/null") {
            // New or deleted files — no path to validate
        } else if line.starts_with("rename from ") || line.starts_with("rename to ")
            || line.starts_with("copy from ") || line.starts_with("copy to ")
        {
            let path = line.splitn(3, ' ').nth(2).unwrap_or("").trim_matches('"');
            validate_single_path(path)?;
        }
    }
    Ok(())
}

/// Truncate an error response body to avoid embedding large or sensitive content.
fn truncate_error_body(body: &str, max_len: usize) -> &str {
    match body.char_indices().nth(max_len) {
        Some((idx, _)) => &body[..idx],
        None => body,
    }
}

/// Simple glob matching for excluded-files patterns.
/// Supports `*` (match any sequence within a path segment) and leading `**/`
/// (match any directory prefix). Patterns without `/` are treated as basename
/// matches (e.g., `*.lock` matches `subdir/Cargo.lock`).
/// Match a file path against a glob pattern for excluded-files filtering.
/// Patterns without `/` are treated as basename matches (e.g., `*.lock` matches
/// `subdir/Cargo.lock`). Patterns with `**/` prefix match at any depth.
/// Uses the `glob-match` crate for correct glob semantics (`*` does not cross `/`).
fn glob_match_simple(pattern: &str, path: &str) -> bool {
    if !pattern.contains('/') {
        // Basename-only pattern: auto-prefix with **/ for any-depth matching
        let full_pattern = format!("**/{}", pattern);
        return glob_match::glob_match(&full_pattern, path);
    }
    glob_match::glob_match(pattern, path)
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
    let mut in_diff = false;
    for line in patch_content.lines() {
        // Only extract paths after the first diff --git header to avoid
        // false positives from commit messages that quote patch fragments
        if line.starts_with("diff --git") {
            in_diff = true;
            continue;
        }
        if !in_diff {
            continue;
        }
        // A new commit envelope resets — skip until next diff --git
        if line.starts_with("From ") {
            in_diff = false;
            continue;
        }
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

/// Count the number of distinct files changed in a patch.
/// Reuses `extract_paths_from_patch` which correctly handles quoted paths,
/// renames, copies, and multi-commit deduplication.
fn count_patch_files(patch_content: &str) -> usize {
    extract_paths_from_patch(patch_content).len()
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

        // Check path prefixes (git diffs always use forward slashes)
        for prefix in PROTECTED_PATH_PREFIXES {
            if lower_path.starts_with(prefix) {
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
        assert!(config.fallback_record_branch);
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
    fn test_validate_patch_paths_rename_with_spaces() {
        // "rename from some dir/../.git/config" must be rejected.
        // Previously split_whitespace().last() would only see "dir/../.git/config"
        // (or just "config"), missing the traversal in the full path.
        let patch = "diff --git a/old b/new\n\
                     rename from some dir/../.git/config\n\
                     rename to new name\n";
        let result = validate_patch_paths(patch);
        assert!(result.is_err(), "rename with spaces and traversal should be rejected");

        // Also verify copy paths with spaces are validated correctly
        let patch_copy = "diff --git a/old b/new\n\
                          copy from some dir/../.git/config\n\
                          copy to new name\n";
        let result_copy = validate_patch_paths(patch_copy);
        assert!(
            result_copy.is_err(),
            "copy with spaces and traversal should be rejected"
        );
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

    #[test]
    fn test_find_protected_files_mixed_case_manifests() {
        let paths = vec![
            "Cargo.toml".to_string(),
            "Cargo.lock".to_string(),
            "Pipfile".to_string(),
            "Gemfile".to_string(),
            "Gemfile.lock".to_string(),
            "Directory.Build.props".to_string(),
            "sub/Directory.Build.targets".to_string(),
        ];
        let protected = find_protected_files(&paths);
        assert_eq!(protected.len(), 7);
    }

    // ─── Glob matching ────────────────────────────────────────────────────

    #[test]
    fn test_glob_match_simple_basename_wildcard() {
        assert!(glob_match_simple("*.lock", "Cargo.lock"));
        assert!(glob_match_simple("*.lock", "subdir/Cargo.lock"));
        assert!(glob_match_simple("*.lock", "a/b/c/yarn.lock"));
        assert!(!glob_match_simple("*.lock", "Cargo.toml"));
    }

    #[test]
    fn test_glob_match_simple_basename_exact() {
        assert!(glob_match_simple("package.json", "package.json"));
        assert!(glob_match_simple("package.json", "subdir/package.json"));
        assert!(!glob_match_simple("package.json", "other.json"));
    }

    #[test]
    fn test_glob_match_simple_double_star_prefix() {
        assert!(glob_match_simple("**/Cargo.lock", "Cargo.lock"));
        assert!(glob_match_simple("**/Cargo.lock", "subdir/Cargo.lock"));
        assert!(glob_match_simple("**/Cargo.lock", "a/b/c/Cargo.lock"));
        assert!(!glob_match_simple("**/Cargo.lock", "Cargo.toml"));
    }

    #[test]
    fn test_glob_match_simple_double_star_wildcard() {
        assert!(glob_match_simple("**/*.json", "package.json"));
        assert!(glob_match_simple("**/*.json", "subdir/package.json"));
        assert!(!glob_match_simple("**/*.json", "package.lock"));
    }

    #[test]
    fn test_glob_match_simple_path_with_slash() {
        assert!(glob_match_simple("src/*.rs", "src/main.rs"));
        assert!(!glob_match_simple("src/*.rs", "tests/main.rs"));
        // * should NOT cross directory boundaries
        assert!(!glob_match_simple("src/*.rs", "src/nested/file.rs"));
    }

    #[test]
    fn test_glob_match_does_not_match_adjacent_protected() {
        // *.lock should not match package.json
        assert!(!glob_match_simple("*.lock", "package.json"));
        assert!(!glob_match_simple("*.lock", "go.mod"));
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

    #[test]
    fn test_default_config_draft_true_autocomplete_false() {
        let config = CreatePrConfig::default();
        assert!(config.draft, "draft should default to true");
        assert!(!config.auto_complete, "auto_complete should default to false");
    }

    #[test]
    fn test_config_deserialize_draft_false_autocomplete_true() {
        let yaml = r#"
            target-branch: main
            draft: false
            auto-complete: true
        "#;
        let config: CreatePrConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.draft);
        assert!(config.auto_complete);
    }

    // ─── truncate_error_body ────────────────────────────────────────────────

    #[test]
    fn test_truncate_error_body_shorter_than_max() {
        assert_eq!(truncate_error_body("hello", 100), "hello");
    }

    #[test]
    fn test_truncate_error_body_exact_max() {
        assert_eq!(truncate_error_body("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_error_body_longer_than_max() {
        assert_eq!(truncate_error_body("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_error_body_multibyte_boundary() {
        // "héllo" — é is 2 bytes; truncation must not split a multi-byte char.
        // max_len is char count: 3 chars = "hél".
        let s = "héllo";
        let result = truncate_error_body(s, 3);
        assert_eq!(result, "hél");
    }

    #[test]
    fn test_truncate_error_body_empty_input() {
        assert_eq!(truncate_error_body("", 5), "");
    }
}
