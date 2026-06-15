---
name: "Daily safe-output smoke: add-pr-comment"
description: "Exercises the add-pr-comment safe output once a day"
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
  add-pr-comment:
    comment-prefix: "ado-aw-smoke: "
    allowed-repositories:
      - agent-definitions
    max: 1
    include-stats: false
---

## Daily smoke for add-pr-comment

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
the perma PR at `$(permaPullRequestId)` in the AgentPlayground ADO repo
`agent-definitions` (the YAML for this pipeline lives in GitHub, so we
must address the ADO repo explicitly rather than via `self`). Call
exactly one safe-output tool: `add-pr-comment`. Use these literal
values (no improvisation):

- pull_request_id: $(permaPullRequestId)
- content: "ado-aw-smoke-$(Build.BuildId)-add-pr-comment exercising the add-pr-comment safe output for build $(Build.BuildId)."
- repository: "agent-definitions"

Do not call any other tool. After the safe output is emitted, stop.
