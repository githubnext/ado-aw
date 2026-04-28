# Safe Outputs Configuration & Tool Reference

_Part of the [ado-aw documentation](../AGENTS.md)._

## Safe Outputs Configuration

The front matter supports a `safe-outputs:` field for configuring specific tool behaviors:

```yaml
safe-outputs:
  create-work-item:
    work-item-type: Task
    assignee: "user@example.com"
    tags:
      - automated
      - agent-created
  create-pull-request:
    target-branch: main
    auto-complete: true
    delete-source-branch: true
    squash-merge: true
    reviewers:
      - "user@example.com"
    labels:
      - automated
      - agent-created
    work-items:
      - 12345
```

Safe output configurations are passed to Stage 3 execution and used when processing safe outputs.

## Available Safe Output Tools

### comment-on-work-item
Adds a comment to an existing Azure DevOps work item. This is the ADO equivalent of gh-aw's `add-comment` tool.

**Agent parameters:**
- `work_item_id` - The work item ID to comment on (required, must be positive)
- `body` - Comment text in markdown format (required, must be at least 10 characters)

**Configuration options (front matter):**
- `max` - Maximum number of comments per run (default: 1)
- `include-stats` - Whether to append agent execution stats to the comment body (default: true)
- `target` - **Required** — scoping policy for which work items can be commented on:
  - `"*"` - Any work item in the project (unrestricted, must be explicit)
  - `12345` - A specific work item ID
  - `[12345, 67890]` - A list of allowed work item IDs
  - `"Some\\Path"` - Work items under the specified area path prefix (any string that isn't `"*"`, validated via ADO API at Stage 3)

**Example configuration:**
```yaml
safe-outputs:
  comment-on-work-item:
    max: 3
    target: "4x4\\QED"
```

**Note:** The `target` field is required. If omitted, compilation fails with an error. This ensures operators are intentional about which work items agents can comment on.

### create-work-item
Creates an Azure DevOps work item.

**Agent parameters:**
- `title` - A concise title for the work item (required, must be more than 5 characters)
- `description` - Work item description in markdown format (required, must be more than 30 characters)

**Configuration options (front matter):**
- `work-item-type` - Work item type (default: "Task")
- `area-path` - Area path for the work item
- `iteration-path` - Iteration path for the work item
- `assignee` - User to assign (email or display name)
- `tags` - List of tags to apply
- `custom-fields` - Map of custom field reference names to values (e.g., `Custom.MyField: "value"`)
- `max` - Maximum number of create-work-item outputs allowed per run (default: 1)
- `include-stats` - Whether to append agent execution stats to the work item description (default: true)
- `artifact-link` - Configuration for GitHub Copilot artifact linking:
  - `enabled` - Whether to add an artifact link (default: false)
  - `repository` - Repository name override (defaults to BUILD_REPOSITORY_NAME)
  - `branch` - Branch name to link to (default: "main")

### update-work-item
Updates an existing Azure DevOps work item. Each field that can be modified requires explicit opt-in via configuration to prevent unintended updates.

**Agent parameters:**
- `id` - Work item ID to update (required, must be a positive integer)
- `title` - New title for the work item (optional, requires `title: true` in config)
- `body` - New description in markdown format (optional, requires `body: true` in config)
- `state` - New state (e.g., `"Active"`, `"Resolved"`, `"Closed"`; optional, requires `status: true` in config)
- `area_path` - New area path (optional, requires `area-path: true` in config)
- `iteration_path` - New iteration path (optional, requires `iteration-path: true` in config)
- `assignee` - New assignee email or display name (optional, requires `assignee: true` in config)
- `tags` - New tags, replaces all existing tags (optional, requires `tags: true` in config)

At least one field must be provided for update.

**Configuration options (front matter):**
```yaml
safe-outputs:
  update-work-item:
    status: true              # enable state/status updates via `state` parameter (default: false)
    title: true               # enable title updates (default: false)
    body: true                # enable body/description updates (default: false)
    markdown-body: true       # store body as markdown in ADO (default: false; requires ADO Services or Server 2022+)
    title-prefix: "[bot] "    # only update work items whose title starts with this prefix
    tag-prefix: "agent-"      # only update work items that have at least one tag starting with this prefix
    max: 3                    # maximum number of update-work-item outputs allowed per run (default: 1)
    target: "*"               # "*" (default) allows any work item ID, or set to a specific work item ID number
    area-path: true           # enable area path updates (default: false)
    iteration-path: true      # enable iteration path updates (default: false)
    assignee: true            # enable assignee updates (default: false)
    tags: true                # enable tag updates (default: false)
```

**Security note:** Every field that can be modified requires explicit opt-in (`true`) in the front matter configuration. If the `max` limit is exceeded, additional entries are skipped rather than aborting the entire batch.

### create-pull-request
Creates a pull request with code changes made by the agent. When invoked:
1. Generates a patch file from `git diff` capturing all changes in the specified repository
2. Saves the patch to the safe outputs directory
3. Creates a JSON record with PR metadata (title, description, source branch, repository)

During Stage 3 execution, the repository is validated against the allowed list (from `checkout:` + "self"), then the patch is applied and a PR is created in Azure DevOps.

**Stage 3 Execution Architecture (Hybrid Git + ADO API):**

```
┌─────────────────────────────────────────────────────────────────┐
│                        Stage 3 Execution                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. Security Validation                                         │
│     ├── Patch file size limit (5 MB)                           │
│     └── Path validation (no .., .git, absolute paths)          │
│                                                                 │
│  2. Git Worktree (local operations only)                       │
│     ├── Create worktree at target branch                       │
│     ├── git apply --check (dry run)                            │
│     ├── git apply (apply patch correctly)                      │
│     └── git status --porcelain (detect changes)                │
│                                                                 │
│  3. ADO REST API (authenticated, no git config needed)         │
│     ├── Read full file contents from worktree                  │
│     ├── POST /pushes (create branch + commit)                  │
│     ├── POST /pullrequests (create PR)                         │
│     ├── PATCH (set auto-complete if configured)                │
│     └── PUT (add reviewers)                                    │
│                                                                 │
│  4. Cleanup                                                     │
│     └── WorktreeGuard removes worktree on drop                 │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

This hybrid approach combines:
- **Git worktree + apply**: Correct patch application using git's battle-tested diff parser
- **ADO REST API**: No git config (user.email/name) needed, authentication handled via token

**Agent parameters:**
- `title` - PR title (required, 5-200 characters)
- `description` - PR description in markdown (required, 10+ characters)
- `repository` - Repository to create PR in: "self" for pipeline repo, or alias from `checkout:` list (default: "self")

Note: The source branch name is auto-generated from a sanitized version of the PR title plus a unique suffix (e.g., `agent/fix-bug-in-parser-a1b2c3`). This format is human-readable while preventing injection attacks.

**Configuration options (front matter):**
- `target-branch` - Target branch to merge into (default: "main")
- `auto-complete` - Set auto-complete on the PR (default: false)
- `delete-source-branch` - Delete source branch after merge (default: true)
- `squash-merge` - Squash commits on merge (default: true)
- `reviewers` - List of reviewer emails to add
- `labels` - List of labels to apply
- `work-items` - List of work item IDs to link
- `max` - Maximum number of create-pull-request outputs allowed per run (default: 1)
- `include-stats` - Whether to append agent execution stats (token usage, duration, model) to the PR description (default: true)

**Multi-repository support:**
When `workspace: root` and multiple repositories are checked out, agents can create PRs for any allowed repository:
```json
{"title": "Fix in main repo", "description": "...", "repository": "self"}
{"title": "Fix in other repo", "description": "...", "repository": "other-repo"}
```
The `repository` value must be "self" or an alias from the `checkout:` list in the front matter.

### noop
Reports that no action was needed. Use this to provide visibility when analysis is complete but no changes or outputs are required.

**Agent parameters:**
- `context` - Optional context about why no action was taken

### missing-data
Reports that data or information needed to complete the task is not available.

**Agent parameters:**
- `data_type` - Type of data needed (e.g., 'API documentation', 'database schema')
- `reason` - Why this data is required
- `context` - Optional additional context about the missing information

### missing-tool
Reports that a tool or capability needed to complete the task is not available.

**Agent parameters:**
- `tool_name` - Name of the tool that was expected but not found
- `context` - Optional context about why the tool was needed

### report-incomplete
Reports that a task could not be completed.

**Agent parameters:**
- `reason` - Why the task could not be completed (required, at least 10 characters)
- `context` - Optional additional context about what was attempted

### add-pr-comment
Adds a new comment thread to a pull request.

**Agent parameters:**
- `pull_request_id` - The PR ID to comment on (required, must be positive)
- `content` - Comment text in markdown format (required, at least 10 characters)
- `repository` - Repository alias (default: "self")
- `file_path` *(optional)* - File path for an inline comment anchored to a specific file

**Configuration options (front matter):**
```yaml
safe-outputs:
  add-pr-comment:
    comment-prefix: "[Agent Review] "  # Optional — prepended to all comments
    allowed-repositories: []           # Optional — restrict which repos can be commented on
    max: 1                             # Maximum per run (default: 1)
    include-stats: true                # Append agent stats to comment (default: true)
```

### reply-to-pr-comment
Replies to an existing review comment thread on a pull request.

**Agent parameters:**
- `pull_request_id` - The PR ID containing the thread (required)
- `thread_id` - The thread ID to reply to (required)
- `content` - Reply text in markdown format (required, at least 10 characters)
- `repository` - Repository alias (default: "self")

**Configuration options (front matter):**
```yaml
safe-outputs:
  reply-to-pr-comment:
    comment-prefix: "[Agent] "     # Optional — prepended to all replies
    allowed-repositories: []       # Optional — restrict which repos can be replied on
    max: 1                         # Maximum per run (default: 1)
```

### resolve-pr-thread
Resolves or updates the status of a pull request review thread.

**Agent parameters:**
- `pull_request_id` - The PR ID containing the thread (required)
- `thread_id` - The thread ID to resolve (required)
- `status` - Target status: `fixed`, `wont-fix`, `closed`, `by-design`, or `active` (to reactivate)
- `repository` - Repository alias (default: "self")

**Configuration options (front matter):**
```yaml
safe-outputs:
  resolve-pr-thread:
    allowed-repositories: []     # Optional — restrict which repos can be operated on
    allowed-statuses: []         # REQUIRED — empty list rejects all status transitions
    max: 1                       # Maximum per run (default: 1)
```

### submit-pr-review
Submits a review vote on a pull request.

**Agent parameters:**
- `pull_request_id` - The PR ID to review (required)
- `event` - Review decision: `approve`, `approve-with-suggestions`, `request-changes`, or `comment` (required)
- `body` *(optional)* - Review rationale in markdown (required for `request-changes`, at least 10 characters)
- `repository` - Repository alias (default: "self")

**Configuration options (front matter):**
```yaml
safe-outputs:
  submit-pr-review:
    allowed-events: []           # REQUIRED — empty list rejects all events
    allowed-repositories: []     # Optional — restrict which repos can be reviewed
    max: 1                       # Maximum per run (default: 1)
```

### update-pr
Updates pull request metadata (reviewers, labels, auto-complete, vote, description).

**Agent parameters:**
- `pull_request_id` - The PR ID to update (required)
- `operation` - Update operation: `add-reviewers`, `add-labels`, `set-auto-complete`, `vote`, or `update-description` (required)
- `reviewers` - Reviewer emails (required for `add-reviewers`)
- `labels` - Label names (required for `add-labels`)
- `vote` - Vote value: `approve`, `approve-with-suggestions`, `wait-for-author`, `reject`, or `reset` (required for `vote`)
- `description` - New PR description in markdown (required for `update-description`, at least 10 characters)
- `repository` - Repository alias (default: "self")

**Configuration options (front matter):**
```yaml
safe-outputs:
  update-pr:
    allowed-operations: []          # Optional — restrict which operations are permitted (empty = all)
    allowed-repositories: []        # Optional — restrict which repos can be updated
    allowed-votes: []               # REQUIRED for vote operation — empty rejects all votes
    delete-source-branch: true      # For set-auto-complete (default: true)
    merge-strategy: "squash"        # For set-auto-complete: squash, noFastForward, rebase, rebaseMerge
    max: 1                          # Maximum per run (default: 1)
```

### link-work-items
Links two Azure DevOps work items together.

**Agent parameters:**
- `source_id` - Source work item ID (required, must be positive)
- `target_id` - Target work item ID (required, must differ from source)
- `link_type` - Relationship type: `parent`, `child`, `related`, `predecessor`, `successor`, `duplicate`, `duplicate-of` (required)
- `comment` *(optional)* - Description of the relationship

**Configuration options (front matter):**
```yaml
safe-outputs:
  link-work-items:
    allowed-link-types: []       # Optional — restrict which link types are allowed (empty = all)
    target: "*"                  # Scoping policy (same as comment-on-work-item target)
    max: 5                       # Maximum per run (default: 5)
```

### queue-build
Queues an Azure DevOps pipeline build by definition ID.

**Agent parameters:**
- `pipeline_id` - Pipeline definition ID to trigger (required, must be positive)
- `branch` *(optional)* - Branch to build (defaults to configured default or "main")
- `parameters` *(optional)* - Template parameter key-value pairs
- `reason` *(optional)* - Human-readable reason for triggering the build (at least 5 characters)

**Configuration options (front matter):**
```yaml
safe-outputs:
  queue-build:
    allowed-pipelines: []        # REQUIRED — pipeline definition IDs that can be triggered (empty rejects all)
    allowed-branches: []         # Optional — branches allowed to be built (empty = any)
    allowed-parameters: []       # Optional — parameter keys allowed to be passed (empty = any)
    default-branch: "main"       # Optional — default branch when agent doesn't specify one
    max: 3                       # Maximum per run (default: 3)
```

### create-git-tag
Creates a git tag on a repository ref.

**Agent parameters:**
- `tag_name` - Tag name (e.g., `v1.2.3`; 3-100 characters, alphanumeric plus `.`, `-`, `_`, `/`)
- `commit` *(optional)* - Commit SHA to tag (40-character hex; defaults to HEAD of default branch)
- `message` *(optional)* - Tag annotation message (at least 5 characters; creates annotated tag)
- `repository` - Repository alias (default: "self")

**Configuration options (front matter):**
```yaml
safe-outputs:
  create-git-tag:
    tag-pattern: "^v\\d+\\.\\d+\\.\\d+$"  # Optional — regex pattern tag names must match
    allowed-repositories: []                # Optional — restrict which repos can be tagged
    message-prefix: "[Release] "            # Optional — prefix prepended to tag message
    max: 1                                  # Maximum per run (default: 1)
```

### add-build-tag
Adds a tag to an Azure DevOps build.

**Agent parameters:**
- `build_id` - Build ID to tag (required, must be positive)
- `tag` - Tag value (1-100 characters, alphanumeric and dashes only)

**Configuration options (front matter):**
```yaml
safe-outputs:
  add-build-tag:
    allowed-tags: []             # Optional — restrict which tags can be applied (supports prefix wildcards)
    tag-prefix: "agent-"         # Optional — prefix prepended to all tags
    allow-any-build: false       # When false, only the current pipeline build can be tagged (default: false)
    max: 1                       # Maximum per run (default: 1)
```

### create-branch
Creates a new branch from an existing ref.

**Agent parameters:**
- `branch_name` - Branch name to create (1-200 characters)
- `source_branch` *(optional)* - Branch to create from (default: "main")
- `source_commit` *(optional)* - Specific commit SHA to branch from (overrides source_branch; 40-character hex)
- `repository` - Repository alias (default: "self")

**Configuration options (front matter):**
```yaml
safe-outputs:
  create-branch:
    branch-pattern: "^agent/.*$"       # Optional — regex pattern branch names must match
    allowed-repositories: []           # Optional — restrict which repos can have branches created
    allowed-source-branches: []        # Optional — restrict which source branches can be branched from
    max: 1                             # Maximum per run (default: 1)
```

### upload-attachment
Uploads a workspace file as an attachment to an Azure DevOps work item.

**Agent parameters:**
- `work_item_id` - Work item ID to attach the file to (required, must be positive)
- `file_path` - Relative path to the file in the workspace (no directory traversal)
- `comment` *(optional)* - Description of the attachment (at least 3 characters)

**Configuration options (front matter):**
```yaml
safe-outputs:
  upload-attachment:
    max-file-size: 5242880       # Maximum file size in bytes (default: 5 MB)
    allowed-extensions: []       # Optional — restrict file types (e.g., [".png", ".pdf"])
    comment-prefix: "[Agent] "   # Optional — prefix prepended to the comment
    max: 1                       # Maximum per run (default: 1)
```

### cache-memory (moved to `tools:`)
Memory is now configured as a first-class tool under `tools: cache-memory:` instead of `safe-outputs: memory:`. See the [Cache Memory section](./tools.md#cache-memory-cache-memory) in `docs/tools.md` for details.

### create-wiki-page
Creates a new Azure DevOps wiki page. The page must **not** already exist; the tool enforces an atomic create-only operation (via `If-Match: ""`). Attempting to create a page that already exists results in an explicit failure.

**Agent parameters:**
- `path` - Wiki page path to create (e.g. `/Overview/NewPage`). Must not be empty and must not contain `..`.
- `content` - Markdown content for the wiki page (at least 10 characters).
- `comment` *(optional)* - Commit comment describing the change. Defaults to the value configured in the front matter, or `"Created by agent"` if not set.

**Configuration options (front matter):**
```yaml
safe-outputs:
  create-wiki-page:
    wiki-name: "MyProject.wiki"     # Required — wiki identifier (name or GUID)
    wiki-project: "OtherProject"    # Optional — ADO project that owns the wiki; defaults to current pipeline project
    branch: "main"                  # Optional — git branch override; auto-detected for code wikis (see note below)
    path-prefix: "/agent-output"    # Optional — prepended to the agent-supplied path (restricts write scope)
    title-prefix: "[Agent] "        # Optional — prepended to the last path segment (the page title)
    comment: "Created by agent"     # Optional — default commit comment when agent omits one
    max: 1                          # Maximum number of create-wiki-page outputs allowed per run (default: 1)
    include-stats: true             # Append agent stats to wiki page content (default: true)
```

Note: `wiki-name` is required. If it is not set, execution fails with an explicit error message.

**Code wikis vs project wikis:** The executor automatically detects code wikis (type 1) and resolves the published branch from the wiki metadata. You only need to set `branch` explicitly to override the auto-detected value (e.g. targeting a non-default branch). Project wikis (type 0) need no branch configuration.

### update-wiki-page
Updates the content of an existing Azure DevOps wiki page. The wiki page must already exist; this tool edits its content but does not create new pages.

**Agent parameters:**
- `path` - Wiki page path to update (e.g. `/Overview/Architecture`). Must not be empty and must not contain `..`.
- `content` - Markdown content for the wiki page (at least 10 characters).
- `comment` *(optional)* - Commit comment describing the change. Defaults to the value configured in the front matter, or `"Updated by agent"` if not set.

**Configuration options (front matter):**
```yaml
safe-outputs:
  update-wiki-page:
    wiki-name: "MyProject.wiki"     # Required — wiki identifier (name or GUID)
    wiki-project: "OtherProject"    # Optional — ADO project that owns the wiki; defaults to current pipeline project
    branch: "main"                  # Optional — git branch override; auto-detected for code wikis (see note below)
    path-prefix: "/agent-output"    # Optional — prepended to the agent-supplied path (restricts write scope)
    title-prefix: "[Agent] "        # Optional — prepended to the last path segment (the page title)
    comment: "Updated by agent"     # Optional — default commit comment when agent omits one
    max: 1                          # Maximum number of update-wiki-page outputs allowed per run (default: 1)
    include-stats: true             # Append agent stats to wiki page content (default: true)
```

Note: `wiki-name` is required. If it is not set, execution fails with an explicit error message.

**Code wikis vs project wikis:** The executor automatically detects code wikis (type 1) and resolves the published branch from the wiki metadata. You only need to set `branch` explicitly to override the auto-detected value (e.g. targeting a non-default branch). Project wikis (type 0) need no branch configuration.

