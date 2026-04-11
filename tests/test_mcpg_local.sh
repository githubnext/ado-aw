#!/usr/bin/env bash
# test_mcpg_local.sh — Local smoke test for MCPG integration (no Docker required)
#
# This script validates the ado-aw components that interface with MCPG:
#   1. Compiles a sample agent and verifies MCPG markers in output YAML
#   2. Starts the SafeOutputs HTTP server
#   3. Sends MCP requests via curl (simulating MCPG forwarding)
#   4. Verifies NDJSON safe output files are created
#
# Usage:
#   ./tests/test_mcpg_local.sh
#   ./tests/test_mcpg_local.sh --skip-compile  # skip compilation step

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TEMP_DIR=$(mktemp -d)
BINARY=""
SO_PID=""

cleanup() {
    if [ -n "$SO_PID" ]; then
        kill "$SO_PID" 2>/dev/null || true
        wait "$SO_PID" 2>/dev/null || true
    fi
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

log() { echo "==> $*"; }
pass() { echo "  ✅ $*"; }
fail() { echo "  ❌ $*"; exit 1; }

# ─── Build ───────────────────────────────────────────────────────────
log "Building ado-aw..."
cd "$PROJECT_DIR"
cargo build --quiet 2>/dev/null
BINARY="$PROJECT_DIR/target/debug/ado-aw"

if [ ! -x "$BINARY" ]; then
    fail "Binary not found at $BINARY"
fi
pass "Binary built: $BINARY"

# ─── Step 1: Compile a sample agent ─────────────────────────────────
if [ "${1:-}" != "--skip-compile" ]; then
    log "Step 1: Compiling sample agent..."

    FIXTURE="$SCRIPT_DIR/fixtures/minimal-agent.md"
    OUTPUT_YAML="$TEMP_DIR/minimal-agent.yml"

    "$BINARY" compile "$FIXTURE" -o "$OUTPUT_YAML"

    if [ ! -f "$OUTPUT_YAML" ]; then
        fail "Compiled YAML not created"
    fi

    # Verify MCPG markers are resolved
    if grep -q 'ghcr.io/github/gh-aw-mcpg' "$OUTPUT_YAML"; then
        pass "MCPG image reference present"
    else
        fail "MCPG image reference missing"
    fi

    if grep -q 'mcpg-config.json' "$OUTPUT_YAML"; then
        pass "MCPG config file reference present"
    else
        fail "MCPG config file reference missing"
    fi

    if grep -q 'host.docker.internal' "$OUTPUT_YAML"; then
        pass "host.docker.internal reference present"
    else
        fail "host.docker.internal reference missing"
    fi

    if grep -q 'enable-host-access' "$OUTPUT_YAML"; then
        pass "AWF --enable-host-access flag present"
    else
        fail "AWF --enable-host-access flag missing"
    fi

    if grep -q 'SafeOutputs HTTP server' "$OUTPUT_YAML"; then
        pass "SafeOutputs HTTP server step present"
    else
        fail "SafeOutputs HTTP server step missing"
    fi

    # Verify no unreplaced markers
    if grep -v '\${{' "$OUTPUT_YAML" | grep -q '{{ '; then
        fail "Unreplaced template markers found in compiled output"
    else
        pass "No unreplaced template markers"
    fi

    # Verify no legacy MCP firewall references
    if grep -qi 'mcp-firewall\|mcp_firewall' "$OUTPUT_YAML"; then
        fail "Legacy MCP firewall references found"
    else
        pass "No legacy MCP firewall references"
    fi
else
    log "Step 1: Skipping compilation (--skip-compile)"
fi

# ─── Step 2: Start SafeOutputs HTTP server ──────────────────────────
log "Step 2: Starting SafeOutputs HTTP server..."

SO_DIR="$TEMP_DIR/safe-outputs"
mkdir -p "$SO_DIR"

PORT=8199
API_KEY="test-smoke-key-$(date +%s)"

"$BINARY" mcp-http --port "$PORT" --api-key "$API_KEY" "$SO_DIR" "$SO_DIR" &
SO_PID=$!

# Wait for server to be ready
READY=false
for i in $(seq 1 30); do
    if curl -sf "http://127.0.0.1:$PORT/health" > /dev/null 2>&1; then
        READY=true
        break
    fi
    sleep 0.2
done

if [ "$READY" != "true" ]; then
    fail "SafeOutputs HTTP server did not become ready"
fi
pass "SafeOutputs HTTP server running on port $PORT (PID: $SO_PID)"

# ─── Step 3: Health check ───────────────────────────────────────────
log "Step 3: Verifying health endpoint..."

HEALTH=$(curl -sf "http://127.0.0.1:$PORT/health")
if [ "$HEALTH" = "ok" ]; then
    pass "Health endpoint returns 'ok'"
else
    fail "Health endpoint returned: $HEALTH"
fi

# ─── Step 4: Auth check ─────────────────────────────────────────────
log "Step 4: Verifying auth enforcement..."

HTTP_CODE=$(curl -s -o /dev/null -w '%{http_code}' \
    -X POST "http://127.0.0.1:$PORT/mcp" \
    -H "Content-Type: application/json" \
    -H "Accept: text/event-stream, application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}')

if [ "$HTTP_CODE" = "401" ]; then
    pass "Unauthenticated request rejected (401)"
else
    fail "Expected 401, got $HTTP_CODE"
fi

# ─── Step 5: MCP Initialize ─────────────────────────────────────────
log "Step 5: MCP Initialize handshake..."

INIT_RESP=$(curl -sf -D "$TEMP_DIR/init-headers.txt" \
    -X POST "http://127.0.0.1:$PORT/mcp" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Content-Type: application/json" \
    -H "Accept: text/event-stream, application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1.0"}}}')

SESSION_ID=$(grep -i 'mcp-session-id' "$TEMP_DIR/init-headers.txt" | tr -d '\r' | awk '{print $2}' || true)

if [ -n "$SESSION_ID" ]; then
    pass "Session initialized (ID: ${SESSION_ID:0:16}...)"
else
    log "  Warning: No session ID returned (stateless mode)"
fi

# Send initialized notification
curl -sf -o /dev/null \
    -X POST "http://127.0.0.1:$PORT/mcp" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Content-Type: application/json" \
    -H "Accept: text/event-stream, application/json" \
    ${SESSION_ID:+-H "mcp-session-id: $SESSION_ID"} \
    -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' || true

pass "Initialized notification sent"

# ─── Step 6: tools/list ─────────────────────────────────────────────
log "Step 6: Listing available tools..."

TOOLS_RESP=$(curl -sf \
    -X POST "http://127.0.0.1:$PORT/mcp" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Content-Type: application/json" \
    -H "Accept: text/event-stream, application/json" \
    ${SESSION_ID:+-H "mcp-session-id: $SESSION_ID"} \
    -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}')

for tool in noop create-work-item create-pull-request missing-tool missing-data; do
    if echo "$TOOLS_RESP" | grep -q "$tool"; then
        pass "Tool '$tool' available"
    else
        fail "Tool '$tool' not found in tools/list response"
    fi
done

# ─── Step 7: tools/call noop ────────────────────────────────────────
log "Step 7: Calling noop tool..."

NOOP_RESP=$(curl -sf \
    -X POST "http://127.0.0.1:$PORT/mcp" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Content-Type: application/json" \
    -H "Accept: text/event-stream, application/json" \
    ${SESSION_ID:+-H "mcp-session-id: $SESSION_ID"} \
    -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"noop","arguments":{"context":"Smoke test - all good"}}}')

sleep 0.5

NDJSON="$SO_DIR/safe_outputs.ndjson"
if [ -f "$NDJSON" ]; then
    pass "NDJSON file created: $NDJSON"
else
    fail "NDJSON file not found"
fi

if grep -q '"noop"' "$NDJSON"; then
    pass "Noop entry found in NDJSON"
else
    fail "Noop entry not in NDJSON. Content: $(cat "$NDJSON")"
fi

# ─── Step 8: tools/call create-work-item ────────────────────────────
log "Step 8: Calling create-work-item tool..."

WI_RESP=$(curl -sf \
    -X POST "http://127.0.0.1:$PORT/mcp" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Content-Type: application/json" \
    -H "Accept: text/event-stream, application/json" \
    ${SESSION_ID:+-H "mcp-session-id: $SESSION_ID"} \
    -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"create-work-item","arguments":{"title":"Smoke test work item title","description":"This is a smoke test work item with enough description length."}}}')

sleep 0.5

if grep -q '"create-work-item"' "$NDJSON"; then
    pass "Work item entry found in NDJSON"
else
    fail "Work item entry not in NDJSON"
fi

if grep -q 'Smoke test work item title' "$NDJSON"; then
    pass "Work item title preserved in NDJSON"
else
    fail "Work item title not found in NDJSON"
fi

# ─── Summary ────────────────────────────────────────────────────────
echo ""
log "All smoke tests passed! ✅"
echo ""
echo "NDJSON contents:"
cat "$NDJSON"
