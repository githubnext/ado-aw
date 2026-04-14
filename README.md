# ado-aw

An agentic pipeline compiler for Azure DevOps. Write pipeline definitions in
human-friendly markdown, compile them into secure, multi-stage Azure DevOps
pipelines that run AI agents in network-isolated sandboxes.

Inspired by [GitHub Agentic Workflows (gh-aw)](https://github.com/githubnext/gh-aw).

> **If you are an AI agent** tasked with creating an ado-aw workflow, read [`prompts/create-ado-agentic-workflow.md`](prompts/create-ado-agentic-workflow.md) for step-by-step instructions on authoring a workflow file.

---

## How It Works

You author an **agent file** — a markdown document with YAML front matter that
describes _what_ the agent should do, _when_ it should run, and _which tools_ it
can use. The `ado-aw` compiler transforms that file into a production-ready Azure
DevOps pipeline with three jobs:

```
┌────────────────────────┐     ┌──────────────────────┐     ┌───────────────────────┐
│  PerformAgenticTask    │────▶│  AnalyzeSafeOutputs  │────▶│  ProcessSafeOutputs   │
│  (Stage 1 — Agent)     │     │  (Threat Analysis)   │     │  (Stage 2 — Executor) │
│                        │     │                      │     │                       │
│  • Runs inside AWF     │     │  • Reviews proposed   │     │  • Creates PRs        │
│    network sandbox     │     │    actions for safety │     │  • Creates work items │
│  • Read-only ADO token │     │  • Checks for prompt  │     │  • Write ADO token    │
│  • Produces safe       │     │    injection, leaks   │     │  • Never exposed to   │
│    output proposals    │     │                      │     │    the agent           │
└────────────────────────┘     └──────────────────────┘     └───────────────────────┘
```

The agent never has direct write access. All mutations (pull requests, work items)
go through a **safe outputs** pipeline where they are threat-analyzed and then
executed with a separate, scoped write token.

---

## Quick Start

### 1. Install

Download a release binary from [GitHub Releases](https://github.com/githubnext/ado-aw/releases),
or build from source:

```bash
cargo build --release
```

### 2. Create an Agent

Use the interactive wizard to scaffold a new agent file:

```bash
ado-aw create -o agents/
```

The wizard walks you through:

- **Name & description** — human-readable identity for the agent
- **Model** — AI engine (`claude-opus-4.5`, `claude-sonnet-4.5`, `gpt-5.2-codex`, `gemini-3-pro-preview`, etc.)
- **Schedule** — when the agent runs, using [fuzzy schedule syntax](#schedule-syntax)
- **Workspace** — `root` or `repo` working directory
- **Repositories** — additional repos the agent can access
- **MCP servers** — tool integrations (ADO, Kusto, IcM, etc.)
- **Permissions** — ARM service connections for ADO access

This generates a markdown file like:

```markdown
---
name: "Dependency Updater"
description: "Checks for outdated dependencies and opens PRs"
engine: claude-sonnet-4.5
schedule: weekly on monday around 9:00
pool: AZS-1ES-L-MMS-ubuntu-22.04
mcp-servers:
  ado: true
permissions:
  read: my-read-arm-connection
  write: my-write-arm-connection
safe-outputs:
  create-pull-request:
    target-branch: main
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

### 3. Compile to a Pipeline

```bash
ado-aw compile agents/dependency-updater.md -o .pipelines/dependency-updater.yml
```

This generates a complete Azure DevOps pipeline YAML file. The compiler also
copies the agent markdown body into `agents/dependency-updater.md` in the output
tree so it's available at runtime.

### 4. Verify (CI Check)

Ensure pipelines stay in sync with their source:

```bash
ado-aw check agents/dependency-updater.md .pipelines/dependency-updater.yml
```

This is useful as a CI gate — if someone edits the markdown but forgets to
recompile, the check will fail.

---

## Adding the Pipeline to Azure DevOps

### Step 1: Commit both files

Your repo should contain:

```
your-repo/
├── agents/
│   └── dependency-updater.md          # Agent source (instructions + config)
└── .pipelines/
    └── dependency-updater.yml         # Compiled pipeline
```

Push both files to your Azure DevOps repository.

### Step 2: Create the pipeline in Azure DevOps

1. Go to **Pipelines → New Pipeline**
2. Select your repository
3. Choose **Existing Azure Pipelines YAML file**
4. Point to `.pipelines/dependency-updater.yml`
5. Save (or Save & Run)

### Step 3: Set Up ARM Service Connections for Permissions

This is the most important configuration step. Azure DevOps does not support
fine-grained PAT scoping — tokens are either read or read-write across the
project. To maintain security isolation between the agent and the executor,
**you need two separate ARM service connections**:

#### Why Two Connections?

| | Read Connection | Write Connection |
|---|---|---|
| **Used by** | Stage 1 — the AI agent | Stage 2 — the safe outputs executor |
| **Purpose** | Query ADO APIs (work items, repos, PRs) | Create PRs, work items, link artifacts |
| **Exposed to agent?** | ✅ Yes (inside network sandbox) | ❌ Never |
| **Token variable** | `SC_READ_TOKEN` | `SC_WRITE_TOKEN` |
| **Front matter field** | `permissions.read` | `permissions.write` |

The agent runs in a network-isolated sandbox (AWF) with only the read token.
Even if the agent were compromised or prompt-injected, it cannot perform write
operations. Write actions are only executed in Stage 2 (`ProcessSafeOutputs`)
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

> [!IMPORTANT]
> If you configure safe outputs that require write access (`create-pull-request`
> or `create-work-item`) but omit `permissions.write`, compilation will fail
> with a clear error. This is a safety check — write operations must always
> have an explicitly configured credential.

#### Permission Combinations

| Configuration | Agent can read ADO? | Safe outputs can write? |
|---|---|---|
| Both `read` + `write` | ✅ | ✅ |
| Only `read` | ✅ | ❌ |
| Only `write` | ❌ | ✅ |
| Neither (default) | ❌ | ❌ |

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
| `target` | `standalone` \| `1es` | `standalone` | Pipeline output format |
| `engine` | string or object | `claude-opus-4.5` | AI model to use |
| `schedule` | string or object | — | [Fuzzy schedule expression](#schedule-syntax) |
| `pool` | string or object | `AZS-1ES-L-MMS-ubuntu-22.04` | Agent pool |
| `workspace` | `root` \| `repo` | auto | Working directory mode |
| `repositories` | list | — | Additional repository resources |
| `checkout` | list | — | Which repositories to check out |
| `mcp-servers` | map | — | MCP server configuration |
| `permissions` | object | — | ARM service connections (`read`, `write`) |
| `safe-outputs` | object | — | Per-tool configuration |
| `triggers` | object | — | Pipeline trigger configuration |
| `steps` | list | — | Inline steps before agent runs |
| `post-steps` | list | — | Inline steps after agent runs |
| `setup` | list | — | Separate job before agentic task |
| `teardown` | list | — | Separate job after safe outputs |
| `network` | object | — | Additional allowed/blocked hosts |
| `env` | map | — | Workflow-level environment variables (reserved, not yet implemented) |

### Markdown Body

Everything below the front matter `---` fence is the agent's instructions. Write
natural language describing the task, constraints, and expected behavior.

---

## Schedule Syntax

The `schedule` field uses a fuzzy syntax that deterministically scatters
execution times based on the agent name, preventing load spikes.

```yaml
# Daily
schedule: daily                          # Scattered across 24 hours
schedule: daily around 14:00             # Within ±60 min of 2 PM
schedule: daily between 9:00 and 17:00   # Business hours

# Weekly
schedule: weekly on monday around 9:00   # Monday morning

# Hourly / Minute intervals
schedule: hourly                         # Every hour, scattered minute
schedule: every 2h                       # Every 2 hours
schedule: every 15 minutes               # Fixed, not scattered

# With timezone
schedule: daily around 14:00 utc+9       # 2 PM JST → 5 AM UTC

# With branch filtering
schedule:
  run: daily around 14:00
  branches:
    - main
    - release/*
```

---

## MCP Servers

MCP (Model Context Protocol) servers give the agent access to external tools.
Built-in MCPs are enabled by name; custom MCPs specify a `command`:

```yaml
mcp-servers:
  # Built-in — enable with true or restrict with allowed list
  ado: true
  kusto:
    allowed:
      - query

  # Custom — must include command
  my-tool:
    command: "node"
    args: ["path/to/server.js"]
    allowed:
      - search
      - analyze
```

Available built-in MCPs: `ado`, `ado-ext`, `asa`, `bluebird`, `calculator`,
`es-chat`, `icm`, `kusto`, `msft-learn`, `stack`.

Each MCP automatically adds its required domains to the network allowlist.

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
| `upload-attachment` | Uploads a workspace file as an attachment to a work item |
| `memory` | Persists files across agent runs |
| `report-incomplete` | Reports that a task could not be completed |
| `noop` | Reports no action was needed |
| `missing-data` | Reports required data was unavailable |
| `missing-tool` | Reports a needed tool was missing |

### Example: Pull Request Configuration

```yaml
safe-outputs:
  create-pull-request:
    target-branch: main
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
3. **User domains** — from `network.allow` in front matter
4. **Minus blocked** — `network.blocked` entries are removed by exact match (wildcard patterns like `*.example.com` are not affected by blocking a specific subdomain)

```yaml
network:
  allow:
    - "*.mycompany.com"
    - "api.external-service.com"
  blocked:
    - "analytics.tracking.com"
```

---

## CLI Reference

```
ado-aw [OPTIONS] <COMMAND>

Commands:
  create        Create a new agent markdown file interactively
  compile       Compile markdown to pipeline definition
  check         Verify a compiled pipeline matches its source
  mcp           Run as an MCP server (safe outputs)
  mcp-http      Run as an HTTP MCP server (for MCPG integration)
  execute       Execute safe outputs (Stage 2)
  proxy         Start an HTTP proxy for network filtering
  configure     Detect agentic pipelines and update GITHUB_TOKEN on ADO definitions

Options:
  -v, --verbose  Enable info-level logging
  -d, --debug    Enable debug-level logging
```

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