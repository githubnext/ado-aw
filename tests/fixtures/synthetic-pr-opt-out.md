---
name: "Synthetic PR Opt-Out Agent"
description: "Fixture exercising on.pr with synthetic-from-ci=false (issue #916 back-compat)"
on:
  pr:
    branches:
      include: [main]
    synthetic-from-ci: false
---

## Synthetic PR Opt-Out Agent

This agent has `on.pr` configured with `synthetic-from-ci: false`,
preserving the pre-synthesis behaviour (requires an operator-installed
Build Validation branch policy for PR triggering to work).

The compiled YAML must contain NONE of the synthesis artefacts —
`synthPr`, `AW_SYNTHETIC_PR`, `PR_SYNTH_SPEC`, or `exec-context-pr-synth`
— so back-compat is preserved.
