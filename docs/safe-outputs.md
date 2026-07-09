# Safe Outputs Configuration & Tool Reference

_Part of the [ado-aw documentation](../AGENTS.md)._

> ℹ️ The debug-only `create-issue` tool (used by dogfood pipelines to file
> failure reports back to GitHub) is **not** a safe output and is not
> configurable here. It is gated by a separate `ado-aw-debug:` front-matter
> section and stripped from the SafeOutputs MCP server unless explicitly
> enabled. See [`docs/ado-aw-debug.md`](ado-aw-debug.md).

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
    draft: false             # default is true; set false to publish immediately (required for auto-complete)
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

### Manual review (`require-approval`)

High-impact safe outputs can be gated behind a human approval step
(`ManualValidation@1`) that pauses the run until a reviewer approves or rejects
in the Azure DevOps UI. This lets agents propose more consequential actions
(PRs, branches, queued builds, work items) safely.

Set `require-approval` at the **section level** for a pipeline-wide default,
and/or inside an **individual tool** to override the default for that tool:

```yaml
safe-outputs:
  require-approval: true          # global default: every output below needs review
  create-pull-request:
    target-branch: main
  add-pr-comment:
    require-approval: false       # …except low-impact comments, which auto-apply
```

`require-approval` accepts either a bare boolean or an object for finer control:

```yaml
safe-outputs:
  create-pull-request:
    require-approval:
      approvers: ["[MyOrg]\\release-team"]   # who may approve (empty → anyone with run permission)
      notify-users: ["ops@example.com"]      # who is emailed (empty → no email)
      timeout-minutes: 120                    # pending period before on-timeout fires (omit → pipeline default)
      on-timeout: reject                      # reject (default, fail-closed) | resume
      instructions: "Verify the proposed PR before approving."
```

Resolution per tool: the tool's own `require-approval` wins; otherwise the
section-level `require-approval` applies; otherwise the tool is **not** gated.

**Defaults (bare `require-approval: true`)** — the run pauses on a Review panel;
**anyone with run permission** can approve or reject; **no** notification emails
are sent; and the validation **fails closed** on timeout (`on-timeout: reject`),
so un-approved outputs are never applied.

**Timeout (`timeout-minutes` / `on-timeout`)** — `timeout-minutes` bounds the
`ManualValidation@1` task's pending period; when it elapses the task applies
`on-timeout` (`reject` by default, or `resume` to auto-approve). The agentless
`ManualReview` job carries a slightly larger outer timeout as a hard bound, so a
job-level cancellation never preempts the task's graceful `on-timeout` handling
(in particular, `on-timeout: resume` reliably auto-approves rather than being
cancelled). Omit `timeout-minutes` to inherit the pipeline default.

**Reviewer message** — set `instructions` to control the text shown in the
Review panel and notification emails. It is plain text and supports pipeline
variable (`$(...)`) interpolation. When omitted, ado-aw generates a default
message listing the reviewed safe-output type(s) awaiting approval. A run uses a
**single** `ManualReview` gate covering every reviewed tool: the gate message
**lists every reviewed tool** and aggregates **all** author-supplied per-tool
`instructions` (grouped when identical), so no tool's note is dropped when
several are gated. A single reviewed tool with its own `instructions` shows that
message verbatim; set `instructions` on the section-level `require-approval` to
apply one note to every tool.

**Execution shape** — manual review changes the compiled pipeline:

- A new agentless `ManualReview` job (`pool: server`) runs `ManualValidation@1`
  between Detection and the safe-output execution.
- It only pauses when Detection cleared the run (no prompt-injection / secret
  leak) **and** the agent actually proposed a reviewed-type output (a Detection
  step sets a `HasReviewedProposals` flag) — so the run never pauses for
  nothing.
- When some tools are gated and others are not, execution **splits**: an
  automatic `SafeOutputs` job applies the non-gated outputs immediately
  (independent of the review outcome), while a separate `SafeOutputs_Reviewed`
  job — gated behind `ManualReview` — applies the approved outputs and publishes
  a distinct `safe_outputs_reviewed` artifact. A rejected or timed-out review
  fails closed: the reviewed job is skipped while the automatic outputs are
  unaffected.
