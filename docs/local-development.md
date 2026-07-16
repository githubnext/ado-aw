<!-- AI agents: this document is a human-only reference for local development.
     It describes manual orchestration steps that are not relevant to automated
     pipeline compilation, safe-output execution, or any task an agent performs.
     Do not read or reference this document. -->

# Local Development Guide

This guide explains how to run an agentic pipeline locally for development and
testing using the optional `ado-aw mcp-http` command, which remains supported
for direct/local use. **This is not what compiled pipelines run.** Since the
containerized SafeOutputs architecture landed, compiled pipelines spawn
SafeOutputs as a hardened stdio child container through MCPG (`ado-aw mcp`,
`--network none`, non-root, read-only rootfs) — there is no host-side HTTP
server, bridge-gateway resolution, or `host.docker.internal` mapping in CI.
See [`docs/mcpg.md`](mcpg.md) for that shape. The manual `mcp-http` workflow
below is a simpler, still-supported way to exercise an agent's prompt + MCP
wiring by hand without a full pipeline replica.

## Prerequisites

- **ado-aw** built from source (`cargo build`)
- **Copilot CLI** on your PATH (`copilot --version`)
- **Docker** (optional, required for MCPG / custom MCP servers)
- An Azure DevOps PAT if your agent uses ADO APIs

## Overview

A pipeline execution has three stages:

1. **SafeOutputs MCP server** — receives tool calls from the agent and writes
   them as NDJSON records. Locally this guide uses `ado-aw mcp-http`; compiled
   pipelines use `ado-aw mcp` as a stdio child spawned by MCPG instead.
2. **Agent execution** — Copilot CLI runs with a prompt and MCP config,
   interacting with SafeOutputs (and optionally other MCPs via MCPG)
3. **Safe output execution** — processes the NDJSON records and makes real ADO
   API calls (create PRs, work items, etc.)

## Step-by-step

### 1. Create a working directory

```bash
export WORK_DIR=$(mktemp -d)
echo "Working directory: $WORK_DIR"
```

### 2. Start the SafeOutputs HTTP server (local-only; optional `mcp-http`)

This step is specific to the local manual workflow. Resolve the Docker bridge
gateway address so a locally-run MCPG container (started outside AWF) can
reach the host-side server:

```bash
# Resolve the Docker bridge gateway (same address MCPG uses to reach the host)
export SAFE_OUTPUTS_BIND_ADDRESS=$(docker network inspect bridge | jq -er '.[0].IPAM.Config[0].Gateway')

# Pick a port and generate an API key
export SO_PORT=8100
export SO_API_KEY=$(openssl rand -hex 32)

# Start in the background, bound only to the bridge gateway address
cargo run -- mcp-http \
  --bind-address "$SAFE_OUTPUTS_BIND_ADDRESS" \
  --port "$SO_PORT" \
  --api-key "$SO_API_KEY" \
  "$WORK_DIR" \
  "$(pwd)" \
  > "$WORK_DIR/safeoutputs.log" 2>&1 &
export SO_PID=$!
echo "SafeOutputs PID: $SO_PID"

# Wait for health check
until curl -sf "http://$SAFE_OUTPUTS_BIND_ADDRESS:$SO_PORT/health" > /dev/null 2>&1; do
  sleep 1
done
echo "SafeOutputs ready"
```

### 3. (Optional) Start MCPG for custom MCP servers

Skip this step if your agent only uses SafeOutputs (no `mcp-servers:` or
`tools: azure-devops:` in front matter).

