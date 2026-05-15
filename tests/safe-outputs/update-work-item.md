---
name: "Daily safe-output smoke: update-work-item"
description: "Exercises the update-work-item safe output once a day"
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
  update-work-item:
    target: "*"
    body: true
    max: 1
---

## Daily smoke for update-work-item

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
a perma work item at `$(permaWorkItemId)`. Call exactly one safe-output
tool: `update-work-item`. Update only the body. Use these literal values
(no improvisation):

- id: $(permaWorkItemId)
- body: "ado-aw-smoke-$(Build.BuildId)-update-work-item — last updated by build $(Build.BuildId) exercising the update-work-item safe output."

Do not call any other tool. After the safe output is emitted, stop.
