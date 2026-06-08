---
name: "Synthetic PR Default Agent"
description: "Fixture exercising on.pr with default synthetic-from-ci=true (issue #916)"
on:
  pr:
    branches:
      include: [main]
---

## Synthetic PR Default Agent

This agent has `on.pr` configured with default `synthetic-from-ci: true`.
On a CI-triggered build (no Build Validation policy), the Setup-job
`synthPr` step looks up the open PR for `Build.SourceBranch` and exposes
its identifiers so the gate evaluator and exec-context-pr bundles
behave as if `Build.Reason == PullRequest`.

The compiled YAML must contain:

- A `synthPr` step in the Setup job, before the gate
- A `PR_SYNTH_SPEC:` env var carrying the base64 spec
- The broadened `exec-context-pr.js` condition (`or(...)`)
- The Agent-job `dependsOn` condition with the `AW_SYNTHETIC_PR_SKIP` guard
- A narrowed top-level `trigger:` block mirroring `pr.branches.include`
