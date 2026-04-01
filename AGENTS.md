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
│   ├── compile/          # Pipeline compilation module
│   │   ├── mod.rs        # Module entry point and Compiler trait
│   │   ├── common.rs     # Shared helpers across targets
│   │   ├── standalone.rs # Standalone pipeline compiler
│   │   ├── onees.rs      # 1ES Pipeline Template compiler
│   │   └── types.rs      # Front matter grammar and types
│   ├── create.rs         # Interactive agent creation wizard
│   ├── execute.rs        # Stage 2 safe output execution
│   ├── fuzzy_schedule.rs # Fuzzy schedule parsing
│   ├── logging.rs        # File-based logging infrastructure
│   ├── mcp.rs            # SafeOutputs MCP server
│   ├── mcp_firewall.rs   # MCP Firewall server
│   ├── mcp_metadata.rs   # Bundled MCP metadata
│   ├── ndjson.rs         # NDJSON parsing utilities
│   ├── proxy.rs          # Network proxy implementation
│   ├── sanitize.rs       # Input sanitization for safe outputs
│   └── tools/            # MCP tool implementations
│       ├── mod.rs
│       ├── comment_on_work_item.rs
│       ├── create_pr.rs
│       ├── create_wiki_page.rs
│       ├── create_work_item.rs
│       ├── update_work_item.rs
│       ├── update_wiki_page.rs
│       ├── memory.rs
│       ├── missing_data.rs
│       ├── missing_tool.rs
│       ├── noop.rs
│       └── result.rs
├── templates/
│   ├── base.yml          # Base pipeline template for standalone
│   ├── 1es-base.yml      # Base pipeline template for 1ES target
│   └── threat-analysis.md # Threat detection analysis prompt template
├── mcp-metadata.json     # Bundled MCP tool definitions
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
engine: claude-opus-4.5 # AI engine to use. Defaults to claude-opus-4.5. Other options include claude-sonnet-4.5, gpt-5.2-codex, gemini-3-pro-preview, etc.
# engine:                        # Alternative object format (with additional options)
#   model: claude-opus-4.5
#   max-turns: 50
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
# env:                          # RESERVED: workflow-level environment variables (not yet implemented)
#   CUSTOM_VAR: "value"
mcp-servers:
  ado: true                    # built-in, enabled with defaults
  bluebird: true
  es-chat: true
  msft-learn: true
  icm:
    allowed:                   # built-in with restricted functions
      - create_incident
      - get_incident
  kusto:
    allowed:
      - query
  my-custom-tool:              # custom MCP server (has command field)
    command: "node"
    args: ["path/to/mcp-server.js"]
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
  allow:                       # additional allowed host patterns
    - "*.mycompany.com"
  blocked:                     # blocked host patterns (takes precedence over allow)
    - "evil.example.com"
permissions:                   # optional ADO access token configuration
  read: my-read-arm-connection   # ARM service connection for read-only ADO access (Stage 1 agent)
  write: my-write-arm-connection # ARM service connection for write ADO access (Stage 2 executor only)
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

### Tools Configuration

The `tools` field controls which tools are available to the agent. Both sub-fields are optional and have sensible defaults.

#### Default Bash Command Allow-list

When `tools.bash` is omitted, the agent can invoke the following shell commands:

```
cat, date, echo, grep, head, ls, pwd, sort, tail, uniq, wc, yq
```

#### Configuring Bash Access

