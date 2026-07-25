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

### Threat detection (`threat-detection`)

Threat Detection runs between the Agent and SafeOutputs jobs. Configuration
lives under `safe-outputs:` because it controls the gate protecting safe-output
execution, matching gh-aw.

```yaml
safe-outputs:
  create-pull-request: {}
  threat-detection:
    enabled: true
    prompt: |
      Also check for:
      - authentication bypasses
      - unsafe deserialization
      - hardcoded credentials
    engine:
      id: copilot
      model: gpt-5-mini
      args: [--reasoning-effort=high]
      env:
        DETECTION_MODE: strict
    steps:
      - bash: ./scripts/prepare-security-scanner.sh
        displayName: Prepare security scanner
    post-steps:
      - bash: ./scripts/run-security-scanner.sh
        displayName: Run security scanner
```

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `true` | Run AI threat analysis. Runtime expressions are not supported. |
| `prompt` | string | *(none)* | Literal additional instructions appended under `## Additional Instructions`. ADO expressions (`$(...)`, `${{ ... }}`, `$[...]`) are rejected. The fixed detector prompt and `THREAT_DETECTION_RESULT:` contract are never replaced. |
| `engine` | string/object | top-level `engine` | Detection-specific Copilot configuration overlay. Non-Copilot IDs remain unsupported. |
| `steps` | ADO step list | `[]` | Trusted host-side steps after artifact/prompt preparation and before credential minting + AI execution. |
| `post-steps` | ADO step list | `[]` | Trusted host-side steps after AI execution and token revocation, before verdict parsing. |

Boolean shorthand is also supported:

```yaml
safe-outputs:
  create-pull-request: {}
  threat-detection: false
```

When disabled, the Detection job remains in the pipeline as a pass-through: it
copies the Agent artifact to `analyzed_outputs`, publishes
`threatAnalysis.SafeToProcess=true`, and still detects approval-gated proposals.
Safe outputs therefore proceed without AI analysis, but manual-review gates and
the existing job/artifact graph remain intact. Custom Detection `steps` and
`post-steps` do not run while disabled.

The engine overlay inherits top-level settings unless a nested value is
supplied. Detection `env` merges by key (Detection wins); `args` inherits when
omitted and replaces the inherited list when supplied, so `args: []` clears
top-level arguments for Detection. Install version, command, auth/provider,
BYOK credential isolation, and firewall hosts are all resolved from the
effective Detection engine.

Detection inherits the top-level `engine.timeout-minutes`. Set
`safe-outputs.threat-detection.engine.timeout-minutes` to override that limit
for Detection only.

`steps` and `post-steps` run outside AWF and are trusted with the checked-out
repository and Detection artifacts. A failing custom step fails Detection and
blocks SafeOutputs. Use `pool.overrides.detection` to select a different
Detection pool; there is no duplicate `runs-on` key.

Not currently supported: runtime/expression-controlled enablement,
`continue-on-error`, `engine: false` custom-scanner-only mode, and a
Detection-specific AI-credit budget.

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

The Detection job always runs first. When AI threat analysis is enabled, a
flagged run applies nothing — automatic or reviewed.

