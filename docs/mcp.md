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
      AZURE_DEVOPS_EXT_PAT:
        pipeline-variable: AZURE_DEVOPS_EXT_PAT
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
- `entrypoint-args:` - Arguments passed to the container entrypoint
- `args:` - Additional Docker runtime arguments (inserted before the image in `docker run`). **Security note**: dangerous flags like `--privileged`, `--network host` will trigger compile-time warnings.
- `mounts:` - Volume mounts in `"source:dest:mode"` format (e.g., `["/host/data:/app/data:ro"]`)
- `env:` - Environment variables for the MCP server process. Use a string for
  a static value or `{ pipeline-variable: NAME }` to read an ADO pipeline,
  variable-group, queue-time, or earlier-same-job variable at runtime.
- `azure-auth:` - Renewable Azure workload identity from an ARM service
  connection. The compiler supplies the Azure Identity environment contract
  and rotates the federated assertion for the lifetime of the Agent job.
  Supported only for containerized stdio servers.
  - `service-connection:` - Required ARM workload-identity service connection.
  - `mount-path:` - Optional container directory for the assertion; defaults
    to `/var/run/ado-aw/azure`.

**HTTP servers:**
- `url:` - HTTP endpoint URL for the remote MCP server
- `headers:` - HTTP headers to include in requests (e.g., `Authorization`, `X-MCP-Toolsets`)

**Common (both types):**
- `enabled:` - Whether this MCP server is active (default: `true`). Set to `false` to temporarily disable an entry without removing it from the front matter.
- `allowed:` - Array of tool names the agent is permitted to call. Optional — when omitted or empty, all tools from that MCP server are accessible to the agent. **Strongly recommended for security**: restrict to only the tools the agent needs.

HTTP MCPs ignore `env:`; use `headers:` for HTTP authentication instead.

## Environment Variable Passthrough

MCP containers may need secrets from the pipeline (e.g., ADO tokens). The
`env:` field uses an explicit pipeline-variable source:

```yaml
env:
  MCP_AUTH_TOKEN:                 # Name visible inside the MCP container
    pipeline-variable: ADO_TOKEN  # ADO variable/variable-group source
  STATIC_CONFIG: "some-value"     # Literal value embedded in config
```

The compiler emits `MCP_AUTH_TOKEN: $(ADO_TOKEN)` on the MCPG pipeline step,
then Docker forwards the resulting process value with `-e MCP_AUTH_TOKEN`.
Secret values remain runtime-only and are never written into compiled YAML.
The source variable must exist before the MCPG step runs: pipeline,
variable-group, and queue-time variables exist from job start; a
`task.setvariable` source must be published by an earlier step in the same job.
Cross-job/stage output expressions are not accepted by `pipeline-variable`.

## Renewable Azure workload identity

Use `azure-auth` when a containerized MCP server uses an Azure Identity SDK and
may need to acquire an Azure token late in a long-running Agent job:

```yaml
mcp-servers:
  kusto:
    container: "node:22-slim"
    entrypoint: "sh"
    entrypoint-args:
      - "-c"
      - "exec npx -y @azure/mcp@latest server start --namespace kusto"
    azure-auth:
      service-connection: my-arm-service-connection
      # Optional; defaults to /var/run/ado-aw/azure
      mount-path: /var/run/ado-aw/azure
```

The compiler injects these values into the MCP container:

```text
AZURE_CLIENT_ID=<service-connection client ID>
AZURE_TENANT_ID=<service-connection tenant ID>
AZURE_FEDERATED_TOKEN_FILE=/var/run/ado-aw/azure/token
```

An AzureCLI@3 setup task obtains the initial workload-identity assertion and
starts a trusted refresh sidecar. The sidecar uses the job's
`System.AccessToken` and `System.OidcRequestUri` to request replacement
assertions before their JWT expiry and atomically rotates the private token
file. The MCP container receives the token directory through a read-only
mount, so inode-replacing rotation remains visible; the agent and MCP server never receive
`System.AccessToken`.

`azure-auth` fails closed when:

- the server is HTTP or has no `container`;
- the service connection does not use workload identity federation;
- the author also sets `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, or
  `AZURE_FEDERATED_TOKEN_FILE`;
- a user mount overlaps the compiler-owned destination;
- the initial assertion or refresher readiness check fails.

The assertion is not an Azure access token. Azure Identity inside the MCP
container exchanges it for the resource-specific access token requested by the
server. AzureCLI@3's experimental `keepAzSessionActive` option does not replace
this feature: it refreshes only while that AzureCLI task remains running, but
the MCP server is used later during the separate Agent step.

The first-party `tools.azure-devops` integration is deliberately different:
it gives the MCP a non-secret sentinel in `ADO_MCP_AUTH_TOKEN`. The real
`SC_READ_TOKEN` is delivered only to `ado-proxy` over stdin and is injected
into an upstream request only after policy allows it. This behavior does not
apply to arbitrary user-defined `mcp-servers:` entries.

## Example: Azure DevOps MCP with Authentication

```yaml
mcp-servers:
  azure-devops:
    container: "node:20-slim"
    entrypoint: "npx"
    entrypoint-args: ["-y", "@azure-devops/mcp", "myorg"]
    env:
      AZURE_DEVOPS_EXT_PAT:
        pipeline-variable: AZURE_DEVOPS_EXT_PAT
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

1. **Allow-listing**: When `allowed:` is set, only the listed tools are accessible to the agent. When omitted or empty, **all** tools from that server are accessible. Always specify an explicit `allowed:` list to limit the agent's tool surface.
2. **Containerization**: Stdio MCP servers run as isolated Docker containers (per MCPG spec §3.2.1)
3. **Environment Isolation**: MCP containers are spawned by MCPG with only the configured environment variables
4. **MCPG Gateway**: All MCP traffic flows through the MCP Gateway which enforces tool-level filtering
5. **Trusted egress**: MCPG and the stdio/HTTP backends it spawns from `mcp-servers:` front matter are trusted infrastructure that runs outside the agent's Squid-enforced allowlist — they have direct network egress and are not subject to `network.allowed`/`network.blocked`. Only the Copilot agent process itself is confined to the AWF sandbox and its domain allowlist; see [`docs/mcpg.md`](mcpg.md) and [`docs/network.md`](network.md) for the topology.
6. **SafeOutputs is further hardened**: unlike arbitrary `mcp-servers:` entries, the compiler-owned `safeoutputs` MCPG backend is not a user-configurable trusted-egress container — it is a dedicated stdio child spawned by MCPG from the pinned AWF `agent` image with `--network none`, `--cap-drop ALL`, a read-only rootfs, and the host ADO runner's non-root UID/GID. It has no network access at all, trusted or otherwise; see [`docs/mcpg.md`](mcpg.md).
7. **Azure credential custody**: `azure-auth` keeps `System.AccessToken` in the
   trusted AzureCLI@3/refresher path. Only the short-lived federated assertion
   is mounted into the target MCP, read-only. Credential files are created
   beneath `$(Agent.TempDirectory)`, never runner `/tmp`, because AWF exposes
   runner `/tmp` inside the agent sandbox.
