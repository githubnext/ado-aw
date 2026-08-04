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

First-class Azure DevOps MCP integration. Auto-configures the ADO MCP
container, credential-isolated policy proxy, and MCPG entry.

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
- Requires `permissions.read` as the trusted proxy's token source
- Installs the pinned `@azure-devops/mcp` package on the runner and mounts it
  read-only into an unchanged `node:20-slim` container; the isolated container
  needs no npm registry access
- Runs that container on an internal network with `dev.azure.com` redirected
  to `ado-proxy` and a public interception CA trusted only by that process
- Gives the MCP a non-secret sentinel in `ADO_MCP_AUTH_TOKEN`; the real token
  exists only in `ado-proxy`, which strips client credentials and attaches its
  bearer after an allow decision
- Auto-infers org from the git remote URL at compile time (overridable via `org:` field)
- Fails compilation if org cannot be determined (no explicit override and no ADO git remote)

The generated `az` wrapper similarly carries only a sentinel PAT and routes
Azure DevOps traffic through the proxy. Catalogued reads (`az devops`,
`az repos`, `az pipelines`, `az boards`, and `az rest`) work without signing
in; writes and secret-bearing route families fail closed.

## Built-in CLIs

Two CLI tools are always available to the agent's bash tool without
opting in. This mirrors gh-aw's "the runner has `gh`" assumption: the
host is presumed to have each binary pre-installed.

### Azure CLI (`az`)

Every compiled pipeline adds the Azure auth and management hosts
(`login.microsoftonline.com`, `login.windows.net`,
`management.azure.com`, `graph.microsoft.com`, `aka.ms`) to the AWF
allowlist and emits a *Detect Azure CLI on host* prepare step in the
Agent job. The compiler does not install `az`.

**Runtime detection + graceful degradation.** The detection step does
two things at pipeline time:

1. If `/usr/bin/az` (the launcher shim) and `/opt/az` (the Python
   venv that `az` runs in) both exist on the runner, it sets the
   pipeline variable
   `AW_AZ_MOUNTS=--mount /opt/az:/opt/az:ro --mount /usr/bin/az:/usr/bin/az:ro`.
2. If either is missing, it emits a yellow ADO warning
   (`##vso[task.logissue type=warning]`) and sets the variable to
   the *empty string* (leaving it undefined would make ADO render
   the literal `$(AW_AZ_MOUNTS)` in the AWF bash step, where bash
   would interpret it as command substitution and kill the step
   under `set -e`).

The AWF invocation includes a `$(AW_AZ_MOUNTS) \` line in its
`--mount` chain. ADO expands the variable at step start: present →
the two mounts appear; absent → the line collapses to nothing. No
static `--mount` is emitted for `/opt/az` or `/usr/bin/az`, so the
pipeline never crashes `docker run` with "bind source path does not
exist" on runners without `az`. See
[`docs/network.md`](network.md#always-on-azure-cli-az) for the full
design.

**Conditional agent prompt advisory.** When (and only when) `az` is
detected, a follow-up *Append Azure CLI prompt* step appends an
Azure CLI advisory section to the agent prompt. The agent then knows
`az` is on PATH and what it's good for (use cases and auth model
below). The step is gated by
`condition: ne(variables['AW_AZ_MOUNTS'], '')`; on runners without
`az` it is skipped and the agent never sees Azure CLI guidance —
preventing "told to use `az`, fails with command not found" loops.

| Host posture                          | What you get                                              |
| ------------------------------------- | --------------------------------------------------------- |
| Microsoft-hosted `ubuntu-latest`      | Detected → mounted → `az` available in the sandbox        |
| 1ES self-hosted pool with `azure-cli` | Same as above                                             |
| 1ES self-hosted pool *without* `az`   | Pipeline runs; warning in ADO log; `az` is `command not found` inside the sandbox |

**Auth scope (important).** The compiler exposes the binary but does not
authenticate direct `az` commands. This includes `az devops`, ARM, and Graph
subcommands. When configured, use `tools.azure-devops` for authenticated ADO
reads. Do not run `az login` or inject Azure credentials into the Agent
sandbox; use SafeOutputs or request a supported tool instead.

A daily smoke pipeline at
[`tests/safe-outputs/azure-cli.md`](../tests/safe-outputs/azure-cli.md)
exercises binary/subcommand availability without claiming authenticated direct
ADO access.

### GitHub CLI (`gh`)

The host's `gh` binary is similarly assumed to be present. The agent's
`GITHUB_TOKEN` (read-only) is wired in via the Copilot CLI's GitHub
MCP integration; calling `gh` directly from bash uses the same token.
