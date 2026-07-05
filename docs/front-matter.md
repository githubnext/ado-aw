# Agent File Format (Markdown + YAML Front Matter)

_Part of the [ado-aw documentation](../AGENTS.md)._

## Input Format (Markdown with Front Matter)

The compiler expects markdown files with YAML front matter similar to gh-aw:

```markdown
---
name: "name for this agent"
description: "One line description for this agent"
target: standalone # Optional: "standalone" (default), "1es", "job", or "stage". See docs/targets.md.
engine: copilot # Engine identifier. Defaults to copilot. Currently only 'copilot' (GitHub Copilot CLI) is supported.
# engine:                        # Alternative object format (with additional options)
#   id: copilot
#   model: claude-opus-4.7
#   timeout-minutes: 30
workspace: repo # Optional: "root", "repo" (alias: "self"), or a checked-out repository alias. If not specified, defaults to "root" when no additional repositories are listed in `repos:`, and to "repo" when one or more additional repos are checked out. See "Workspace Defaults" below.
pool:                          # Optional pool configuration
  vmImage: ubuntu-22.04        # Microsoft-hosted (default for non-1ES targets)
# pool:                        # Self-hosted pool
#   name: MySelfHostedPool
# pool:                        # 1ES pool format
#   name: AZS-1ES-L-MMS-ubuntu-22.04
#   os: linux                  # Operating system: "linux" or "windows". Defaults to "linux".
repos:                           # compact repository declarations (replaces repositories: + checkout:)
  - my-org/my-repo               # shorthand: alias="my-repo", type=git, ref=refs/heads/main, checkout=true
  - reponame=my-org/another-repo # shorthand with explicit alias
  - name: my-org/templates       # object form for full control
    ref: refs/heads/release/2.x
    checkout: false              # declared as resource only, not checked out by the agent
tools:                         # optional tool configuration
  bash: ["cat", "ls", "grep"]  # explicit bash allow-list; when omitted, all bash tools are allowed (unrestricted)
  edit: true                   # enable file editing tool (default: true)
  cache-memory: true           # persistent memory across runs (see docs/tools.md)
  # cache-memory:              # Alternative object format (with options)
  #   allowed-extensions: [.md, .json]
  azure-devops: true           # first-class ADO MCP integration (see docs/tools.md)
  # azure-devops:              # Alternative object format (with scoping)
  #   toolsets: [repos, wit]
  #   allowed: [wit_get_work_item]
  #   org: myorg
runtimes:                      # optional runtime configuration (language environments)
  lean: true                   # Lean 4 theorem prover (see docs/runtimes.md)
  # lean:                      # Alternative object format (with toolchain pinning)
  #   toolchain: "leanprover/lean4:v4.29.1"
  # python: true               # Python runtime — auto-installs via UsePythonVersion@0 (see docs/runtimes.md)
  # python:                    # Alternative object format (pin version, configure internal feed)
  #   version: "3.12"
  #   feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"
  # node: true                 # Node.js runtime — auto-installs via UseNode@1 (see docs/runtimes.md)
  # node:                      # Alternative object format (pin version, configure internal feed)
  #   version: "22.x"
  #   feed-url: "https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/"
  # dotnet: true               # .NET runtime — auto-installs via UseDotNet@2 (see docs/runtimes.md)
  # dotnet:                    # Alternative object format (pin version, configure internal feed via nuget.config)
  #   version: "8.0.x"          # use "global.json" to pin from the repo's global.json
  #   feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json"
# env:                         # workflow-level environment variables (accepted by parser, not yet forwarded to compiled pipeline output)
#   CUSTOM_VAR: "value"
# inlined-imports: false        # When true, resolve {{#runtime-import ...}} markers at compile time
#                               # (default: false — markers are resolved at pipeline runtime, so
#                               # prompt-body edits do not require recompilation).
#                               # See docs/runtime-imports.md for full details.
mcp-servers:
  my-custom-tool:              # containerized MCP server (requires container field)
    enabled: true
    container: "node:20-slim"
    entrypoint: "node"
    entrypoint-args: ["path/to/mcp-server.js"]
    args: ["--pull=always"]     # Docker runtime args inserted before the image
    mounts: ["$(Build.SourcesDirectory):/workspace:ro"]
    env:
      CUSTOM_TOKEN: ""          # empty string = pass through from pipeline env
    allowed:
      - custom_function_1
      - custom_function_2
  remote-tool:                  # HTTP MCP server (see docs/mcp.md)
    url: "https://mcp.example.com"
    headers:
      Authorization: "Bearer $(MCP_TOKEN)"
    allowed: [search, fetch]
safe-outputs:                  # optional per-tool configuration for safe outputs
  create-work-item:
    work-item-type: Task
    assignee: "user@example.com"
    tags:
      - automated
      - agent-created
    artifact-link:             # optional: link work item to repository branch
      enabled: true
      branch: main
on:                            # trigger configuration (unified under on: key)
  schedule: daily around 14:00 # fuzzy schedule - see docs/schedule-syntax.md
  pipeline:
    name: "Build Pipeline"     # source pipeline name
    project: "OtherProject"    # optional: project name if different
    branches:                  # optional: branches to trigger on
      - main
      - release/*
    filters:                   # optional runtime filters (compiled to gate step)
      source-pipeline: "Build*"
      branch: "refs/heads/main"  # triggering branch (Build.SourceBranch)
      time-window:
        start: "09:00"
        end: "17:00"
      build-reason:
        include: [IndividualCI]
        exclude: [Schedule]
      expression: "eq(variables['Custom.Flag'], 'true')"  # raw ADO condition
  pr:                          # PR trigger
    branches:
      include: [main]
    paths:
      include: [src/*]
    mode: synthetic            # synthetic (default) | policy. Controls how
                               # `on.pr` builds reach the pipeline.
                               #   - synthetic: a Setup-job script calls the
                               #     ADO REST API on every CI build, finds the
                               #     open PR for `Build.SourceBranch`, and
                               #     promotes the build to PR semantics if it
                               #     matches `branches`/`paths`. No Build
                               #     Validation branch policy required. Zero
                               #     or multiple matches → Agent job
                               #     self-skips cleanly. CI trigger stays at
                               #     the ADO default (all branches).
                               #   - policy: the operator has installed a
                               #     Build Validation branch policy. Compiler
                               #     omits all synth wiring AND emits
                               #     `trigger: none` so feature-branch pushes
                               #     do not queue duplicate CI builds. Real
                               #     PR-typed builds drive everything.
                               # See "PR Triggering in Azure Repos" below.
    filters:                   # runtime PR filters (compiled to gate step)
      title: "*[review]*"
      author:
        include: ["alice@corp.com"]
      draft: false
      labels:
        any-of: ["run-agent"]
      source-branch: "feature/*"
      target-branch: "main"
      commit-message: "*[skip-agent]*"
      changed-files:
        include: ["src/**/*.rs"]
      min-changes: 5
      max-changes: 100
      time-window:
        start: "09:00"
        end: "17:00"
      build-reason:
        include: [PullRequest]
      expression: "eq(variables['Custom.Flag'], 'true')"  # raw ADO condition
execution-context:             # optional execution-context plugin (see docs/execution-context.md)
  enabled: true                # master switch; defaults to true. Set false to disable globally.
  pr:                          # PR-context contributor. Activates on PR-triggered builds when on.pr is set.
    enabled: true              # defaults to true when on.pr is configured. Set false to opt out
                               # (also suppresses auto-adding the read-only git commands to the
                               # agent's bash allow-list).
    checks:
      enabled: false            # OPT-IN: include PR Build Validation check results (default off)
  manual:
    enabled: true               # defaults to true when parameters are declared
    include-email: false
  pipeline:
    enabled: true               # defaults to true when on.pipeline is configured
  ci-push:
    enabled: false              # opt-in "since last green build" CI/push context
  workitem:
    enabled: true               # defaults to true with PR context
    max-items: 5
    max-body-kb: 32
  schedule:
    enabled: false              # OPT-IN: "since last run" diff context for scheduled builds (requires on.schedule)
  repo:
    enabled: false              # opt-in repository identity context
    conventions: false          # opt-in deeper probe (CODEOWNERS / CONTRIBUTING.md / .editorconfig)
steps:                         # inline steps before agent runs (same job, generate context)
  - bash: echo "Preparing context for agent"
    displayName: "Prepare context"
post-steps:                    # inline steps after agent runs (same job, process artifacts)
  - bash: echo "Processing agent outputs"
    displayName: "Post-steps"
setup:                         # separate job BEFORE agentic task
  - bash: echo "Setup job step"
    displayName: "Setup step"
teardown:                      # separate job AFTER safe outputs processing
  - bash: echo "Teardown job step"
    displayName: "Teardown step"
network:                       # optional network policy (standalone target only)
  allowed:                       # allowed host patterns and/or ecosystem identifiers
    - python                   # ecosystem identifier — expands to Python/PyPI domains
    - "*.mycompany.com"        # raw domain pattern
  blocked:                     # blocked host patterns or ecosystems (removes from allow list)
    - "evil.example.com"
permissions:                   # optional ADO access token configuration
  read: my-read-arm-connection   # ARM service connection for read-only ADO access (Stage 1 agent)
  write: my-write-arm-connection # OPTIONAL ARM SC for Stage 3 executor writes.
                                 # Default: executor uses $(System.AccessToken).
                                 # Set this only for cross-org writes or
                                 # named-identity attribution.
supply-chain:                  # optional internal supply-chain mirror (see docs/supply-chain.md)
  feed:                          # mirror binaries (compiler, AWF, ado-script) from an ADO Artifacts feed
    name: my-project/my-feed     # feed name or project/feed; scalar `feed: my-feed` shorthand also works
    service-connection: feed-conn  # optional; omit for same-org feeds (uses $(System.AccessToken))
  registry:                      # mirror AWF/MCPG images from an internal ACR
    name: myacr.azurecr.io/mirror  # registry host or base path (artifact names kept under it)
    service-connection: acr-conn   # REQUIRED when registry is set (ACR has no System.AccessToken path)
  service-connection: shared-conn  # optional shared fallback for whichever target omits its own
# ado-aw-debug:                 # debug-only knobs; see docs/ado-aw-debug.md
#   skip-integrity: false       # omit generated pipeline integrity verification
#   create-issue: false         # dogfood-only GitHub issue filing for debug reports
parameters:                    # optional ADO runtime parameters (surfaced in UI when queuing a run)
  - name: clearMemory
    displayName: "Clear agent memory"
    type: boolean
    default: false
---

## Task

Describe the agent's task here. This markdown body is read by the AI agent at
runtime — write it as clear, structured natural-language instructions.
```

