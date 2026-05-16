---
name: "Daily safe-output smoke: create-pull-request"
description: "Exercises the create-pull-request safe output once a day"
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
  create-pull-request:
    target-branch: main
    title-prefix: "ado-aw-smoke: "
    draft: true
    auto-complete: false
    delete-source-branch: true
    if-no-changes: error
    max: 1
    include-stats: false
---

## Daily smoke for create-pull-request

You are a smoke test. Call exactly one safe-output tool:
`create-pull-request`. First touch a file under the working tree so the
PR has a real diff — append the line
`ado-aw-smoke-$(Build.BuildId)-create-pull-request` to
`.ado-aw-smoke-marker` at the repo root. Then call
`create-pull-request` with these literal values (no improvisation):

- title: "ado-aw-smoke-$(Build.BuildId)-create-pull-request"
- description: "ado-aw daily smoke exercising the create-pull-request safe output for build $(Build.BuildId). This draft PR will be abandoned by the weekly janitor."

Do not call any other tool. After the safe output is emitted, stop.
