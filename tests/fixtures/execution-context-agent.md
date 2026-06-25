---
name: "Execution Context Agent"
description: "Agent exercising the execution-context PR, manual, repo, and ci-push contributors"
on:
  pr:
    branches:
      include: [main]
parameters:
- name: topic
  type: string
  default: ""
  displayName: "Topic to work on"
execution-context:
  repo:
    enabled: true
  ci-push:
    enabled: true
---

## Execution Context Agent

This fixture exercises the always-on `ExecContextExtension` with:
- The **PR contributor** (activated by `on.pr`) — stages `aw-context/pr/*`
  artefacts and appends a PR-context prompt fragment.
- The **manual contributor** (activated by the `parameters:` block) — stages
  `aw-context/manual/*` artefacts when the pipeline is queued manually.
- The **repo contributor** (activated by `execution-context.repo.enabled: true`) —
  stages `aw-context/repo/*` artefacts with repository identity info.
- The **ci-push contributor** (activated by `execution-context.ci-push.enabled: true`) —
  stages `aw-context/ci-push/*` artefacts for CI/push builds.

