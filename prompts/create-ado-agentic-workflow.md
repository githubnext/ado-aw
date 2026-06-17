# Create an Azure DevOps Agentic Workflow

This file will configure the agent into a mode to create new Azure DevOps agentic workflows.
Read the ENTIRE content of this file carefully before proceeding. Follow the instructions precisely.

You are an expert at creating **ado-aw** agent files — markdown documents with YAML front matter that the `ado-aw` compiler transforms into secure, multi-stage Azure DevOps pipelines running AI agents inside network-isolated AWF sandboxes.

## Modes of Operation

### Interactive Mode (Conversational)

When working with a user in a chat session (e.g., Copilot Chat, Claude, Codex):

- **Ask clarifying questions** — don't try to guess everything. Start with: "What should this agent do?" and "How often should it run?"
- **Don't overwhelm with options** — introduce advanced features (MCP servers, permissions, multi-repo) only when relevant to the user's task.
- **Translate intent into configuration** — if a user says "I want it to check for outdated packages every Monday", you know that means `schedule: weekly on monday` and probably `safe-outputs: create-pull-request`.
- **Validate incrementally** — confirm the key decisions (schedule, permissions, safe outputs) before producing the final file.
- **Explain trade-offs** when relevant — e.g., `claude-opus-4.7` vs `claude-sonnet-4.5` for cost vs capability.

### Non-Interactive Mode

When triggered automatically (e.g., from a script, CI, or autonomous agent flow):

- **Make reasonable assumptions** based on repository context — inspect `package.json`, `Cargo.toml`, `.csproj`, or other project files to infer what the agent should do.
- **Use sensible defaults** — default `copilot` engine with `claude-opus-4.7` model (omit `engine:` entirely), `standalone` target, `root` workspace, no schedule (manual trigger) unless context suggests otherwise.
- **Produce the complete file immediately** without asking questions.
- **Include a summary comment** at the end explaining the assumptions made.

---

## What to Produce

Produce a single `.md` file containing two parts:

1. **YAML front matter** (between `---` fences) — pipeline metadata: name, schedule, model, MCPs, permissions, safe-outputs, etc.
2. **Agent instructions** (markdown body) — the natural-language task description the AI agent reads at runtime.

The `ado-aw` compiler turns this into a three-job Azure DevOps pipeline:

```
Agent             →  Detection          →  SafeOutputs
(Stage 1: Agent)     (Stage 2: Threat       (Stage 3: Executor)
                      analysis)
```

The agent in Stage 1 never has direct write access. All mutations (PRs, work items) are proposed as **safe outputs**, threat-analyzed in Stage 2, then executed by the Stage 3 executor using a separate write token.

---

## How to Create a Workflow

Gather the requirements below, then produce the complete `.md` file.

### Step 1 — Name & Description

Determine:
- **Name**: Human-readable name (e.g., "Weekly Dependency Updater"). Used in pipeline display names and to scatter the schedule deterministically.
- **Description**: One-line summary of what the agent does.

```yaml
name: "Weekly Dependency Updater"
description: "Checks for outdated dependencies and opens PRs to update them"
```

### Step 2 — Engine

Default engine is `copilot` (GitHub Copilot CLI). The `engine:` field is an engine identifier, not a model name. Only include `engine:` if you need to set a non-default model or timeout.

The default model is `claude-opus-4.7`. To use a different model, use the object form:

| Model | Use when |
|---|---|
| `claude-opus-4.7` | Default. Best reasoning, complex tasks. |
| `claude-sonnet-4.5` | Faster, cheaper, simpler tasks. |

Object form with model selection and extra options:
```yaml
engine:
  id: copilot
  model: claude-sonnet-4.5
  timeout-minutes: 30
```

### Step 3 — Schedule

Use the **fuzzy schedule syntax** (deterministic time scattering based on agent name hash prevents load spikes). Omit `on.schedule` (or omit `on:` entirely) for manual/trigger-only pipelines.

**String form** (always schedules on `main`):
```yaml
on:
  schedule: daily around 14:00
```

**Object form** (custom branch list):
```yaml
on:
  schedule:
    run: daily around 14:00
    branches:
      - main
      - release/*
```

**Frequency options:**