> **Trust boundary note for `pool.overrides:`:** When `pool.overrides:` is used
> to move Detection, SafeOutputs, or Conclusion onto a different **self-hosted**
> pool than the Agent job, that pool's administrators and agents are trusted with
> the pipeline artifacts and credentials available to those jobs — including the
> safe-output NDJSON and the write-capable `SC_WRITE_TOKEN`. Using a
> Microsoft-hosted `vmImage:` override does not change the trust boundary.
> See [`docs/front-matter.md`](front-matter.md#per-job-pool-overrides-pooloverrides)
> for the full reference.

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

## Custom safe-output jobs

Reusable components imported with [`imports:`](imports.md) can add custom
agent-callable tools under `safe-outputs.jobs.<name>`. Each definition compiles
to a dedicated Azure Pipelines job:

```text
Agent proposal -> Detection -> optional ManualReview -> Custom_<tool>
```

The Agent sees only the generated MCP description and closed input schema. The
custom job receives its authored environment variables and steps only after the
proposal artifact has passed Detection (and approval when configured).

### Job fields

| Field | Description |
|-------|-------------|
| `display-name` | Optional ADO job display name. |
| `description` | Required MCP tool description. |
| `condition` | Optional ADO condition ANDed with compiler-owned gates. |
| `needs` | Optional additional custom/canonical job dependencies. |
| `timeout-minutes` | Optional ADO job timeout. Omission uses the platform/pool default. |
| `max` | Per-run MCP call budget. Defaults to 1. |
| `inputs` | Closed Agent-facing schema. Types: `string`, `boolean`, `choice`. |
| `env` | Literal strings or explicit ADO macros such as `$(SHARED_TOKEN)`. |
| `output` | Static acknowledgement returned when the Agent records a proposal; it is not the later job result. |
| `steps` | Self-contained inline Bash/PowerShell steps or explicitly versioned ADO tasks. |

Input definitions use the gh-aw-compatible fields `description`, `required`,
`default`, `type`, and `options`. String arguments are limited to 10 KiB. Extra,
missing, mistyped, invalid-choice, and oversized arguments are rejected by the
MCP server and revalidated before the job runs.

Custom job names stay hyphenated in MCP and in `item.type`; generated ADO job
identifiers replace non-identifier characters with underscores.

### Agent-output file

Every custom job receives the same transport-sanitized aggregate file through
`ADO_AW_AGENT_OUTPUT`:

```json
{
  "items": [
    {"type":"send-notification","title":"Release blocked","severity":"critical"}
  ]
}
```

The aggregate contains **only custom safe-output proposals**. Built-in
proposals (`create-pull-request`, `create-work-item`, `add-build-tag`, …) are
applied by Stage 3 itself and are never surfaced to a custom job, so an
imported component cannot read proposal content it does not own.

A component filters the items it owns:

```bash
jq -c '.items[] | select(.type == "send-notification")' \
  "$ADO_AW_AGENT_OUTPUT"
```

The custom job's ADO timeline result is the execution outcome. There is no
custom results file or per-proposal execution-record protocol.

### Example

```yaml
safe-outputs:
  jobs:
    send-notification:
      display-name: Send release notification
      description: Notify release operators when human action is required.
      max: 2
      output: Notification proposal accepted.
      inputs:
        title:
          description: Short operator-facing title.
          type: string
          required: true
        severity:
          description: Operational severity.
          type: choice
          options: [info, warning, critical]
          required: true
      env:
        NOTIFICATION_DESTINATION: release-operations
        NOTIFICATION_TOKEN: $(SHARED_NOTIFICATION_TOKEN)
      steps:
        - bash: |
            set -euo pipefail
            jq -c '.items[] | select(.type == "send-notification")' \
              "$ADO_AW_AGENT_OUTPUT" |
            while IFS= read -r item; do
              if [ "$ADO_AW_SAFE_OUTPUTS_STAGED" = "true" ]; then
                printf 'STAGED: %s\n' "$item"
                continue
              fi
              curl -fsS https://notify.example/api/messages \
                -H "Authorization: Bearer $NOTIFICATION_TOKEN" \
                -H 'Content-Type: application/json' \
                --data "$item"
            done
          displayName: Send notifications
```

`ADO_AW_AGENT_OUTPUT` is a Stage-3 materialized copy of the analyzed proposals,
restricted to custom safe-output items. Before the file is written, ado-aw
revalidates custom schemas and budgets, strips ANSI/unsafe control characters,
and neutralizes Azure Pipelines logging commands (`##vso[` and `##[`) in string
values and object keys. Custom values are revalidated after sanitization so
required/type/size guarantees apply to the data the job receives; sanitized key
collisions fail closed. URLs, mentions, HTML, markdown, and other
external-system payload text are otherwise preserved. The analyzed proposal
artifact remains unchanged for Detection and audit.
String size is revalidated after transport sanitization, so a value whose
neutralized form exceeds the 10 KiB custom-input limit fails materialization
before any authored step runs.

Treat the materialized values as untrusted integration data even after this
transport sanitization. Parse JSON structurally, build outbound request bodies
with tools such as `jq -n --arg`, and apply API-specific validation and escaping.
Avoid printing raw scalar fields when they are not needed.

Supported authored steps are inline `bash`, `powershell`, or `pwsh`, plus ADO
tasks with an explicit numeric version such as `PowerShell@2`. Custom jobs reject
`template:`, authored checkout, containers, and unversioned tasks. They do not
automatically checkout the consumer or component repository; executor logic must
be self-contained in the compiled steps.

### Approval and dependencies

Configure approval through the same top-level per-tool policy as built-ins:

```yaml
safe-outputs:
  send-notification:
    require-approval: true
```

Only directly reviewed tools appear in ManualReview. A non-reviewed custom job
that depends on a reviewed job runs after the reviewed chain, uses the reviewed
safe-output pool, and is not separately presented for approval.

### Staged mode

Custom staged mode follows gh-aw's cooperative model. Global or per-tool policy
sets `ADO_AW_SAFE_OUTPUTS_STAGED=true`; trusted component steps must avoid the
write and render their own preview. ado-aw does not claim to prove that arbitrary
privileged component code made no external write.

```yaml
safe-outputs:
  staged: false
  send-notification:
    staged: true
```

### Pools and secrets

Components cannot choose pools. Consumers select the execution trust boundary:

```yaml
pool:
  vmImage: ubuntu-22.04
  overrides:
    safe-outputs:
      name: PrivilegedWriters
    safe-outputs-reviewed:
      name: ReviewedWriters
```

`env` values are emitted verbatim. Use ADO secret variables or authorized
variable groups for macros such as `$(SHARED_NOTIFICATION_TOKEN)`. Secret values
are not resolved into generated config or artifacts; runtime log masking is
provided by Azure Pipelines. Custom component code is trusted privileged code.

## GitHub issue safe outputs

`create-issue` and `set-issue-type` call GitHub only from Stage 3, after threat
detection. The GitHub write credential is never exposed to Agent or Detection.
The MCP routes are configured-only: GitHub auth by itself does not expose them
to the agent; each tool appears only when its own front-matter key is present.

### Authentication

With no explicit auth, Stage 3 uses the secret ADO pipeline variable
`ADO_AW_GITHUB_TOKEN`:

```yaml
safe-outputs:
  create-issue:
    target-repo: octo-org/octo-repo
```

Set it with:

```text
ado-aw secrets set ADO_AW_GITHUB_TOKEN <fine-grained-token>
```

The token needs **Issues: read and write** on the target repository. To use a
differently named secret, provide exactly one ADO macro:

```yaml
safe-outputs:
  github-token: "$(MY_GITHUB_ISSUES_TOKEN)"
  github-api-url: https://ghe.example.com/api/v3  # optional PAT API base
  create-issue:
    target-repo: octo-org/octo-repo
```

Literal tokens and compound expressions are rejected at compile time.
`$(GITHUB_TOKEN)` is intentionally not the default because that variable is
used by the read-only Agent/Detection path, and it is rejected if supplied
explicitly. `github-api-url` defaults to `https://api.github.com`, accepts only
an `https://` URL, and applies only to PAT auth; App auth uses its nested
`api-url`.

#### Shared GitHub App

When `engine.github-app-token` is configured and neither SafeOutputs auth key is
present, the App credentials are reused but the tokens are not:

```yaml
engine:
  id: copilot
  github-app-token:
    app-id: 1234567
    private-key: GITHUB_APP_PRIVATE_KEY
    owner: octo-org
    repositories: [octo-repo]
    permissions:
      contents: read
      issues: read
      pull-requests: read

safe-outputs:
  create-issue:
    target-repo: octo-org/octo-repo
```

Agent and Detection each mint their own explicitly read-only token. SafeOutputs
mints a separate token scoped to the configured target repository with only
`issues: write`, then revokes it after execution. Shared credentials are
rejected when the engine permission map is absent or contains a repository
`write` permission.

#### Separate SafeOutputs GitHub App

Use `safe-outputs.github-app` to isolate the write App:

```yaml
safe-outputs:
  github-app:
    client-id: Iv23liSafeOutputsApp
    private-key: SAFE_OUTPUTS_GITHUB_APP_PRIVATE_KEY
    owner: octo-org
    repositories: [octo-repo]
    api-url: https://api.github.com
    skip-token-revocation: false
  create-issue:
    target-repo: octo-org/octo-repo
```

`client-id` and `app-id` are aliases. `private-key` names an ADO secret
variable; it is not the PEM value or a `$(...)` macro. SafeOutputs derives the
minimum permission request, so arbitrary `github-app.permissions` overrides are
not accepted here. `github-app` and `github-token` are mutually exclusive.

### Target repository

`target-repo` is operator-controlled. It may be omitted only when the ADO build
source provider is GitHub and `BUILD_REPOSITORY_NAME` is an `owner/repo` slug.
Azure Repos workflows must set it explicitly.

### Temporary IDs and approval

Use a gh-aw-compatible temporary ID to refer to a newly created issue before
its real number exists:

```json
{"title":"Build failure","body":"Detailed failure report long enough for validation.","temporary_id":"#aw_bug1"}
{"issue_number":"#aw_bug1","issue_type":"Bug"}
```

The ID format is `#aw_` plus 3-12 ASCII alphanumeric/underscore characters;
the leading `#` is optional. `create-issue` must run first and succeed.
Duplicate, unresolved, cross-repository, or reversed references fail before an
API call.

When both tools are configured, they must have the same effective
`require-approval` setting so they execute in the same SafeOutputs job. A
section-level gate is the simplest form:

```yaml
safe-outputs:
  require-approval: true
  create-issue:
    target-repo: octo-org/octo-repo
    require-temporary-id: true
  set-issue-type:
    target-repo: octo-org/octo-repo
```

## Available Safe Output Tools

### create-issue

Creates a GitHub issue.

```yaml
safe-outputs:
  create-issue:
    target-repo: octo-org/octo-repo
    title-prefix: "[agent] "
    labels: [automation]
    allowed-labels: ["agent-*", bug]
    assignees: [octocat]
    require-temporary-id: true
    max: 1
```

- `target-repo` *(optional only for GitHub-backed builds)* - fixed
  `owner/repo` target.
- `title-prefix` *(optional)* - prepended in Stage 3.
- `labels` *(optional)* - static labels always applied.
- `allowed-labels` *(optional)* - allowlist for agent labels. Empty/absent is
  default-deny; `["*"]` permits any label.
- `assignees` *(optional)* - static assignees merged with agent input.
- `require-temporary-id` *(optional, default `false`)* - reject proposals that
  omit `temporary_id`.
- `max` *(optional, default `1`)* - per-run creation budget.

Agent parameters are `title`, `body`, optional `labels`, optional `assignees`,
and optional `temporary_id`.

### set-issue-type

Sets or clears a native GitHub Issue Type:

```yaml
safe-outputs:
  set-issue-type:
    target-repo: octo-org/octo-repo
    allowed: [Bug, Feature, Task]
    max: 5
```

- `target-repo` *(optional only for GitHub-backed builds)* - target for numeric
  issue numbers; temporary IDs carry their created repository.
- `allowed` *(optional)* - case-insensitive type allowlist. Empty/absent allows
  any configured repository type. Clearing is always allowed.
- `max` *(optional, default `5`)* - per-run update budget.

Agent parameters are required `issue_number` (positive number or temporary ID)
and required `issue_type`. Pass `""` to clear the type.

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

**Shallow-clone agent pools (automatic):** The diff base is computed at agent
time from the checked-out repository. For same-organization Azure Repos,
`prepare-pr-base.js` asks the ADO Diffs API for the exact `commonCommit`,
`aheadCount`, and `behindCount`, then fetches only the source and target ranges
needed to make that base locally reachable. It verifies the server result with
`git merge-base --all` before the containerized SafeOutputs MCP server can
generate a patch. Non-Azure/cross-organization/unavailable-REST cases use bounded
dual-ref depths 200/500/2000 and fail clearly rather than silently fetching full
history.

The compiler emits separate modes in the two isolated ADO jobs:

- **Agent — `patch-base`:** prepares and verifies both sides of the merge-base.
- **SafeOutputs — `target-worktree`:** fetches only `origin/<target>` at depth 1
  for the executor's `git worktree add` (issue #1453).

No full-history checkout or `--unshallow` fallback is forced. Authors can
explicitly set `repos: [{ name: self, fetch-depth: 0 }]` when they accept that
potentially large cost. The generated lock remains source-controlled and the
runtime integrity check stays enabled. See [`docs/ado-script.md`](ado-script.md)
(`prepare-pr-base.js`).

> **Branch semantics.** Each repo carries its checkout source ref and its
> resolved PR destination. By default every repo targets the single
> `target-branch`; enable `infer-target-from-checkout-ref` (and/or
> `target-branches`) to give each repo its own base branch.

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
    target: "*"                  # Required — "*" allows any work item ID, or set to a specific ID
    max: 5                       # Maximum per run (default: 5)
```

**Note:** The `target` field is required. If omitted, Stage 3 execution fails with an error. Use the same scoping options as `comment-on-work-item`: `"*"` for any work item, a numeric ID for a specific item, a list of IDs, or an area path prefix string.

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
