---
name: "Candidate compiler smoke: task-setvariable runtime model"
description: "Proves a preceding same-job task.setvariable selects the Agent Copilot model"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  timeout-minutes: 15
steps:
  - bash: |
      set -euo pipefail
      echo "##vso[task.setvariable variable=ADO_AW_MODEL_AGENT_COPILOT]auto"
    displayName: Select Agent runtime model
safe-outputs:
  noop: {}
  threat-detection:
    enabled: false
---

## Same-job runtime model smoke

Call the `noop` safe-output tool exactly once with context
`runtime-model-set-variable-$(Build.BuildId)`, then stop.
