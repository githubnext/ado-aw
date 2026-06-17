---
name: "Execution Context Agent"
description: "Agent exercising the execution-context PR contributor and manual contributor"
on:
  pr:
    branches:
      include: [main]
parameters:
- name: topic
  type: string
  default: ""
  displayName: "Topic to work on"
---

## Execution Context Agent

This fixture exercises the always-on `ExecContextExtension` with:
- The **PR contributor** (activated by `on.pr`) — stages `aw-context/pr/*`
  artefacts and appends a PR-context prompt fragment.
- The **manual contributor** (activated by the `parameters:` block) — stages
  `aw-context/manual/*` artefacts when the pipeline is queued manually.

