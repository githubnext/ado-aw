---
name: "Daily safe-output smoke: update-pr"
description: "Exercises the update-pr safe output once a day"
on:
  schedule: daily around 03:00
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: gpt-5-mini
  timeout-minutes: 15
permissions:
  read: agent-playground-read
  write: agent-playground-write
repos:
  - agent-definitions=agent-definitions
safe-outputs:
  update-pr:
    allowed-operations:
      - update-description
    allowed-repositories:
      - agent-definitions
    max: 1
---

## Daily smoke for update-pr

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
the perma PR at `$(permaPullRequestId)` in the AgentPlayground ADO repo
`agent-definitions` (the YAML lives in GitHub, so address the ADO repo
explicitly). Call exactly one safe-output tool: `update-pr`. Use the
`update-description` operation only — vote / add-reviewers / add-labels
are not enabled in this fixture. Use these literal values (no
improvisation):

- pull_request_id: $(permaPullRequestId)
- operation: "update-description"
- description: "ado-aw-smoke-$(Build.BuildId)-update-pr — perma-PR description last refreshed by build $(Build.BuildId) exercising the update-pr safe output."
- repository: "agent-definitions"

Do not call any other tool. After the safe output is emitted, stop.
