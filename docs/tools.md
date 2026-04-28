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
