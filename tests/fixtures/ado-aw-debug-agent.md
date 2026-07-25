---
name: "Ado AW Debug Test Agent"
description: "Fixture exercising skip-integrity plus public GitHub issue output"
ado-aw-debug:
  skip-integrity: true
safe-outputs:
  create-issue:
    target-repo: githubnext/ado-aw
    title-prefix: "[pipeline-failure] "
    labels:
      - pipeline-failure
    allowed-labels:
      - "agent-*"
      - "pipeline-failure"
    assignees:
      - jamesdevine
    max: 3
---

## Test Agent

Files a GitHub issue when a pipeline run fails. Used by the
`test_compile_ado_aw_debug_fixture` integration test.
