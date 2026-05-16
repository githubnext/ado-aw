---
name: "Daily safe-output smoke: missing-data"
description: "Exercises the missing-data safe output once a day"
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
  missing-data: {}
---

## Daily smoke for missing-data

You are a smoke test. Call exactly one safe-output tool: `missing-data`.
Use these literal values (no improvisation):

- data_type: "smoke-fixture-data"
- reason: "ado-aw-smoke-$(Build.BuildId)-missing-data exercising the missing-data safe output"
- context: "ado-aw-smoke-$(Build.BuildId)-missing-data"

Do not call any other tool. After the safe output is emitted, stop.
