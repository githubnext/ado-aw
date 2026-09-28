---
name: "Live PR source branch push"
description: "Prove source-head preparation, MCP patch capture and guarded Stage 3 pushing"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  timeout-minutes: 10
permissions:
  read: agent-playground-read
  write: agent-playground-write
tools:
  edit: true
safe-outputs:
  report-failure-as-work-item: false
  push-to-pull-request-branch:
    target: triggering
    allowed-repositories: [self]
    allowed-branches: ["ado-aw-smoke-candidate/*"]
    max-files: 1
    max: 1
  update-pull-request:
    target: triggering
    title: false
    include-stats: false
    max: 1
  add-build-tag:
    tag-prefix: "ado-aw-pr-boundary-"
    max: 1
---

This is a disposable test PR. Do not inspect unrelated repositories or modify
any files except the one specified below.

Read `/tmp/ado-aw/pr-source-snapshot.json`. Use its exact repository alias and
expected_head_sha. The checkout has already been prepared at this PR source
head. Do not create a branch, fetch, merge, rebase or commit.

Create `smoke-pr-push-proof.txt` at the checkout root containing exactly
`ado-aw-pr-push-$(Build.BuildId)` followed by a newline.

Emit exactly three proposals, in this order:

1. `push-to-pull-request-branch` with the repository and expected_head_sha from
   the snapshot. Omit the PR ID to use the trusted triggering target.
2. `update-pull-request` with body `ado-aw-pr-boundary-$(Build.BuildId)` and
   operation `replace`.
3. `add-build-tag` with build_id $(Build.BuildId), tag "$(Build.BuildId)".

Stop. Do not emit any other proposals.