```yaml
# Default: safe built-in command list (bash field omitted)
tools:
  edit: true

# Unrestricted bash access (use with caution)
tools:
  bash: [":*"]

# Explicit command allow-list
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

### Target Platforms

The `target` field in the front matter determines the output format and execution environment for the compiled pipeline.

#### `standalone` (default)

Generates a self-contained Azure DevOps pipeline with:
- Full 3-job pipeline: `PerformAgenticTask` → `AnalyzeSafeOutputs` → `ProcessSafeOutputs`
- AWF (Agentic Workflow Firewall) L7 domain whitelisting via Squid proxy + Docker
- MCP firewall with tool-level filtering and custom MCP server support
- Setup/teardown job support
- All safe output features (create-pull-request, create-work-item, etc.)

This is the recommended target for maximum flexibility and security controls.

#### `1es`

Generates a pipeline that extends the 1ES Unofficial Pipeline Template:
- Uses `templateContext.type: agencyJob` for the main agent job
- Integrates with 1ES SDL scanning and compliance tools
- Custom jobs for threat analysis and safe output processing
- **Limitations:**
  - MCP servers use service connections (no custom `command:` support)
  - Network isolation is handled by OneBranch (no custom proxy allow-lists)
  - Requires 1ES Pipeline Templates repository access

Example:
```yaml
target: 1es
```

When using `target: 1es`, the pipeline will extend `1es/1ES.Unofficial.PipelineTemplate.yml@1ESPipelinesTemplates` and MCPs will require corresponding service connections (naming convention: `mcp-<name>-service-connection`).

### Output Format (Azure DevOps YAML)

The compiler transforms the input into valid Azure DevOps pipeline YAML based on the target platform:

- **Standalone**: Uses `templates/base.yml`
- **1ES**: Uses `templates/1es-base.yml`

Explicit markings are embedded in these templates that the compiler is allowed to replace e.g. `{{ agency_params }}` denotes parameters which are passed to the agency command line tool. The compiler should not replace sections denoted by `${{ some content }}`. What follows is a mapping of markings to responsibilities (primarily for the standalone template).

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

All checkout steps across all jobs (PerformAgenticTask, AnalyzeSafeOutputs, ProcessSafeOutputs, SetupJob, TeardownJob) use this marker.

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

## {{ agency_params }}

Additional params provided to agency CLI. The compiler generates:
- `--model <model>` - AI model from `engine` front matter field (default: claude-opus-4.5)
- `--disable-builtin-mcps` - Disables all built-in MCPs initially
- `--no-ask-user` - Prevents interactive prompts
- `--allow-tool <tool>` - Explicitly allows specific tools (github, safeoutputs, write, shell commands like cat, date, echo, grep, head, ls, pwd, sort, tail, uniq, wc, yq)
- `--disable-mcp-server <name>` - Disables specific MCPs (all built-in MCPs are disabled by default and must be explicitly enabled via mcp-servers config)
- `--mcp <name>` - Enables MCPs specified in front matter

Only built-in MCPs are passed via params. Custom MCPs (with command field) are handled separately.

## {{ pool }}

Should be replaced with the agent pool name from the `pool` front matter field. Defaults to `AZS-1ES-L-MMS-ubuntu-22.04` if not specified.

The pool configuration accepts both string and object formats:
- **String format**: `pool: AZS-1ES-L-MMS-ubuntu-22.04`
- **Object format**: `pool: { name: AZS-1ES-L-MMS-ubuntu-22.04, os: linux }`

The `os` field (defaults to "linux") is primarily used for 1ES target compatibility.

## {{ setup_job }}

Generates a separate setup job YAML if `setup` contains steps. The job:
- Runs before `PerformAgenticTask`
- Uses the same pool as the main agentic task
- Includes a checkout of self
- Display name: `<agent_name> - Setup`

If `setup` is empty, this is replaced with an empty string.

## {{ teardown_job }}

Generates a separate teardown job YAML if `teardown` contains steps. The job:
- Runs after `ProcessSafeOutputs` (depends on it)
- Uses the same pool as the main agentic task
- Includes a checkout of self
- Display name: `<agent_name> - Teardown`

If `teardown` is empty, this is replaced with an empty string.

## {{ prepare_steps }}

Generates inline steps that run inside the `PerformAgenticTask` job, **before** the agent runs. These steps can generate context files, fetch secrets, or prepare the workspace for the agent.

Steps are inserted after the agent prompt is prepared but before AWF network isolation starts.

If `steps` is empty, this is replaced with an empty string.

## {{ finalize_steps }}

Generates inline steps that run inside the `PerformAgenticTask` job, **after** the agent completes. These steps can validate outputs, process workspace artifacts, or perform cleanup.

Steps are inserted after the AWF-isolated agent completes but before logs are collected.

If `post-steps` is empty, this is replaced with an empty string.

## {{ agentic_depends_on }}

Generates a `dependsOn: SetupJob` clause for `PerformAgenticTask` if a setup job is configured. The setup job is identified by the job name `SetupJob`, ensuring the agentic task waits for the setup job to complete.

If no setup job is configured, this is replaced with an empty string.

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

This is used for the `workingDirectory` property of the agency copilot task.

## {{ source_path }}

Should be replaced with the path to the agent markdown source file for Stage 2 execution. The path is relative to the workspace and depends on the effective workspace setting (see `{{ working_directory }}` for resolution logic):
- `root`: `$(Build.SourcesDirectory)/agents/<filename>.md`
- `repo`: `$(Build.SourcesDirectory)/$(Build.Repository.Name)/agents/<filename>.md`

Used by the execute command's --source parameter.

## {{ pipeline_path }}

Should be replaced with the path to the compiled pipeline YAML file for runtime integrity checking. The path is derived from the output path's filename and uses `{{ working_directory }}` as the base (which gets resolved before this placeholder):
- `root`: `$(Build.SourcesDirectory)/<filename>.yml`
- `repo`: `$(Build.SourcesDirectory)/$(Build.Repository.Name)/<filename>.yml`

Used by the pipeline's integrity check step to verify the pipeline hasn't been modified outside the compilation process.

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

## {{ firewall_config }}

Should be replaced with the MCP firewall configuration JSON generated from the `mcp-servers:` front matter. This configuration defines which MCP servers to spawn and which tools are allowed for each upstream.

## {{ allowed_domains }}

Should be replaced with the comma-separated domain list for AWF's `--allow-domains` flag. The list includes:
1. Core Azure DevOps/GitHub endpoints (from `allowed_hosts.rs`)
2. MCP-specific endpoints for each enabled MCP
3. User-specified additional hosts from `network.allow:` front matter

The output is formatted as a comma-separated string (e.g., `github.com,*.dev.azure.com,api.github.com`).

## {{ cancel_previous_builds }}

When `triggers.pipeline` is configured, this generates a bash step that cancels any previously queued or in-progress builds of the same pipeline definition. This prevents multiple builds from accumulating when the upstream pipeline triggers rapidly (e.g., multiple PRs merged in quick succession).

The step:
- Uses the Azure DevOps REST API to query builds for the current pipeline definition
- Filters to only `notStarted` and `inProgress` builds
- Excludes the current build from cancellation
- Cancels each older build via PATCH request

Example output:
```yaml
- bash: |
    CURRENT_BUILD_ID=$(Build.BuildId)
    BUILDS=$(curl -s -u ":$SYSTEM_ACCESSTOKEN" \
      "$(System.CollectionUri)$(System.TeamProject)/_apis/build/builds?definitions=$(System.DefinitionId)&statusFilter=notStarted,inProgress&api-version=7.1" \
      | jq -r --arg current "$CURRENT_BUILD_ID" '.value[] | select(.id != ($current | tonumber)) | .id')
    # ... cancels each build
  displayName: "Cancel previous queued builds"
  env:
    SYSTEM_ACCESSTOKEN: $(System.AccessToken)
