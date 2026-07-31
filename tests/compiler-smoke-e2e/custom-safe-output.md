---
name: "Candidate compiler smoke: custom safe outputs"
description: "Exercises an imported jobs-style custom safe output"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: claude-sonnet-4.6
  timeout-minutes: 15
imports:
  - ./component-fixture/components/custom-build-tags/component.md
safe-outputs:
  noop: {}
---

## Candidate custom safe-output smoke

You are a deterministic smoke test. Call exactly this safe-output tool:

1. `candidate-job-build-tag`
   - `proof`: `candidate-smoke`

This is an actual custom MCP tool, not a label or alias for another tool.
Do **not** call `noop`, and do **not** substitute the built-in
`add-build-tag` tool. After the custom safe output is emitted, stop.
