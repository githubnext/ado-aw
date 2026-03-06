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
    ) -> Self {
        Self {
            name: Self::NAME.to_string(),
            title,
            description,
            source_branch,
            patch_file,
            repository,
        }
    }
}

/// Configuration for the create_pr tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   create_pr:
///     auto_complete: true
///     delete_source_branch: true
///     squash_merge: true
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

    /// Whether to set auto-complete on the PR (default: false)
    #[serde(default, rename = "auto-complete")]
    pub auto_complete: bool,

    /// Whether to delete source branch after merge (default: true)
    #[serde(default = "default_true", rename = "delete-source-branch")]
    pub delete_source_branch: bool,

    /// Whether to squash commits on merge (default: true)
    #[serde(default = "default_true", rename = "squash-merge")]
    pub squash_merge: bool,

    /// Reviewers to add to the PR (email addresses or user IDs)
    #[serde(default)]
    pub reviewers: Vec<String>,

    /// Labels to add to the PR
    #[serde(default)]
    pub labels: Vec<String>,

    /// Work item IDs to link to the PR
    #[serde(default, rename = "work-items")]
    pub work_items: Vec<i32>,
}

fn default_target_branch() -> String {
    "main".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for CreatePrConfig {
    fn default() -> Self {
        Self {
            target_branch: default_target_branch(),
            auto_complete: false,
            delete_source_branch: true,
            squash_merge: true,
            reviewers: Vec::new(),
            labels: Vec::new(),
            work_items: Vec::new(),
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
        debug!("Auto-complete: {}", config.auto_complete);
        debug!("Squash merge: {}", config.squash_merge);

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

        // Security: Validate patch with git apply --check first (dry run)
        debug!("Running git apply --check (dry run)");
        let check_output = Command::new("git")
            .args(["apply", "--check", &patch_path.to_string_lossy()])
            .current_dir(&worktree_path)
            .output()
            .await
            .context("Failed to run git apply --check")?;

        if !check_output.status.success() {
            warn!(
                "Patch dry-run failed: {}",
                String::from_utf8_lossy(&check_output.stderr)
            );
            return Ok(ExecutionResult::failure(format!(
                "Patch validation failed (git apply --check): {}",
                String::from_utf8_lossy(&check_output.stderr)
            )));
        }
        debug!("Patch dry-run passed");

        // Apply the patch
        debug!("Applying patch");
        let apply_output = Command::new("git")
            .args(["apply", &patch_path.to_string_lossy()])
            .current_dir(&worktree_path)
            .output()
            .await
            .context("Failed to run git apply")?;

        if !apply_output.status.success() {
            warn!(
                "Failed to apply patch: {}",
                String::from_utf8_lossy(&apply_output.stderr)
            );
            return Ok(ExecutionResult::failure(format!(
                "Failed to apply patch: {}",
                String::from_utf8_lossy(&apply_output.stderr)
            )));
        }
        debug!("Patch applied successfully");

        // Get list of changed files using git status
        debug!("Getting list of changed files");
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

        // Parse changed files and build ADO push payload
        let status_str = String::from_utf8_lossy(&status_output.stdout);
        debug!("Git status output:\n{}", status_str);
        let changes = collect_changes_from_worktree(&worktree_path, &status_str).await?;
        debug!("Collected {} file changes for push", changes.len());

        if changes.is_empty() {
            warn!("No changes detected after applying patch");
            return Ok(ExecutionResult::failure(
                "No changes detected after applying patch".to_string(),
            ));
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
                "comment": self.title,
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
            "title": self.title,
            "description": self.description,
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

        // Add labels if configured
        if !config.labels.is_empty() {
            debug!("Adding {} labels", config.labels.len());
            pr_body["labels"] = serde_json::json!(
                config
                    .labels
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
            "PR #{} created successfully: {} -> {}",
            pr_id, self.source_branch, target_branch
        );

        Ok(ExecutionResult::success_with_data(
            format!("Created pull request #{}: {}", pr_id, self.title),
            serde_json::json!({
                "pull_request_id": pr_id,
                "url": pr_web_url,
                "source_branch": self.source_branch,
                "target_branch": target_branch
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
                    let content = tokio::fs::read_to_string(&full_path)
                        .await
                        .with_context(|| format!("Failed to read new file: {}", file_path))?;

                    changes.push(serde_json::json!({
                        "changeType": "add",
                        "item": {
                            "path": format!("/{}", file_path)
                        },
                        "newContent": {
                            "content": content,
                            "contentType": "rawtext"
                        }
                    }));
                }
            }
            // Modified files
            " M" | "M " | "MM" => {
                if full_path.is_file() {
                    let content = tokio::fs::read_to_string(&full_path)
                        .await
                        .with_context(|| format!("Failed to read modified file: {}", file_path))?;

                    changes.push(serde_json::json!({
                        "changeType": "edit",
                        "item": {
                            "path": format!("/{}", file_path)
                        },
                        "newContent": {
                            "content": content,
                            "contentType": "rawtext"
                        }
                    }));
                }
            }
            // Renamed files - format is "R  old_path -> new_path"
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
                }
            }
            // Other statuses - try to handle as edit if file exists
            _ => {
                if full_path.is_file() {
                    let content = tokio::fs::read_to_string(&full_path)
                        .await
                        .with_context(|| format!("Failed to read file: {}", file_path))?;

                    changes.push(serde_json::json!({
                        "changeType": "edit",
                        "item": {
                            "path": format!("/{}", file_path)
                        },
                        "newContent": {
                            "content": content,
                            "contentType": "rawtext"
                        }
                    }));
                }
            }
        }
    }

    Ok(changes)
}

/// Validate that patch file paths don't contain dangerous patterns
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_params_valid() {
        let params = CreatePrParams {
            title: "Fix bug in parser".to_string(),
            description: "This PR fixes a critical bug in the parser module.".to_string(),
            repository: None,
        };
        assert!(params.validate().is_ok());
    }

    #[test]
    fn test_validate_params_short_title() {
        let params = CreatePrParams {
            title: "Fix".to_string(),
            description: "This PR fixes a critical bug in the parser module.".to_string(),
            repository: None,
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_validate_params_short_description() {
        let params = CreatePrParams {
            title: "Fix bug in parser".to_string(),
            description: "Fix bug".to_string(),
            repository: None,
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_config_default_target_branch() {
        let config = CreatePrConfig::default();
        assert_eq!(config.target_branch, "main");
        assert!(!config.auto_complete);
        assert!(config.delete_source_branch);
        assert!(config.squash_merge);
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
}