```

## {{ threat_analysis_prompt }}

Should be replaced with the embedded threat detection analysis prompt from `templates/threat-analysis.md`. This prompt template includes markers for `{{ source_path }}`, `{{ agent_name }}`, `{{ agent_description }}`, and `{{ working_directory }}` which are replaced during compilation.

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

## {{ copilot_ado_env }}

Generates environment variable entries for the copilot AWF step when `permissions.read` is configured. Sets both `AZURE_DEVOPS_EXT_PAT` and `SYSTEM_ACCESSTOKEN` to the read service connection token (`SC_READ_TOKEN`).

If `permissions.read` is not configured, this marker is replaced with an empty string, and ADO access tokens are omitted from the copilot invocation.

## {{ acquire_write_token }}

Generates an `AzureCLI@2` step that acquires a write-capable ADO-scoped access token from the ARM service connection specified in `permissions.write`. This token is used only by the executor in Stage 2 (`ProcessSafeOutputs` job) and is never exposed to the agent.

The step:
- Uses the ARM service connection from `permissions.write`
- Calls `az account get-access-token` with the ADO resource ID
- Stores the token in a secret pipeline variable `SC_WRITE_TOKEN`

If `permissions.write` is not configured, this marker is replaced with an empty string.

## {{ executor_ado_env }}

Generates environment variable entries for the Stage 2 executor step when `permissions.write` is configured. Sets `SYSTEM_ACCESSTOKEN` to the write service connection token (`SC_WRITE_TOKEN`).

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

## {{ copilot_version }}

Should be replaced with the pinned version of the `Microsoft.Copilot.CLI.linux-x64` NuGet package (defined as `COPILOT_CLI_VERSION` constant in `src/compile/common.rs`). This version is used in the pipeline step that installs the Copilot CLI tool from Azure Artifacts.

The generated pipelines install the package from:
```
https://pkgs.dev.azure.com/msazuresphere/_packaging/Guardian1ESPTUpstreamOrgFeed/nuget/v3/index.json
```

### 1ES-Specific Template Markers

The following markers are specific to the 1ES target (`target: 1es`) and are not used in standalone pipelines:

## {{ agent_context_root }}

Should be replaced with the agent context root for 1ES Agency jobs. This determines the working directory context for the agent:
- `repo`: `$(Build.Repository.Name)` - the repository subfolder
- `root`: `.` - the checkout root

## {{ mcp_configuration }}

Should be replaced with the MCP server configuration for 1ES templates. For each enabled built-in MCP, generates service connection references:

```yaml
ado:
  serviceConnection: mcp-ado-service-connection
