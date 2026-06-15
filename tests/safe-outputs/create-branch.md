---
name: "Daily safe-output smoke: create-branch"
description: "Exercises the create-branch safe output once a day"
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
repos:
  - agent-definitions=agent-definitions
safe-outputs:
  create-branch:
    branch-pattern: "ado-aw-smoke-*"
    allowed-repositories:
      - agent-definitions
    max: 1
---

## Daily smoke for create-branch

You are a smoke test. The smoke targets the AgentPlayground ADO repo
`agent-definitions` (the YAML lives in GitHub, so address the ADO repo
explicitly). Call exactly one safe-output tool: `create-branch`. Use
these literal values (no improvisation):

- branch_name: "ado-aw-smoke-$(Build.BuildId)-create-branch"
- source_branch: "main"
- repository: "agent-definitions"

Do not call any other tool. After the safe output is emitted, stop.
