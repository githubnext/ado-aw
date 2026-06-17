---
name: "Self-Optimization Live Agent"
description: "Fixture exercising self-optimization with staged: false"
self-optimization:
  enabled: true
  staged: false
  max-proposals-per-run: 5
  allowed-sections:
    - steps
    - post-steps
    - setup
---

## Test Agent

Exercises `self-optimization.staged: false` (live mode). Used by
`test_self_optimization_staged_false_compiles_identically` to confirm
that the compiled YAML is the same regardless of the `staged` flag
(staging is a Stage 3 runtime decision, not a compile-time fork).
