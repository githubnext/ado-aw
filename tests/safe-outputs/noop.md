---
name: "Daily safe-output smoke: noop"
description: "Exercises the noop safe output once a day"
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
  noop: {}
---

## Daily smoke for noop

You are a smoke test. Call exactly one safe-output tool: `noop`.
Use these literal values (no improvisation):

- context: "ado-aw-smoke-$(Build.BuildId)-noop"

Do not call any other tool. After the safe output is emitted, stop.
