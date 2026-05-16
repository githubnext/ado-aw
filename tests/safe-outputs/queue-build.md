---
name: "Daily safe-output smoke: queue-build"
description: "Exercises the queue-build safe output once a day"
on:
  schedule: daily around 03:00
target: standalone
engine:
  id: copilot
  model: gpt-5-mini
  timeout-minutes: 15
permissions:
  read: agent-playground-read
  write: agent-playground-write
safe-outputs:
  queue-build:
    allowed-pipelines:
      - $(noopPipelineId)
    allowed-branches:
      - main
    default-branch: main
    max: 1
---

## Daily smoke for queue-build

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
a no-op target pipeline at `$(noopPipelineId)`. Call exactly one
safe-output tool: `queue-build`. Use these literal values (no
improvisation):

- pipeline_id: $(noopPipelineId)
- branch: "main"
- reason: "ado-aw-smoke-$(Build.BuildId)-queue-build"

Do not call any other tool. After the safe output is emitted, stop.
