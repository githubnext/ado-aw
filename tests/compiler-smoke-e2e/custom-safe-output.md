---
name: "Candidate compiler smoke: custom safe outputs"
description: "Exercises imported scripts-style and jobs-style custom safe outputs"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: gpt-5-mini
  timeout-minutes: 15
imports:
  - AgentPlayground/ado-aw-e2e-fixture/components/custom-build-tags/component.md@aa711dd17c4dfcde492b2bfad62e5fb1baad71f6
---

## Candidate custom safe-output smoke

You are a deterministic smoke test. Call exactly these two safe-output tools,
in this order:

1. `candidate-script-build-tag`
   - `proof`: `candidate-smoke`

2. `candidate-job-build-tag`
   - `proof`: `candidate-smoke`

Do not call any other tool. After both safe outputs are emitted, stop.
