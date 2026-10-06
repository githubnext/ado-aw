---
name: "Candidate compiler smoke: queue-time runtime model"
description: "Proves an ADO queue-time variable selects the Agent Copilot model"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  timeout-minutes: 15
safe-outputs:
  noop: {}
---

## Queue-time runtime model smoke

Call the `noop` safe-output tool exactly once with context
`runtime-model-queue-$(Build.BuildId)`, then stop.
