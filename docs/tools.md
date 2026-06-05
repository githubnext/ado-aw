# Tools Configuration

_Part of the [ado-aw documentation](../AGENTS.md)._

## Tools Configuration

The `tools` field controls which tools are available to the agent. Both sub-fields are optional and have sensible defaults.

### Default Bash Command Allow-list

When `tools.bash` is omitted, the agent defaults to **unrestricted bash access** (`--allow-all-tools`). This matches gh-aw's sandbox behavior — since ado-aw agents always run inside the AWF sandbox, all tools are allowed by default.

### Configuring Bash Access

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

### Disabling File Writes

By default, the `edit` tool (file writing) is enabled. To disable it:

```yaml
tools:
  edit: false
```

### Cache Memory (`cache-memory:`)

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

### Azure DevOps MCP (`azure-devops:`)

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

## Built-in CLIs

Two CLI tools are always available to the agent's bash tool without
opting in. This mirrors gh-aw's "the runner has `gh`" assumption: the
host is presumed to have each binary pre-installed.

### Azure CLI (`az`)

Every compiled pipeline mounts the host's `az` binary into the AWF
container (`/opt/az` + `/usr/bin/az`, read-only) and adds the Azure
auth and management hosts (`login.microsoftonline.com`,
`login.windows.net`, `management.azure.com`, `graph.microsoft.com`,
`aka.ms`) to the AWF allowlist. The compiler does not install `az` —
the host is assumed to already have `azure-cli` installed.

| Host posture                          | What you get                                              |
| ------------------------------------- | --------------------------------------------------------- |
| Microsoft-hosted `ubuntu-latest`      | Works out of the box (`az` is pre-installed)              |
| 1ES self-hosted pool image            | Works if the pool operator baked `azure-cli` into the image |
| Host missing `/opt/az`                | AWF mount fails at runtime with a clear error             |

**Auth scope (important).** The compiler does not authenticate `az` for
general use. Two paths are supported:

1. **`az devops *` subcommands** (work items, repos, pipelines, etc.)
   are automatically authenticated via `AZURE_DEVOPS_EXT_PAT`, which
   the compiler populates inside AWF whenever `permissions.read` is
   configured. No extra steps needed.
2. **General `az` / ARM / Graph commands** (`az account get-access-token`,
   `az resource ...`, `az ad ...`, etc.) require their own
   authentication. The agent has no inherited cloud identity; you
   must `az login` explicitly (e.g. via a federated identity flow you
   provision yourself) before calling these commands.

A daily smoke pipeline at
[`tests/safe-outputs/azure-cli.md`](../tests/safe-outputs/azure-cli.md)
exercises this wiring (calls `az --version` and `az devops project list`
against the host org) — see its compiled lock file for the exact
generated YAML.

### GitHub CLI (`gh`)

The host's `gh` binary is similarly assumed to be present. The agent's
`GITHUB_TOKEN` (read-only) is wired in via the Copilot CLI's GitHub
MCP integration; calling `gh` directly from bash uses the same token.