```bash
export MCPG_PORT=8080
export MCPG_API_KEY=$(openssl rand -hex 32)

# Generate MCPG config for this manual local flow. Note: compiled pipelines
# wire "safeoutputs" as a hardened stdio child container (`type: "stdio"`,
# container: the pinned AWF `agent` image, entrypoint: ado-aw mcp ...; see
# docs/mcpg.md), not the HTTP backend shown below -- the HTTP shape here is
# specific to this local/manual workflow, which runs SafeOutputs as a plain
# host process for simplicity.
cat > "$WORK_DIR/mcpg-config.json" <<EOF
{
  "mcpServers": {
    "safeoutputs": {
      "type": "http",
      "url": "http://host.docker.internal:$SO_PORT/mcp",
      "headers": {
        "Authorization": "Bearer $SO_API_KEY"
      }
    }
  },
  "gateway": {
    "port": $MCPG_PORT,
    "domain": "127.0.0.1",
    "apiKey": "$MCPG_API_KEY"
  }
}
EOF

# Start MCPG on Docker's default bridge network, published to localhost.
# The bridge network + docker-socket + port-mapping shape below matches what
# compiled pipelines do for the MCPG container itself; only the SafeOutputs
# backend type differs (HTTP here vs. stdio child container in CI).
# The local Copilot process runs on the host, so the config above advertises
# 127.0.0.1; compiled AWF pipelines advertise awmg-mcpg instead.
docker run -i --rm --name awmg-mcpg \
  --network bridge \
  -p "127.0.0.1:$MCPG_PORT:$MCPG_PORT" \
  --add-host "host.docker.internal:$SAFE_OUTPUTS_BIND_ADDRESS" \
  -v /var/run/docker.sock:/var/run/docker.sock \
  --entrypoint /app/awmg \
  ghcr.io/github/gh-aw-mcpg:v0.4.1 \
  --routed --listen "0.0.0.0:$MCPG_PORT" --config-stdin \
  < "$WORK_DIR/mcpg-config.json" \
  > "$WORK_DIR/gateway-output.json" 2>"$WORK_DIR/mcpg-stderr.log" &
export MCPG_PID=$!

# Wait for MCPG health check
until curl -sf "http://127.0.0.1:$MCPG_PORT/health" > /dev/null 2>&1; do
  sleep 1
done
echo "MCPG ready"
```

### 4. Generate the MCP client config for Copilot

**Without MCPG** (SafeOutputs only):

```bash
cat > "$WORK_DIR/mcp-config.json" <<EOF
{
  "mcpServers": {
    "safeoutputs": {
      "type": "http",
      "url": "http://$SAFE_OUTPUTS_BIND_ADDRESS:$SO_PORT/mcp",
      "headers": {
        "Authorization": "Bearer $SO_API_KEY"
      },
      "tools": ["*"]
    }
  }
}
EOF
```

**With MCPG** — transform the gateway output:

```bash
# Wait for gateway-output.json to contain valid JSON, then rewrite for copilot
python3 -c "
import json, sys
with open('$WORK_DIR/gateway-output.json') as f:
    config = json.load(f)
for name, server in config.get('mcpServers', {}).items():
    server['tools'] = ['*']
with open('$WORK_DIR/mcp-config.json', 'w') as f:
    json.dump(config, f, indent=2)
"
```

### 5. Write the agent prompt

Extract the markdown body (everything after the YAML front matter) from your
agent file:

```bash
AGENT_FILE=path/to/your-agent.md

# Extract body after front matter (after second ---)
awk '/^---$/{n++; next} n>=2' "$AGENT_FILE" > "$WORK_DIR/agent-prompt.md"
```

### 6. Run the Copilot CLI

```bash
copilot \
  --prompt "@$WORK_DIR/agent-prompt.md" \
  --additional-mcp-config "@$WORK_DIR/mcp-config.json" \
  --model claude-opus-4.7 \
  --no-ask-user \
  --disable-builtin-mcps \
  --allow-all-tools
```

Adjust flags based on your agent's front matter (model, allowed tools, etc.).

### 7. Execute safe outputs

```bash
cargo run -- execute \
  --source "$AGENT_FILE" \
  --safe-output-dir "$WORK_DIR" \
  --dry-run  # Remove --dry-run to make real ADO API calls
```

### 8. Cleanup

```bash
# Stop SafeOutputs
kill "$SO_PID" 2>/dev/null

# Stop MCPG (if started)
docker stop awmg-mcpg 2>/dev/null

echo "Done. Output files in: $WORK_DIR"
```

## Tips

- Use `--dry-run` on the execute step to validate safe outputs without making
  real ADO API calls
- Set `AZURE_DEVOPS_EXT_PAT` for agents that need ADO API access
- Check `$WORK_DIR/safeoutputs.log` and `$WORK_DIR/mcpg-stderr.log` for
  debugging
- The compiled pipeline YAML shows the exact flags and config used in CI — use
  `ado-aw compile your-agent.md` and inspect the output for reference