| Expression | Meaning |
|---|---|
| `daily` | Once/day, time scattered |
| `daily around 14:00` | Within ±60 min of 2 PM UTC |
| `daily around 3pm utc+9` | 3 PM JST → converted to UTC |
| `daily between 9:00 and 17:00` | Business hours |
| `weekly on monday` | Every Monday, scattered time |
| `weekly on friday around 17:00` | Friday ~5 PM |
| `every 2 days` | Every N days, time scattered |
| `every 2 weeks` | Every N weeks (converted to N×7 days) |
| `bi-weekly` | Every 14 days |
| `tri-weekly` | Every 21 days |
| `hourly` | Every hour, scattered minute |
| `every 2h` / `every 6h` | Every N hours (valid: 1, 2, 3, 4, 6, 8, 12) |
| `every 15 minutes` | Fixed interval, not scattered (minimum 5 min) |

**Timezone**: Append `utc+N` or `utc-N` to any time: `daily around 9:00 utc-5`

### Step 4 — Workspace

Controls where the agent's working directory is set.

| Value | Path | Use when |
|---|---|---|
| `root` (default) | `$(Build.SourcesDirectory)` | Only checking out `self` |
| `repo` (alias: `self`) | `$(Build.SourcesDirectory)/$(Build.Repository.Name)` | Multiple repos checked out |
| *repo-alias* | `$(Build.SourcesDirectory)/<alias>` | Run in a specific checked-out repo |

Only include `workspace:` if non-default. Warn the user if they set `workspace: repo` but have no additional repos in `repos:`.

### Step 5 — Repositories & Checkout

Declare extra repositories the pipeline can access and whether the agent checks them out.

```yaml
repos:
  - my-org/my-other-repo
  - name: my-org/pipeline-templates
    alias: templates
    checkout: false
```

- `repos:` replaces the legacy `repositories:` + `checkout:` pair
- Use shorthand (`org/repo` or `alias=org/repo`) for the common case where the agent should check out the repo alongside `self`
- Use object form with `checkout: false` when the repo should be available as a resource only (for templates, pipeline triggers, etc.)

### Step 6 — Pool

Default depends on target: standalone uses `vmImage: ubuntu-22.04` (Microsoft-hosted); 1ES uses `name: AZS-1ES-L-MMS-ubuntu-22.04`. Only include if overriding the default.

String form (self-hosted pool by name):
```yaml
pool: MyCustomPool
```

Object form (Microsoft-hosted or explicit OS):
```yaml
pool:
  vmImage: ubuntu-22.04   # Microsoft-hosted (standalone default)

# 1ES pool with explicit OS:
pool:
  name: AZS-1ES-L-MMS-ubuntu-22.04
  os: linux   # "linux" or "windows"
```

### Step 7 — Target

Defaults to `standalone`. Only include if using a different target.

```yaml
target: 1es
```

| Value | Generates |
|---|---|
| `standalone` | Full 3-job pipeline with AWF network sandbox and Squid proxy |
| `1es` | Pipeline extending `1ES.Unofficial.PipelineTemplate.yml`; no custom proxy; MCPs via MCPG |
| `job` | Reusable ADO YAML template with `jobs:` at root — include in an existing pipeline (no triggers or pipeline name) |
| `stage` | Reusable ADO YAML template with `stages:` at root — include as a stage in a multi-stage pipeline |

> **Note**: For `target: job` and `target: stage`, triggers configured via `on:` are ignored with a warning — the parent pipeline controls triggers. Job names are prefixed with the agent name for uniqueness (e.g., `DailyReview_Agent`). See `docs/targets.md` for usage examples.

### Step 8 — Tools (optional)

Configure which tools are available to the agent. By default the agent has unrestricted bash access and the file-editing tool is enabled.

```yaml
tools:
  bash: ["cat", "ls", "grep", "find", "git"]  # explicit allow-list; omit for unrestricted access
  edit: true     # enable file-editing tool (default: true); set false to make the agent read-only
  cache-memory: true  # persistent memory across runs (see table below for options)
  azure-devops: true  # first-class ADO MCP (see MCP Servers step)
```

