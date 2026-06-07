---
name: "Dedupe Gate Only"
description: "Gate active, runtime imports inlined — download must land in Setup only"
inlined-imports: true
execution-context:
  pr:
    enabled: false
on:
  pr:
    branches:
      include: [main]
    filters:
      title: "*[agent]*"
---

## Dedupe Gate Only

Used by `test_gate_only_pipeline_downloads_bundle_in_setup_job_not_agent`.