- When **every** configured tool requires approval (no automatic tools),
  execution is **not** split — the single `SafeOutputs` job is gated behind
  `ManualReview` in its entirety. Note this also defers the always-enabled
  diagnostic outputs (`noop`, `report-incomplete`, `missing-tool`,
  `missing-data`) until after approval, since they share that one job. If you
  want diagnostics to apply without waiting on a human, leave at least one
  low-impact tool (e.g. `add-pr-comment`) non-gated so the automatic split job
  is created.

The Detection threat gate always runs first, so a flagged run applies nothing —
automatic or reviewed.

### Safe-outputs summary tab

Every run that proposes safe outputs publishes a human-readable **build summary
tab** titled **`ado-aw-safe-outputs`**, listing what the agent proposed. This is
always on — it does **not** require `require-approval` — so non-elevated runs get
the same transparency, and it is the panel a reviewer reads before approving a
gated run.

- The summary is rendered at the **end of the Agent job** (the job that produced
  the proposals) by the `approval-summary` ado-script bundle, and attached via
  `##vso[task.uploadsummary]`. It is **not** produced by the Detection
  (threat-analysis) stage, whose only job is inspecting proposals for threats.
- Each proposal is shown with per-tool key fields (e.g. PR title + target branch,
  work-item title) plus a truncated excerpt of any long body. All content is
  **agent-generated** and is sanitized for display (markdown/HTML escaped, code
  fences neutralised, control characters stripped, long values truncated) so a
  proposal cannot forge UI or break the layout.
- When manual review is configured, the **pending-approval** proposals are listed
  first (under a `⏳ Pending approval` heading), followed by the automatic ones.
  With no approval configured, a single list is shown. The default review
  message points approvers at this tab.
- Rendering is best-effort: if it fails it is logged as a warning and never fails
  the build or blocks the review gate.

**Coexistence with your own summary tabs.** ADO derives a summary section's title
from the uploaded file's base name and does not de-duplicate, so this feature uses
a namespaced base name (`ado-aw-safe-outputs.md` → the `ado-aw-safe-outputs`
section). It is additive and build-scoped: it appears as one extra section
alongside any `task.uploadsummary` tabs your own steps publish (including under
`target: job` / `target: stage`), and never collides with them.

### Executor authentication

All write-bearing safe outputs (e.g. `create-pull-request`,
`create-work-item`, `add-pr-comment`, `upload-build-attachment`) run in the
Stage 3 `SafeOutputs` job and authenticate to Azure DevOps using
`SYSTEM_ACCESSTOKEN`. By default this is `$(System.AccessToken)` — the
pipeline's built-in OAuth token running as the *Project Collection Build
Service* identity. Set `permissions.write` to override this with an
ARM-minted token, e.g. for cross-org writes or named-identity attribution.
See [`docs/network.md`](network.md) and
[`docs/ir.md`](ir.md) for the typed SafeOutputs job wiring.

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
- `tags` - Tags to apply to the work item (optional list; each tag must not contain a semicolon). May be subject to the `allowed-tags` allowlist. Merged with any static `tags` configured in front matter.

**Configuration options (front matter):**
- `work-item-type` - Work item type (default: "Task")
- `area-path` - Area path for the work item
- `iteration-path` - Iteration path for the work item
- `assignee` - User to assign (email or display name). When omitted, falls back to the email of the last person who committed changes to the agent source markdown file (discovered via `git log` at Stage 3).
- `tags` - Static list of tags always applied to the work item (regardless of agent input)
- `allowed-tags` - Allowlist of tags the agent is permitted to use via the `tags` parameter. If empty, any agent-provided tags are accepted. Supports `*` wildcards anywhere in the pattern (e.g., `"agent-*"` matches `"agent-created"`; `"copilot:repo=org/project/*@main"` matches any repo name).
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
    target: "*"               # Required — "*" allows any work item ID, or set to a specific work item ID number
    area-path: true           # enable area path updates (default: false)
    iteration-path: true      # enable iteration path updates (default: false)
    assignee: true            # enable assignee updates (default: false)
    tags: true                # enable tag updates (default: false)
    allowed-tags: []          # Optional — restrict which tags the agent can set (empty = any; supports * wildcards anywhere in the pattern, e.g. "agent-*" or "copilot:repo=org/project/*@main")
