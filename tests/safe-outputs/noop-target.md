---
name: "ado-aw smoke noop target"
description: "No-op target pipeline used by the queue-build smoke fixture"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: gpt-5-mini
  timeout-minutes: 10
permissions:
  read: agent-playground-read
safe-outputs:
  noop: {}
---

## Noop target pipeline

You are the no-op target of the `queue-build` smoke. Invoke exactly one MCP
tool: `noop` from the `safeoutputs` server. Use these literal values (no
improvisation):

- context: "ado-aw-smoke-noop-target build $(Build.BuildId) queued from queue-build smoke"

Do not print or describe a JSON tool request. Actually invoke the MCP tool.
Do not call any other tool. After the safe output is emitted, stop.