kusto:
  serviceConnection: mcp-kusto-service-connection
```

Custom MCP servers (with `command:` field) are not supported in 1ES target. Only built-in MCPs with corresponding service connections are supported.

## {{ global_options }}

Reserved for future use. Currently replaced with an empty string.

## {{ log_level }}

Reserved for future use. Currently replaced with an empty string.

### CLI Commands

Global flags (apply to all subcommands): `--verbose, -v` (enable info-level logging), `--debug, -d` (enable debug-level logging, implies verbose)

- `create` - Interactively create a new agent markdown file
  - `--output, -o <path>` - Output directory for the generated file (defaults to current directory)
  - Guides you through: name, description, engine selection, schedule, workspace, repositories, checkout, and MCPs
  - The generated file includes a placeholder for agent instructions that you edit directly
- `compile <path>` - Compile a markdown file to Azure DevOps pipeline YAML
  - `--output, -o <path>` - Optional output path for generated YAML
- `check <pipeline>` - Verify that a compiled pipeline matches its source markdown
  - `<pipeline>` - Path to the pipeline YAML file to verify
  - The source markdown path is auto-detected from the `@ado-aw` header in the pipeline file
  - Useful for CI checks to ensure pipelines are regenerated after source changes
- `mcp <output_directory> <bounding_directory>` - Run as an MCP server for safe outputs
- `execute` - Execute safe outputs from Stage 1 (Stage 2 of pipeline)
  - `--source, -s <path>` - Path to source markdown file
  - `--safe-output-dir <path>` - Directory containing safe output NDJSON (default: current directory)
  - `--output-dir <path>` - Output directory for processed artifacts (e.g., agent memory)
  - `--ado-org-url <url>` - Azure DevOps organization URL override
  - `--ado-project <name>` - Azure DevOps project name override
- `proxy` - Start an HTTP proxy for network filtering
  - `--allow <host>` - Allowed hosts (supports wildcards, can be repeated)
- `mcp-firewall` - Start an MCP firewall server that proxies tool calls
  - `--config, -c <path>` - Path to firewall configuration JSON file

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

Safe output configurations are passed to Stage 2 execution and used when processing safe outputs.

### Available Safe Output Tools

#### comment-on-work-item
Adds a comment to an existing Azure DevOps work item. This is the ADO equivalent of gh-aw's `add-comment` tool.

**Agent parameters:**
- `work_item_id` - The work item ID to comment on (required, must be positive)
- `body` - Comment text in markdown format (required, must be at least 10 characters)

**Configuration options (front matter):**
- `max` - Maximum number of comments per run (default: 1)
- `target` - **Required** — scoping policy for which work items can be commented on:
  - `"*"` - Any work item in the project (unrestricted, must be explicit)
  - `12345` - A specific work item ID
  - `[12345, 67890]` - A list of allowed work item IDs
  - `"Some\\Path"` - Work items under the specified area path prefix (any string that isn't `"*"`, validated via ADO API at Stage 2)

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

During Stage 2 execution, the repository is validated against the allowed list (from `checkout:` + "self"), then the patch is applied and a PR is created in Azure DevOps.

**Stage 2 Execution Architecture (Hybrid Git + ADO API):**

```
┌─────────────────────────────────────────────────────────────────┐
│                        Stage 2 Execution                        │
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

