# Create an Azure DevOps Agentic Workflow

Create an **ado-aw** agent file — a markdown document with YAML front matter that the `ado-aw` compiler transforms into a secure, multi-stage Azure DevOps pipeline running an AI agent inside a network-isolated AWF sandbox.

## What to Produce

Produce a single `.md` file (placed in `agents/`) containing two parts:

1. **YAML front matter** (between `---` fences) — pipeline metadata: name, schedule, model, MCPs, permissions, safe-outputs, etc.
2. **Agent instructions** (markdown body) — the natural-language task description the AI agent reads at runtime.

The `ado-aw` compiler turns this into a three-job Azure DevOps pipeline:

```
PerformAgenticTask  →  AnalyzeSafeOutputs  →  ProcessSafeOutputs
(Stage 1: Agent)       (Threat analysis)       (Stage 2: Executor)
```

The agent in Stage 1 never has direct write access. All mutations (PRs, work items) are proposed as **safe outputs**, threat-analyzed, then executed by the Stage 2 executor using a separate write token.

---

## How to Create a Workflow

Gather the requirements below, then produce the complete `.md` file. Follow the order used by `ado-aw create`.

### Step 1 — Name & Description

Determine:
- **Name**: Human-readable name (e.g., "Weekly Dependency Updater"). Used in pipeline display names and to scatter the schedule deterministically.
- **Description**: One-line summary of what the agent does.

```yaml
name: "Weekly Dependency Updater"
description: "Checks for outdated dependencies and opens PRs to update them"
```

### Step 2 — AI Model (engine)

Default is `claude-opus-4.5`. Only include `engine:` if the user requests a different model.

| Value | Use when |
|---|---|
| `claude-opus-4.5` | Default. Best reasoning, complex tasks. |
| `claude-sonnet-4.5` | Faster, cheaper, simpler tasks. |
| `gpt-5.2-codex` | Code-heavy tasks. |
| `gemini-3-pro-preview` | Google ecosystem tasks. |

Object form with extra options:
```yaml
engine:
  model: claude-sonnet-4.5
  timeout-minutes: 30
```

### Step 3 — Schedule

Use the **fuzzy schedule syntax** (deterministic time scattering based on agent name hash prevents load spikes). Omit `schedule:` entirely for manual/trigger-only pipelines.

**String form** (always schedules on `main`):
```yaml
schedule: daily around 14:00
```

**Object form** (custom branch list):
```yaml
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
| `hourly` | Every hour, scattered minute |
| `every 2h` / `every 6h` | Every N hours (must divide 24) |
| `every 15 minutes` | Minimum 5 min interval |
| `bi-weekly` | Every 14 days |
| `tri-weekly` | Every 21 days |

**Timezone**: Append `utc+N` or `utc-N` to any time: `daily around 9:00 utc-5`

### Step 4 — Workspace

Controls where the agent's working directory is set.

| Value | Path | Use when |
|---|---|---|
| `root` (default) | `$(Build.SourcesDirectory)` | Only checking out `self` |
| `repo` | `$(Build.SourcesDirectory)/$(Build.Repository.Name)` | Multiple repos checked out |

Only include `workspace:` if non-default. Warn the user if they set `workspace: repo` but have no additional repos in `checkout:`.

### Step 5 — Repositories & Checkout

Declare extra repositories the pipeline can access, then select which ones the agent actually checks out.

```yaml
repositories:
  - repository: my-other-repo        # alias
    type: git
    name: my-org/my-other-repo       # org/repo
  - repository: templates
    type: git
    name: my-org/pipeline-templates

checkout:
  - my-other-repo    # only check this one out; "templates" stays as a resource only
```

- `repositories:` — pipeline-level resources (for templates, pipeline triggers, etc.)
- `checkout:` — which aliases the agent actually checks out alongside `self`
- Omit `checkout:` entirely to check out only `self`

### Step 6 — Pool

Defaults to `AZS-1ES-L-MMS-ubuntu-22.04`. Only include if overriding.

String form:
```yaml
pool: MyCustomPool
```

Object form (needed for 1ES target with explicit OS):
```yaml
pool:
  name: AZS-1ES-L-MMS-ubuntu-22.04
  os: linux   # "linux" or "windows"
```

### Step 7 — Target

Defaults to `standalone`. Only include if using 1ES Pipeline Templates.

```yaml
target: 1es
```

| Value | Generates |
|---|---|
| `standalone` | Full 3-job pipeline with AWF network sandbox and Squid proxy |
| `1es` | Pipeline extending `1ES.Unofficial.PipelineTemplate.yml`; no custom proxy; MCPs via service connections |

### Step 8 — MCP Servers

MCP servers give the agent tools at runtime. Two kinds:

**Built-in** (no `command:` field):
```yaml
mcp-servers:
  ado: true                  # All ADO tools
  ado-ext: true              # Extended ADO functionality
  kusto:
    allowed:
      - query                # Restrict to specific tools
  icm:
    allowed:
      - create_incident
      - get_incident
  bluebird: true
  es-chat: true
  msft-learn: true
  stack: true
  asa: true
  calculator: true
```

**Custom** (has `command:` field):
```yaml
mcp-servers:
  my-tool:
    command: "node"
    args: ["path/to/server.js"]
    env:
      API_KEY: "$(MY_SECRET)"
    allowed:
      - do_thing
      - get_status
