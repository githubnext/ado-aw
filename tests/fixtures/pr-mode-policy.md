---
name: "PR Mode Policy Agent"
description: "Fixture exercising on.pr with mode: policy (issue #916)"
on:
  pr:
    branches:
      include: [main]
    mode: policy
---

## PR Mode Policy Agent

This agent has `on.pr` configured with `mode: policy`, meaning the
operator has installed an Azure DevOps Build Validation branch policy
that fires real `Build.Reason == PullRequest` builds and the compiler
must:

- emit none of the synth artefacts (`synthPr`, `AW_SYNTHETIC_PR`,
  `PR_SYNTH_SPEC`, `exec-context-pr-synth`), and
- emit `trigger: none` at the top level so feature-branch pushes do not
  queue duplicate CI builds alongside the policy-driven PR build.