#### memory
Provides persistent memory across agent runs. When enabled, the agent can read and write files to a memory directory that persists between pipeline executions.

**Configuration options (front matter):**
```yaml
safe-outputs:
  memory:
    allowed-extensions:    # Optional: restrict file types (defaults to all)
      - .md
      - .json
      - .txt
```

**How it works:**
1. During Stage 1 (agent execution), the agent can write files to `/tmp/awf-tools/staging/agent_memory/`
2. A prompt is automatically appended to inform the agent about its memory location
3. During Stage 2 execution, memory files are validated and sanitized:
   - Path traversal attempts are blocked
   - Files are checked for `##vso[` command injection
   - Total size is limited to 5 MB
   - File extensions can be restricted via configuration
4. Sanitized memory files are published as a pipeline artifact
5. On the next run, the previous memory is downloaded and restored to the staging directory

**Security validations:**
- Maximum total memory size: 5 MB
- Path validation: no `..`, `.git`, absolute paths, or null bytes
- Content validation: text files are scanned for `##vso[` commands
- Extension filtering: can restrict to specific file types

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
    path-prefix: "/agent-output"    # Optional — prepended to the agent-supplied path (restricts write scope)
    title-prefix: "[Agent] "        # Optional — prepended to the last path segment (the page title)
    comment: "Created by agent"     # Optional — default commit comment when agent omits one
    max: 1                          # Maximum number of create-wiki-page outputs allowed per run (default: 1)
```

Note: `wiki-name` is required. If it is not set, execution fails with an explicit error message.

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
    path-prefix: "/agent-output"    # Optional — prepended to the agent-supplied path (restricts write scope)
    title-prefix: "[Agent] "        # Optional — prepended to the last path segment (the page title)
    comment: "Updated by agent"     # Optional — default commit comment when agent omits one
    max: 1                          # Maximum number of update-wiki-page outputs allowed per run (default: 1)
```

Note: `wiki-name` is required. If it is not set, execution fails with an explicit error message.

### Adding New Features

When extending the compiler:

1. **New CLI commands**: Add variants to the `Commands` enum in `main.rs`
2. **New compile targets**: Implement the `Compiler` trait in a new file under `src/compile/`
3. **New front matter fields**: Add fields to `FrontMatter` in `src/compile/types.rs`
4. **New template markers**: Handle replacements in the target-specific compiler (e.g., `standalone.rs` or `onees.rs`)
5. **Validation**: Add compile-time validation for safe outputs and permissions

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

### Add a new dependency

```bash
cargo add <crate-name>
```

## File Naming Conventions

- Pipeline source files: `*.md` (markdown with YAML front matter)
- Compiled output: `*.yml` (Azure DevOps pipeline YAML)
- Rust source: `snake_case.rs`

## MCP Configuration

The `mcp-servers:` field provides a unified way to configure both built-in and custom MCP (Model Context Protocol) servers. The compiler distinguishes between them by checking for the `command:` field—if present, it's a custom server; otherwise, it's a built-in.

### Built-in MCP Servers

Enable built-in servers with `true` or configure them with options:

```yaml
mcp-servers:
  ado: true                    # enabled with all default functions
  ado-ext: true                # Extended ADO functionality
  asa: true                    # Azure Stream Analytics MCP
  bluebird: true               # Bluebird MCP
  calculator: true             # Calculator MCP
  es-chat: true
  icm:                         # enabled with restricted functions
    allowed:
      - create_incident
      - get_incident
  kusto:
    allowed:
      - query
  msft-learn: true
  stack: true                  # Stack MCP
```

### Custom MCP Servers

Define custom servers by including a `command:` field:

```yaml
mcp-servers:
  my-custom-tool:
    command: "node"
    args: ["path/to/mcp-server.js"]
    allowed:
      - custom_function_1
      - custom_function_2
```

