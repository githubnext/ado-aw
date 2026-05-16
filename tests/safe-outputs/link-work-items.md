---
name: "Daily safe-output smoke: link-work-items"
description: "Exercises the link-work-items safe output once a day"
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
safe-outputs:
  link-work-items:
    target: "*"
    allowed-link-types:
      - related
    max: 1
---

## Daily smoke for link-work-items

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
two perma work items at `$(permaWorkItemId)` and `$(permaWorkItem2Id)`.
Call exactly one safe-output tool: `link-work-items`. Use these literal
values (no improvisation):

- source_id: $(permaWorkItemId)
- target_id: $(permaWorkItem2Id)
- link_type: "related"
- comment: "ado-aw-smoke-$(Build.BuildId)-link-work-items"

Do not call any other tool. After the safe output is emitted, stop.
