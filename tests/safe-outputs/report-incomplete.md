---
name: "Daily safe-output smoke: report-incomplete"
description: "Exercises the report-incomplete safe output once a day"
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
  report-incomplete: {}
---

## Daily smoke for report-incomplete

You are a smoke test. Call exactly one safe-output tool: `report-incomplete`.
Use these literal values (no improvisation):

- reason: "ado-aw-smoke-$(Build.BuildId)-report-incomplete exercising the report-incomplete safe output"
- context: "ado-aw-smoke-$(Build.BuildId)-report-incomplete"

Do not call any other tool. After the safe output is emitted, stop.