### Configuration Properties

**For built-in MCPs:**
- `true` - Enable with all default functions
- `allowed:` - Array of function names to restrict available tools
- `service-connection:` - (1ES target only) Override the service connection name used for this MCP. If not specified, defaults to `mcp-<name>-service-connection` (e.g., `mcp-ado-service-connection` for the `ado` MCP)

**For custom MCPs (requires `command:`):**
- `command:` - The executable to run (e.g., `"node"`, `"python"`, `"dotnet"`)
- `args:` - Array of command-line arguments passed to the command
- `allowed:` - Array of function names agents are permitted to call (required for security)
- `env:` - Optional environment variables for the MCP server process

### Example: Mixed Configuration

```yaml
mcp-servers:
  # Built-in servers
  ado: true
  ado-ext: true
  es-chat: true
  icm:
    allowed: [create_incident, get_incident]

  # Custom Python MCP server
  data-processor:
    command: "python"
    args: ["-m", "my_mcp_server"]
    env:
      DATA_DIR: "/data"
    allowed:
      - process_data
      - query_database

  # Custom .NET MCP server
  azure-tools:
    command: "dotnet"
    args: ["./tools/AzureMcp.dll"]
    allowed:
      - list_resources
      - get_deployment_status
```

### Security Notes

1. **Allow-listing**: Only functions explicitly listed in `allowed:` are accessible to agents
2. **Command Validation**: The compiler validates that commands are from a trusted set
3. **Argument Sanitization**: Arguments are validated to prevent injection attacks
4. **Environment Isolation**: MCP servers run in the same isolated sandbox as the pipeline
5. **Built-in Trust**: Built-in MCPs are pre-vetted; custom MCPs require explicit `allowed:` list

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
| `config.edge.skype.com` | Agency configuration |

### Adding Additional Hosts

Agents can specify additional allowed hosts in their front matter:

```yaml
network:
  allow:
    - "*.mycompany.com"
    - "api.external-service.com"
```

All hosts (core + MCP-specific + user-specified) are combined into a comma-separated domain list passed to AWF's `--allow-domains` flag.

### Permissions (ADO Access Tokens)

ADO does not support fine-grained permissions — there are two access levels: blanket read and blanket write. Tokens are minted from ARM service connections; `System.AccessToken` is never used for agent or executor operations.

```yaml
permissions:
  read: my-read-arm-connection    # Stage 1 agent — read-only ADO access
  write: my-write-arm-connection  # Stage 2 executor — write access for safe-outputs
```

#### Security Model

- **`permissions.read`**: Mints a read-only ADO-scoped token given to the agent inside the AWF sandbox (Stage 1). The agent can query ADO APIs but cannot write.
- **`permissions.write`**: Mints a write-capable ADO-scoped token used **only** by the executor in Stage 2 (`ProcessSafeOutputs` job). This token is never exposed to the agent.
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

## MCP Firewall

The MCP Firewall is a security layer that acts as a filtering proxy between agents and their configured MCP servers. It provides policy-based access control and audit logging for all tool calls.

### Purpose

When agents are configured with multiple MCPs (e.g., `ado`, `kusto`, `icm`), the firewall:

1. **Loads tool definitions** from pre-generated metadata (`mcp-metadata.json`)
2. **Enforces allow-lists** - only exposes tools explicitly permitted in the config
3. **Namespaces tools** - tools appear as `upstream:tool_name` (e.g., `icm:create_incident`)
4. **Spawns upstream MCPs lazily** as child processes when tools are actually called
5. **Routes tool calls** to the appropriate upstream server
6. **Logs all attempts** for security auditing

### Architecture

```
┌─────────────┐     ┌──────────────────┐     ┌─────────────────┐
│             │     │                  │     │  ado MCP        │
│   Agent     │────▶│   MCP Firewall   │────▶│  (agency mcp ado)│
│  (Agency)   │     │                  │     └─────────────────┘
│             │     │  - Policy check  │     ┌─────────────────┐
└─────────────┘     │  - Tool routing  │────▶│  icm MCP        │
                    │  - Audit logging │     │  (agency mcp icm)│
                    └──────────────────┘     └─────────────────┘
                                             ┌─────────────────┐
                                        ────▶│  custom MCP     │
                                             │  (node server.js)│
                                             └─────────────────┘
```

