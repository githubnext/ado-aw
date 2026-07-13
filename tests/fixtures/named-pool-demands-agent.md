---
name: "Named Pool Demands Agent"
description: "Fixture that exercises named pool demands"
pool:
  name: CustomPool
  demands:
    - CustomCapability -equals required-value
    - Agent.OS -equals Linux
---

## Instructions

This fixture verifies that named pool demands compile through the full pipeline.