> **Conclusion job**: when `safe-outputs:` is configured the compiler
> automatically emits an always-running **Conclusion** job that files a work-item
> report on pipeline failures and surfaces diagnostic signals. See
> [`docs/conclusion.md`](conclusion.md).

## Inline step validation (`setup` / `steps` / `post-steps` / `teardown`)

Inline steps are authored as raw Azure DevOps YAML and are emitted into the
generated pipeline **verbatim** (a passthrough). For steps that invoke a
built-in ADO task the compiler also knows (e.g. `CopyFiles@2`, `Docker@2`,
`DotNetCoreCLI@2`, and most other first-party tasks), `ado-aw lint` performs an
**advisory** validation of the `inputs:` mapping against the task's typed
schema — checking for missing required inputs, unknown input keys, bad
constrained values, and (for command/mode tasks) inputs supplied for the wrong
command.

This validation is surfaced through the **lint** channel, not compile:

- Run `ado-aw lint <agent.md>` (add `--json` for machine-readable findings), or
  call the `lint_workflow` tool on the author-facing MCP server. Each invalid
  step produces a `task-input-invalid` **warning** finding with the offending
  task id and which step list it came from.
- It is **warning-only**: findings never fail `lint` (exit code stays 0) and
  never affect `compile` or the emitted YAML — the step is always passed through
  unchanged.
