---
name: "Daily safe-output smoke: canary"
description: "Omnibus canary exercising noop + create-work-item + add-build-tag in one agentic run"
on:
  schedule: daily around 03:00
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: claude-sonnet-4.6
  timeout-minutes: 20
permissions:
  read: agent-playground-read
  write: agent-playground-write
safe-outputs:
  noop: {}
  create-work-item:
    work-item-type: Task
    assignee: devinejames@microsoft.com
    max: 1
    include-stats: false
  add-build-tag:
    tag-prefix: "ado-aw-smoke-"
    max: 1
---

## Daily omnibus canary

You are a smoke test. Call **exactly three** safe-output tools, in this
order:

1. `noop`

   - context: "ado-aw-smoke-$(Build.BuildId)-canary proof-of-life"

2. `create-work-item`

   - title: "ado-aw-smoke-$(Build.BuildId)-canary"
   - description: "ado-aw daily canary smoke. Build $(Build.BuildId). Will be deleted by the weekly janitor."
   - tags: []

3. `add-build-tag`

   - build_id: $(Build.BuildId)
   - tag: "$(Build.BuildId)-canary"

Call the tools in exactly this order. Do not call any other tool. After
all three safe outputs are emitted, stop.
