---
name: "Live synthetic PR boundary"
description: "Exercise real agent/detection/executor boundaries against an orchestrator-owned disposable PR"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  timeout-minutes: 10
permissions:
  read: agent-playground-read
  write: agent-playground-write
safe-outputs:
  report-failure-as-work-item: false
  add-build-tag:
    tag-prefix: "ado-aw-pr-boundary-"
    max: 1
  update-pull-request:
    target: triggering
    title: false
    include-stats: false
    allowed-repositories: [self]
    max: 1
---

This is a test against a disposable pull request owned by this test run.
Do not create branches, edit files, or inspect unrelated repositories.
Emit exactly these two proposals:

1. `add-build-tag`: build_id $(Build.BuildId), tag "$(Build.BuildId)".
2. `update-pull-request`: body "ado-aw-pr-boundary-$(Build.BuildId)",
   operation "replace". Omit the PR ID and repository to use the trusted
   triggering PR identity.

Do not add any other text to the description. Stop after the two proposals.
