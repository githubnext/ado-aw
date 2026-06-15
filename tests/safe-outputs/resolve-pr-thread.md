---
name: "Daily safe-output smoke: resolve-pr-thread"
description: "Exercises the resolve-pr-thread safe output once a day"
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
  resolve-pr-thread:
    allowed-statuses:
      - fixed
      - closed
      - wontFix
      - byDesign
    allowed-repositories:
      - agent-definitions
    max: 1
setup:
  - bash: |
      set -euo pipefail
      # TODO(smoke): open a fresh transient comment thread on
      # $(permaPullRequestId) via `az repos pr` or the ADO REST API and
      # export its thread ID as ADO_AW_SMOKE_THREAD_ID for the agent
      # prompt. Until that's wired, the agent will use the perma-thread
      # variable and Stage 3 will fail closed if it has already been
      # resolved this run.
      echo "ado-aw-smoke setup placeholder for resolve-pr-thread build $(Build.BuildId)"
    displayName: "Setup: open transient PR thread"
---

## Daily smoke for resolve-pr-thread

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
the perma PR at `$(permaPullRequestId)` and a thread to resolve at
`$(permaThreadId)`, both in the AgentPlayground ADO repo
`agent-definitions` (the YAML lives in GitHub, so address the ADO repo
explicitly). Call exactly one safe-output tool: `resolve-pr-thread`.
Use these literal values (no improvisation):

- pull_request_id: $(permaPullRequestId)
- thread_id: $(permaThreadId)
- status: "fixed"
- repository: "agent-definitions"

Do not call any other tool. After the safe output is emitted, stop.
