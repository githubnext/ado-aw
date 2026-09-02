# Safe Outputs Configuration & Tool Reference

_Part of the [ado-aw documentation](../AGENTS.md)._

## Safe Outputs Configuration

The front matter supports a `safe-outputs:` field for configuring specific tool behaviors:

```yaml
safe-outputs:
  create-work-item:
    work-item-type: Task
    tags:
      - automated
      - agent-created
  assign-work-item:
    target: "*"
    allowed: ["user@example.com"]
    blocked: ["svc-*"]
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

### Markdown body sanitization

Safe outputs whose body is stored as Markdown (today: `create-work-item`'s
`description`) go through a Markdown-aware sanitizer rather than blanket HTML
escaping, so headings, lists, tables and code fences survive.

The policy is:

- **Transport sanitization applies to the whole document.** Control characters
  and ANSI escapes are removed, Azure DevOps logging commands (`##vso[`, `##[`)
  are wrapped in backticks, HTML comments are removed, and the content size and
  line count caps are enforced.
- **Code is left alone.** [pulldown-cmark](https://docs.rs/pulldown-cmark)
  identifies code spans, fenced and indented code blocks; no rendering
  transformation is applied inside them, so a fence showing `<script>` or
  `@mention` stays readable.
- **Inline HTML is allowlisted, not blocklisted.**
  [ammonia](https://docs.rs/ammonia) keeps a small set of formatting tags that
  have a Markdown equivalent (`a`, `b`/`strong`, `i`/`em`, `code`, `pre`,
  headings, lists, tables, `img`, `blockquote`, …) with a small attribute
  allowlist (`href`, `src`, `alt`, `title`, `width`, `height`, `colspan`,
  `rowspan`, `align`). Everything else — `script`, `style`, `iframe`, `form`,
  SVG/MathML, `on*` handlers, `style`/`class`/`id` attributes — is dropped;
  `script` and `style` also lose their contents.
- **URLs are scheme-allowlisted.** `http`, `https`, `mailto` and relative
  destinations are allowed everywhere (HTML attributes, Markdown links and
  images, autolinks and reference definitions). Any other scheme is dropped from
  HTML attributes and replaced with `(redacted)` in Markdown destinations.
- **Mentions and bot triggers are neutralized outside code.** `@name`,
  `fixes #123` and `AB#123` are wrapped in backticks so they cannot notify
  people or link work items.

Text that only looks like markup is escaped rather than deleted: `Vec<String>`
in prose is stored as `Vec&lt;String&gt;`, which Markdown renders as the author
wrote it, and that escaping is also what stops dropped markup from being
re-parsed. Markup a browser really would parse as a tag still goes to the
allowlist and is dropped there, so a dangerous payload never reappears as
visible text.

Cleaning normalizes HTML, so a few inputs come back rendering the same but
written differently: `\r\n` becomes `\n`, `<br />` becomes `<br>`, a table gains
an implied `<tbody>`, a mid-line `>` is stored as `&gt;`, and the content of a
removed raw-text element such as `<noscript>` is escaped rather than kept as
markup.

Azure DevOps then applies its own server-side Markdown sanitization when the
work item is stored. That service-side pass may further normalize safe HTML and
remove dangerous HTML even when it appeared inside a code fence. The compiler
tests pin ado-aw's pre-storage output separately from the executor E2E fixture
that pins the representation returned by Azure DevOps.

### Executor authentication

All write-bearing safe outputs (e.g. `create-pull-request`,
`create-work-item`, `add-pr-comment`, `upload-build-attachment`) run in the
Stage 3 `SafeOutputs` job and authenticate to Azure DevOps using
`SYSTEM_ACCESSTOKEN`. By default this is `$(System.AccessToken)` — the
pipeline's built-in OAuth token running as the *Project Collection Build
Service* identity. Set `permissions.write` to override this with an
AzureCLI@3-minted token, e.g. for cross-org writes or named-identity
attribution.
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

The GitHub-qualified safe outputs call GitHub only from Stage 3, after threat
detection and any configured manual review. The GitHub write credential is
never exposed to Agent or Detection. All thirteen routes are
**configured-only**: authentication alone exposes no GitHub mutation tool; each
tool appears only when its exact front-matter key is present.

### Authentication

With no explicit auth, Stage 3 uses the secret ADO pipeline variable
`ADO_AW_GITHUB_TOKEN`:

```yaml
safe-outputs:
  create-github-issue:
    target-repo: octo-org/octo-repo
```

Set it with:

```text
ado-aw secrets set ADO_AW_GITHUB_TOKEN <fine-grained-token>
```

The PAT needs read/write access for every enabled capability: **Issues** for
issues, **Pull requests** when a tool permits PR targets, and **Discussions**
when comment minimization permits discussion comments. To use a differently
named secret, provide exactly one ADO macro:

```yaml
safe-outputs:
  github-token: "$(MY_GITHUB_ISSUES_TOKEN)"
  github-api-url: https://ghe.example.com/api/v3  # optional PAT API base
  create-github-issue:
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
  create-github-issue:
    target-repo: octo-org/octo-repo
```

Agent and Detection each mint their own explicitly read-only token. SafeOutputs
mints a separate token scoped to the repositories used by that execution job,
requests only the required `issues: write`, `pull-requests: write`, and/or
`discussions: write` permissions, then revokes it after execution. Automatic
and reviewed jobs derive permissions independently. Shared credentials are
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
  create-github-issue:
    target-repo: octo-org/octo-repo
```

`client-id` and `app-id` are aliases. `private-key` names an ADO secret
variable; it is not the PEM value or a `$(...)` macro. SafeOutputs derives the
minimum permission request, so arbitrary `github-app.permissions` overrides are
not accepted here. `github-app` and `github-token` are mutually exclusive.

### Repository selection

Every tool accepts operator-controlled `target-repo` and `allowed-repos`.
Agent parameters accept optional `repository` where selection is meaningful.

1. An explicit agent `repository` must exactly equal `target-repo` or an entry
   in `allowed-repos`.
2. Without an agent selection, `target-repo` is used.
3. Without either, ado-aw uses `BUILD_REPOSITORY_NAME` only for a GitHub or
   GitHub Enterprise source build whose name is a valid `owner/repo` slug.

Azure Repos workflows must therefore configure `target-repo`. Repository globs,
wildcards, and ADO runtime expressions are rejected; `allowed-repos` is an
exact operator allowlist. PAT auth may target repositories owned by different
accounts. GitHub App auth requires `target-repo` and every `allowed-repos`
entry to share one owner because one installation token is minted per job.

`create-github-issue` supports repository selection but has no
`required-labels` or `required-title-prefix` filter because no target object
exists yet.

### Mutation filters

Existing-object tools support these shared fail-closed filters:

| Field | Behavior |
|---|---|
| `required-labels` | The current target must have **all** listed labels before the first write. |
| `required-title-prefix` | The current title must start with this exact value before the first write. |
| `issues` | Permit issue targets (default `true` where the tool has an issue/PR switch). |
| `pull-requests` | Permit pull-request targets. Defaults to `false` for `comment-on-github-issue` and `add-github-issue-labels`, but `true` for `update-github-issue`; requests `pull-requests: write`. |
| `discussions` | Permit discussion comments where supported (default `false`; requests `discussions: write`). |

Multi-call operations preflight repository policy, filters, capability
switches, and requested values before writing, so a denied later value cannot
leave a partial mutation.

### Temporary IDs and approval

Use a gh-aw-compatible temporary ID to refer to a newly created issue before
its real number exists:

```json
{"title":"Build failure","body":"Detailed failure report long enough for validation.","temporary_id":"#aw_bug1"}
{"issue_number":"#aw_bug1","issue_type":"Bug"}
```

The ID format is `#aw_` plus 3-12 ASCII alphanumeric/underscore characters;
the leading `#` is optional. `create-github-issue` must run first and succeed.
Every tool whose `issue_number` parameter accepts a number also accepts a
same-run temporary ID. The ID retains the repository selected at creation; an
explicit `repository` on a consumer must match it. Duplicate, unresolved,
cross-repository, or reversed references fail before an API call.

`create-github-issue` and **every configured temporary-ID consumer** must have
the same effective `require-approval` setting so they execute in the same
SafeOutputs process and share the in-memory ID map. Mixing automatic and
reviewed groups is rejected at compile time. A section-level gate is the
simplest form:

```yaml
safe-outputs:
  require-approval: true
  create-github-issue:
    target-repo: octo-org/octo-repo
    require-temporary-id: true
  set-github-issue-type:
    target-repo: octo-org/octo-repo
```

The `ado-aw-safe-outputs` build-summary tab shows each proposal using the exact
tool name, repository, target IDs, requested state, labels/users/field values,
and a sanitized body excerpt. Reviewed GitHub proposals are grouped under
**Pending approval**; non-gated proposals remain under **Automatic**.

### REST, GraphQL, and GitHub Enterprise

Most mutations use GitHub REST. `hide-github-issue-comment`,
`close-github-issue` with `duplicate_of`, `set-github-issue-field`, and
`link-github-sub-issue` require GraphQL; `comment-on-github-issue` also uses
GraphQL when `hide-older-comments` is enabled. Numeric REST comment IDs are
resolved to GraphQL node IDs before minimization.

For PAT auth, `github-api-url` selects the REST base. For App auth, use
`github-app.api-url`. ado-aw derives the corresponding GitHub.com or GHES
GraphQL endpoint. GitHub Enterprise availability depends on the server version
and enabled product features, especially issue fields, duplicate marking,
comment minimization, and sub-issues. Unsupported REST/GraphQL operations fail
the safe output with the sanitized API status/message; requested mutations are
never silently skipped.

## Available Safe Output Tools

### create-github-issue

Creates a GitHub issue.

```yaml
safe-outputs:
  create-github-issue:
    target-repo: octo-org/octo-repo
    allowed-repos: [octo-org/other-repo]
    title-prefix: "[agent] "
    labels: [automation]
    allowed-labels: ["agent-*", bug]
    assignees: [octocat]
    require-temporary-id: true
    max: 1
```

- `target-repo` *(optional only for GitHub-backed builds)* - fixed
  `owner/repo` target.
- `allowed-repos` *(optional, default `[]`)* - exact additional repositories
  the agent may select with `repository`.
- `title-prefix` *(optional)* - prepended in Stage 3.
- `labels` *(optional)* - static labels always applied.
- `allowed-labels` *(optional)* - allowlist for agent labels. Empty/absent is
  default-deny; `["*"]` permits any label.
- `assignees` *(optional)* - static assignees merged with agent input.
- `require-temporary-id` *(optional, default `false`)* - reject proposals that
  omit `temporary_id`.
- `max` *(optional, default `1`)* - per-run creation budget.

Agent parameters are `title`, `body`, optional `labels`, optional `assignees`,
optional `temporary_id`, and optional `repository`. The body receives the
stable ado-aw trace footer.

### set-github-issue-type

Sets or clears a native GitHub Issue Type:

```yaml
safe-outputs:
  set-github-issue-type:
    target-repo: octo-org/octo-repo
    allowed: [Bug, Feature, Task]
    max: 5
```

- `target-repo` *(optional only for GitHub-backed builds)* - target for numeric
  issue numbers; temporary IDs carry their created repository.
- `allowed` *(optional)* - case-insensitive type allowlist. Empty/absent allows
  any configured repository type. Clearing is always allowed.

  > **Note the deliberate asymmetry** with `create-github-issue.allowed-labels`,
  > which is default-**deny**. Issue types are a closed set defined by the
  > repository owner, so "any configured repository type" is already bounded by
  > configuration the agent cannot influence. Labels are free-form strings the
  > agent can invent, so an empty `allowed-labels` accepts none.
- `max` *(optional, default `5`)* - per-run update budget.

Agent parameters are required `issue_number` (positive number or temporary ID)
and required `issue_type`, plus optional `repository`. Pass `""` to clear the
type. This tool also accepts the shared repository and mutation-filter fields.

### GitHub mutation tool matrix

Every row accepts `max`, `require-approval`, `target-repo`, `allowed-repos`,
`required-labels`, and `required-title-prefix` unless noted otherwise. Agent
JSON uses the snake_case parameter names below.

| Tool | Agent parameters | Tool-specific configuration | Default `max` |
|---|---|---|---:|
| `comment-on-github-issue` | `issue_number`, `body`, optional `repository` | `hide-older-comments`, `allowed-reasons`, `issues`, `pull-requests`, `footer` | 1 |
| `hide-github-issue-comment` | `comment_id`, optional `reason`, optional `repository` | `allowed-reasons`, `discussions` | 5 |
| `add-github-issue-labels` | `issue_number`, `labels`, optional `repository` | `allowed`, `blocked`, `issues`, `pull-requests` | 5 |
| `remove-github-issue-labels` | `issue_number`, `labels`, optional `repository` | `allowed`, `blocked` | 5 |
| `close-github-issue` | `issue_number`, optional `body`, `state_reason`, `duplicate_of`, `repository` | `state-reason`, `allowed-state-reason`, `allow-body` | 1 |
| `update-github-issue` | `issue_number`, one or more of `status`/`title`/`body`/`labels`/`assignees`/`milestone`, optional body `operation`, optional `repository` | `status`, `title`, `body`, `labels`, `assignees`, `milestone`, `allowed-labels`, `footer`, `issues`, `pull-requests` | 1 |
| `set-github-issue-field` | `issue_number`, `value`, exactly one of `field_name`/`field_node_id`, optional `repository` | `allowed-fields` | 5 |
| `assign-github-issue-milestone` | `issue_number`, exactly one of `milestone_number`/`milestone_title`, optional `repository` | `allowed`, `auto-create` | 1 |
| `assign-github-issue-to-user` | `issue_number`, `assignee` or `assignees`, optional `repository` | `allowed`, `blocked`, `unassign-first` | 1 |
| `unassign-github-issue-from-user` | `issue_number`, `assignee` or `assignees`, optional `repository` | `allowed`, `blocked` | 1 |
| `link-github-sub-issue` | `parent_issue_number`, `sub_issue_number`, optional `repository` | `parent-required-labels`, `parent-title-prefix`, `sub-required-labels`, `sub-title-prefix` | 5 |

#### Comments and comment minimization

`comment-on-github-issue` posts a comment with a stable hidden ado-aw pipeline
marker and, by default, a trace footer (`footer: false` disables the visible
footer). `hide-older-comments: true` first minimizes older comments carrying
the same pipeline marker **and** authored by the authenticated actor. It never
trusts agent-authored marker text. With PAT authentication, Stage 3 resolves
the actor through GitHub's `GET /user` endpoint. With
`safe-outputs.github-app`, the JWT-authenticated token-mint step captures the
installation's App slug and passes the derived `<slug>[bot]` login to Stage 3;
installation access tokens are not sent to `/user` or `/installation` for
actor discovery. Actor resolution, older-comment discovery, and minimization
remain fail-closed and complete before the replacement comment is posted.
`allowed-reasons` restricts minimization reasons; supported reasons are
`SPAM`, `ABUSE`, `OFF_TOPIC`, `OUTDATED` (default), `RESOLVED`, and
`LOW_QUALITY`.

`hide-github-issue-comment` accepts either a numeric REST comment ID or a
GraphQL node ID. It resolves the owning issue, PR, or discussion and applies
repository/filter policy before calling `minimizeComment`. An omitted `reason`
uses `OUTDATED`.

#### Labels

Both label tools use gh-aw-compatible glob patterns. `blocked` is evaluated
first and always wins. When `allowed` is omitted, remaining labels are
unrestricted; an explicit allowlist narrows them. Removal treats a label that
is already absent as success.

#### Closing and updating

`close-github-issue` defaults to `completed`; the other state reasons are
`not_planned` and `duplicate`. Configure a fixed `state-reason`, or
`allowed-state-reason` to let the agent choose from a bounded set.
`allow-body` defaults to `true`; when false, no closing comment is posted.
`duplicate_of` is validated against repository policy before any comment or
close, then creates the native duplicate relationship. Already-closed targets
are idempotent success.

`update-github-issue` requires at least one of `status`, `title`, `body`,
`labels`, `assignees`, or `milestone`. Every mutable field is independently
disabled by default: the operator must set the matching `status`, `title`,
`body`, `labels`, `assignees`, or `milestone` configuration flag to `true`.
For example, enabling all fields explicitly looks like:

```yaml
safe-outputs:
  update-github-issue:
    target-repo: octo-org/octo-repo
    status: true
    title: true
    body: true
    labels: true
    assignees: true
    milestone: true
    allowed-labels: [bug, "agent-*"]
```

`allowed-labels` additionally bounds replacement labels when non-empty; an
empty or omitted allowlist permits any label. Both issue and pull-request
targets are permitted by default (`issues: true` and `pull-requests: true`).
Set either switch to `false` to restrict the target kind; at least one must
remain enabled.

Body `operation` modes are:

- `append` *(default)* - add the new content after the current body, separated
  by a horizontal rule.
- `prepend` - add the new content before the current body, separated by a
  horizontal rule.
- `replace` - replace the entire current body.
- `replace-island` - replace only the single ado-aw status island for the
  current pipeline definition; missing, duplicate, or out-of-order markers
  fail the update.

Body updates include the standard trace footer by default; set `footer: false`
to omit it. Status is `open` or `closed`. `labels` and `assignees` replace
their complete existing lists, and `milestone` selects an existing milestone
by positive number. All requested changes are preflighted before the first
write.

#### Fields, milestones, and assignees

`set-github-issue-field` rejects built-in fields and limits repository-defined
fields with `allowed-fields`. It discovers field metadata, then coerces
single-select, number, date, or text values. Repository/API versions without
the issue-field GraphQL feature fail explicitly.

`assign-github-issue-milestone` resolves milestones by number or exact title
with pagination. `allowed` restricts titles. With `auto-create: true`, a
missing allowed milestone is created before assignment; otherwise the
assignment fails without creating it.

Assignment tools accept one `assignee` or an `assignees` list, deduplicate
usernames, and apply `blocked` before `allowed` glob policy. Setting
`unassign-first: true` clears existing assignees before adding the approved
set. Removing an already-absent assignee is idempotent success.

#### Sub-issues

`link-github-sub-issue` requires distinct parent and child issues in the same
repository. Both targets are resolved and checked against their respective
label/title filters before the GraphQL mutation. An existing parent
relationship is handled idempotently; a child already linked to a different
parent fails without changing either issue.

### comment-on-work-item
Adds a comment to an Azure DevOps work item. This is the ADO equivalent of gh-aw's `add-comment` tool.

**Agent parameters:**
- `work_item_id` - A positive numeric work-item ID, or a temporary ID (`#aw_...`) returned by an earlier `create-work-item` call in the same run (required)
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

`target` scopes numeric, pre-existing work-item IDs only, and is still required
in front matter. Temporary IDs are resolved at Stage 3 against the
`create-work-item` proposals that already succeeded in the same SafeOutputs
job — that create is scoped by its own configuration — so they are not checked
against `target`. A temporary ID that cannot be traced to such a create is
rejected before any request is sent. When both tools are configured they must
have the same effective `require-approval` setting, so temporary-ID state stays
within a single SafeOutputs job.

```json
{"title":"Investigate build failure","description":"Detailed failure report long enough for validation."}
{"temporary_id":"#aw_a1b2c3d4"}
{"work_item_id":"#aw_a1b2c3d4","body":"Root cause analysis for the failure above."}
```

### create-work-item
Creates an Azure DevOps work item. The agent-provided description is written as
Markdown, and the executor sets `multilineFieldsFormat` to `Markdown` for the
field that receives the body.

**Agent parameters:**
- `title` - A concise title for the work item (required, must be more than 5 characters)
- `description` - Work item description in Markdown format (required, must be more than 30 characters). Markdown is preserved; see [Markdown body sanitization](#markdown-body-sanitization) for the HTML and URL policy applied to it.
- `tags` - Tags to apply to the work item (optional list; each tag must not contain a semicolon). May be subject to the `allowed-tags` allowlist. Merged with any static `tags` configured in front matter.

On success, the MCP tool returns a generated gh-aw-compatible `#aw_...`
`temporary_id` in both structured output and its text response. Agents do not
choose this ID; they use the returned value in later safe-output calls.

**Configuration options (front matter):**
- `work-item-type` - Work item type (default: "Task")
- `description-field` - Field reference name that receives the agent-provided description. Defaults to `Microsoft.VSTS.TCM.ReproSteps` for `Bug` work items and `System.Description` for all other work item types.
- `area-path` - Area path for the work item
- `iteration-path` - Iteration path for the work item
- `assignee` - Static user to assign (email, UPN, or display name). When omitted, the work item is created unassigned.
- `tags` - Static list of tags always applied to the work item (regardless of agent input)
- `allowed-tags` - Allowlist of tags the agent is permitted to use via the `tags` parameter. If empty, any agent-provided tags are accepted. Supports `*` wildcards anywhere in the pattern (e.g., `"agent-*"` matches `"agent-created"`; `"copilot:repo=org/project/*@main"` matches any repo name).
- `custom-fields` - Map of custom field reference names to values (e.g., `Custom.MyField: "value"`)
- `max` - Maximum number of create-work-item outputs allowed per run (default: 1)
- `include-stats` - Whether to append agent execution stats to the work item description (default: true)
- `artifact-link` - Configuration for GitHub Copilot artifact linking:
  - `enabled` - Whether to add an artifact link (default: false)
  - `repository` - Repository name override (defaults to BUILD_REPOSITORY_NAME)
  - `branch` - Branch name to link to (default: "main")

### assign-work-item

Assigns one Azure DevOps identity to a work item. Use this separately from
`create-work-item` when the agent should choose ownership.

```yaml
safe-outputs:
  require-approval: true
  create-work-item: {}
  assign-work-item:
    target: "*"
    allowed: [alice@example.com, bob@example.com]
    blocked: ["svc-*"]
    max: 3
```

The agent can create and then assign an item in proposal order. The first tool
call returns the temporary ID used by the second:

```json
{"title":"Investigate build failure","description":"Detailed failure report long enough for validation."}
{"temporary_id":"#aw_a1b2c3d4"}
{"work_item_id":"#aw_a1b2c3d4","assignee":"alice@example.com"}
```

**Agent parameters:**
- `work_item_id` - A positive numeric work-item ID or a temporary ID from an earlier successful `create-work-item`.
- `assignee` - The single ADO identity to assign.

**Configuration options:**
- `target` - Scope for numeric, pre-existing work-item IDs: `"*"` or one exact positive ID. Temporary IDs created in the current run do not require `target`.
- `allowed` - Optional case-insensitive exact identity allowlist. Empty/absent permits any identity.
- `blocked` - Optional case-insensitive wildcard blocklist.
- `max` - Maximum assignments per run (default: 1).

When both create and assign are configured, they must have the same effective
`require-approval` setting so temporary-ID state remains in one SafeOutputs
job. Unresolved, duplicate, reversed, or failed-create references fail before
assignment.

`Agency` and `GitHub Copilot` are reserved non-assignable identities. They are
rejected case-insensitively in static `create-work-item.assignee`,
`assign-work-item.assignee`, and `update-work-item.assignee`; configuration
cannot override this rule. ADO performs final organization-specific identity
resolution, so ado-aw does not require an email-shaped value.

### update-work-item
Updates an existing Azure DevOps work item. Each field that can be modified requires explicit opt-in via configuration to prevent unintended updates.

**Agent parameters:**
- `id` - Work item ID to update (required) - a positive numeric ID, or a temporary ID (`#aw_...`) returned by an earlier `create-work-item` call in the same run
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
The reserved identities `Agency` and `GitHub Copilot` cannot be assigned.

### create-pull-request
Creates a pull request with code changes made by the agent. When invoked:
1. Generates a patch file from `git diff` capturing all changes in the specified repository
2. Saves the patch to the safe outputs directory
3. Creates a JSON record with PR metadata (title, description, source branch, repository)

During Stage 3 execution, the repository is validated against the allowed list
(from `checkout:` + "self"), resolved to an exact
organization/project/repository target, then the patch is applied and a PR is
created in Azure DevOps.

> **Cross-organization repositories.** `create-pull-request`,
> `create-branch`, and `create-git-tag` can target a checked-out Azure Repos
> repository in another organization when its `repos:` object declares
> `organization:` plus `endpoint:`, and expanded `permissions.write` uses
> `connection-type: azureDevOps` with an exact organization/project/repository
> allow scope. The Stage 3 token is never exposed to the Agent. Missing routing,
> connection type, or scope produces a compile warning and a target-time
> rejection, including under `--dry-run`; it never falls back to the pipeline
> organization.

**Shallow-clone agent pools (automatic):** The diff base is computed at agent
time from the checked-out repository. For Azure Repos,
`prepare-pr-base.js` asks the ADO Diffs API for the exact `commonCommit`,
`aheadCount`, and `behindCount`, then fetches only the source and target ranges
needed to make that base locally reachable. Cross-org preparation runs in a
trusted AzureCLI@3 task and passes its short-lived bearer only to the bundle
child process; the credential is not persisted or exposed to the Agent. It
verifies the server result with
`git merge-base --all` before the containerized SafeOutputs MCP server can
generate a patch. Non-Azure/unavailable-REST cases use bounded
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
- `source_id` - Source work item ID (required) - a positive numeric ID, or a temporary ID (`#aw_...`) returned by an earlier `create-work-item` call in the same run
- `target_id` - Target work item ID (required, must differ from source) - a positive numeric ID, or a temporary ID (`#aw_...`) returned by an earlier `create-work-item` call in the same run
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

Cross-organization tag creation uses the same `repos.organization` and
expanded `permissions.write` contract described under
[`create-pull-request`](#create-pull-request).

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

Cross-organization branch creation uses the same `repos.organization` and
expanded `permissions.write` contract described under
[`create-pull-request`](#create-pull-request).

### upload-workitem-attachment
Uploads a workspace file as an attachment to an Azure DevOps work item.

**Agent parameters:**
- `work_item_id` - Work item ID to attach the file to (required) - a positive numeric ID, or a temporary ID (`#aw_...`) returned by an earlier `create-work-item` call in the same run
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
- Requires `BUILD_CONTAINERID`, `BUILD_BUILDID`, and `SYSTEM_TEAMPROJECTID` (all set automatically inside an Azure DevOps pipeline job) and `vso.build_execute` scope on the executor's token (granted to `$(System.AccessToken)` by default, and to the configured service-connection token when `permissions.write` is set).

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
