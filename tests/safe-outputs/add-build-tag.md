---
name: "Daily safe-output smoke: add-build-tag"
description: "Exercises the add-build-tag safe output once a day"
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
  add-build-tag:
    tag-prefix: "ado-aw-smoke-"
    max: 1
---

## Daily smoke for add-build-tag

You are a smoke test. Call exactly one safe-output tool: `add-build-tag`.
Use these literal values (no improvisation) — the tag is applied to the
current build, so use `$(Build.BuildId)` as the build_id.

- build_id: "$(Build.BuildId)"
- tag: "ado-aw-smoke-$(Build.BuildId)-add-build-tag"

Do not call any other tool. After the safe output is emitted, stop.