- A task the compiler does not model, or a non-task step (`bash:`/`script:`/
  `checkout:`), produces no finding.

Surfacing this through lint (rather than as a compile-time stderr warning) keeps
the feedback in the structured channel that authoring agents already consume to
check the steps they synthesise, and keeps the in-pipeline integrity recompile
quiet. So adding validation coverage can only ever *surface* authoring mistakes
— it never rejects a workflow that compiled before. See [`ir.md`](ir.md)
(`tasks/parse.rs`) for the mechanism and how to extend coverage.

## Debug-only `ado-aw-debug:`

`ado-aw-debug:` is accepted in front matter for repository dogfooding and
local diagnostics. It is **not** a regular safe-output tool. Use
`skip-integrity` to omit generated pipeline integrity verification, or
`create-issue` to file a GitHub issue from debug pipelines; see
[`ado-aw-debug.md`](ado-aw-debug.md) for the full reference.

## Workspace Defaults

The `workspace:` field controls which directory the agent runs in. When it is
not set explicitly, the compiler chooses a default based on which repositories
are checked out (entries in `repos:` with `checkout: true`, which is the
default):

- If no additional repositories are checked out (i.e. only the pipeline's own
  repository is checked out via the implicit `self`), `workspace:` defaults to
  **`root`** — the agent runs in the pipeline's working directory root.
