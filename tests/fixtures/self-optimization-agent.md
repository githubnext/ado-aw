---
name: "Self-Optimization Test Agent"
description: "Fixture exercising the opt-in self-optimization front matter section"
self-optimization:
  enabled: true
  staged: true
  max-proposals-per-run: 3
  allowed-sections:
    - steps
    - post-steps
---

## Test Agent

Exercises the `self-optimization:` opt-in feature. The compiler must
add `propose-step-optimization` to the agent's `--enabled-tools`
list, and the wiring must round-trip through compile/validate
without errors. Used by the
`test_self_optimization_enables_propose_step_optimization_tool`
integration test.

Do nothing else. The agent body is intentionally minimal.
