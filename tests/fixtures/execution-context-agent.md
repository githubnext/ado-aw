---
name: "Execution Context Agent"
description: "Agent exercising the execution-context PR contributor"
on:
  pr:
    branches:
      include: [main]
execution-context:
  pr:
    scope:
      - "src/**"
      - "docs/**"
    unified: 5
    max-diff-bytes: 262144
    snapshots: true
---

## Execution Context Agent

This fixture exercises the always-on `ExecContextExtension` with the PR
contributor in its default configuration plus a custom scope, unified
context size, max-diff-bytes cap, and snapshots toggle.

When triggered by a pull request, the precompute step stages
`aw-context/pr/*` under `$(Build.SourcesDirectory)/aw-context/`. The
agent reads `aw-context/pr/status.txt` first and, if `OK`, consumes the
unified diff and changed-files listings from the directory.