- If one or more additional repositories are checked out, `workspace:` defaults
  to **`repo`** — the agent runs inside the trigger repository's directory.

Set `workspace:` explicitly to `root`, `repo` (alias `self`), or a specific
checked-out repository alias to override this behavior.

### Deprecated directory markers

Earlier releases substituted the directory markers `{{ workspace }}`,
`{{ working_directory }}`, and `{{ trigger_repo_directory }}` inside custom
`steps:` / `post-steps:` / `setup:` / `teardown:` blocks. These are
**deprecated** — they encouraged hard-coding a fixed path anchor, which is
incorrect under multi-checkout where `$(Build.SourcesDirectory)` is the shared
root of every checked-out repository.

Reference the explicit ADO path instead:

- `$(Build.SourcesDirectory)` — the checkout root (the trigger repo root when
  only `self` is checked out).
- `$(Build.SourcesDirectory)/$(Build.Repository.Name)` — the trigger repo when
  one or more additional repositories are checked out.
- `$(Build.SourcesDirectory)/<alias>` — a specific checked-out repository.

The `legacy_path_markers` codemod automatically rewrites any remaining markers
in front matter to the path they resolved to on the next `compile` (see
[`docs/codemods.md`](codemods.md)). Markers left in the **agent body** cannot be
migrated automatically and are reported as a compile warning. The compiler also
emits warning-only advisories when a `$(Build.SourcesDirectory)/<seg>` reference
or a `{{#runtime-import …}}` target points at a path that will not exist under
the resolved checkout layout.

## Repositories (`repos:`)

The `repos:` field provides a compact way to declare additional repository
resources and control which ones the agent checks out. It replaces the legacy
`repositories:` + `checkout:` pair.

Each entry can be:

| Form | Syntax | Description |
|------|--------|-------------|
| **Shorthand** | `- org/repo` | Alias derived from last segment, type=git, ref=refs/heads/main, checkout=true |
| **Shorthand with alias** | `- alias=org/repo` | Explicit alias before `=` |
| **Object** | `- name: org/repo` | Full control over all fields |

Object fields:

| Field         | Default                | Description |
|---------------|------------------------|-------------|
| `name`        | *(required)*           | Full `org/repo` name (maps to ADO `name:`) |
| `alias`       | last segment of `name` | Repository alias (maps to ADO `repository:`) |
| `type`        | `git`                  | ADO repository resource type |
| `ref`         | `refs/heads/main`      | Branch or tag reference |
| `checkout`    | `true`                 | Whether the agent job clones this repo |
| `fetch-depth` | *(ADO default)*        | Shallow-clone depth for this repo's checkout (ADO `fetchDepth`). `0` = full history |
| `fetch-tags`  | *(ADO default)*        | Whether to fetch git tags during checkout (ADO `fetchTags`) |

### Tuning checkout fetch behavior (`fetch-depth` / `fetch-tags`)

On large monorepos the checkout step can dominate the run: ADO's default
`checkout` performs a full-history clone **and** `git fetch --tags`, which may
take tens of minutes. `fetch-depth` and `fetch-tags` let you tune this
per-repository:

```yaml
repos:
  - name: my-org/monorepo
    fetch-depth: 1      # shallow — only the tip commit
    fetch-tags: false   # skip the (often huge) tag fetch
```

- `fetch-depth: 0` means **full history** (no `fetchDepth` is emitted).
- When a field is omitted the ADO default applies, so agents that don't set
  these compile **unchanged**.
- Setting `fetch-depth`/`fetch-tags` on an entry with `checkout: false` has no
  effect (no checkout step is emitted for it); the compiler emits a warning.

#### Tuning the trigger repository (`self`)

The trigger repository is always checked out as `checkout: self` and is not
otherwise a `repos:` entry. To tune its fetch behavior, add a reserved entry
whose `name` is exactly `self`:

```yaml
repos:
  - name: self
    fetch-depth: 1
    fetch-tags: false
```

A `self` entry contributes **only** fetch tuning — it does not declare an extra
repository resource or an additional checkout. The tuning is applied to the
`checkout: self` step in every job (Setup, Agent, Detection, SafeOutputs,
Teardown). Because the tuning comes from source, the compiled lock stays in
sync and the runtime **"Verify pipeline integrity"** step keeps passing — no
need to hand-edit the lock or set `ado-aw-debug.skip-integrity`.