```

**Note:** The `target` field is required. If omitted, compilation fails with an error. This ensures operators are intentional about which work items agents can update.

**Security note:** Every field that can be modified requires explicit opt-in (`true`) in the front matter configuration. If the `max` limit is exceeded, additional entries are skipped rather than aborting the entire batch.

### create-pull-request
Creates a pull request with code changes made by the agent. When invoked:
1. Generates a patch file from `git diff` capturing all changes in the specified repository
2. Saves the patch to the safe outputs directory
3. Creates a JSON record with PR metadata (title, description, source branch, repository)

During Stage 3 execution, the repository is validated against the allowed list (from `checkout:` + "self"), then the patch is applied and a PR is created in Azure DevOps.

**Shallow-clone agent pools (automatic):** The diff base for the patch is
computed at agent time from the checked-out repository. On agent pools whose
default git fetch is shallow (`fetchDepth: 1`), a bare `checkout` leaves no
`origin/<target-branch>` ref, which would otherwise prevent the diff base from
being computed. To handle this transparently, whenever `create-pull-request` is
configured the compiler emits a credentialed **prepare step** in the Agent job
(before the agent runs) that fetches and progressively deepens the configured
`target-branch` and points `origin/HEAD` at it — in the `self` checkout **and in
each additional `checkout:` repo dir**, so a PR to *any* allowed repository works.
This means create-pull-request works on shallow-default pools **without** forcing
a full-history checkout and **without** hand-editing the compiled lock (so the
runtime integrity check keeps passing). No configuration is required. See
[`docs/ado-script.md`](ado-script.md) (`prepare-pr-base.js`).

> **Branch semantics.** The step deepens each repo's resolved `target-branch`
> (the PR's **destination/base**) — not the per-repo `repos:` checkout `ref` (the
> source side). By default every repo targets the single `target-branch`; enable
> `infer-target-from-checkout-ref` (and/or `target-branches`) to give each repo
> its own base branch in a multi-checkout setup. The deepened branch always
> matches the branch the PR targets (shared resolution).

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
- `labels` - Labels to add to the PR (optional; validated against `allowed-labels` when configured)

Note: The source branch name is auto-generated from a sanitized version of the PR title plus a unique suffix (e.g., `agent/fix-bug-in-parser-a1b2c3`). This format is human-readable while preventing injection attacks.

**Configuration options (front matter):**
- `target-branch` - Target (base) branch the PR merges into (default: "main"). A
  plain literal branch name, applied to every repo unless overridden below.
- `target-branches` - Optional map of per-repository target-branch overrides,
  keyed by the repository alias the agent passes to `create-pull-request` (`self`
  or a `checkout:` alias). Highest precedence. Lets a multi-checkout ("meta repo")
  agent open a PR into a different base branch per repo.
- `infer-target-from-checkout-ref` - Optional bool (default: false). When `true`,
  a checkout repo with no explicit `target-branches` entry targets its own
  `repos: ref` (the branch it was checked out at). `self` and repos without a
  known ref fall back to `target-branch`. It is a separate boolean (not a magic
  `target-branch` value) so a real branch name can never be mistaken for a
  directive. Only branch refs (`refs/heads/*`) are valid PR targets — if an
  inferred repo is checked out at a **tag** (`refs/tags/*`) the compiler warns
  and you should give it an explicit `target-branches` entry (a PR cannot target
  a tag).

  **Per-repo target resolution precedence** (for a repo `R`): `target-branches[R]`
  → (if `infer-target-from-checkout-ref`) `R`'s checkout ref → `target-branch` →
  `main`. The same resolution drives both the credentialed base-ref deepening (so
  the branch that is fetched/deepened matches the branch the PR targets) and the
  Stage 3 PR creation. Example (meta repo):
  ```yaml
  repos:
    - name: my-org/service   # checked out at refs/heads/main
    - name: my-org/docs
      ref: refs/heads/gh-pages
  safe-outputs:
    create-pull-request:
      target-branch: main                  # self + fallback
      infer-target-from-checkout-ref: true # service → main, docs → gh-pages (from their refs)
      target-branches:
        docs: gh-pages                      # (redundant here; shown as an explicit override)
  ```
- `draft` - Whether to create the PR as a draft (default: **true**). Set to `false` to publish the PR immediately. **Note:** `auto-complete` is silently skipped on draft PRs — set `draft: false` when using `auto-complete: true`.
- `auto-complete` - Set auto-complete on the PR (default: false). Requires `draft: false` to take effect.
- `delete-source-branch` - Delete source branch after merge (default: true)
- `squash-merge` - Squash commits on merge (default: true)
- `title-prefix` - Optional string prepended to all PR titles created by this agent (e.g., `"[Bot] "`)
- `if-no-changes` - Behavior when the agent's patch produces no file changes: `"warn"` (default, succeed with a warning), `"error"` (fail the step), `"ignore"` (succeed silently)
- `max-files` - Maximum number of files allowed in a single PR (default: 100). PRs exceeding this limit are rejected.
- `protected-files` - Controls whether manifest/CI files (e.g., `package-lock.json`, `.github/`, `*.lock`) can be modified: `"blocked"` (default, reject changes to these files) or `"allowed"` (permit all files)
- `excluded-files` - Glob patterns for files to strip from the patch before applying (e.g., `["*.lock", "dist/**"]`)
- `allowed-labels` - Allowlist of labels the agent is permitted to apply. If empty (default), any labels are accepted.
- `reviewers` - List of reviewer emails to add
- `labels` - List of labels to apply
- `work-items` - List of work item IDs to link
- `fallback-record-branch` - When PR creation fails, record the pushed branch name and target branch in the failure response so operators can manually create the PR (default: true)
- `max` - Maximum number of create-pull-request outputs allowed per run (default: 1)
- `include-stats` - Whether to append agent execution stats (token usage, duration, model) to the PR description (default: true)

**Multi-repository support:**
When `workspace: root` and multiple repositories are checked out, agents can create PRs for any allowed repository:
```json
{"title": "Fix in main repo", "description": "...", "repository": "self"}
{"title": "Fix in other repo", "description": "...", "repository": "other-repo"}
```
The `repository` value must be `"self"`, an alias from the `checkout:` list in the front matter, the full Azure DevOps repository name (e.g. `project/repo`), or the bare repository name (case-insensitive, e.g. `sdk-FtdiDeviceControl` for an entry whose ADO name is `4x4/sdk-FtdiDeviceControl`).

### Diagnostic signals

`noop`, `missing-tool`, and `missing-data` are diagnostic safe outputs.
When `safe-outputs:` is configured, the always-running Conclusion job
handles Azure DevOps work-item filing/commenting for these signals. See
[docs/conclusion.md](conclusion.md).

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
- `line` *(optional)* - Line number for an inline comment. Requires `file_path`.
- `start_line` *(optional)* - Starting line for a multi-line inline comment range. Requires `file_path` and `line`, and must be strictly less than `line`.
- `status` *(optional)* - Initial thread status: `"active"` (default), `"fixed"`, `"wont-fix"`, `"closed"`, or `"by-design"`. Subject to the `allowed-statuses` allowlist.

**Configuration options (front matter):**
```yaml
safe-outputs:
  add-pr-comment:
    comment-prefix: "[Agent Review] "  # Optional — prepended to all comments
    allowed-repositories: []           # Optional — restrict which repos can be commented on
    allowed-statuses: []               # Optional — restrict which thread statuses the agent can set (empty = any)
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
    allowed-tags: []             # Optional — restrict which tags can be applied (supports * wildcards anywhere in the pattern, e.g. "agent-*" or "*-approved")
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

### upload-workitem-attachment
Uploads a workspace file as an attachment to an Azure DevOps work item.

**Agent parameters:**
- `work_item_id` - Work item ID to attach the file to (required, must be positive)
- `file_path` - Relative path to the file in the workspace (no directory traversal)
- `comment` *(optional)* - Description of the attachment (at least 3 characters)

**Configuration options (front matter):**
```yaml
safe-outputs:
  upload-workitem-attachment:
    max-file-size: 5242880       # Maximum file size in bytes (default: 5 MB)
    allowed-extensions: []       # Optional — restrict file types (e.g., [".png", ".pdf"])
    comment-prefix: "[Agent] "   # Optional — prefix prepended to the comment
    max: 1                       # Maximum per run (default: 1)
```

### upload-build-attachment

Attaches a workspace file to the **current** Azure DevOps build as a **build
attachment**.

Build attachments are created via the **DistributedTask timeline attachment**
API — the same mechanism as the `##vso[task.addattachment type=…;name=…]<path>`
logging command. The resulting object *is* a build attachment: it is stored once
by `{type}`/`{name}` and read back through the Build ▸ Attachments **Get/List**
API (and by ADO extensions that register for a given attachment `type`). The
executor calls the REST endpoint directly (rather than emitting the `##vso`
command) so it can report a deterministic success/failure and surface the
attachment URL.

> **Current run only.** A timeline attachment can only be added to the job that
> is executing, so this tool always targets the **current** build. There is no
> ADO API to attach to an arbitrary other build. (The tool previously advertised
> a `PUT /_apis/build/builds/{id}/attachments/…` route to attach to any build —
> that route never existed; the Build ▸ Attachments API is read-only.)

> **Not visible in the standard UI.** Build attachments do not appear in the
> build summary UI; they are read via the REST API or a custom Azure DevOps
> extension that registers a tab matching the `attachment-type` value. For
> artifacts that should appear in the **Artifacts tab**, use
> [`upload-pipeline-artifact`](#upload-pipeline-artifact) instead.

The tool stages the file during Stage 1 (MCP) by copying it into the
safe-outputs directory; Stage 3 reads the staged copy and PUTs it to the current
job's timeline record.

**Agent parameters:**
- `build_id` *(optional)* - **Omit** to attach to the current run (recommended). If set, it must equal the current build id; any other value is rejected.
- `artifact_name` - Attachment name (1–100 chars, alphanumeric / `-` / `_` / `.`, no leading `.`)
- `file_path` - Relative path to the file in the workspace (no directory traversal)

**Configuration options (front matter):**
```yaml
safe-outputs:
  upload-build-attachment:
    max-file-size: 52428800              # Maximum file size in bytes (default: 50 MB)
    allowed-extensions: []               # Optional — restrict file types (e.g., [".png", ".pdf", ".log"])
    allowed-artifact-names: []           # Optional — restrict names (suffix `*` = prefix match)
    name-prefix: ""                      # Optional — prepended to the agent-supplied artifact name
    attachment-type: "agent-artifact"    # Optional — {type} segment in the attachment path (default: "agent-artifact")
    max: 3                               # Maximum per run (default: 3)
```

> **Removed:** `allowed-build-ids` is no longer supported here — since a build
> attachment can only target the current run, the allow-list was meaningless. A
> [codemod](codemods.md) auto-removes it from source (with a compile warning) on
> the next `ado-aw compile`. (`allowed-build-ids` remains valid for
> [`upload-pipeline-artifact`](#upload-pipeline-artifact).)

**Notes:**
- Single-file only; directory uploads are not supported.

**About `attachment-type`:** This is the `{type}` segment in the attachment path
(`.../attachments/{type}/{name}`). It acts as a category label. Azure DevOps
extensions can register to display attachments of a specific type — for example,
the built-in code coverage extension displays attachments with type
`CodeCoverageSummary`. The default `agent-artifact` is a custom type; without a
matching ADO extension installed, attachments with this type are only accessible
via the REST API. Change this only if you have a custom extension that displays
attachments of a specific type. Most users should use
[`upload-pipeline-artifact`](#upload-pipeline-artifact) for user-visible
artifacts instead.

### upload-pipeline-artifact

Publishes a workspace file as an Azure DevOps **pipeline artifact** that appears
in the **Artifacts tab** of the build summary page. Uses the ADO build artifacts
REST API in two steps:

1. **Upload bytes** to the agent's own per-build file container (Azure DevOps
   creates one container per build and exposes its ID via `BUILD_CONTAINERID`).
2. **Associate** the artifact record (`name = artifact_name`) with the target
   build via `POST /{project}/_apis/build/builds/{effective_build_id}/artifacts`.

**Omit `build_id` to target the current pipeline run** — the executor resolves
the build ID from the `BUILD_BUILDID` environment variable automatically. When
`build_id` is provided, the artifact record is published to that specific build
("cross-build publishing"). The artifact bytes still live in the agent's own
build container; only the record's pointer is associated with the target build.
This means cross-published artifacts share the agent build's retention — if the
agent's build is purged, the cross-referenced artifact stops being downloadable.
Cross-project publishing is not supported (the associate POST uses the current
pipeline's project).

The tool stages the file during Stage 1 (MCP) by copying it into the
safe-outputs directory; Stage 3 reads the staged copy and executes the two-step
REST flow.

**Agent parameters:**
- `build_id` *(optional)* - Target build ID. Omit to publish to the current pipeline run. Must be positive when specified.
- `artifact_name` - Artifact name shown in the Artifacts tab (1–100 chars, alphanumeric / `-` / `_` / `.`, no leading `.`)
- `file_path` - Relative path to the file in the workspace (no directory traversal)

**Configuration options (front matter):**
```yaml
safe-outputs:
  upload-pipeline-artifact:
    max-file-size: 52428800              # Maximum file size in bytes (default: 50 MB)
    allowed-extensions: []               # Optional — restrict file types (e.g., [".png", ".pdf", ".log"])
    allowed-artifact-names: []           # Optional — restrict names (suffix `*` = prefix match)
    allowed-build-ids: []                # Optional — restrict target builds (skipped when targeting current build)
    name-prefix: ""                      # Optional — prepended to the agent-supplied artifact name
    require-unique-names: false          # Optional — see "Reusing artifact names" below
    max: 3                               # Maximum per run (default: 3)
```

**Reusing artifact names within one agent run:**
By default, the same `artifact_name` may be reused across multiple
`upload-pipeline-artifact` calls in one run (e.g. publishing a `TriageSummary`
to many failing builds at once). The executor inserts a short hash suffix
(`{artifact_name}__{6 hex}`) into the **internal container folder name** so
the calls don't silently overwrite each other's bytes in the agent's shared
build container. The hash lives only in internal addressing — it does not
appear in the `record.name` your downstream consumers query for, in the web UI
"Download as zip" filename, or in the contents of files extracted by the
`DownloadBuildArtifacts@1` / `DownloadPipelineArtifact@2` tasks (all of which
strip the container folder prefix).

Set `require-unique-names: true` to use a clean container folder
(`{artifact_name}` only, no suffix) and reject in-run reuse of
`(effective_build_id, artifact_name)` with a clear early error before any HTTP
call. Use this when you guarantee one artifact per name per run and want the
shortest possible internal addressing.

Two records with the same `name` on the **same** target build still collide at
the record level (ADO returns 409 from the associate call) regardless of this
setting; use distinct `artifact_name` values when targeting one build with
multiple uploads.

**Notes:**
- Single-file only; directory uploads are not supported.
- When `build_id` is omitted and `allowed-build-ids` is configured, the allow-list check is skipped — the current build is implicitly trusted.
- Requires `BUILD_CONTAINERID`, `BUILD_BUILDID`, and `SYSTEM_TEAMPROJECTID` (all set automatically inside an Azure DevOps pipeline job) and `vso.build_execute` scope on the executor's token (granted to `$(System.AccessToken)` by default, and to the ARM-minted token when `permissions.write` is set).

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