### Configuration File Format

The firewall reads a JSON configuration file at runtime:

```json
{
  "upstreams": {
    "ado": {
      "command": "agency",
      "args": ["mcp", "ado"],
      "env": {},
      "allowed": ["*"]
    },
    "icm": {
      "command": "agency",
      "args": ["mcp", "icm"],
      "env": {},
      "allowed": ["create_incident", "get_incident"]
    },
    "kusto": {
      "command": "agency",
      "args": ["mcp", "kusto"],
      "env": {},
      "allowed": ["query"]
    },
    "custom-tool": {
      "command": "node",
      "args": ["server.js"],
      "env": { "NODE_ENV": "production" },
      "allowed": ["process_data", "get_status"],
      "spawn_timeout_secs": 60
    }
  }
}
```

### Configuration Properties (Firewall)

Each upstream configuration supports:

| Property | Required | Default | Description |
|----------|----------|---------|-------------|
| `command` | Yes | - | The executable to spawn |
| `args` | No | `[]` | Arguments passed to the command |
| `env` | No | `{}` | Environment variables for the process |
| `allowed` | Yes | - | Tool names allowed (supports `"*"` and prefix wildcards) |
| `spawn_timeout_secs` | No | `30` | Timeout in seconds for spawning and initializing the MCP server |

### Allow-list Patterns

The `allowed` field supports several patterns:

| Pattern | Description | Example |
|---------|-------------|---------|
| `"*"` | Allow all tools from this upstream | `["*"]` |
| `"exact_name"` | Allow only this specific tool | `["query", "execute"]` |
| `"prefix_*"` | Allow tools starting with prefix | `["get_*", "list_*"]` |

### Tool Namespacing

All tools exposed by the firewall are namespaced with their upstream name:

- `ado:create-work-item` - from the `ado` upstream
- `icm:create_incident` - from the `icm` upstream
- `kusto:query` - from the `kusto` upstream

This prevents tool name collisions and makes it clear which upstream handles each call.

### CLI Usage

```bash
# Start the MCP firewall server
ado-aw mcp-firewall --config /path/to/config.json
```

### Pipeline Integration

The firewall is automatically configured in generated pipelines:

1. **Config Generation**: The compiler generates `mcp-firewall-config.json` from the agent's `mcp-servers:` front matter
2. **MCP Registration**: The firewall is registered in the agency MCP config as `mcp-firewall`
3. **Runtime Launch**: When agency starts, it launches the firewall which spawns upstream MCPs

The firewall config is written to `$(Agent.TempDirectory)/staging/mcp-firewall-config.json` in its own pipeline step, making it easy to inspect and debug.

### Audit Logging

All tool call attempts are logged to the centralized log file at `$HOME/.ado-aw/logs/YYYY-MM-DD.log`:

```
[2026-01-29T10:15:32Z] [INFO] [firewall] ALLOWED icm:create_incident (args: {"title": "...", "severity": 3})
[2026-01-29T10:15:45Z] [INFO] [firewall] BLOCKED icm:delete_incident (not in allowlist)
[2026-01-29T10:16:01Z] [INFO] [firewall] ALLOWED kusto:query (args: {"cluster": "...", "query": "..."})
```

This provides a complete audit trail of agent actions for security review.

### Error Handling

- **Upstream spawn failure**: If an upstream fails to start, the firewall continues with remaining upstreams (partial functionality)
- **Tool not found**: Returns an MCP error if the requested tool doesn't exist
- **Policy violation**: Returns an MCP error if the tool exists but isn't in the allow-list
- **Upstream error**: Propagates errors from upstream MCPs back to the agent

## References

- [GitHub Agentic Workflows](https://github.com/githubnext/gh-aw) - Inspiration for this project
- [Azure DevOps YAML Schema](https://docs.microsoft.com/en-us/azure/devops/pipelines/yaml-schema)
- [OneBranch Documentation](https://aka.ms/onebranchdocs)
- [Clap Documentation](https://docs.rs/clap/latest/clap/)
- [Anyhow Documentation](https://docs.rs/anyhow/latest/anyhow/)