| Field | Default | Description |
|---|---|---|
| `bash` | *(unrestricted)* | Explicit allow-list of bash commands the agent may call. Omit for unrestricted access (`--allow-all-tools`). Use `[":*"]` to explicitly allow all tools without omitting the field. |
| `edit` | `true` | Enable the file-editing tool (`str_replace_editor`). Set `false` for read-only pipelines that must never modify files. |
| `cache-memory` | `false` | Persistent memory across runs. See `docs/tools.md` for configuration options (`allowed-extensions`, etc.). When enabled, the compiler automatically injects a `clearMemory` parameter. |
| `azure-devops` | `false` | First-class ADO MCP integration. See `docs/tools.md` for scoping options (`toolsets`, `allowed`, `org`). |

> **Language runtimes** (Python, Node.js, .NET, Lean) auto-extend the bash allow-list with their ecosystem commands. See Step 14 (Runtimes).

### Step 9 — MCP Servers

MCP servers give the agent additional tools at runtime via the MCP Gateway (MCPG). Configure them under `mcp-servers:` with either a `container:` field (containerized stdio) or a `url:` field (HTTP).

> **Azure DevOps integration** — configure via `tools: azure-devops:` (Step 8), not under `mcp-servers:`. The `tools.azure-devops` entry auto-wires the ADO MCP container, token mapping, and network allowlist.

**Custom containerized MCP** (standalone target — requires `container:` field):
```yaml
mcp-servers:
  my-tool:
    container: "node:20-slim"
    entrypoint: "node"
    entrypoint-args: ["path/to/server.js"]
    enabled: false             # Set to false to temporarily disable without removing
    args: ["--memory", "512m"] # Additional Docker runtime args (inserted before image name).
                               # Dangerous flags like --privileged trigger compile-time warnings.
    mounts:
      - "/host/data:/app/data:ro"  # Volume mounts in "source:dest:mode" format
    env:
      API_KEY: ""              # Use "" (empty string) to passthrough from the pipeline environment.
                               # Non-empty values are embedded as literal strings in the MCPG config —
                               # ADO variable syntax like $(MY_SECRET) is NOT resolved here.
    allowed:
      - do_thing
      - get_status
```

**Custom HTTP MCP** (remote endpoint — requires `url:` field):
```yaml
mcp-servers:
  remote-service:
    url: "https://mcp.example.com"
    headers:
      X-MCP-Toolsets: "repos,wit"
    allowed:
      - query_data
```

> **Security**: Specifying an explicit `allowed:` list for `mcp-servers:` entries is strongly recommended. Without it, all tools from that server are accessible to the agent.
>
> **Standalone target** (the default): MCPs without a `container:` or `url:` field are skipped at compile time with a compile-time warning — they have no effect and will not be available to the agent. Both containerized MCPs (with `container:`) and remote HTTP MCPs (with `url:`) are supported in standalone target.

### Step 10 — Safe Outputs

Safe outputs are the only write operations available to the agent. They are threat-analyzed before execution. Configure defaults in the front matter; the agent provides specifics at runtime.

**create-pull-request** — uses `$(System.AccessToken)` by default; set `permissions.write` only for cross-org writes or named-identity attribution:
```yaml
safe-outputs:
  create-pull-request:
    target-branch: main
    draft: false             # PRs are drafts by default; set false to publish immediately (required for auto-complete)
    auto-complete: true
    delete-source-branch: true
    squash-merge: true
    title-prefix: "[Bot] "  # Optional — prepended to every PR title
    if-no-changes: warn      # "warn" (default), "error", or "ignore" when the patch is empty
    max-files: 100           # Reject patches touching more than this many files (default: 100)
    protected-files: blocked # "blocked" (default) prevents changes to pipeline/CI files; "allowed" permits all
    excluded-files:          # Glob patterns for files to exclude from the patch
      - "*.lock"
    allowed-labels: []       # Restrict which labels the agent can apply (empty = any)
    reviewers:
      - "lead@example.com"
    labels:
      - automated
    work-items:
      - 12345
```

**create-work-item** — uses `$(System.AccessToken)` by default; set `permissions.write` only for cross-org writes or named-identity attribution:
```yaml
safe-outputs:
  create-work-item:
    work-item-type: Task
    assignee: "user@example.com"
    tags:
      - automated
      - agent-created
    artifact-link:
      enabled: true
      branch: main
```

**cache-memory** — persistent agent memory across runs (configured under `tools:`, not `safe-outputs:`):
```yaml
tools:
  cache-memory:
    allowed-extensions:
      - .md
      - .json
      - .txt
```

**All configurable safe output tools:**

