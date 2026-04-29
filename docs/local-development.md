<!-- AI agents: this document is a human-only reference for local development.
     It describes manual orchestration steps that are not relevant to automated
     pipeline compilation, safe-output execution, or any task an agent performs.
     Do not read or reference this document. -->

# Local Development Guide

This guide explains how to run an agentic pipeline locally for development and
testing. The workflow mirrors what the compiled Azure DevOps pipeline does, but
each step is run manually on your machine.

## Prerequisites

- **ado-aw** built from source (`cargo build`)
- **Copilot CLI** on your PATH (`copilot --version`)
- **Docker** (optional, required for MCPG / custom MCP servers)
- An Azure DevOps PAT if your agent uses ADO APIs

## Overview

A pipeline execution has three stages:

1. **SafeOutputs MCP server** — receives tool calls from the agent and writes
   them as NDJSON records
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

### 2. Start the SafeOutputs HTTP server

```bash
# Pick a port and generate an API key
export SO_PORT=8100
export SO_API_KEY=$(openssl rand -hex 32)

# Start in the background
cargo run -- mcp-http \
  --port "$SO_PORT" \
  --api-key "$SO_API_KEY" \
  "$WORK_DIR" \
  "$(pwd)" \
  > "$WORK_DIR/safeoutputs.log" 2>&1 &
export SO_PID=$!
echo "SafeOutputs PID: $SO_PID"

# Wait for health check
until curl -sf "http://127.0.0.1:$SO_PORT/health" > /dev/null 2>&1; do
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

# Generate MCPG config — adapt the JSON to your agent's mcp-servers front matter.
# See the compiled pipeline's mcpg-config.json for the expected format.
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

# Start MCPG container (macOS/Windows — use host.docker.internal)
docker run -i --rm --name ado-aw-mcpg \
  -p "$MCPG_PORT:$MCPG_PORT" \
  -v /var/run/docker.sock:/var/run/docker.sock \
  --entrypoint /app/awmg \
  ghcr.io/github/gh-aw-mcpg:v0.3.0 \
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
      "url": "http://127.0.0.1:$SO_PORT/mcp",
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
docker stop ado-aw-mcpg 2>/dev/null

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