```

> **Security**: Custom MCPs must have an explicit `allowed:` list. Built-in MCPs default to all tools when set to `true`.
>
> **1ES target**: Custom MCPs (`command:`) are not supported — only built-ins with service connections.

### Step 9 — Safe Outputs

Safe outputs are the only write operations available to the agent. They are threat-analyzed before execution. Configure defaults in the front matter; the agent provides specifics at runtime.

**create-pull-request** — requires `permissions.write`:
```yaml
safe-outputs:
  create-pull-request:
    target-branch: main
    auto-complete: true
    delete-source-branch: true
    squash-merge: true
    reviewers:
      - "lead@example.com"
    labels:
      - automated
    work-items:
      - 12345
```

**create-work-item** — requires `permissions.write`:
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

**memory** — persistent agent memory across runs:
```yaml
safe-outputs:
  memory:
    allowed-extensions:
      - .md
      - .json
      - .txt
```

Other safe output tools (no configuration needed): `noop`, `missing-data`, `missing-tool`.

> **Validation**: The compiler enforces that if `create-pull-request` or `create-work-item` are configured, `permissions.write` must be set.

### Step 10 — Permissions

ADO access tokens are minted from ARM service connections. `System.AccessToken` is never used.

```yaml
permissions:
  read: my-read-arm-connection    # Stage 1 agent — read-only ADO access
  write: my-write-arm-connection  # Stage 2 executor only — write access
```

| Config | Effect |
|---|---|
| `read` only | Agent can query ADO; no safe-output writes |
| `write` only | Agent has no ADO API access; safe-outputs can create PRs/work items |
| Both | Agent can read; safe-outputs can write |
| Neither | No ADO tokens anywhere |

### Step 11 — Pipeline Triggers (optional)

Trigger from another pipeline:
```yaml
triggers:
  pipeline:
    name: "Build Pipeline"
    project: "OtherProject"   # optional if same project
    branches:
      - main
      - release/*
```

When `triggers.pipeline` is set: `trigger: none` and `pr: none` are generated automatically, and a step to cancel previous queued builds is included.

### Step 12 — Inline Steps (optional)

Steps that run inside the `PerformAgenticTask` job:

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
setup:             # Separate job BEFORE PerformAgenticTask
  - bash: echo "Provisioning resources..."
    displayName: "Setup"

teardown:          # Separate job AFTER ProcessSafeOutputs
  - bash: echo "Cleanup..."
    displayName: "Teardown"
```

### Step 13 — Network (standalone target only)

Additional allowed domains beyond the built-in allowlist:
```yaml
network:
  allow:
    - "*.mycompany.com"
    - "api.external-service.com"
  blocked:
    - "evil.example.com"
```

The built-in allowlist includes: Azure DevOps, GitHub, Microsoft identity, Azure services, Application Insights, and MCP-specific endpoints for each enabled server.

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
engine: claude-sonnet-4.5
schedule: weekly on monday around 9:00
mcp-servers:
  ado: true
permissions:
  read: my-read-arm-sc
  write: my-write-arm-sc
safe-outputs:
  create-pull-request:
    target-branch: main
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
2. For each outdated package, check whether the new version introduces any breaking changes by reviewing its changelog or release notes (use `msft-learn` if relevant documentation is available).
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
2. **Place it in `agents/`** at the root of the repository (e.g., `agents/dependency-updater.md`).
3. **Omit optional fields when they match defaults** — no `engine:` for `claude-opus-4.5`, no `workspace:` for `root`, no `target:` for `standalone`.
4. **Always validate** that write-requiring safe-outputs (`create-pull-request`, `create-work-item`) have `permissions.write` set.
5. **After writing the file**, tell the user the next steps:

```
Next steps:
  1. Review and customize the agent instructions in agents/<filename>.md
  2. Compile: ado-aw compile agents/<filename>.md -o .pipelines/<filename>.yml
  3. Commit both the .md source and the generated .yml pipeline
  4. Register the .yml as a pipeline in Azure DevOps
```

---

## Common Patterns

### Scheduled Analysis → Work Item

Agent reads data (Kusto, ADO) and files a work item if action is needed.

```yaml
schedule: daily around 10:00
mcp-servers:
  ado: true
  kusto:
    allowed: [query]
permissions:
  read: my-read-sc
  write: my-write-sc
safe-outputs:
  create-work-item:
    work-item-type: Bug
    tags: [automated, agent-detected]
```

### PR-Triggered Code Review

Triggered by another pipeline; reviews and comments via ADO.

```yaml
triggers:
  pipeline:
    name: "CI Build"
    branches: [main, feature/*]
mcp-servers:
  ado: true
permissions:
  read: my-read-sc
  write: my-write-sc
safe-outputs:
  create-work-item:
    work-item-type: Task
```

### Repository Maintenance with PRs

Agent makes code changes and proposes them via PR.

```yaml
schedule: weekly on sunday
mcp-servers:
  ado: true
permissions:
  read: my-read-sc
  write: my-write-sc
safe-outputs:
  create-pull-request:
    target-branch: main
    auto-complete: true
    squash-merge: true
```

### Multi-Repo Agent

Agent checks out and modifies a secondary repository.

```yaml
repositories:
  - repository: shared-config
    type: git
    name: my-org/shared-config
checkout:
  - shared-config
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
- **Compile before committing**: Always compile with `ado-aw compile` and commit both the `.md` source and generated `.yml` together.
- **Check validation**: The compiler will error if write safe-outputs are configured without `permissions.write`.
- **1ES target limits**: No custom MCPs, no custom network allow-lists — these are handled by OneBranch infrastructure.