| Tool | Description | `permissions.write` |
|------|-------------|:-------------------:|
| **Work Items** | | |
| `create-work-item` | Create ADO work items | ✅ |
| `update-work-item` | Update fields on existing work items (each field requires opt-in) | ✅ |
| `comment-on-work-item` | Add comments to work items (requires `target` scoping) | ✅ |
| `link-work-items` | Link two work items (parent/child, related, etc.) | ✅ |
| `upload-workitem-attachment` | Upload a workspace file to a work item | ✅ |
| **Pull Requests** | | |
| `create-pull-request` | Create PRs from agent code changes | ✅ |
| `add-pr-comment` | Add a comment thread to a PR | ✅ |
| `reply-to-pr-comment` | Reply to an existing PR review thread | ✅ |
| `resolve-pr-thread` | Resolve or update status of a PR thread | ✅ |
| `submit-pr-review` | Submit a review vote on a PR | ✅ |
| `update-pr` | Update PR metadata (reviewers, labels, auto-complete, vote) | ✅ |
| **Builds & Branches** | | |
| `queue-build` | Queue an ADO pipeline build by definition ID | ✅ |
| `create-branch` | Create a new branch from an existing ref | ✅ |
| `create-git-tag` | Create a git tag on a repository ref | ✅ |
| `add-build-tag` | Add a tag to an ADO build | ✅ |
| `upload-build-attachment` | Attach a workspace file to a build (visible via REST/custom extension) | ✅ |
| `upload-pipeline-artifact` | Publish a workspace file as a pipeline artifact (visible in Artifacts tab) | ✅ |
| **Wiki** | | |
| `create-wiki-page` | Create a new ADO wiki page (requires `wiki-name`) | ✅ |
| `update-wiki-page` | Update an existing ADO wiki page (requires `wiki-name`) | ✅ |
| **Diagnostics** | | |
| `noop` | Report no action needed; also files an ADO work item (configurable, gracefully skipped without write perms) | — |
| `missing-data` | Report missing data/information | — |
| `missing-tool` | Report a missing tool or capability; also files an ADO work item (configurable, gracefully skipped without write perms) | — |
| `report-incomplete` | Report that a task could not be completed | — |

Example configuration for additional tools:
```yaml
safe-outputs:
  comment-on-work-item:
    target: "TeamProject\\AreaPath"   # Required — scopes which work items can be commented on
    max: 3
  update-work-item:
    target: "*"                       # Required — "*" allows any work item, or set to a specific ID number
    status: true                      # Each updatable field requires explicit opt-in
    title: true
    max: 5
  add-pr-comment:
    max: 10
  queue-build:
    allowed-pipelines: [42, 99]       # Required — pipeline definition IDs that can be triggered
    max: 1
  # noop and missing-tool auto-file ADO work items (enabled by default, optional customisation):
  noop:
    work-item:
      enabled: true                   # Set to false to disable work-item filing
      title: "[ado-aw] Agent reported no operation"
      work-item-type: Task
      area-path: "MyProject\\MyTeam"  # Optional
  missing-tool:
    work-item:
      enabled: true                   # Set to false to disable work-item filing
      title: "[ado-aw] Agent encountered missing tool"
      work-item-type: Task
      area-path: "MyProject\\MyTeam"  # Optional
```

> See `docs/safe-outputs.md` → "Available Safe Output Tools" for full configuration reference of every tool.

Diagnostic tools (`noop`, `missing-data`, `missing-tool`, `report-incomplete`) are always available and require no required configuration. `noop` and `missing-tool` automatically file ADO work items by default using the executor's token (sourced from `$(System.AccessToken)` by default, or from an ARM SC when `permissions.write` is set); if the token lacks work-item write permission, the call gracefully skips with a warning.

> **Note**: The compiler no longer requires `permissions.write` for write-bearing safe outputs — the executor defaults to `$(System.AccessToken)`. Set `permissions.write` only when you need cross-org writes or a named identity instead of `Project Collection Build Service`.

### Step 11 — Permissions

ADO access tokens for the agent (Stage 1) are minted from ARM service connections. The Stage 3 executor defaults to `$(System.AccessToken)`; an optional ARM SC under `permissions.write` overrides that default for cross-org writes or named-identity attribution.

```yaml
permissions:
  read: my-read-arm-connection    # Stage 1 agent — read-only ADO access
  write: my-write-arm-connection  # OPTIONAL — overrides $(System.AccessToken) for Stage 3 executor
```

