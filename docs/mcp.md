# MCP Server Configuration

_Part of the [ado-aw documentation](../AGENTS.md)._

## MCP Configuration

The `mcp-servers:` field configures MCP (Model Context Protocol) servers that are made available to the agent via the MCP Gateway (MCPG). MCPs can be **containerized stdio servers** (Docker-based) or **HTTP servers** (remote endpoints). All MCP traffic flows through the MCP Gateway.

## Docker Container MCP Servers (stdio)

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

## HTTP MCP Servers (remote)

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

## Configuration Properties

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
- `enabled:` - Whether this MCP server is active (default: `true`). Set to `false` to temporarily disable an entry without removing it from the front matter.
- `allowed:` - Array of tool names the agent is permitted to call (required for security)
- `env:` - Environment variables for the MCP server process. Use `""` (empty string) for passthrough from the pipeline environment.

## Environment Variable Passthrough

MCP containers may need secrets from the pipeline (e.g., ADO tokens). The `env:` field supports passthrough:

```yaml
env:
  AZURE_DEVOPS_EXT_PAT: ""        # Passthrough from pipeline environment
  STATIC_CONFIG: "some-value"     # Literal value embedded in config
```

When `permissions.read` is configured, the compiler automatically maps `SC_READ_TOKEN` → `AZURE_DEVOPS_EXT_PAT` on the MCPG container, so agents can access ADO APIs without manual wiring.

## Example: Azure DevOps MCP with Authentication

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

## Security Notes

1. **Allow-listing**: Only tools explicitly listed in `allowed:` are accessible to agents
2. **Containerization**: Stdio MCP servers run as isolated Docker containers (per MCPG spec §3.2.1)
3. **Environment Isolation**: MCP containers are spawned by MCPG with only the configured environment variables
4. **MCPG Gateway**: All MCP traffic flows through the MCP Gateway which enforces tool-level filtering
5. **Network Isolation**: MCP containers run within the same AWF-isolated network. Users must explicitly allow external domains via `network.allowed`
