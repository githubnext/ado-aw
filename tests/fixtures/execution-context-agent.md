---
name: "Execution Context Agent"
description: "Agent exercising the execution-context PR, manual, repo, ci-push, schedule, and pr-checks contributors"
on:
  pr:
    branches:
      include: [main]
  schedule: 'daily around 09:00 UTC'
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
  schedule:
    enabled: true
  pr:
    checks:
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
- The **schedule contributor** (activated by `on.schedule` + `execution-context.schedule.enabled: true`) —
  stages `aw-context/schedule/*` artefacts for scheduled builds.
- The **PR-checks contributor** (activated by `on.pr` + `execution-context.pr.checks.enabled: true`) —
  stages `aw-context/pr/checks/*` artefacts listing Build Validation runs.