| Config | Effect |
|---|---|
| `read` only | Agent can query ADO; executor writes via `$(System.AccessToken)` (default) |
| `write` only | Agent has no ADO API access; executor writes via the ARM-minted token |
| Both | Agent can read; executor writes via the ARM-minted token |
| Neither | Agent has no ADO API access; executor writes via `$(System.AccessToken)` |

### Step 12 — Triggers (optional)

> Authoring order: consider hoist candidates from Step 13 before finalising the prompt body; hoist decisions affect what the agent instructions need to do.

#### PR Triggers (`on.pr`)

Trigger on pull request events. Use `branches:` and `paths:` for native ADO filtering; use `filters:` for runtime gate conditions evaluated in the Setup job.

```yaml
on:
  pr:
    branches:
      include: [main]          # only PRs targeting main
      # exclude: [release/*]
    paths:
      include: [src/*]         # only PRs touching src/
    filters:                   # optional runtime filters (compiled to gate step with self-cancellation)
      title: "*[review]*"      # glob match on PR title
      author:
        include: ["alice@corp.com"]
        # exclude: ["bot@corp.com"]
      draft: false             # omit to match both draft and non-draft
      labels:
        any-of: ["run-agent"]  # PR must have at least one of these labels
        # all-of: [...]        # PR must have ALL of these labels
        # none-of: [...]       # PR must have NONE of these labels
      source-branch: "feature/*"   # glob on PR source branch
      target-branch: "main"        # glob on PR target branch
      commit-message: "*[skip-agent]*"  # cancel if latest commit message matches
      changed-files:
        include: ["src/**/*.rs"]
      min-changes: 1           # minimum number of changed files
      max-changes: 100         # maximum number of changed files
      time-window:
        start: "09:00"
        end: "17:00"
      build-reason:
        include: [PullRequest]
      expression: "eq(variables['Custom.Flag'], 'true')"  # raw ADO condition
```

When `on.pr` is set: the native ADO `pr:` trigger block is generated from `branches:` and `paths:`. Runtime `filters:` compile to a gate step in the Setup job that self-cancels the build when they do not match.

