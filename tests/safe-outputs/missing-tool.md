---
name: "Daily safe-output smoke: missing-tool"
description: "Exercises the missing-tool safe output once a day"
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
  missing-tool:
    work-item:
      enabled: false
---

## Daily smoke for missing-tool

You are a smoke test. Call exactly one safe-output tool: `missing-tool`.
Use these literal values (no improvisation):

- tool_name: "ado-aw-smoke-$(Build.BuildId)-missing-tool"
- context: "ado-aw-smoke-$(Build.BuildId)-missing-tool exercising the missing-tool safe output"

Do not call any other tool. After the safe output is emitted, stop.
