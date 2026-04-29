# Agent File Format (Markdown + YAML Front Matter)

_Part of the [ado-aw documentation](../AGENTS.md)._

## Input Format (Markdown with Front Matter)

The compiler expects markdown files with YAML front matter similar to gh-aw:

```markdown
---
name: "name for this agent"
description: "One line description for this agent"
target: standalone # Optional: "standalone" (default) or "1es". See docs/targets.md.
engine: copilot # Engine identifier. Defaults to copilot. Currently only 'copilot' (GitHub Copilot CLI) is supported.
# engine:                        # Alternative object format (with additional options)
#   id: copilot
#   model: claude-opus-4.7
#   timeout-minutes: 30
schedule: daily around 14:00 # Fuzzy schedule syntax - see docs/schedule-syntax.md
# schedule:                       # Alternative object format (with branch filtering)
#   run: daily around 14:00
#   branches:
#     - main
#     - release/*
workspace: repo # Optional: "root", "repo" (alias: "self"), or a checked-out repository alias. If not specified, defaults to "root" when no additional repositories are listed in `checkout:`, and to "repo" when one or more additional repos are checked out. See "Workspace Defaults" below.
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

## Workspace Defaults

The `workspace:` field controls which directory the agent runs in. When it is
not set explicitly, the compiler chooses a default based on the `checkout:`
list:

- If `checkout:` is empty (i.e. only the pipeline's own repository is checked
  out via the implicit `self`), `workspace:` defaults to **`root`** — the
  agent runs in the pipeline's working directory root.
- If `checkout:` lists one or more additional repository aliases,
  `workspace:` defaults to **`repo`** — the agent runs inside the first
  checked-out repository's directory.

Set `workspace:` explicitly to `root`, `repo` (alias `self`), or a specific
checked-out repository alias to override this behavior.
