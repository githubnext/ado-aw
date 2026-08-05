---
name: "Daily smoke: az CLI access"
description: "Exercises that az is mounted and reachable inside the AWF container"
on:
  schedule: daily around 03:00
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: claude-sonnet-4.6
  timeout-minutes: 15
tools:
  bash:
    - az
    - head
  edit: false
permissions:
  read: agent-playground-read
safe-outputs:
  noop: {}
---

## Daily smoke for Azure CLI (az)

You are a smoke test. Verify the host-mounted Azure CLI is reachable
inside the AWF container, then emit exactly one safe-output.

Steps (run each in turn using your bash tool):

1. Confirm the binary exists and prints its version:

   ```
   az --version | head -3
   ```

2. Confirm the Azure DevOps command group is installed and can render help.
   This smoke does not expect direct ADO authentication:

   ```
   az devops -h | head -20
   ```

   Capture the combined stdout/stderr (truncated to 400 characters if longer)
   for the safe-output context below.

3. Invoke exactly one MCP tool: `noop` from the `safeoutputs`
   server, with:

   - context: a brief one-line proof-of-life containing the az version
     string and command-group help output, prefixed with
     `ado-aw-smoke-$(Build.BuildId)-azure-cli:`.

Use the native Copilot MCP tool interface. Do not inspect MCP configuration,
API keys, processes, files, or HTTP endpoints. Do not invoke SafeOutputs through
bash, `curl`, or raw HTTP. Do not print or describe a JSON tool request.
Actually invoke the MCP tool, then stop.
