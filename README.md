# ado-aw

An agentic pipeline compiler for Azure DevOps. Write pipeline definitions in
human-friendly markdown, compile them into secure, multi-stage Azure DevOps
pipelines that run AI agents in network-isolated sandboxes.

Inspired by [GitHub Agentic Workflows (gh-aw)](https://github.com/githubnext/gh-aw).

> **If you are an AI agent**, use the specialized prompts in [`prompts/`](prompts/) for step-by-step guidance: [create](prompts/create-ado-agentic-workflow.md) · [update](prompts/update-ado-agentic-workflow.md) · [debug](prompts/debug-ado-agentic-workflow.md)

---

## How It Works

You author an **agent file** — a markdown document with YAML front matter that
describes _what_ the agent should do, _when_ it should run, and _which tools_ it
can use. The `ado-aw` compiler transforms that file into a production-ready Azure
DevOps pipeline with three jobs:

```
┌────────────────────────┐     ┌──────────────────────┐     ┌───────────────────────┐
│  Agent                 │────▶│  Detection           │────▶│  SafeOutputs          │
│  (Stage 1 — Agent)     │     │  (Stage 2 — Threats) │     │  (Stage 3 — Executor) │
│                        │     │                      │     │                       │
│  • Runs inside AWF     │     │  • Reviews proposed  │     │  • Creates PRs        │
│    network sandbox     │     │    actions for safety│     │  • Creates work items │
│  • Read-only ADO token │     │  • Checks for prompt │     │  • Write ADO token    │
│  • Produces safe       │     │    injection, leaks  │     │  • Never exposed to   │
│    output proposals    │     │                      │     │    the agent          │
└────────────────────────┘     └──────────────────────┘     └───────────────────────┘
```

The agent never has direct write access. All mutations (pull requests, work items)
go through a **safe outputs** pipeline where they are threat-analyzed and then
executed with a separate, scoped write token.

---

## Quick Start

### 1. Install

Run the first-time installer for your platform:

```bash
# Linux (x64)
curl -fsSL https://github.com/githubnext/ado-aw/releases/latest/download/install-linux.sh | sh

# macOS (Apple Silicon)
curl -fsSL https://github.com/githubnext/ado-aw/releases/latest/download/install-macos.sh | sh
```

```powershell
# Windows (x64)
powershell -ExecutionPolicy Bypass -NoProfile -Command "iwr https://github.com/githubnext/ado-aw/releases/latest/download/install-windows.ps1 -UseBasicParsing | iex"
```

The installers download the release binary, verify it against `checksums.txt`, install it to a standard path (`/usr/local/bin` when writable, otherwise a user-local path), and update your PATH when needed.

### 2. Initialize Your Repository

```bash
ado-aw init
```

This creates a Copilot agent at `.github/agents/ado-aw.agent.md` that helps you create, update, and debug agentic pipelines. The agent automatically downloads the ado-aw compiler and handles compilation.

### 3. Create an Agent with AI

Ask your coding agent (Copilot, Claude, Codex) to create a workflow:

```
Create an ADO agentic workflow using
https://raw.githubusercontent.com/githubnext/ado-aw/main/prompts/create-ado-agentic-workflow.md

The purpose of the workflow is to check for outdated dependencies weekly
and open PRs to update them.
```

Or if you've run `ado-aw init`, simply ask your AI agent:
```
Create an agentic pipeline that checks for outdated dependencies and opens PRs
```

The AI will generate a markdown file like:

```markdown
---
name: "Dependency Updater"
description: "Checks for outdated dependencies and opens PRs"
engine:
  id: copilot
  model: claude-opus-4.7
on:
  schedule: weekly on monday around 9:00
pool:
  vmImage: ubuntu-22.04
tools:
  azure-devops: true
permissions:
  read: my-read-arm-connection
  write: my-write-arm-connection
safe-outputs:
  create-pull-request:
    target-branch: main
    draft: false
    auto-complete: true
    squash-merge: true
    reviewers:
      - "team-lead@example.com"
---

## Instructions

Check all dependency manifests in this repository. For each outdated
dependency, update it to the latest stable version and create a pull
request with a clear description of what changed and why.
```

### 4. Compile to a Pipeline

```bash
# Simple form — generates a `.lock.yml` alongside the source `.md`
ado-aw compile dependency-updater.md

# Or specify a custom output location
ado-aw compile dependency-updater.md -o path/to/dependency-updater.lock.yml
```

This generates a complete Azure DevOps pipeline YAML file. The compiler also
copies the agent markdown body into the output tree so it's available at runtime.

The compiler also writes/updates a `.gitattributes` file at the repository root
that marks every compiled `.lock.yml` pipeline as `linguist-generated=true merge=ours`,
so GitHub hides them from PR diffs and merge conflicts in generated YAML resolve
to the local copy (which can then be rebuilt with `ado-aw compile`).

### 5. Verify (CI Check)

Ensure pipelines stay in sync with their source:

```bash
ado-aw check dependency-updater.lock.yml
```

This is useful as a CI gate — if someone edits the markdown but forgets to
recompile, the check will fail.

---

## Adding the Pipeline to Azure DevOps

### Step 1: Commit both files

Your repo should contain the agent source `.md` and the compiled pipeline `.lock.yml`.
Place them wherever your team's conventions dictate — there is no required directory structure.

Push both files to your Azure DevOps repository.

### Step 2: Create the pipeline in Azure DevOps

1. Go to **Pipelines → New Pipeline**
2. Select your repository
3. Choose **Existing Azure Pipelines YAML file**
4. Point to the compiled `.lock.yml` pipeline file
5. Save (or Save & Run)

### Step 3: Set Up ARM Service Connections for Permissions

This is the most important configuration step. Azure DevOps does not support
fine-grained PAT scoping — tokens are either read or read-write across the
project. To maintain security isolation between the agent and the executor,
**you need two separate ARM service connections**:

#### Why Two Connections?

| | Read Connection | Write Connection |
|---|---|---|
| **Used by** | Stage 1 — the AI agent | Stage 3 — the safe outputs executor |
| **Purpose** | Query ADO APIs (work items, repos, PRs) | Create PRs, work items, link artifacts |
| **Exposed to agent?** | ✅ Yes (inside network sandbox) | ❌ Never |
| **Token variable** | `SC_READ_TOKEN` | `SC_WRITE_TOKEN` |
| **Front matter field** | `permissions.read` | `permissions.write` |

The agent runs in a network-isolated sandbox (AWF) with only the read token.
Even if the agent were compromised or prompt-injected, it cannot perform write
operations. Write actions are only executed in Stage 3 (`SafeOutputs`)
after threat analysis, using a completely separate token that the agent never
sees.

#### Creating the Service Connections

1. **Navigate** to **Project Settings → Service connections → New service connection**
2. Choose **Azure Resource Manager → Service principal (automatic)** (or manual if
   your organization requires it)
3. Create two connections:

   **Read connection** (e.g., `ado-agent-read`):
   - Scope: subscription or resource group level
   - Grants: the ability to mint read-only ADO-scoped tokens
   - Used by: the agent job to call `az account get-access-token` with the
     ADO resource ID (`499b84ac-1321-427f-aa17-267ca6975798`)

   **Write connection** (e.g., `ado-agent-write`):
   - Scope: subscription or resource group level
   - Grants: the ability to mint read-write ADO-scoped tokens
   - Used by: the executor job to create PRs, work items, etc.

4. **Reference them** in your agent front matter:

   ```yaml
   permissions:
     read: ado-agent-read
     write: ado-agent-write
   ```

> [!NOTE]
> `permissions.write` is **optional**. The Stage 3 executor always has a
> write-capable token available via `$(System.AccessToken)` (the pipeline's
> built-in OAuth token, running as *Project Collection Build Service*). Configure
> `permissions.write` only when you need cross-org writes or named-identity
> attribution — it overrides the default token with an ARM-minted credential.

#### Permission Combinations

| Configuration | Agent can read ADO? | Safe outputs can write? |
|---|---|---|
| Both `read` + `write` | ✅ | ✅ (via ARM-minted token) |
| Only `read` | ✅ | ✅ (via `$(System.AccessToken)`) |
| Only `write` | ❌ | ✅ (via ARM-minted token) |
| Neither (default) | ❌ | ✅ (via `$(System.AccessToken)`) |

### Step 4: Authorize the Pipeline

On the first run, Azure DevOps will prompt you to authorize the pipeline to use
the service connections. Approve the permissions and the pipeline is ready.

---

## Agent File Reference

### Front Matter Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | **required** | Human-readable name for the agent |
| `description` | string | **required** | One-line summary of the agent's purpose |
| `target` | `standalone` \| `1es` \| `job` \| `stage` | `standalone` | Pipeline output format. `job` and `stage` generate reusable ADO YAML templates rather than complete pipelines. |
| `engine` | string or object | `copilot` | Engine identifier or object with `id`, `model`, `timeout-minutes`, etc. |
| `on` | object | — | Unified trigger configuration (`schedule`, `pipeline` completion, `pr` triggers). See [schedule syntax](#schedule-syntax). |
| `pool` | string or object | `vmImage: ubuntu-22.04` (standalone) / `AZS-1ES-L-MMS-ubuntu-22.04` (1ES) | Agent pool |
| `workspace` | `root` \| `repo` \| `self` \| *alias* | auto | Working directory mode. `self` is an alias for `repo`; any checked-out repo alias is also accepted. |
| `repos` | list | — | Compact repository declarations (replaces legacy `repositories:` + `checkout:`) |
| `mcp-servers` | map | — | MCP server configuration |
| `tools` | object | — | Tool configuration (`bash`, `edit`, `cache-memory`, `azure-devops`) |
| `runtimes` | object | — | Runtime environment configuration (`lean`, `python`, `node`, `dotnet`) |
| `parameters` | list | — | ADO runtime parameters surfaced in the pipeline queue UI |
| `permissions` | object | — | ARM service connections (`read`, `write`) |
| `safe-outputs` | object | — | Per-tool configuration |
| `steps` | list | — | Inline steps before agent runs |
| `post-steps` | list | — | Inline steps after agent runs |
| `setup` | list | — | Separate job before agentic task |
| `teardown` | list | — | Separate job after safe outputs |
| `network` | object | — | Additional allowed/blocked hosts |
| `inlined-imports` | boolean | `false` | When `true`, resolves all `{{#runtime-import …}}` markers at compile time; the generated YAML is self-contained but prompt-body edits require recompilation. See [runtime-imports.md](docs/runtime-imports.md). |
| `env` | map | — | Workflow-level environment variables (reserved, not yet implemented) |
| `execution-context` | object | — | Configuration for the always-on execution-context plugin (PR context precompute). See [execution-context.md](docs/execution-context.md). |
| `supply-chain` | object | — | Mirror compiler, AWF binary, ado-script bundle, and container images from an internal feed/registry. See [supply-chain.md](docs/supply-chain.md). |

### Markdown Body

Everything below the front matter `---` fence is the agent's instructions. Write
natural language describing the task, constraints, and expected behavior.

---

## Schedule Syntax

The `on.schedule` field uses a fuzzy syntax that deterministically scatters
execution times based on the agent name, preventing load spikes.

The expressions below are the value of the `schedule:` key under `on:`. For
example:

```yaml
on:
  schedule: daily around 14:00
```

```yaml
# Daily
on:
  schedule: daily                          # Scattered across 24 hours
on:
  schedule: daily around 14:00             # Within ±60 min of 2 PM
on:
  schedule: daily between 9:00 and 17:00   # Business hours

# Weekly
on:
  schedule: weekly on monday around 9:00   # Monday morning

# Multi-day / Special periods
on:
  schedule: every 2 days                   # Every 2 days at scattered time
on:
  schedule: every 2 weeks                  # Every 14 days at scattered time
on:
  schedule: bi-weekly                      # Every 14 days at scattered time
on:
  schedule: tri-weekly                     # Every 21 days at scattered time

# Hourly / Minute intervals
on:
  schedule: hourly                         # Every hour, scattered minute
on:
  schedule: every 2h                       # Every 2 hours (valid: 1, 2, 3, 4, 6, 8, 12)
on:
  schedule: every 15 minutes               # Fixed, not scattered

# With timezone
on:
  schedule: daily around 14:00 utc+9       # 2 PM JST → 5 AM UTC

# With branch filtering
on:
  schedule:
    run: daily around 14:00
    branches:
      - main
      - release/*
```

---

## MCP Servers

MCP (Model Context Protocol) servers give the agent access to external tools.

### Built-in Tools (via `tools:`)

First-class integrations are configured under the `tools:` front matter field —
not under `mcp-servers:`. The compiler auto-generates all required pipeline steps
and network allowlist entries.

```yaml
tools:
  # Bash command allow-list. Omit or use [":*"] for unrestricted access;
  # set to [] to disable bash entirely. See docs/tools.md.
  bash: ["git status", "git diff", "npm test"]

  # File editing tool (read/write/patch files in the workspace). Default: true.
  edit: true

  # Azure DevOps MCP — query work items, repos, PRs, etc.
  azure-devops: true

  # With scoping options
  azure-devops:
    toolsets: [repos, wit]
    allowed: [wit_get_work_item, repo_list_repos_by_project]
    org: myorg               # Optional — inferred from git remote by default

  # Persistent memory across agent runs
  cache-memory: true

  # With options
  cache-memory:
    allowed-extensions: [.md, .json]
```

### Custom MCP Servers (via `mcp-servers:`)

For external or third-party MCPs, use the `mcp-servers:` field. Each entry is
either a **containerized stdio server** (Docker-based) or an **HTTP server**
(remote endpoint). All traffic routes through the MCP Gateway (MCPG).

```yaml
mcp-servers:
  # Docker container stdio MCP
  my-tool:
    container: "node:20-slim"
    entrypoint: "node"
    entrypoint-args: ["path/to/mcp-server.js"]
    # enabled: set to false to temporarily disable without removing the entry
    # args: additional Docker runtime arguments inserted before the image name
    #   (e.g. ["--memory", "512m"]).  Dangerous flags like --privileged trigger
    #   a compile-time warning.
    args: []
    # mounts: volume mounts in "source:dest:mode" format
    mounts:
      - "/host/data:/app/data:ro"
    # env: environment variables for the MCP container process.
    #   Use "" (empty string) as the value to forward the variable from the
    #   pipeline environment (passthrough); supply a literal string for static
    #   values.
    env:
      MY_SECRET: ""          # passthrough from pipeline env
      STATIC_VALUE: "hello"  # literal value
    allowed:
      - search
      - analyze

  # Remote HTTP MCP
  remote-api:
    url: "https://mcp.example.com/api"
    headers:
      X-MCP-Toolsets: "repos,wit"
    allowed:
      - wit_get_work_item
```

Custom MCP containers run inside the AWF network sandbox. Add any required
external domains to `network.allowed`.

---

## Safe Outputs

Safe outputs are the only way agents produce side effects. The agent proposes
actions, and the executor processes them after threat analysis.

| Tool | Description |
|------|-------------|
| `create-pull-request` | Creates a PR from the agent's code changes |
| `create-work-item` | Creates an ADO work item (Task, Bug, etc.) |
| `comment-on-work-item` | Adds a comment to an existing ADO work item |
| `update-work-item` | Updates fields on an existing ADO work item |
| `create-wiki-page` | Creates a new Azure DevOps wiki page |
| `update-wiki-page` | Updates the content of an existing wiki page |
| `add-pr-comment` | Adds a comment thread on a pull request |
| `reply-to-pr-comment` | Replies to an existing PR review comment thread |
| `resolve-pr-thread` | Resolves or updates the status of a PR review thread |
| `submit-pr-review` | Submits a review vote on a pull request |
| `update-pr` | Updates pull request metadata (reviewers, labels, auto-complete, etc.) |
| `link-work-items` | Links two ADO work items together |
| `queue-build` | Queues a pipeline build by definition ID |
| `create-git-tag` | Creates a git tag on a repository ref |
| `create-branch` | Creates a new branch from an existing ref |
| `add-build-tag` | Adds a tag to an ADO build |
| `upload-build-attachment` | Attaches a file to a build (accessible via REST API or custom extension only) |
| `upload-pipeline-artifact` | Publishes a file as a pipeline artifact visible in the ADO Artifacts tab |
| `upload-workitem-attachment` | Uploads a workspace file as an attachment to a work item |
| `report-incomplete` | Reports that a task could not be completed |
| `noop` | Reports no action was needed |
| `missing-data` | Reports required data was unavailable |
| `missing-tool` | Reports a needed tool was missing |

### Example: Pull Request Configuration

```yaml
safe-outputs:
  create-pull-request:
    target-branch: main
    draft: false             # PRs are drafts by default; set false to publish immediately (required for auto-complete)
    auto-complete: true
    delete-source-branch: true
    squash-merge: true
    reviewers:
      - "reviewer@example.com"
    labels:
      - automated
    work-items:
      - 12345
```

### Example: Work Item Configuration

```yaml
safe-outputs:
  create-work-item:
    work-item-type: Bug
    area-path: "MyProject\\MyTeam"
    assignee: "developer@example.com"
    tags:
      - agent-created
      - needs-triage
```

---

## Network Isolation

Agents run inside [AWF (Agentic Workflow Firewall)](https://github.com/github/gh-aw-firewall)
containers with L7 domain whitelisting. Only explicitly allowed domains are
reachable. The allowlist is built from:

1. **Core domains** — Azure DevOps, GitHub, Microsoft auth, Azure storage
2. **MCP domains** — automatically added per enabled MCP
3. **User domains** — from `network.allowed` in front matter
4. **Minus blocked** — `network.blocked` entries are removed from the
   combined allowlist. Both ecosystem identifiers (e.g. `python`) and raw
   domain strings are supported. Blocking an ecosystem identifier removes
   all of its domains; blocking a raw domain uses exact-string matching
   (blocking `"github.com"` does **not** also remove `"*.github.com"`).

```yaml
network:
  allowed:
    - "*.mycompany.com"
    - "api.external-service.com"
  blocked:
    - "analytics.tracking.com"
```

---

## CLI Reference

- `audit <build-id-or-url>` - Audit a single Azure DevOps build: download artifacts, analyze logs, render Markdown or JSON report. See [`docs/audit.md`](docs/audit.md).

```
ado-aw [OPTIONS] <COMMAND>

Commands:
  init          Initialize a repository for AI-first agentic pipeline authoring
  compile       Compile markdown to pipeline definition
  check         Verify a compiled pipeline matches its source
  mcp           Run as an MCP server (safe outputs)
  mcp-http      Run as an HTTP MCP server (for MCPG integration)
  execute       Execute safe outputs (Stage 3)
  secrets       Manage pipeline-variable secrets on matched ADO definitions (set/list/delete)
  enable        Register ADO build definitions for compiled pipelines and ensure they are enabled
  disable       Set queueStatus to disabled (or paused) on matched ADO definitions
  remove        Delete matched ADO build definitions (destructive)
  list          List matched ADO definitions with their latest-run state
  status        Per-pipeline status block for matched ADO definitions
  run           Queue builds for matched ADO definitions (optionally poll to completion)
  audit         Audit a single Azure DevOps build: download artifacts, analyze logs, render a report

Options:
  -v, --verbose              Enable info-level logging
  -d, --debug                Enable debug-level logging
      --log-output-dir <path>  Write ado-aw logs to a specific directory (overrides ADO_AW_LOG_DIR)
```

> **Note:** The `configure` command is deprecated (hidden from `--help`) and is now just an alias for `secrets set GITHUB_TOKEN`. Use `secrets set GITHUB_TOKEN` directly.

The `secrets` command has three subcommands:

- `ado-aw secrets set <name> [value]` — set a pipeline variable (`isSecret=true`) on every matched definition. Value may be passed positionally, via `--value-stdin`, or prompted interactively.
- `ado-aw secrets list` — list variable names and flags on every matched definition. Never prints values.
- `ado-aw secrets delete <name>` — delete a pipeline variable from every matched definition.

All three accept `--all-repos` / `--source <path>` for project-scope (Preview-driven) discovery instead of local-fixture matching. See [docs/cli.md](docs/cli.md) for the full flag reference.

---

## Prompts & Skill Files

ado-aw provides specialized prompt files that guide AI agents through common tasks. Use these with any coding agent (Copilot, Claude, Codex, etc.):

| Task | Prompt URL | Description |
|------|-----------|-------------|
| Create a workflow | [create-ado-agentic-workflow.md](prompts/create-ado-agentic-workflow.md) | Step-by-step guide for creating a new agentic pipeline from scratch |
| Update a workflow | [update-ado-agentic-workflow.md](prompts/update-ado-agentic-workflow.md) | Guide for modifying existing agent workflows |
| Debug a pipeline | [debug-ado-agentic-workflow.md](prompts/debug-ado-agentic-workflow.md) | Troubleshoot failing agentic pipelines |

### Using Prompts with Your AI Agent

Paste the raw URL into your coding agent's chat:

```text
Create an ADO agentic workflow using
https://raw.githubusercontent.com/githubnext/ado-aw/main/prompts/create-ado-agentic-workflow.md

The purpose of the workflow is to <describe what you want>
```

The AI agent will fetch the prompt, follow its instructions, and create a complete workflow file for you.

---

## Documentation

The [`docs/`](docs/) directory contains per-concept reference pages. Use this
index to jump to the right page.

**Authoring agent files**

- [`docs/front-matter.md`](docs/front-matter.md) — full agent file format
  (markdown body + YAML front matter grammar) with every supported field.
- [`docs/engine.md`](docs/engine.md) — `engine:` configuration (model,
  `timeout-minutes`, `version`, `agent`, `api-target`, `args`, `env`,
  `command`).
- [`docs/tools.md`](docs/tools.md) — `tools:` configuration (`bash` allow-list,
  `edit`, `cache-memory`, `azure-devops` MCP).
- [`docs/runtimes.md`](docs/runtimes.md) — `runtimes:` configuration (Lean 4,
  Python, Node.js, .NET).
- [`docs/runtime-imports.md`](docs/runtime-imports.md) — runtime prompt-import
  markers, path resolution, and `inlined-imports:` behavior.
- [`docs/schedule-syntax.md`](docs/schedule-syntax.md) — fuzzy schedule time
  syntax with timezones and scattering.
- [`docs/parameters.md`](docs/parameters.md) — ADO runtime parameters surfaced
  in the pipeline UI.
- [`docs/targets.md`](docs/targets.md) — target platforms: `standalone`, `1es`,
  `job`, and `stage`.
- [`docs/safe-outputs.md`](docs/safe-outputs.md) — full reference for every
  safe-output tool plus their per-agent configuration.
- [`docs/safe-output-permissions.md`](docs/safe-output-permissions.md) — diagnosis
  and fix reference for Stage 3 401/403 failures (build identity, token semantics,
  permission fixes).
- [`docs/execution-context.md`](docs/execution-context.md) — built-in
  `aw-context/` precompute: PR target-branch fetch, merge-base resolution,
  `base.sha`/`head.sha` artefacts, and auto-extended bash allow-list.
- [`docs/supply-chain.md`](docs/supply-chain.md) — optional `supply-chain:`
  section for mirroring the compiler, AWF binary, ado-script bundle, and
  container images from an internal feed/registry.
- [`docs/ado-aw-debug.md`](docs/ado-aw-debug.md) — debug-only `ado-aw-debug:`
  front-matter section (`skip-integrity`, `create-issue`).

**Compiler internals & operations**

- [`docs/ir.md`](docs/ir.md) — typed Azure DevOps pipeline IR (`Pipeline`,
  jobs/stages/steps, output refs, graph pass, lowering, and target builders).
- [`docs/cli.md`](docs/cli.md) — `ado-aw` CLI command and flag reference.
- [`docs/mcp.md`](docs/mcp.md) — MCP server configuration (stdio containers,
  HTTP servers, env passthrough).
- [`docs/mcp-author.md`](docs/mcp-author.md) — author-facing MCP server (stdio);
  exposes `inspect`, `graph`, `whatif`, `lint`, `catalog`, `trace`, `audit_build`
  over MCP for IDE/Copilot Chat agents.
- [`docs/mcpg.md`](docs/mcpg.md) — MCP Gateway architecture and pipeline
  integration.
- [`docs/network.md`](docs/network.md) — AWF network isolation, default
  allowed domains, ecosystem identifiers, blocking, and ADO `permissions:`
  service-connection model.
- [`docs/filter-ir.md`](docs/filter-ir.md) — filter expression IR for PR
  trigger filters and gate-step generation.
- [`docs/codemods.md`](docs/codemods.md) — front-matter codemod framework
  (detection-based source rewrites on breaking-change updates).
- [`docs/ado-script.md`](docs/ado-script.md) — `scripts/ado-script/` workspace
  (bundled TypeScript runtime helpers: `gate.js`, `import.js`,
  `exec-context-pr.js`, `exec-context-pr-synth.js`).
- [`docs/extending.md`](docs/extending.md) — adding new CLI commands, compile
  targets, front-matter fields, typed IR extensions, safe-output tools,
  first-class tools, and runtimes.
- [`docs/local-development.md`](docs/local-development.md) — local development
  setup notes.

---

## Development

```bash
# Build
cargo build

# Test
cargo test

# Lint
cargo clippy
```

This project uses [Conventional Commits](https://www.conventionalcommits.org/)
for automated releases via `release-please`.

---

## License

See [LICENSE](LICENSE) for details.