A `self` entry accepts only `fetch-depth` and `fetch-tags`; setting any other
field (`alias`, `type`, `ref`, `checkout`) on it is rejected at compile time.
A bare `self` entry with no fetch fields (e.g. `- name: self` or the `- self`
shorthand) is a harmless no-op — it changes nothing.

> `persistCredentials` is intentionally not exposed on `self`; see
> [`docs/execution-context.md`](execution-context.md) for the trust-boundary
> rationale.

### Examples

Three repos, all checked out (most common case):

```yaml
repos:
  - my-org/tools
  - my-org/schemas
  - my-org/docs
```

Mixed: two checked out, one resource-only (used by templates):

```yaml
repos:
  - my-org/tools
  - my-org/schemas
  - name: my-org/pipeline-templates
    checkout: false
```

Custom ref and explicit alias:

```yaml
repos:
  - name: my-org/docs
    alias: docs-v2
    ref: refs/heads/release/2.x
```

### Legacy syntax (auto-rewritten)

The legacy `repositories:` + `checkout:` fields are auto-converted to
`repos:` by the [`repos_unified` codemod](codemods.md). On the next
`ado-aw compile`, any source that still uses the legacy fields is
rewritten in place to the new shape — each `repositories:` entry
becomes a `repos:` entry, with `checkout: false` added for entries
that weren't listed under `checkout:`. Mixing the legacy fields with
an existing `repos:` block is rejected; pick one shape.

## Inlined Imports

The `inlined-imports:` field controls when `{{#runtime-import ...}}`
markers in the markdown body are resolved. It defaults to `false`.
See [`runtime-imports.md`](runtime-imports.md) for the full marker
syntax, path resolution rules, and runtime behavior.

When `inlined-imports: false`, the compiler leaves runtime-import
markers to be resolved on the pipeline runner. This is the default
behavior, and it means prompt-body edits do not require recompiling the
generated YAML.

When `inlined-imports: true`, the compiler resolves all runtime-import
markers at compile time, including the implicit top-level marker that
normally reloads the body itself. The emitted YAML contains the fully
expanded prompt body, so the pipeline file is self-contained.

The trade-off is that the generated YAML is larger, and prompt-body
edits require `ado-aw compile` plus committing the updated pipeline
file.

