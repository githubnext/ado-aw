# Copilot Instructions for Azure DevOps Agentic Pipelines

This repository contains a compiler for Azure DevOps pipelines that transforms natural language markdown files with YAML front matter into Azure DevOps pipeline definitions. The design is inspired by [GitHub Agentic Workflows (gh-aw)](https://github.com/githubnext/gh-aw).

## Project Overview

### Purpose

The `ado-aw` compiler enables users to write pipeline definitions in a human-friendly markdown format with YAML front matter, which gets compiled into proper Azure DevOps YAML pipeline definitions. This approach:

- Makes pipeline authoring more accessible through natural language
- Enables AI agents to work safely in network-isolated sandboxes (via OneBranch)
- Provides a small, controlled set of tools for agents to complete work
- Validates outputs for correctness and conformity

Alongside the correctly generated pipeline yaml, an agent file is generated from the remaining markdown and placed in `agents/` at the root of a consumer repository. The pipeline yaml references the agent.

### Architecture

```
├── src/
│   ├── main.rs           # Entry point with clap CLI
│   ├── allowed_hosts.rs  # Core network allowlist definitions
│   ├── ecosystem_domains.rs # Ecosystem domain lookups (python, rust, node, etc.)
│   ├── data/
│   │   └── ecosystem_domains.json # Ecosystem domain lists (synced from gh-aw)
│   ├── compile/          # Pipeline compilation module
│   │   ├── mod.rs        # Module entry point and Compiler trait
│   │   ├── common.rs     # Shared helpers across targets
│   │   ├── standalone.rs # Standalone pipeline compiler
│   │   ├── onees.rs      # 1ES Pipeline Template compiler
│   │   ├── extensions/   # CompilerExtension trait and infrastructure extensions
│   │   │   ├── mod.rs    # Trait, Extension enum, collect_extensions(), re-exports
│   │   │   ├── github.rs # Always-on GitHub MCP extension
│   │   │   └── safe_outputs.rs # Always-on SafeOutputs MCP extension
│   │   └── types.rs      # Front matter grammar and types
│   ├── init.rs           # Repository initialization for AI-first authoring
│   ├── execute.rs        # Stage 3 safe output execution
│   ├── fuzzy_schedule.rs # Fuzzy schedule parsing
│   ├── logging.rs        # File-based logging infrastructure
│   ├── mcp.rs            # SafeOutputs MCP server (stdio + HTTP)
│   ├── configure.rs      # `configure` CLI command — detects and updates pipeline variables
│   ├── detect.rs         # Agentic pipeline detection (helper for `configure`)
│   ├── ndjson.rs         # NDJSON parsing utilities
│   ├── sanitize.rs       # Input sanitization for safe outputs
│   ├── agent_stats.rs    # OTel-based agent statistics parsing (token usage, duration, turns)
│   ├── safeoutputs/      # Safe-output MCP tool implementations (Stage 1 → NDJSON → Stage 3)
│   │   ├── mod.rs
│   │   ├── add_build_tag.rs
│   │   ├── add_pr_comment.rs
│   │   ├── comment_on_work_item.rs
│   │   ├── create_branch.rs
│   │   ├── create_git_tag.rs
│   │   ├── create_pr.rs
│   │   ├── create_wiki_page.rs
│   │   ├── create_work_item.rs
│   │   ├── link_work_items.rs
│   │   ├── missing_data.rs
│   │   ├── missing_tool.rs
│   │   ├── noop.rs
│   │   ├── queue_build.rs
│   │   ├── reply_to_pr_comment.rs
│   │   ├── report_incomplete.rs
│   │   ├── resolve_pr_thread.rs
│   │   ├── result.rs
│   │   ├── submit_pr_review.rs
│   │   ├── update_pr.rs
│   │   ├── update_wiki_page.rs
│   │   ├── update_work_item.rs
│   │   └── upload_attachment.rs
│   ├── runtimes/         # Runtime environment implementations (one dir per runtime)
│   │   ├── mod.rs        # Module entry point
│   │   └── lean/         # Lean 4 theorem prover runtime
│   │       ├── mod.rs    # Config types, install helpers
│   │       └── extension.rs # CompilerExtension impl
│   ├── data/
│   │   ├── base.yml          # Base pipeline template for standalone
│   │   ├── 1es-base.yml      # Base pipeline template for 1ES target
│   │   ├── ecosystem_domains.json # Network allowlists per ecosystem
│   │   ├── init-agent.md     # Dispatcher agent template for `init` command
│   │   └── threat-analysis.md # Threat detection analysis prompt template
│   └── tools/            # First-class tool implementations (one dir per tool)
│       ├── mod.rs
│       ├── azure_devops/  # Azure DevOps MCP tool
│       │   ├── mod.rs
│       │   └── extension.rs # CompilerExtension impl
│       └── cache_memory/  # Persistent agent memory tool
│           ├── mod.rs
│           ├── extension.rs # CompilerExtension impl (compile-time)
│           └── execute.rs   # Stage 3 runtime (validate/copy)
├── examples/             # Example agent definitions
├── tests/                # Integration tests and fixtures
├── Cargo.toml            # Rust dependencies
└── README.md             # Project documentation
```

## Technology Stack

- **Language**: Rust (2024 edition) - Note: Rust 2024 edition exists and is the edition used by this project
- **CLI Framework**: clap v4 with derive macros
- **Error Handling**: anyhow for ergonomic error propagation
- **Async Runtime**: tokio with full features
- **YAML Parsing**: serde_yaml
- **MCP Server**: rmcp with server and transport-io features
- **Target Platform**: Azure DevOps Pipelines / OneBranch

## Development Guidelines

### Commit Message Convention

This project uses [Conventional Commits](https://www.conventionalcommits.org/) for automated releases via `release-please`. All commit messages **must** follow the format:

```
type(optional scope): description
```

Common types: `feat`, `fix`, `chore`, `docs`, `refactor`, `test`, `ci`. Commits that don't follow this format will be ignored by release-please and won't trigger a release.

### Rust Code Style

1. Use `anyhow::Result` for fallible functions
2. Leverage clap's derive macros for CLI argument parsing
3. Prefer explicit error messages with `anyhow::bail!` or `.context()`
4. Keep the binary fast—avoid unnecessary allocations and prefer streaming parsers

### Input Format (Markdown with Front Matter)

The compiler expects markdown files with YAML front matter similar to gh-aw:

```markdown
---
name: "name for this agent"
description: "One line description for this agent"
target: standalone # Optional: "standalone" (default) or "1es". See Target Platforms section below.
engine: copilot # Engine identifier. Defaults to copilot. Currently only 'copilot' (GitHub Copilot CLI) is supported.
# engine:                        # Alternative object format (with additional options)
#   id: copilot
#   model: claude-opus-4.5
#   timeout-minutes: 30
schedule: daily around 14:00 # Fuzzy schedule syntax - see Schedule Syntax section below
# schedule:                       # Alternative object format (with branch filtering)
#   run: daily around 14:00
#   branches:
#     - main
#     - release/*
workspace: repo # Optional: "root" or "repo". If not specified, defaults based on checkout configuration (see below).
pool: AZS-1ES-L-MMS-ubuntu-22.04 # Agent pool name (string format). Defaults to AZS-1ES-L-MMS-ubuntu-22.04.
# pool:                        # Alternative object format (required for 1ES if specifying os)
#   name: AZS-1ES-L-MMS-ubuntu-22.04
#   os: linux                  # Operating system: "linux" or "windows". Defaults to "linux".
repositories: # a list of repository resources available to the pipeline (for pre/post jobs, templates, etc.)
  - repository: reponame
    type: git
    name: my-org/my-repo
  - repository: another-repo
    type: git
    name: my-org/another-repo
checkout: # optional list of repository aliases for the agent to checkout and work with (must be subset of repositories)
  - reponame # only checkout reponame, not another-repo
tools:                         # optional tool configuration
  bash: ["cat", "ls", "grep"]  # bash command allow-list (defaults to safe built-in list)
  edit: true                   # enable file editing tool (default: true)
  cache-memory: true           # persistent memory across runs (see Cache Memory section)
  # cache-memory:              # Alternative object format (with options)
  #   allowed-extensions: [.md, .json]
  azure-devops: true           # first-class ADO MCP integration (see Azure DevOps MCP section)
  # azure-devops:              # Alternative object format (with scoping)
  #   toolsets: [repos, wit]
  #   allowed: [wit_get_work_item]
  #   org: myorg
runtimes:                      # optional runtime configuration (language environments)
  lean: true                   # Lean 4 theorem prover (see Runtimes section)
  # lean:                      # Alternative object format (with toolchain pinning)
  #   toolchain: "leanprover/lean4:v4.29.1"
# env:                          # RESERVED: workflow-level environment variables (not yet implemented)
#   CUSTOM_VAR: "value"
mcp-servers:
  my-custom-tool:              # containerized MCP server (requires container field)
    container: "node:20-slim"
    entrypoint: "node"
    entrypoint-args: ["path/to/mcp-server.js"]
    allowed:
      - custom_function_1
      - custom_function_2
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
triggers:                      # optional pipeline triggers
  pipeline:
    name: "Build Pipeline"     # source pipeline name
    project: "OtherProject"    # optional: project name if different
    branches:                  # optional: branches to trigger on
      - main
      - release/*
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
  write: my-write-arm-connection # ARM service connection for write ADO access (Stage 3 executor only)
parameters:                    # optional ADO runtime parameters (surfaced in UI when queuing a run)
  - name: clearMemory
    displayName: "Clear agent memory"
    type: boolean
    default: false
---


## Build and Test

Build the project and run all tests...
```

### Schedule Syntax (Fuzzy Schedule Time Syntax)

The `schedule` field supports a human-friendly fuzzy schedule syntax that automatically distributes execution times to prevent server load spikes. The syntax is based on the [Fuzzy Schedule Time Syntax Specification](https://github.com/githubnext/gh-aw/blob/main/docs/src/content/docs/reference/fuzzy-schedule-specification.md).

#### Daily Schedules

```yaml
schedule: daily                          # Scattered across full 24-hour day
schedule: daily around 14:00             # Within ±60 minutes of 2 PM
schedule: daily around 3pm               # 12-hour format supported
schedule: daily around midnight          # Keywords: midnight, noon
schedule: daily between 9:00 and 17:00   # Business hours (9 AM - 5 PM)
schedule: daily between 22:00 and 02:00  # Overnight (handles midnight crossing)
```

#### Weekly Schedules

```yaml
schedule: weekly                              # Any day, scattered time
schedule: weekly on monday                    # Monday, scattered time
schedule: weekly on friday around 17:00       # Friday, within ±60 min of 5 PM
schedule: weekly on wednesday between 9:00 and 12:00  # Wednesday morning
```

Valid weekdays: `sunday`, `monday`, `tuesday`, `wednesday`, `thursday`, `friday`, `saturday`

#### Hourly Schedules

```yaml
schedule: hourly       # Every hour at a scattered minute
schedule: every 2h     # Every 2 hours at scattered minute
schedule: every 6h     # Every 6 hours at scattered minute
```

Valid hour intervals: 1, 2, 3, 4, 6, 8, 12 (factors of 24 for even distribution)

#### Minute Intervals (Fixed, Not Scattered)

```yaml
schedule: every 5 minutes     # Every 5 minutes (minimum interval)
schedule: every 15 minutes    # Every 15 minutes
schedule: every 30m           # Short form supported
```

Note: Minimum interval is 5 minutes (GitHub Actions/Azure DevOps constraint).

#### Special Periods

```yaml
schedule: bi-weekly    # Every 14 days at scattered time
schedule: tri-weekly   # Every 21 days at scattered time
schedule: every 2 days # Every 2 days at scattered time
```

#### Timezone Support

All time specifications support UTC offsets for timezone conversion:

```yaml
schedule: daily around 14:00 utc+9      # 2 PM JST → 5 AM UTC
schedule: daily around 3pm utc-5        # 3 PM EST → 8 PM UTC
schedule: daily between 9am utc+05:30 and 5pm utc+05:30  # IST business hours
```

Supported offset formats: `utc+9`, `utc-5`, `utc+05:30`, `utc-08:00`

#### How Scattering Works

The compiler uses a deterministic hash of the agent name to scatter execution times:
- Same agent always gets the same execution time (stable across recompilations)
- Different agents get different times (distributes load)
- Times stay within the specified constraints (around, between, etc.)

This prevents load spikes that occur when many workflows use convenient times like midnight or on-the-hour.

#### Schedule Branch Filtering

By default, when no branches are explicitly configured, the schedule fires only on the `main` branch. To specify different branches, use the object form:

```yaml
# Default: fires only on main branch (string form)
schedule: daily around 14:00

# Custom branches: fires on listed branches (object form)
schedule:
  run: daily around 14:00
  branches:
    - main
    - release/*
```

### Engine Configuration

The `engine` field specifies which engine to use for the agentic task. The string form is an engine identifier (currently only `copilot` is supported). The object form uses `id` for the engine identifier plus additional options like model selection and timeout.

```yaml
# Simple string format (engine identifier, defaults to copilot)
engine: copilot

# Object format with additional options
engine:
  id: copilot
  model: claude-opus-4.5
  timeout-minutes: 30
```

#### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | `copilot` | Engine identifier. Currently only `copilot` (GitHub Copilot CLI) is supported. |
| `model` | string | `claude-opus-4.5` | AI model to use. Options include `claude-sonnet-4.5`, `gpt-5.2-codex`, `gemini-3-pro-preview`, etc. |
| `timeout-minutes` | integer | *(none)* | Maximum time in minutes the agent job is allowed to run. Sets `timeoutInMinutes` on the `Agent` job in the generated pipeline. |
| `version` | string | *(none)* | Engine CLI version to install (e.g., `"0.0.422"`, `"latest"`). When set, overrides `COPILOT_CLI_VERSION`. When `"latest"`, omits the `-Version` flag from NuGet install. |
| `agent` | string | *(none)* | Custom agent file identifier (Copilot only — references `.github/agents/<agent>.agent.md`). When set, adds `--agent <value>` to Copilot CLI args. Must be alphanumeric with hyphens only. |
| `api-target` | string | *(none)* | Custom API endpoint hostname for GHES/GHEC (e.g., `"api.acme.ghe.com"`). **Not yet wired** — parsed but ignored with a warning. |
| `args` | list | `[]` | Custom CLI arguments injected before the prompt. **Not yet wired** — parsed but ignored with a warning. |
| `env` | map | *(none)* | Engine-specific environment variables. **Not yet wired** — parsed but ignored with a warning. |
| `command` | string | *(none)* | Custom engine executable path (skips default installation). When set, the NuGet install steps are omitted and the specified path is used in the AWF invocation. Must contain only safe path characters. |

> **Deprecated:** `max-turns` is still accepted in front matter for backwards compatibility but is ignored at compile time (a warning is emitted). It was specific to Claude Code and is not supported by Copilot CLI.

> **Note:** Fields marked "not yet wired" are accepted in the schema for forward compatibility with gh-aw but produce a compile-time warning. Pipeline wiring for these fields is incremental.

#### `timeout-minutes`

The `timeout-minutes` field sets a wall-clock limit (in minutes) for the entire agent job. It maps to the Azure DevOps `timeoutInMinutes` job property on `Agent`. This is useful for:

- **Budget enforcement** — hard-capping the total runtime of an agent to control compute costs.
- **Pipeline hygiene** — preventing agents from occupying a runner indefinitely if they stall or enter long retry loops.
- **SLA compliance** — ensuring scheduled agents complete within a known window.

When omitted, Azure DevOps uses its default job timeout (60 minutes). When set, the compiler emits `timeoutInMinutes: <value>` on the agentic job.

### Runtime Parameters

The `parameters` field defines Azure DevOps [runtime parameters](https://learn.microsoft.com/en-us/azure/devops/pipelines/process/runtime-parameters) that are surfaced in the ADO UI when manually queuing a pipeline run. Parameters are emitted as a top-level `parameters:` block in the generated pipeline YAML.

```yaml
parameters:
  - name: verbose
    displayName: "Verbose output"
    type: boolean
    default: false
  - name: region
    displayName: "Target region"
    type: string
    default: "us-east"
    values:
      - us-east
      - eu-west
      - ap-south
```

#### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Parameter identifier (valid ADO identifier) |
| `displayName` | string | No | Human-readable label in the ADO UI |
| `type` | string | No | ADO parameter type: `boolean`, `string`, `number`, `object` |
| `default` | any | No | Default value when not specified at queue time |
| `values` | list | No | Allowed values (for `string`/`number` parameters) |

Parameters can be referenced in custom steps using `${{ parameters.paramName }}`.

#### Auto-injected `clearMemory` Parameter

When `safe-outputs.memory` is configured, the compiler automatically injects a `clearMemory` boolean parameter (default: `false`) at the beginning of the parameters list. This parameter:

- Is surfaced in the ADO UI when manually queuing a run
- When set to `true`, skips downloading the previous agent memory artifact
- Creates an empty memory directory so the agent starts fresh

If you define your own `clearMemory` parameter in the front matter, the auto-injected one is suppressed — your definition takes precedence.

### Tools Configuration

The `tools` field controls which tools are available to the agent. Both sub-fields are optional and have sensible defaults.

#### Default Bash Command Allow-list

When `tools.bash` is omitted, the agent defaults to **unrestricted bash access** (`--allow-all-tools`). This matches gh-aw's sandbox behavior — since ado-aw agents always run inside the AWF sandbox, all tools are allowed by default.

#### Configuring Bash Access

```yaml
# Default: unrestricted bash access (bash field omitted → --allow-all-tools)
tools:
  edit: true

# Explicit unrestricted bash (same as default) — also accepts "*"
tools:
  bash: [":*"]

# Explicit command allow-list (restricts to named commands only)
tools:
  bash: ["cat", "ls", "grep", "find"]

# Disable bash entirely (empty list)
tools:
  bash: []
```

#### Disabling File Writes

By default, the `edit` tool (file writing) is enabled. To disable it:

```yaml
tools:
  edit: false
```

#### Cache Memory (`cache-memory:`)

Persistent memory storage across agent runs. The agent reads/writes files to a memory directory that persists between pipeline executions via pipeline artifacts.

```yaml
# Simple enablement
tools:
  cache-memory: true

# With options
tools:
  cache-memory:
    allowed-extensions: [.md, .json, .txt]
```

When enabled, the compiler auto-generates pipeline steps to:
- Download previous memory from the last successful run's artifact
- Restore files to `/tmp/awf-tools/staging/agent_memory/`
- Append a memory prompt to the agent instructions
- Auto-inject a `clearMemory` pipeline parameter (allows clearing memory from the ADO UI)

During Stage 3 execution, memory files are validated (path safety, extension filtering, `##vso[` injection detection, 5 MB size limit) and published as a pipeline artifact.

#### Azure DevOps MCP (`azure-devops:`)

First-class Azure DevOps MCP integration. Auto-configures the ADO MCP container, token mapping, MCPG entry, and network allowlist.

```yaml
# Simple enablement (auto-infers org from git remote)
tools:
  azure-devops: true

# With scoping options
tools:
  azure-devops:
    toolsets: [repos, wit, core]                    # ADO API toolset groups
    allowed: [wit_get_work_item, core_list_projects] # Explicit tool allow-list
    org: myorg                                       # Optional override (inferred from git remote)
```

When enabled, the compiler:
- Generates a containerized stdio MCP entry (`node:20-slim` + `npx @azure-devops/mcp`) in the MCPG config
- Auto-maps `AZURE_DEVOPS_EXT_PAT` token passthrough when `permissions.read` is configured
- Adds ADO-specific hosts to the network allowlist
- Auto-infers org from the git remote URL at compile time (overridable via `org:` field)
- Fails compilation if org cannot be determined (no explicit override and no ADO git remote)

### Runtimes Configuration

The `runtimes` field configures language environments that are installed before the agent runs. Unlike tools (which are agent capabilities like edit, bash, memory), runtimes are execution environments that the compiler auto-installs via pipeline steps.

Aligned with [gh-aw's `runtimes:` front matter field](https://github.github.com/gh-aw/reference/frontmatter/#runtimes-runtimes).

#### Lean 4 (`lean:`)

Lean 4 theorem prover runtime. Auto-installs the Lean toolchain via elan, extends the bash command allow-list, adds Lean-specific domains to the network allowlist, and appends a prompt supplement informing the agent that Lean is available.

```yaml
# Simple enablement (installs latest stable toolchain)
runtimes:
  lean: true

# With options (pin specific toolchain version)
runtimes:
  lean:
    toolchain: "leanprover/lean4:v4.29.1"
```

When enabled, the compiler:
- Injects an elan installation step into `{{ prepare_steps }}` (runs before AWF network isolation)
- Defaults to the `stable` toolchain; if a `lean-toolchain` file exists in the repo, elan overrides to that version automatically
- Auto-adds `lean`, `lake`, and `elan` to the bash command allow-list
- Adds Lean-specific domains to the network allowlist: `elan.lean-lang.org`, `leanprover.github.io`, `lean-lang.org`
- Symlinks lean tools into `/tmp/awf-tools/` for AWF chroot compatibility
- Appends a prompt supplement informing the agent about Lean 4 availability and basic commands
- Emits a compile-time warning if `tools.bash` is empty (Lean requires bash access)

**Note:** In the 1ES target, the bash command allow-list is updated but elan installation must be done manually via `steps:` front matter. The 1ES target handles network isolation separately.

### Target Platforms

The `target` field in the front matter determines the output format and execution environment for the compiled pipeline.

#### `standalone` (default)

Generates a self-contained Azure DevOps pipeline with:
- Full 3-job pipeline: `Agent` → `Detection` → `Execution`
- AWF (Agentic Workflow Firewall) L7 domain whitelisting via Squid proxy + Docker
- MCP Gateway (MCPG) for MCP routing with SafeOutputs HTTP backend
- Setup/teardown job support
- All safe output features (create-pull-request, create-work-item, etc.)

This is the recommended target for maximum flexibility and security controls.

#### `1es`

Generates a pipeline that extends the 1ES Unofficial Pipeline Template:
- Uses `templateContext.type: buildJob` with Copilot CLI + AWF + MCPG (same execution model as standalone)
- Integrates with 1ES SDL scanning and compliance tools
- Full 3-job pipeline: Agent → Detection → Execution
- Requires 1ES Pipeline Templates repository access

Example:
```yaml
target: 1es
```

When using `target: 1es`, the pipeline will extend `1es/1ES.Unofficial.PipelineTemplate.yml@1ESPipelinesTemplates`.

### Output Format (Azure DevOps YAML)

The compiler transforms the input into valid Azure DevOps pipeline YAML based on the target platform:

- **Standalone**: Uses `src/data/base.yml`
- **1ES**: Uses `src/data/1es-base.yml`

Explicit markings are embedded in these templates that the compiler is allowed to replace e.g. `{{ copilot_params }}` denotes parameters which are passed to the copilot command line tool. The compiler should not replace sections denoted by `${{ some content }}`. What follows is a mapping of markings to responsibilities (primarily for the standalone template).

## {{ parameters }}

Should be replaced with the top-level `parameters:` block generated from the `parameters` front matter field. If no parameters are defined (and no auto-injected parameters apply), this marker is replaced with an empty string.

When `safe-outputs.memory` is configured, the compiler auto-injects a `clearMemory` boolean parameter (default: `false`) unless one is already user-defined.

Example output:
```yaml
parameters:
- name: clearMemory
  displayName: Clear agent memory
  type: boolean
  default: false
- name: verbose
  displayName: Verbose output
  type: boolean
  default: false
```

## {{ repositories }}
For each additional repository specified in the front matter append:

```yaml
- repository: reponame
  type: git
  name: reponame
  ref: refs/heads/main
```

## {{ schedule }}

This marker should be replaced with a cron-style schedule block generated from the fuzzy schedule syntax. The compiler parses the human-friendly schedule expression and generates a deterministic cron expression based on the agent name hash.

By default, when no branches are explicitly configured, the schedule defaults to `main` branch only. When the object form is used with a `branches` list, a `branches.include` block is generated with the specified branches.

```yaml
# Default (string form) — defaults to main branch
schedules:
  - cron: "43 14 * * *"    # Generated from "daily around 14:00"
    displayName: "Scheduled run"
    branches:
      include:
        - main
    always: true

# With custom branches (object form)
schedules:
  - cron: "43 14 * * *"
    displayName: "Scheduled run"
    branches:
      include:
        - main
        - release/*
    always: true
```

Examples of fuzzy schedule → cron conversion:
- `daily` → scattered across 24 hours (e.g., `"43 5 * * *"`)
- `daily around 14:00` → within 13:00-15:00 (e.g., `"13 14 * * *"`)
- `hourly` → every hour at scattered minute (e.g., `"43 * * * *"`)
- `weekly on monday` → Monday at scattered time (e.g., `"43 5 * * 1"`)
- `every 2h` → every 2 hours at scattered minute (e.g., `"53 */2 * * *"`)
- `bi-weekly` → every 14 days (e.g., `"43 5 */14 * *"`)

## {{ checkout_self }}

Should be replaced with the `checkout: self` step. This generates a simple checkout of the triggering branch.

All checkout steps across all jobs (Agent, Detection, Execution, Setup, Teardown) use this marker.

## {{ checkout_repositories }}
Should be replaced with checkout steps for additional repositories the agent will work with. The behavior depends on the `checkout:` front matter:

- **If `checkout:` is omitted or empty**: No additional repositories are checked out. Only `self` is checked out (from the template).
- **If `checkout:` is specified**: The listed repository aliases are checked out in addition to `self`. Each entry must exist in `repositories:`.

This distinction allows resources (like templates) to be available as pipeline resources without being checked out into the workspace for the agent to analyze.

```yaml
- checkout: reponame
```

## {{ agent_name }}

Should be replaced with the human-readable name from the front matter (e.g., "Daily Code Review"). This is used for display purposes like stage names.

## {{ copilot_params }}

Additional params provided to copilot CLI. The compiler generates:
- `--model <model>` - AI model from `engine` front matter field (default: claude-opus-4.5)
- `--agent <agent>` - Custom agent file identifier from `engine.agent` (only when set; must be alphanumeric + hyphens)
- `--no-ask-user` - Prevents interactive prompts
- `--disable-builtin-mcps` - Disables all built-in Copilot CLI MCPs (single flag, no argument)
- `--allow-all-tools` - When bash is omitted (default) or has a wildcard (`":*"` or `"*"`), allows all tools instead of individual `--allow-tool` flags
- `--allow-tool <tool>` - When bash is NOT wildcard, explicitly allows configured tools (github, safeoutputs, write, and shell commands from the `bash:` field plus any runtime-required commands)
- `--allow-all-paths` - When `edit` tool is enabled (default), allows the agent to write to any file path

MCP servers are handled entirely by the MCP Gateway (MCPG) and are not passed as copilot CLI params.

## {{ pool }}

Should be replaced with the agent pool name from the `pool` front matter field. Defaults to `AZS-1ES-L-MMS-ubuntu-22.04` if not specified.

The pool configuration accepts both string and object formats:
- **String format**: `pool: AZS-1ES-L-MMS-ubuntu-22.04`
- **Object format**: `pool: { name: AZS-1ES-L-MMS-ubuntu-22.04, os: linux }`

The `os` field (defaults to "linux") is primarily used for 1ES target compatibility.

## {{ setup_job }}

Generates a separate setup job YAML if `setup` contains steps. The job:
- Runs before `Agent`
- Uses the same pool as the main agentic task
- Includes a checkout of self
- Display name: `Setup`

If `setup` is empty, this is replaced with an empty string.

## {{ teardown_job }}

Generates a separate teardown job YAML if `teardown` contains steps. The job:
- Runs after `Execution` (depends on it)
- Uses the same pool as the main agentic task
- Includes a checkout of self
- Display name: `Teardown`

If `teardown` is empty, this is replaced with an empty string.

## {{ prepare_steps }}

Generates inline steps that run inside the `Agent` job, **before** the agent runs. These steps can generate context files, fetch secrets, or prepare the workspace for the agent.

Steps are inserted after the agent prompt is prepared but before AWF network isolation starts.

If `steps` is empty, this is replaced with an empty string.

## {{ finalize_steps }}

Generates inline steps that run inside the `Agent` job, **after** the agent completes. These steps can validate outputs, process workspace artifacts, or perform cleanup.

Steps are inserted after the AWF-isolated agent completes but before logs are collected.

If `post-steps` is empty, this is replaced with an empty string.

## {{ agentic_depends_on }}

Generates a `dependsOn: Setup` clause for `Agent` if a setup job is configured. The setup job is identified by the job name `Setup`, ensuring the agentic task waits for the setup job to complete.

If no setup job is configured, this is replaced with an empty string.

## {{ job_timeout }}

Generates a `timeoutInMinutes: <value>` job property for `Agent` when `engine.timeout-minutes` is configured. This sets the Azure DevOps job-level timeout for the agentic task.

If `timeout-minutes` is not configured, this is replaced with an empty string.

## {{ working_directory }}

Should be replaced with the appropriate working directory based on the effective workspace setting.

**Workspace Resolution Logic:**
1. If `workspace` is explicitly set in front matter, that value is used
2. If `workspace` is not set and `checkout:` contains additional repositories, defaults to `repo`
3. If `workspace` is not set and only `self` is checked out, defaults to `root`

**Warning:** If `workspace: repo` is explicitly set but no additional repositories are in `checkout:`, a warning is emitted because when only `self` is checked out, `$(Build.SourcesDirectory)` already contains the repository content directly.

**Values:**
- `root`: `$(Build.SourcesDirectory)` - the checkout root directory
- `repo`: `$(Build.SourcesDirectory)/$(Build.Repository.Name)` - the repository's subfolder

This is used for the `workingDirectory` property of the copilot task.

## {{ source_path }}

Should be replaced with the path to the agent markdown source file for Stage 3 execution. The path is relative to the workspace and depends on the effective workspace setting (see `{{ working_directory }}` for resolution logic):
- `root`: `$(Build.SourcesDirectory)/agents/<filename>.md`
- `repo`: `$(Build.SourcesDirectory)/$(Build.Repository.Name)/agents/<filename>.md`

Used by the execute command's --source parameter.

## {{ pipeline_path }}

Should be replaced with the path to the compiled pipeline YAML file for runtime integrity checking. The path is derived from the output path's filename and uses `{{ working_directory }}` as the base (which gets resolved before this placeholder):
- `root`: `$(Build.SourcesDirectory)/<filename>.yml`
- `repo`: `$(Build.SourcesDirectory)/$(Build.Repository.Name)/<filename>.yml`

Used by the pipeline's integrity check step to verify the pipeline hasn't been modified outside the compilation process.

## {{ integrity_check }}

Generates the "Verify pipeline integrity" pipeline step that downloads the released ado-aw compiler and runs `ado-aw check` against the compiled pipeline YAML. This step ensures the pipeline file hasn't been modified outside the compilation process.

When the compiler is built with `--skip-integrity` (debug builds only), this placeholder is replaced with an empty string and the integrity step is omitted from the generated pipeline.

## {{ mcpg_debug_flags }}

Generates MCPG debug environment flags for the Docker run command. When `--debug-pipeline` is passed (debug builds only), this inserts `-e DEBUG="*"` to enable verbose MCPG logging.

When `--debug-pipeline` is not passed, this placeholder is replaced with a bare `\` to maintain bash line continuation.

## {{ verify_mcp_backends }}

Generates a pipeline step that probes each configured MCPG backend with an MCP initialize + tools/list handshake. This forces MCPG's lazy initialization and catches failures (e.g., container timeout, network blocked) before the agent runs, surfacing them as ADO pipeline warnings.

When `--debug-pipeline` is not passed (the default), this placeholder is replaced with an empty string.

## {{ pr_trigger }}

Generates PR trigger configuration. When a schedule or pipeline trigger is configured, this generates `pr: none` to disable PR triggers. Otherwise, it generates an empty string, allowing the default PR trigger behavior.

## {{ ci_trigger }}

Generates CI trigger configuration. When a schedule or pipeline trigger is configured, this generates `trigger: none` to disable CI triggers. Otherwise, it generates an empty string, allowing the default CI trigger behavior.

## {{ pipeline_resources }}

Generates pipeline resource YAML when `triggers.pipeline` is configured in the front matter. Creates a pipeline resource with appropriate trigger configuration based on the specified branches. If no branches are specified, the pipeline triggers on any branch.

Example output when `triggers.pipeline` is configured:
```yaml
resources:
  pipelines:
    - pipeline: source_pipeline
      source: Build Pipeline
      project: OtherProject
      trigger:
        branches:
          include:
            - main
            - release/*
```

## {{ agent_content }}

Should be replaced with the markdown body (agent instructions) extracted from the source markdown file, excluding the YAML front matter. This content provides the agent with its task description and guidelines.

## {{ mcpg_config }}

Should be replaced with the MCP Gateway (MCPG) configuration JSON generated from the `mcp-servers:` front matter. This configuration defines the MCPG server entries and gateway settings.

The generated JSON has two top-level sections:
- `mcpServers`: Maps server names to their configuration (type, container/url, tools, etc.)
- `gateway`: Gateway settings (port, domain, apiKey, payloadDir)

SafeOutputs is always included as an HTTP backend (`type: "http"`) pointing to `localhost` (MCPG runs with `--network host`, so `localhost` is the host loopback). Containerized MCPs with `container:` are included as stdio servers (`type: "stdio"` with `container`, `entrypoint`, `entrypointArgs`). HTTP MCPs with `url:` are included as HTTP servers. MCPs without a container or url are skipped.

Runtime placeholders (`${SAFE_OUTPUTS_PORT}`, `${SAFE_OUTPUTS_API_KEY}`, `${MCP_GATEWAY_API_KEY}`) are substituted by the pipeline at runtime before passing the config to MCPG.

## {{ mcpg_docker_env }}

Should be replaced with additional `-e` flags for the MCPG Docker run command, enabling environment variable passthrough from the pipeline to MCP containers.

When `permissions.read` is configured, the compiler automatically adds `-e AZURE_DEVOPS_EXT_PAT="$(SC_READ_TOKEN)"` to forward the ADO access token to MCP containers that need it (e.g., Azure DevOps MCP).

Additionally, any env vars in MCP configs with empty string values (`""`) are collected and forwarded as `-e VAR_NAME` flags, enabling passthrough from the pipeline environment through MCPG to MCP child containers.

Environment variable names are validated against `[A-Za-z_][A-Za-z0-9_]*` to prevent Docker flag injection.

If no passthrough env vars are needed, this marker is replaced with an empty string.

## {{ mcp_client_config }}

**Removed.** The Copilot CLI `mcp-config.json` is no longer generated at compile time. Instead, it is derived at **pipeline runtime** from MCPG's actual gateway output, matching gh-aw's `convert_gateway_config_copilot.cjs` pattern.

The "Start MCP Gateway (MCPG)" pipeline step:
1. Redirects MCPG's stdout to `gateway-output.json`
2. Waits for the health check and for valid JSON output
3. Transforms the output with a Python script that:
   - Rewrites URLs from `127.0.0.1` → `host.docker.internal` (AWF container loopback vs host)
   - Ensures `tools: ["*"]` on each server entry (Copilot CLI requirement)
   - Preserves all other fields (headers, type, etc.)
4. Writes the result to `/tmp/awf-tools/mcp-config.json` and `$HOME/.copilot/mcp-config.json`

This ensures the Copilot CLI config reflects MCPG's actual runtime state rather than a compile-time prediction.

## {{ allowed_domains }}

Should be replaced with the comma-separated domain list for AWF's `--allow-domains` flag. The list includes:
1. Core Azure DevOps/GitHub endpoints (from `allowed_hosts.rs`)
2. MCP-specific endpoints for each enabled MCP
3. Ecosystem identifier expansions from `network.allowed:` (e.g., `python` → PyPI/pip domains)
4. User-specified additional hosts from `network.allowed:` front matter

The output is formatted as a comma-separated string (e.g., `github.com,*.dev.azure.com,api.github.com`).

## {{ enabled_tools_args }}

Should be replaced with `--enabled-tools <name>` CLI arguments for the SafeOutputs MCP HTTP server. The tool list is derived from `safe-outputs:` front matter keys plus always-on diagnostic tools (`noop`, `missing-data`, `missing-tool`, `report-incomplete`).

When `safe-outputs:` is empty (or omitted), this is replaced with an empty string and all tools remain available (backward compatibility). When non-empty, the replacement includes a trailing space to prevent concatenation with the next positional argument in the shell command.

Tool names are validated at compile time:
- Names must contain only ASCII alphanumerics and hyphens (shell injection prevention)
- Unrecognized names (not in `ALL_KNOWN_SAFE_OUTPUTS`) emit a warning to catch typos

## {{ threat_analysis_prompt }}

Should be replaced with the embedded threat detection analysis prompt from `src/data/threat-analysis.md`. This prompt template includes markers for `{{ source_path }}`, `{{ agent_name }}`, `{{ agent_description }}`, and `{{ working_directory }}` which are replaced during compilation.

The threat analysis prompt instructs the security analysis agent to check for:
- Prompt injection attempts
- Secret leaks
- Malicious patches (suspicious web calls, backdoors, encoded strings, suspicious dependencies)

## {{ agent_description }}

Should be replaced with the description field from the front matter. This is used in display contexts and the threat analysis prompt template.

## {{ acquire_ado_token }}

Generates an `AzureCLI@2` step that acquires a read-only ADO-scoped access token from the ARM service connection specified in `permissions.read`. This token is used by the agent in Stage 1 (inside the AWF sandbox).

The step:
- Uses the ARM service connection from `permissions.read`
- Calls `az account get-access-token` with the ADO resource ID
- Stores the token in a secret pipeline variable `SC_READ_TOKEN`

If `permissions.read` is not configured, this marker is replaced with an empty string.

## {{ engine_env }}

Generates engine-specific environment variable entries for the AWF sandbox step via `Engine::env()`. For the Copilot engine, this produces:

- `GITHUB_TOKEN: $(GITHUB_TOKEN)` — GitHub authentication
- `GITHUB_READ_ONLY: 1` — Restricts GitHub API to read-only access
- `COPILOT_OTEL_ENABLED`, `COPILOT_OTEL_EXPORTER_TYPE`, `COPILOT_OTEL_FILE_EXPORTER_PATH` — OpenTelemetry file-based tracing for agent statistics

ADO access tokens (`AZURE_DEVOPS_EXT_PAT`, `SYSTEM_ACCESSTOKEN`) are not part of this marker — they are injected separately by `{{ acquire_ado_token }}` and extension pipeline variable mappings when `permissions.read` is configured.

## {{ acquire_write_token }}

Generates an `AzureCLI@2` step that acquires a write-capable ADO-scoped access token from the ARM service connection specified in `permissions.write`. This token is used only by the executor in Stage 3 (`Execution` job) and is never exposed to the agent.

The step:
- Uses the ARM service connection from `permissions.write`
- Calls `az account get-access-token` with the ADO resource ID
- Stores the token in a secret pipeline variable `SC_WRITE_TOKEN`

If `permissions.write` is not configured, this marker is replaced with an empty string.

## {{ executor_ado_env }}

Generates environment variable entries for the Stage 3 executor step when `permissions.write` is configured. Sets `SYSTEM_ACCESSTOKEN` to the write service connection token (`SC_WRITE_TOKEN`).

If `permissions.write` is not configured, this marker is replaced with an empty string. Note: `System.AccessToken` is never used directly — all ADO tokens come from explicitly configured service connections.

## {{ compiler_version }}

Should be replaced with the version of the `ado-aw` compiler that generated the pipeline (derived from `CARGO_PKG_VERSION` at compile time). This version is used to construct the GitHub Releases download URL for the `ado-aw` binary.

The generated pipelines download the compiler binary from:
```
https://github.com/githubnext/ado-aw/releases/download/v{VERSION}/ado-aw-linux-x64
```

A `checksums.txt` file is also downloaded and verified via `sha256sum -c checksums.txt --ignore-missing` to ensure binary integrity.

## {{ firewall_version }}

Should be replaced with the pinned version of the AWF (Agentic Workflow Firewall) binary (defined as `AWF_VERSION` constant in `src/compile/common.rs`). This version is used to construct the GitHub Releases download URL for the AWF binary.

The generated pipelines download the AWF binary from:
```
https://github.com/github/gh-aw-firewall/releases/download/v{VERSION}/awf-linux-x64
```

A `checksums.txt` file is also downloaded and verified via `sha256sum -c checksums.txt --ignore-missing` to ensure binary integrity.

## {{ mcpg_version }}

Should be replaced with the pinned version of the MCP Gateway (defined as `MCPG_VERSION` constant in `src/compile/common.rs`). Used to tag the MCPG Docker image in the pipeline.

## {{ mcpg_image }}

Should be replaced with the MCPG Docker image name (defined as `MCPG_IMAGE` constant in `src/compile/common.rs`). Currently `ghcr.io/github/gh-aw-mcpg`.

## {{ mcpg_port }}

Should be replaced with the MCPG listening port (defined as `MCPG_PORT` constant in `src/compile/common.rs`, currently `80`). Used in the pipeline to set the `MCP_GATEWAY_PORT` ADO variable and in the MCPG health-check URL.

## {{ mcpg_domain }}

Should be replaced with the domain the AWF-sandboxed agent uses to reach MCPG on the host (defined as `MCPG_DOMAIN` constant in `src/compile/common.rs`, currently `host.docker.internal`). Used in the pipeline to set the `MCP_GATEWAY_DOMAIN` ADO variable. Docker's `host.docker.internal` resolves to the host loopback from inside containers.

## {{ copilot_version }}

Should be replaced with the pinned version of the `Microsoft.Copilot.CLI.linux-x64` NuGet package (defined as `COPILOT_CLI_VERSION` constant in `src/compile/common.rs`). This constant serves as the default version; it can be overridden per-agent via the `engine.version` front matter field.

**Note:** This marker is no longer used directly in the pipeline templates — it is consumed internally by `{{ engine_install_steps }}`. It remains available as a replacement marker for backwards compatibility.

## {{ engine_install_steps }}

Generates the engine CLI install steps (NuGet authentication, package install, binary copy, and version output). The behavior depends on the `engine` front matter configuration:

- **Default** (no `engine.version` or `engine.command`): Generates the full NuGet install sequence using `COPILOT_CLI_VERSION`.
- **`engine.version: "0.0.422"`**: Uses the specified version instead of `COPILOT_CLI_VERSION` in the `-Version` NuGet flag.
- **`engine.version: "latest"`**: Omits the `-Version` flag entirely, installing the latest available package.
- **`engine.command: /path/to/binary`**: Returns an empty string — no install steps are generated because the user provides their own engine binary.

## {{ copilot_command }}

Should be replaced with the path to the engine binary inside the AWF container.

- **Default**: `/tmp/awf-tools/copilot` (the default location where the installed binary is copied).
- **`engine.command: /path/to/binary`**: The custom path specified in front matter.

### 1ES-Specific Template Markers

The 1ES target uses the same template markers as standalone, plus the 1ES-specific `extends:` / `stages:` / `templateContext` wrapping. The 1ES template includes `templateContext.type: buildJob` for all jobs, and the pool is specified at the top-level `parameters.pool` rather than per-job.

Both targets share the same execution model (Copilot CLI + AWF + MCPG) and the same set of template markers.

### CLI Commands

Global flags (apply to all subcommands): `--verbose, -v` (enable info-level logging), `--debug, -d` (enable debug-level logging, implies verbose)

- `init` - Initialize a repository for AI-first agentic pipeline authoring
  - `--path <path>` - Target directory (defaults to current directory)
  - `--force` - Overwrite existing agent file
  - Creates `.github/agents/ado-aw.agent.md` — a Copilot dispatcher agent that routes to specialized prompts for creating, updating, and debugging agentic pipelines
  - The agent auto-downloads the ado-aw compiler and handles the full lifecycle (create → compile → check)
- `compile [<path>]` - Compile a markdown file to Azure DevOps pipeline YAML. If no path is given, auto-discovers and recompiles all detected agentic pipelines in the current directory.
  - `--output, -o <path>` - Optional output path for generated YAML (only valid when a path is provided)
  - `--skip-integrity` - *(debug builds only)* Omit the "Verify pipeline integrity" step from the generated pipeline. Useful during local development when the compiled output won't match a released compiler version. This flag is not available in release builds.
  - `--debug-pipeline` - *(debug builds only)* Include MCPG debug diagnostics in the generated pipeline: `DEBUG=*` environment variable for verbose MCPG logging, stderr streaming to log files, and a "Verify MCP backends" step that probes each backend with MCP initialize + tools/list before the agent runs. This flag is not available in release builds.
- `check <pipeline>` - Verify that a compiled pipeline matches its source markdown
  - `<pipeline>` - Path to the pipeline YAML file to verify
  - The source markdown path is auto-detected from the `@ado-aw` header in the pipeline file
  - Useful for CI checks to ensure pipelines are regenerated after source changes
- `mcp <output_directory> <bounding_directory>` - Run SafeOutputs as a stdio MCP server
  - `--enabled-tools <name>` - Restrict available tools to those named (repeatable)
- `mcp-http <output_directory> <bounding_directory>` - Run SafeOutputs as an HTTP MCP server (for MCPG integration)
  - `--port <port>` - Port to listen on (default: 8100)
  - `--api-key <key>` - API key for authentication (auto-generated if not provided)
  - `--enabled-tools <name>` - Restrict available tools to those named (repeatable)
- `execute` - Execute safe outputs from Stage 1 (Stage 3 of pipeline)
  - `--source, -s <path>` - Path to source markdown file
  - `--safe-output-dir <path>` - Directory containing safe output NDJSON (default: current directory)
  - `--output-dir <path>` - Output directory for processed artifacts (e.g., agent memory)
  - `--ado-org-url <url>` - Azure DevOps organization URL override
  - `--ado-project <name>` - Azure DevOps project name override
  - `--dry-run` - Validate inputs but skip ADO API calls (useful for local testing and QA review)

- `run <path>` - Run an agent locally (local development mode). Orchestrates the full agent lifecycle: starts SafeOutputs, optionally starts MCPG via Docker, generates configs, runs copilot, and executes safe outputs.
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - Azure DevOps PAT for API access (passed to MCPG for ADO MCP, copilot env, and Stage 3 execution)
  - `--org <url>` - Azure DevOps organization URL (overrides auto-inference from git remote)
  - `--project <name>` - Azure DevOps project name
  - `--dry-run` - Skip ADO API calls in the execute stage
  - `--skip-mcpg` - Skip MCPG/Docker (only SafeOutputs MCP available; auto-enabled when Docker is unavailable)
  - `--output-dir <path>` - Output directory for safe outputs and artifacts (defaults to a temp directory)

- `configure` - Detect agentic pipelines in a local repository and update the `GITHUB_TOKEN` pipeline variable on their Azure DevOps build definitions
  - `--token <token>` / `GITHUB_TOKEN` env var - The new GITHUB_TOKEN value (prompted if omitted)
  - `--org <url>` - Override: Azure DevOps organization URL (inferred from git remote by default)
  - `--project <name>` - Override: Azure DevOps project name (inferred from git remote by default)
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (prompted if omitted)
  - `--path <path>` - Path to the repository root (defaults to current directory)
  - `--dry-run` - Preview changes without applying them
  - `--definition-ids <ids>` - Explicit pipeline definition IDs to update (comma-separated, skips auto-detection)

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

### Available Safe Output Tools

#### comment-on-work-item
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

#### create-work-item
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

#### update-work-item
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

#### create-pull-request
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

#### noop
Reports that no action was needed. Use this to provide visibility when analysis is complete but no changes or outputs are required.

**Agent parameters:**
- `context` - Optional context about why no action was taken

#### missing-data
Reports that data or information needed to complete the task is not available.

**Agent parameters:**
- `data_type` - Type of data needed (e.g., 'API documentation', 'database schema')
- `reason` - Why this data is required
- `context` - Optional additional context about the missing information

#### missing-tool
Reports that a tool or capability needed to complete the task is not available.

**Agent parameters:**
- `tool_name` - Name of the tool that was expected but not found
- `context` - Optional context about why the tool was needed

#### report-incomplete
Reports that a task could not be completed.

**Agent parameters:**
- `reason` - Why the task could not be completed (required, at least 10 characters)
- `context` - Optional additional context about what was attempted

#### add-pr-comment
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

#### reply-to-pr-comment
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

#### resolve-pr-thread
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

#### submit-pr-review
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

#### update-pr
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

#### link-work-items
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

#### queue-build
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

#### create-git-tag
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

#### add-build-tag
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

#### create-branch
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

#### upload-attachment
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

#### cache-memory (moved to `tools:`)
Memory is now configured as a first-class tool under `tools: cache-memory:` instead of `safe-outputs: memory:`. See the [Cache Memory](#cache-memory-cache-memory) section under Tools Configuration for details.

#### create-wiki-page
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

#### update-wiki-page
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

### Adding New Features

When extending the compiler:

1. **New CLI commands**: Add variants to the `Commands` enum in `main.rs`
2. **New compile targets**: Implement the `Compiler` trait in a new file under `src/compile/`
3. **New front matter fields**: Add fields to `FrontMatter` in `src/compile/types.rs`
4. **New template markers**: Handle replacements in the target-specific compiler (e.g., `standalone.rs` or `onees.rs`)
5. **New safe-output tools**: Add to `src/safeoutputs/` — implement `ToolResult`, `Executor`, register in `mod.rs`, `mcp.rs`, `execute.rs`
6. **New first-class tools**: Create `src/tools/<name>/` with `mod.rs` and `extension.rs` (CompilerExtension impl). Add `execute.rs` if the tool has Stage 3 runtime logic. Extend `ToolsConfig` in `types.rs`, add collection in `collect_extensions()`
7. **New runtimes**: Create `src/runtimes/<name>/` with `mod.rs` (config types) and `extension.rs` (CompilerExtension impl). Extend `RuntimesConfig` in `types.rs`, add collection in `collect_extensions()`
8. **Validation**: Add compile-time validation for safe outputs and permissions

#### Code Organization Principles

The codebase follows a **colocation** principle for tools and runtimes:

- **Tools** (`tools:` front matter) live in `src/tools/<name>/` — one directory per tool, containing both compile-time (`extension.rs`) and runtime (`execute.rs`) code. This means you can look at a single directory to understand everything a tool does.
- **Runtimes** (`runtimes:` front matter) live in `src/runtimes/<name>/` — one directory per runtime, with config types in `mod.rs` and the `CompilerExtension` impl in `extension.rs`.
- **Infrastructure extensions** (GitHub MCP, SafeOutputs MCP) that are always-on and not user-configured stay in `src/compile/extensions/`. These are internal plumbing, not user-facing tools.
- **Safe outputs** (`safe-outputs:` front matter) stay in `src/safeoutputs/` — they follow a different lifecycle (Stage 1 NDJSON → Stage 3 execution) and are not `CompilerExtension` implementations.

The `src/compile/extensions/mod.rs` file owns the `CompilerExtension` trait, the `Extension` enum, and `collect_extensions()`. It re-exports tool/runtime extension types from their colocated homes so the rest of the compiler can import them from a single path.

#### `CompilerExtension` Trait

Runtimes and first-party tools declare their compilation requirements via the `CompilerExtension` trait (`src/compile/extensions/mod.rs`). Instead of scattering special-case `if` blocks across the compiler, each runtime/tool implements this trait and the compiler collects requirements generically:

```rust
pub trait CompilerExtension: Send {
    fn name(&self) -> &str;                                    // Display name
    fn required_hosts(&self) -> Vec<String>;                   // AWF network allowlist
    fn required_bash_commands(&self) -> Vec<String>;           // Agent bash allow-list
    fn prompt_supplement(&self) -> Option<String>;              // Agent prompt markdown
    fn prepare_steps(&self) -> Vec<String>;                    // Pipeline steps (install, etc.)
    fn mcpg_servers(&self, ctx) -> Result<Vec<(String, McpgServerConfig)>>; // MCPG entries
    fn validate(&self, ctx) -> Result<Vec<String>>;            // Compile-time warnings
}
```

To add a new runtime or tool: (1) create a directory under `src/tools/` or `src/runtimes/`, (2) implement `CompilerExtension` in `extension.rs`, (3) add a variant to the `Extension` enum and a collection check in `collect_extensions()` in `src/compile/extensions/mod.rs`.

### Security Considerations

Following the gh-aw security model:

1. **Safe Outputs**: Only allow write operations through sanitized safe-output declarations
2. **Network Isolation**: Pipelines run in OneBranch's network-isolated environment
3. **Tool Allow-listing**: Agents have access to a limited, controlled set of tools
4. **Input Sanitization**: Validate and sanitize all inputs before transformation
5. **Permission Scoping**: Default to minimal permissions, require explicit elevation

## Testing

```bash
# Build the compiler
cargo build

# Run tests
cargo test

# Check for issues
cargo clippy
```

## Common Tasks

### Compile a markdown pipeline

```bash
cargo run -- compile ./path/to/agent.md
```

### Recompile all agentic pipelines in the current directory

```bash
# Auto-discovers and recompiles all detected agentic pipelines
cargo run -- compile
```

### Add a new dependency

```bash
cargo add <crate-name>
```

## File Naming Conventions

- Pipeline source files: `*.md` (markdown with YAML front matter)
- Compiled output: `*.yml` (Azure DevOps pipeline YAML)
- Rust source: `snake_case.rs`

## MCP Configuration

The `mcp-servers:` field configures MCP (Model Context Protocol) servers that are made available to the agent via the MCP Gateway (MCPG). MCPs can be **containerized stdio servers** (Docker-based) or **HTTP servers** (remote endpoints). All MCP traffic flows through the MCP Gateway.

### Docker Container MCP Servers (stdio)

Run containerized MCP servers. MCPG spawns these as sibling Docker containers:

```yaml
mcp-servers:
  azure-devops:
    container: "node:20-slim"
    entrypoint: "npx"
    entrypoint-args: ["-y", "@azure-devops/mcp", "myorg", "-d", "core", "work-items"]
    env:
      AZURE_DEVOPS_EXT_PAT: ""
    allowed:
      - core_list_projects
      - wit_get_work_item
      - wit_create_work_item
```

### HTTP MCP Servers (remote)

Connect to remote MCP servers accessible via HTTP:

```yaml
mcp-servers:
  remote-ado:
    url: "https://mcp.dev.azure.com/myorg"
    headers:
      X-MCP-Toolsets: "repos,wit"
      X-MCP-Readonly: "true"
    allowed:
      - wit_get_work_item
      - repo_list_repos_by_project
```

### Configuration Properties

**Container stdio servers:**
- `container:` - Docker image to run (e.g., `"node:20-slim"`, `"ghcr.io/org/tool:latest"`)
- `entrypoint:` - Container entrypoint override (equivalent to `docker run --entrypoint`)
- `entrypoint-args:` - Arguments passed to the entrypoint (after the image in `docker run`)
- `args:` - Additional Docker runtime arguments (inserted before the image in `docker run`). **Security note**: dangerous flags like `--privileged`, `--network host` will trigger compile-time warnings.
- `mounts:` - Volume mounts in `"source:dest:mode"` format (e.g., `["/host/data:/app/data:ro"]`)

**HTTP servers:**
- `url:` - HTTP endpoint URL for the remote MCP server
- `headers:` - HTTP headers to include in requests (e.g., `Authorization`, `X-MCP-Toolsets`)

**Common (both types):**
- `allowed:` - Array of tool names the agent is permitted to call (required for security)
- `env:` - Environment variables for the MCP server process. Use `""` (empty string) for passthrough from the pipeline environment.

### Environment Variable Passthrough

MCP containers may need secrets from the pipeline (e.g., ADO tokens). The `env:` field supports passthrough:

```yaml
env:
  AZURE_DEVOPS_EXT_PAT: ""        # Passthrough from pipeline environment
  STATIC_CONFIG: "some-value"     # Literal value embedded in config
```

When `permissions.read` is configured, the compiler automatically maps `SC_READ_TOKEN` → `AZURE_DEVOPS_EXT_PAT` on the MCPG container, so agents can access ADO APIs without manual wiring.

### Example: Azure DevOps MCP with Authentication

```yaml
mcp-servers:
  azure-devops:
    container: "node:20-slim"
    entrypoint: "npx"
    entrypoint-args: ["-y", "@azure-devops/mcp", "myorg"]
    env:
      AZURE_DEVOPS_EXT_PAT: ""
    allowed:
      - core_list_projects
      - wit_get_work_item
permissions:
  read: my-read-arm-connection
network:
  allowed:
    - "dev.azure.com"
    - "*.dev.azure.com"
```

### Security Notes

1. **Allow-listing**: Only tools explicitly listed in `allowed:` are accessible to agents
2. **Containerization**: Stdio MCP servers run as isolated Docker containers (per MCPG spec §3.2.1)
3. **Environment Isolation**: MCP containers are spawned by MCPG with only the configured environment variables
4. **MCPG Gateway**: All MCP traffic flows through the MCP Gateway which enforces tool-level filtering
5. **Network Isolation**: MCP containers run within the same AWF-isolated network. Users must explicitly allow external domains via `network.allowed`

## Network Isolation (AWF)

Network isolation is provided by AWF (Agentic Workflow Firewall), which provides L7 (HTTP/HTTPS) egress control using Squid proxy and Docker containers. AWF restricts network access to a whitelist of approved domains.

The `ado-aw` compiler binary is distributed via [GitHub Releases](https://github.com/githubnext/ado-aw/releases) with SHA256 checksum verification. The AWF binary is distributed via [GitHub Releases](https://github.com/github/gh-aw-firewall/releases) with SHA256 checksum verification. Docker is sourced via the `DockerInstaller@0` ADO task.

### Default Allowed Domains

The following domains are always allowed (defined in `allowed_hosts.rs`):

| Host Pattern | Purpose |
|-------------|---------|
| `dev.azure.com`, `*.dev.azure.com` | Azure DevOps |
| `vstoken.dev.azure.com` | Azure DevOps tokens |
| `vssps.dev.azure.com` | Azure DevOps identity |
| `*.visualstudio.com` | Visual Studio services |
| `*.vsassets.io` | Visual Studio assets |
| `*.vsblob.visualstudio.com` | Visual Studio blob storage |
| `*.vssps.visualstudio.com` | Visual Studio identity |
| `pkgs.dev.azure.com`, `*.pkgs.dev.azure.com` | Azure DevOps Artifacts/NuGet |
| `aex.dev.azure.com`, `aexus.dev.azure.com` | Azure DevOps CDN |
| `vsrm.dev.azure.com`, `*.vsrm.dev.azure.com` | Visual Studio Release Management |
| `github.com` | GitHub main site |
| `api.github.com` | GitHub API |
| `*.githubusercontent.com` | GitHub raw content |
| `*.github.com` | GitHub services |
| `*.copilot.github.com` | GitHub Copilot |
| `*.githubcopilot.com` | GitHub Copilot |
| `copilot-proxy.githubusercontent.com` | GitHub Copilot proxy |
| `login.microsoftonline.com` | Microsoft identity (OAuth) |
| `login.live.com` | Microsoft account authentication |
| `login.windows.net` | Azure AD authentication |
| `*.msauth.net`, `*.msftauth.net` | Microsoft authentication assets |
| `*.msauthimages.net` | Microsoft authentication images |
| `graph.microsoft.com` | Microsoft Graph API |
| `management.azure.com` | Azure Resource Manager |
| `*.blob.core.windows.net` | Azure Blob storage |
| `*.table.core.windows.net` | Azure Table storage |
| `*.queue.core.windows.net` | Azure Queue storage |
| `*.applicationinsights.azure.com` | Application Insights telemetry |
| `*.in.applicationinsights.azure.com` | Application Insights ingestion |
| `dc.services.visualstudio.com` | Visual Studio telemetry |
| `rt.services.visualstudio.com` | Visual Studio runtime telemetry |
| `config.edge.skype.com` | Configuration |
| `host.docker.internal` | MCP Gateway (MCPG) on host |

### Adding Additional Hosts

Agents can specify additional allowed hosts in their front matter using either ecosystem identifiers or raw domain patterns:

```yaml
network:
  allowed:
    - python                     # Ecosystem identifier — expands to Python/PyPI domains
    - rust                       # Ecosystem identifier — expands to Rust/crates.io domains
    - "*.mycompany.com"          # Raw domain pattern
    - "api.external-service.com" # Raw domain
```

#### Ecosystem Identifiers

Ecosystem identifiers are shorthand names that expand to curated domain lists for common language ecosystems and services. The domain lists are sourced from [gh-aw](https://github.com/github/gh-aw) and kept up to date via an automated workflow.

Available ecosystem identifiers include:

| Identifier | Includes |
|------------|----------|
| `defaults` | Certificate infrastructure, Ubuntu mirrors, common package registries |
| `github` | GitHub domains (`github.com`, `*.githubusercontent.com`, etc.) |
| `local` | Loopback addresses (`localhost`, `127.0.0.1`, `::1`) |
| `containers` | Docker Hub, GHCR, Quay, Kubernetes |
| `linux-distros` | Debian, Alpine, Fedora, CentOS, Arch Linux package repositories |
| `dev-tools` | CI/CD and developer tool services (Codecov, Shields.io, Snyk, etc.) |
| `python` | PyPI, pip, Conda, Anaconda |
| `rust` | crates.io, rustup, static.rust-lang.org |
| `node` | npm, Yarn, pnpm, Bun, Deno, Node.js |
| `go` | proxy.golang.org, pkg.go.dev, Go module proxy |
| `java` | Maven Central, Gradle, JDK downloads |
| `dotnet` | NuGet, .NET SDK |
| `ruby` | RubyGems, Bundler |
| `swift` | Swift.org, CocoaPods |
| `terraform` | HashiCorp releases, Terraform registry |

Additional ecosystems: `bazel`, `chrome`, `clojure`, `dart`, `deno`, `elixir`, `fonts`, `github-actions`, `haskell`, `julia`, `kotlin`, `lua`, `node-cdns`, `ocaml`, `perl`, `php`, `playwright`, `powershell`, `r`, `scala`, `zig`.

The full domain lists are defined in `src/data/ecosystem_domains.json`.

All hosts (core + MCP-specific + ecosystem expansions + user-specified) are combined into a comma-separated domain list passed to AWF's `--allow-domains` flag.

#### Blocking Hosts

The `network.blocked` field removes hosts from the combined allowlist. Both ecosystem identifiers and raw domain strings are supported. Blocking an ecosystem identifier removes all of its domains. Blocking a raw domain uses exact-string matching — blocking `"github.com"` does **not** also remove `"*.github.com"`.

```yaml
network:
  allowed:
    - python
    - node
  blocked:
    - python                 # Remove all Python ecosystem domains
    - "github.com"           # Remove exact domain
    - "*.github.com"         # Remove wildcard variant too
```

### Permissions (ADO Access Tokens)

ADO does not support fine-grained permissions — there are two access levels: blanket read and blanket write. Tokens are minted from ARM service connections; `System.AccessToken` is never used for agent or executor operations.

```yaml
permissions:
  read: my-read-arm-connection    # Stage 1 agent — read-only ADO access
  write: my-write-arm-connection  # Stage 3 executor — write access for safe-outputs
```

#### Security Model

- **`permissions.read`**: Mints a read-only ADO-scoped token given to the agent inside the AWF sandbox (Stage 1). The agent can query ADO APIs but cannot write.
- **`permissions.write`**: Mints a write-capable ADO-scoped token used **only** by the executor in Stage 3 (`Execution` job). This token is never exposed to the agent.
- **Both omitted**: No ADO tokens are passed anywhere. The agent has no ADO API access.

#### Compile-Time Validation

If write-requiring safe-outputs (`create-pull-request`, `create-work-item`) are configured but `permissions.write` is missing, compilation fails with a clear error message.

#### Examples

```yaml
# Agent can read ADO, safe-outputs can write
permissions:
  read: my-read-sc
  write: my-write-sc

# Agent can read ADO, no write safe-outputs needed
permissions:
  read: my-read-sc

# Agent has no ADO access, but safe-outputs can create PRs/work items
permissions:
  write: my-write-sc
```

## MCP Gateway (MCPG)

The MCP Gateway ([gh-aw-mcpg](https://github.com/github/gh-aw-mcpg)) is the upstream MCP routing layer that connects agents to their configured MCP servers. It replaces the previous custom MCP firewall with the standard gh-aw gateway implementation.

### Architecture

```
                          Host
┌─────────────────────────────────────────────────┐
│                                                 │
│  ┌──────────────┐     ┌──────────────────────┐  │
│  │ SafeOutputs  │     │  MCPG Gateway        │  │
│  │ HTTP Server  │◀────│  (Docker, --network   │  │
│  │ (ado-aw      │     │   host, port 80)     │  │
│  │  mcp-http)   │     │                      │  │
│  │ port 8100    │     │  Routes tool calls   │  │
│  └──────────────┘     │  to upstreams        │  │
│                       └──────────┬───────────┘  │
│                                  │              │
│          ┌─────────────────┐     │              │
│          │  Custom MCP     │◀────┘              │
│          │  (stdio server) │                    │
│          └─────────────────┘                    │
└─────────────────────────────────────────────────┘
                       │
          host.docker.internal:80
                       │
┌─────────────────────────────────────────────────┐
│                  AWF Container                   │
│                                                 │
│  ┌──────────┐                                   │
│  │  Copilot │──── HTTP ──── MCPG (via host)     │
│  │  Agent   │                                   │
│  └──────────┘                                   │
└─────────────────────────────────────────────────┘
```

### How It Works

1. **SafeOutputs HTTP server** starts on the host (port 8100) via `ado-aw mcp-http`
2. **MCPG container** starts on the host network (`docker run --network host`)
3. **MCPG config** (generated by the compiler) defines:
   - SafeOutputs as an HTTP backend (`type: "http"`, URL points to localhost:8100)
   - Custom MCPs as stdio servers (`type: "stdio"`, spawned by MCPG)
   - Gateway settings (port 80, API key, payload directory)
4. **Agent inside AWF** connects to MCPG via `http://host.docker.internal:80/mcp`
5. MCPG routes tool calls to the appropriate upstream (SafeOutputs or custom MCPs)
6. After the agent completes, MCPG and SafeOutputs are stopped

### MCPG Configuration Format

The compiler generates MCPG configuration JSON from the `mcp-servers:` front matter:

```json
{
  "mcpServers": {
    "safeoutputs": {
      "type": "http",
      "url": "http://localhost:8100/mcp",
      "headers": {
        "Authorization": "Bearer <api-key>"
      }
    },
    "custom-tool": {
      "type": "stdio",
      "container": "node:20-slim",
      "entrypoint": "node",
      "entrypointArgs": ["server.js"],
      "tools": ["process_data", "get_status"]
    }
  },
  "gateway": {
    "port": 80,
    "domain": "host.docker.internal",
    "apiKey": "<gateway-api-key>",
    "payloadDir": "/tmp/gh-aw/mcp-payloads"
  }
}
```

Runtime placeholders (`${SAFE_OUTPUTS_PORT}`, `${SAFE_OUTPUTS_API_KEY}`, `${MCP_GATEWAY_API_KEY}`) are substituted by the pipeline before passing the config to MCPG.

### Pipeline Integration

The MCPG is automatically configured in generated standalone pipelines:

1. **Config Generation**: The compiler generates `mcpg-config.json` from the agent's `mcp-servers:` front matter
2. **SafeOutputs Start**: `ado-aw mcp-http` starts as a background process on the host
3. **MCPG Start**: The MCPG Docker container starts on the host network with config via stdin
4. **Agent Execution**: AWF runs the agent with `--enable-host-access`, copilot connects to MCPG via HTTP
5. **Cleanup**: Both MCPG and SafeOutputs are stopped after the agent completes (condition: always)

The MCPG config is written to `$(Agent.TempDirectory)/staging/mcpg-config.json` in its own pipeline step, making it easy to inspect and debug.

## References

- [GitHub Agentic Workflows](https://github.com/githubnext/gh-aw) - Inspiration for this project
- [MCP Gateway (gh-aw-mcpg)](https://github.com/github/gh-aw-mcpg) - MCP routing gateway
- [AWF (gh-aw-firewall)](https://github.com/github/gh-aw-firewall) - Network isolation firewall
- [Azure DevOps YAML Schema](https://docs.microsoft.com/en-us/azure/devops/pipelines/yaml-schema)
- [OneBranch Documentation](https://aka.ms/onebranchdocs)
- [Clap Documentation](https://docs.rs/clap/latest/clap/)
- [Anyhow Documentation](https://docs.rs/anyhow/latest/anyhow/)
