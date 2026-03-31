---
name: "Pipeline Trigger Agent"
description: "Agent triggered by an upstream pipeline"
triggers:
  pipeline:
    name: "Build Pipeline"
    project: "OtherProject"
    branches:
      - main
---

## Task

Process artifacts from upstream build.
