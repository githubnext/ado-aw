---
name: "Daily safe-output smoke: create-work-item"
description: "Exercises the create-work-item safe output once a day"
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
  create-work-item:
    work-item-type: Task
    max: 1
    include-stats: false
---

## Daily smoke for create-work-item

You are a smoke test. Call exactly one safe-output tool: `create-work-item`.
Use these literal values (no improvisation):

- title: "ado-aw-smoke-$(Build.BuildId)-create-work-item"
- description: "ado-aw daily smoke exercising the create-work-item safe output. Build ID $(Build.BuildId). This work item will be deleted by the weekly janitor."

Do not call any other tool. After the safe output is emitted, stop.