A small, fixed set of ADO path-anchor variables —
`$(Build.SourcesDirectory)` and `$(Build.Repository.Name)` — is
substituted into the prompt consistently in **both** modes. Arbitrary
`$(...)` macros and pipeline/secret variables are not expanded; see
[ADO variables in the prompt](runtime-imports.md#ado-variables-in-the-prompt).

## Filter Validation

The compiler validates filter configurations at compile time and will emit
errors for impossible or conflicting combinations:

| Condition | Severity | Message |
|-----------|----------|---------|
| `min-changes` > `max-changes` | Error | No PR can satisfy both constraints |
| `time-window.start` = `time-window.end` | Error | Zero-width window never matches |
| Same value in `author.include` and `author.exclude` | Error | Conflicting include/exclude |
| Same value in `build-reason.include` and `build-reason.exclude` | Error | Conflicting include/exclude *(both PR and pipeline filters)* |
| Label in both `labels.any-of` and `labels.none-of` | Error | Label both required and blocked |
| Label in both `labels.all-of` and `labels.none-of` | Error | Label both required and blocked |
| Empty `labels` filter (no any-of/all-of/none-of) | Warning | No label checks applied |

Errors cause compilation to fail. Fix the conflicting filter configuration
before recompiling.

## Filter Behavior Notes

### Time Windows

Time windows use **half-open intervals**: `[start, end)`. A window of
`start: "09:00", end: "17:00"` matches from 09:00 up to but **not
including** 17:00. A build triggered at exactly 17:00 UTC will not match.

Overnight windows are supported: `start: "22:00", end: "06:00"` matches
from 22:00 through midnight to 05:59.

All times are evaluated in **UTC**.

### Changed Files

The `changed-files` filter checks the list of files modified in the PR.
If the PR has no changed files (empty diff) and an `include` pattern is
set, the filter will not match. An exclude-only filter (no `include`)
with no changed files passes vacuously (no excluded files are present).

### Expression Escape Hatch

The `expression` field on `pr.filters` and `pipeline.filters` is an
**advanced, unsafe escape hatch**. Its value is inserted verbatim into
the Agent job's ADO `condition:` field. It can reference any ADO
pipeline variable, including secrets. The compiler validates against
`##vso[` injection and ADO compile-time template expressions (`${{`), but otherwise trusts the
value. Only use this if the built-in filters are insufficient.

### Pipeline Requirements

The filter gate step uses `System.AccessToken` for self-cancellation
(PATCH to the builds REST API) and PR metadata retrieval. This requires:

1. **"Allow scripts to access the OAuth token"** must be enabled on the
   pipeline definition in ADO (Project Settings → Pipelines → Settings).
2. The pipeline's build service account must have permission to cancel
   builds.

If the token is unavailable, the gate step logs a warning and the build
completes as "Succeeded" (with the agent job skipped via condition)
rather than "Cancelled".

## PR Triggering in Azure Repos

Azure DevOps Services **ignores the YAML `pr:` block unless a per-branch
Build Validation branch policy is registered server-side**. Without that
policy, a `git push` to a feature branch fires the compiled pipeline as
`Build.Reason = IndividualCI` even when an open PR exists — the gate
evaluator's "not a PR build" bypass triggers and `exec-context-pr.js`
is skipped. PR-aware agents (e.g. PR reviewers) silently degrade.

`ado-aw` lets the agent author pick one of two coherent strategies via
`on.pr.mode`:

| `on.pr.mode` | Synthesis wiring | Top-level `trigger:` | Use when |
|---|---|---|---|
| `synthetic` (default) | emitted (synthPr Setup step, coalesced env, broadened conditions) | ADO default (all branches) | No branch policy. **The vast majority of agents.** |
| `policy` | omitted | `trigger: none` | Operator has installed a Build Validation branch policy and wants real PR-typed builds only, no duplicate CI builds. |

### `mode: synthetic` — how it works under the hood

On every CI build:

1. **Real PR build?** If `Build.Reason == PullRequest` (a branch policy
   is configured), the synth step no-ops and the existing PR path
   handles everything.
2. **GitHub-typed repo resource?** GitHub repos already get correct
   `pr:` semantics from ADO. The synth step no-ops.
3. **Look up the PR.** Otherwise, the script calls
   `GET /{project}/_apis/git/repositories/{repoId}/pullrequests`
   filtered by `sourceRefName == Build.SourceBranch` and
   `status = active`.
4. **Filter by target branch.** PRs whose `targetRefName` does not match
   `on.pr.branches.include` (respecting `exclude`) are dropped.
5. **Exactly one match.** Zero or multiple matches → emit
   `AW_SYNTHETIC_PR_SKIP=true`; the Agent job self-skips cleanly with a
   single info log line. Never noisy, never red.
6. **Path filter.** If `on.pr.paths` is configured, the script enforces
   it against the PR's changed-file list (which ADO's CI trigger
   ignores). Empty intersection → skip.
7. **Promote.** Otherwise, emit `AW_SYNTHETIC_PR=true` plus the PR
   identifiers as Setup-job outputs. Downstream `gate.js` and
   `exec-context-pr.js` env blocks coalesce these with the real
   `System.PullRequest.*` variables, so the gate evaluator runs the
   full PR-spec predicates and `aw-context/pr/{base.sha,head.sha}` is
   staged for the agent.

### Why the CI trigger is not auto-narrowed in `mode: synthetic`

`pr.branches.include` lists PR **target** branches (e.g. `main`), but
ADO `trigger:` fires on pushes **to** the listed branches. Narrowing
`trigger:` to `pr.branches.include` would suppress CI on the feature
branches synthPr actually needs to react to (pushing to `feature/x`
with an open PR `feature/x → main` would never queue a build). The
compiler therefore leaves the top-level `trigger:` at the ADO default
("trigger on every branch") in synth mode, and relies on the synthPr
Setup step's fast-exit for cost control: a single
`listActivePullRequestsBySourceRef` call returns `[]` on branches
without a matching PR and the Agent job self-skips cleanly via
`AW_SYNTHETIC_PR_SKIP=true`.

### `mode: policy` — when to choose it

Choose `mode: policy` when the operator has explicitly installed an
Azure DevOps Build Validation branch policy targeting the compiled
pipeline. In this mode the compiler:

- Omits all synth wiring (`synthPr` step, `PR_SYNTH_SPEC` env,
  `AW_SYNTHETIC_PR_SKIP` guard, coalesced env macros, broadened
  `exec-context-pr.js` condition).
- Emits `trigger: none` so feature-branch pushes do not queue
  duplicate CI builds alongside the policy-driven PR build.

Result: every PR update fires exactly one PR-typed build (`Build.Reason
== PullRequest`); commit-driven CI is fully silenced.