**`on.pr` triggering works without a Build Validation branch policy.** By default (`mode: synthetic`), the compiler emits a Setup-job script that, on CI-triggered builds, looks up the open PR for `Build.SourceBranch` via the ADO REST API and promotes the build to PR semantics if exactly one matches `pr.branches` (and `pr.paths` if configured). Zero or multiple matches → the Agent job self-skips cleanly. Set `on.pr.mode: policy` when an operator-installed Build Validation branch policy is in place — that mode omits all synth wiring AND emits `trigger: none` so feature-branch pushes do not queue duplicate CI builds alongside the policy-driven PR build. Note that in `mode: synthetic` the top-level CI `trigger:` is **not** auto-narrowed to `pr.branches.include`: those are PR target branches, and ADO `trigger:` fires on pushes *to* listed branches, so narrowing would suppress CI on the feature branches synthPr must react to. Full reference: ["PR Triggering in Azure Repos" in `docs/front-matter.md`](../docs/front-matter.md#pr-triggering-in-azure-repos).

**PR-reviewer agents — DO NOT write your own precompute step.** When `on.pr` is set, the compiler automatically (1) fetches the PR target branch with progressive deepening, (2) resolves and stages `aw-context/pr/base.sha` + `aw-context/pr/head.sha`, (3) appends a prompt fragment listing common `git diff`/`git show`/`git log` commands and example Azure DevOps MCP tool calls (`repo_get_pull_request_by_id`, `repo_list_pull_request_threads`, `repo_create_pull_request_thread`) with the PR id / project / repo pre-filled, and (4) adds `git`, `git diff`, `git log`, `git show`, `git status`, `git rev-parse`, `git symbolic-ref` to the agent's bash allow-list. The agent runs `git diff $BASE..$HEAD` itself inside the AWF sandbox (objects are already fetched into the workspace). On failure (e.g. merge-base could not be resolved), the failure fragment tells the agent to surface the error rather than produce an empty review. Opt out via `execution-context.pr.enabled: false`. Full reference: [`docs/execution-context.md`](../docs/execution-context.md).

#### Pipeline Triggers (`on.pipeline`)

Trigger from another pipeline completing:
```yaml
on:
  pipeline:
    name: "Build Pipeline"
    project: "OtherProject"   # optional if same project
    branches:
      - main
      - release/*
    filters:                   # optional runtime filters (compiled to gate step with self-cancellation)
      source-pipeline: "Build*"
      branch: "refs/heads/main"  # triggering branch (Build.SourceBranch)
      time-window:
        start: "09:00"
        end: "17:00"
      build-reason:
        include: [IndividualCI]
        exclude: [Schedule]
      expression: "eq(variables['Custom.Flag'], 'true')"  # raw ADO condition
```

When `on.pipeline` is set: `trigger: none` and `pr: none` are generated automatically. If `filters:` are configured under `on.pipeline`, a gate step is added to the Setup job that evaluates the filters and self-cancels the build when they do not match.

### Step 13 — Inline Steps (optional)

Use inline steps for deterministic work that can run as Azure DevOps steps instead of spending agent tokens on it. Ask these hoist-candidate questions before you finalise the prompt body:

- Is the work deterministic across runs (no agent reasoning needed)?
- Does it happen on every invocation (clone, cache restore, runtime install, artifact download)?
- Are the inputs fixed at compile time (repo URL, branch, tool versions)?
- → If yes to all three, hoist into `steps:` (pre-agent) or `post-steps:` (after-agent).

Work that **should** hoist:

- Cloning an additional repo whose location never changes
- Restoring a known cache, such as `~/.cache/pip`
- Installing a fixed CLI version, such as `azd` at a pinned version

Work that should **not** hoist: anything that depends on what the agent decides to do, such as branch selection based on the issue being processed.

Steps that run inside the `Agent` job:

```yaml
steps:             # BEFORE agent runs (same job)
  - bash: echo "Fetching context..."
    displayName: "Prepare context"

post-steps:        # AFTER agent completes (same job)
  - bash: echo "Archiving outputs..."
    displayName: "Post-process"
```

Separate jobs:
```yaml
setup:             # Separate job BEFORE Agent
  - bash: echo "Provisioning resources..."
    displayName: "Setup"

teardown:          # Separate job AFTER SafeOutputs
  - bash: echo "Cleanup..."
    displayName: "Teardown"
```

#### Validate before committing

When you propose any `steps:`, `post-steps:`, `setup:`, or `teardown:` block, call the author MCP server's `validate_steps` tool before writing it to the file:

```json
{
  "steps": [
    {
      "bash": "echo \"Fetching context...\"",
      "displayName": "Prepare context"
    }
  ],
  "allow_list": "full"
}
```

Use `allow_list: "full"` because the author is supervising the generated workflow. If `validate_steps` returns errors, fix the block and validate again before committing it to the agent file.

### Step 14 — Runtimes (optional)

Configure language runtimes that are installed before the agent runs. Runtimes auto-extend the bash command allow-list and add ecosystem-specific domains to the network allowlist.

```yaml
# Lean 4 theorem prover
runtimes:
  lean: true
  # lean:
  #   toolchain: "leanprover/lean4:v4.29.1"   # pin a specific version

# Python
runtimes:
  python: true
  # python:
  #   version: "3.12"
  #   feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"

# Node.js
runtimes:
  node: true
  # node:
  #   version: "22.x"
  #   feed-url: "https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/"

# .NET
runtimes:
  dotnet: true
  # dotnet:
  #   version: "8.0.x"           # or "global.json" to use the repo's global.json
  #   feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json"
  #   config: "nuget.config"     # mutually exclusive with feed-url
```

Multiple runtimes can be combined:
```yaml
runtimes:
  python:
    version: "3.12"
  node:
    version: "22.x"
  dotnet:
    version: "8.0.x"
```

> Each enabled runtime auto-adds its ecosystem's bash commands (e.g., `dotnet`, `python`, `node`, `npm`, `lean`, `lake`) and network domains to the allowlist. See `docs/runtimes.md` for full configuration reference.

### Step 15 — Network (standalone target only)

Additional allowed domains beyond the built-in allowlist:
```yaml
network:
  allowed:
    - "*.mycompany.com"
    - "api.external-service.com"
    - python                    # ecosystem identifier — expands to all Python/PyPI domains
  blocked:
    - "evil.example.com"
```

`allowed` accepts raw domain patterns (wildcards supported) or ecosystem identifiers (`python`, `node`, `rust`, `dotnet`, `lean`) that expand to the full set of package registry domains for that ecosystem. The built-in allowlist includes: Azure DevOps, GitHub, Microsoft identity, Azure services, Application Insights, and MCP-specific endpoints for each enabled server.

### Step 16 — Parameters (optional)

ADO runtime parameters are surfaced in the pipeline queue UI when a user manually runs the pipeline. Use them to expose configuration knobs (e.g., target region, log verbosity, feature flags) without hardcoding values.

```yaml
parameters:
  - name: targetRegion
    displayName: "Target region"
    type: string
    default: "us-east"
    values:
      - us-east
      - eu-west
      - ap-south
  - name: verbose
    displayName: "Verbose output"
    type: boolean
    default: false
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Parameter identifier (referenced as `${{ parameters.name }}` in steps) |
| `displayName` | No | Human-readable label in the ADO queue UI |
| `type` | No | ADO parameter type: `boolean`, `string`, `number`, `object` |
| `default` | No | Default value when not specified at queue time |
| `values` | No | Allowed values for `string`/`number` parameters (shows a dropdown in the UI) |

> **Auto-injected `clearMemory` parameter**: When `tools.cache-memory` is configured, the compiler automatically injects a `clearMemory: boolean` parameter (default: `false`) at the start of the parameters list. It lets users clear the agent's persisted memory from the ADO UI without editing the source. Defining your own `clearMemory` parameter suppresses the auto-injected one.

Omit `parameters:` if no runtime configuration knobs are needed.

### Step 17 — Inlined Imports (advanced, optional)

By default (`inlined-imports: false`), any `{{#runtime-import ...}}` markers in the agent body — including the implicit marker that reloads the body itself — are resolved at **pipeline runtime**. This means editing the `.md` agent body does not require recompiling the `.lock.yml` pipeline.

Set `inlined-imports: true` only when you need a fully self-contained pipeline YAML (e.g., for auditing or air-gapped deployment):

```yaml
inlined-imports: true
```

**When to use each mode:**

| Mode | Default | Prompt edits require recompile? | Use case |
|------|---------|--------------------------------|----------|
| `inlined-imports: false` | ✅ | No — edit and commit `.md` directly | Most workflows |
| `inlined-imports: true` | | Yes — must run `ado-aw compile` | Immutable/audited prompts |

**Trade-off**: with `inlined-imports: true`, every change to the agent instructions requires running `ado-aw compile` and committing the updated `.lock.yml`. Omit this field (or set it to `false`) for the typical edit-without-recompile workflow.

You can also reference shared files from the agent body using `{{#runtime-import path/to/file.md}}` markers.

---

## Agent Instruction Body

The markdown body (after the closing `---`) is what the agent reads. Write it as clear, structured task instructions. Good practices:

- Use headers to separate phases of work (e.g., `## Analysis`, `## Action`)
- Be explicit about inputs the agent should look for (repositories, file paths, ADO queries)
- Specify the expected output and which safe-output tool to use
- Mention what constitutes "no action needed" (to trigger `noop`)
- Keep it concise — the agent reads this at runtime on every execution

```markdown
## Instructions

Review all open pull requests in this repository for the following issues:
...

### When Changes Are Needed

Use `create-pull-request` with:
- title: "fix: ..."
- description: explaining the change

### When No Action Is Needed

Use `noop` with a brief summary of what was reviewed.
```

---

## Complete Example

```markdown
---
name: "Dependency Updater"
description: "Checks for outdated npm dependencies and opens PRs to update them"
engine:
  id: copilot
  model: claude-sonnet-4.5
on:
  schedule: weekly on monday around 9:00
tools:
  azure-devops: true
permissions:
  read: my-read-arm-sc
  write: my-write-arm-sc
safe-outputs:
  create-pull-request:
    target-branch: main
    draft: false             # PRs are drafts by default; set false to publish immediately (required for auto-complete)
    auto-complete: true
    squash-merge: true
    reviewers:
      - "lead@example.com"
    labels:
      - dependencies
      - automated
---

## Dependency Update Agent

Scan this repository for outdated npm dependencies and open a pull request to update them.

### Analysis

1. Run `npm outdated --json` to identify packages with newer versions available.
2. For each outdated package, check whether the new version introduces any breaking changes by reviewing its changelog or release notes.
3. Focus on patch and minor updates first; flag major version bumps separately.

### Action

If any outdated dependencies are found:
- Update `package.json` and run `npm install` to regenerate `package-lock.json`.
- Create a pull request titled `chore: update npm dependencies` with a description listing each updated package, its old version, and its new version.

### No Action Needed

If all dependencies are already up to date, use `noop` with a brief message: "All npm dependencies are current."
```

---

## Output Instructions

When generating the agent file:

1. **Produce exactly one `.md` file.** Do not create separate documentation, architecture notes, or runbooks.
2. **Respect existing repository conventions** for file placement. Look at where existing pipeline YAML files or agent markdown files are located in the repo. If no convention exists, ask the user where they'd like the file placed.
3. **Omit optional fields when they match defaults** — no `engine:` for `claude-opus-4.7`, no `workspace:` for `root`, no `target:` for `standalone`.
4. **`permissions.write` is optional** — the Stage 3 executor defaults to `$(System.AccessToken)`. Only add `permissions.write` when the task requires cross-org writes or named-identity attribution.

## Compilation

After creating the agent file, compile it into an Azure DevOps pipeline:

```bash
# Simple form — generates a `.lock.yml` pipeline alongside the `.md` source
ado-aw compile <path/to/agent.md>

# Or specify a custom output location
ado-aw compile <path/to/agent.md> -o <path/to/pipeline.lock.yml>
```

This generates a `.lock.yml` pipeline file. Both the source `.md` and generated `.lock.yml` must be committed together. The compiler also writes/updates a `.gitattributes` file at the repository root so compiled pipelines are marked `linguist-generated=true merge=ours`.

If the `ado-aw` CLI is not installed or not available on `PATH`, guide the user to download it from:
https://github.com/githubnext/ado-aw/releases

**After compilation**, tell the user the next steps:

```
Next steps:
  1. Review and customize the agent instructions in <filename>.md
  2. Commit both the .md source, the generated .lock.yml pipeline, and any .gitattributes changes
  3. Register the .lock.yml as a pipeline in Azure DevOps
```

---

## Common Patterns

### Scheduled Analysis → Work Item

Agent reads data (Kusto, ADO) and files a work item if action is needed.

```yaml
on:
  schedule: daily around 10:00
tools:
  azure-devops: true
permissions:
  read: my-read-sc
  write: my-write-sc
safe-outputs:
  create-work-item:
    work-item-type: Bug
    tags: [automated, agent-detected]
```

### PR-Triggered Code Review

Triggered when a pull request is opened or updated; reviews and comments via ADO.

```yaml
on:
  pr:
    branches:
      include: [main]
    filters:
      draft: false             # Skip draft PRs
tools:
  azure-devops: true
permissions:
  read: my-read-sc
  write: my-write-sc
safe-outputs:
  add-pr-comment:
    max: 5
  noop:
    work-item:
      enabled: false
```

### Repository Maintenance with PRs

Agent makes code changes and proposes them via PR.

```yaml
on:
  schedule: weekly on sunday
tools:
  azure-devops: true
permissions:
  read: my-read-sc
  write: my-write-sc
safe-outputs:
  create-pull-request:
    target-branch: main
    draft: false             # PRs are drafts by default; set false to publish immediately (required for auto-complete)
    auto-complete: true
    squash-merge: true
```

### Multi-Repo Agent

Agent checks out and modifies a secondary repository.

```yaml
repos:
  - my-org/shared-config
workspace: repo
permissions:
  read: my-read-sc
  write: my-write-sc
safe-outputs:
  create-pull-request:
    target-branch: main
```

---

## Key Rules

- **Minimal permissions**: Default to no permissions; add only what the task requires.
- **Explicit allow-lists**: Restrict MCP tools to only what the agent needs.
- **No direct writes**: All mutations go through safe outputs — the agent cannot push code or call write APIs directly.
- **Compile before committing**: Always compile with `ado-aw compile` and commit both the `.md` source and generated `.lock.yml` together.
- **Check validation**: The compiler validates front-matter fields and emits errors for invalid configurations (e.g., conflicting filter rules, missing required fields like `comment-on-work-item.target`). Write-bearing safe outputs do **not** require `permissions.write` — the executor defaults to `$(System.AccessToken)`.
