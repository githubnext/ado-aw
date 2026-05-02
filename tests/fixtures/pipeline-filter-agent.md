---
name: "Pipeline Filter Agent"
description: "Agent triggered by upstream pipeline with filters"
on:
  pipeline:
    name: "Build Pipeline"
    project: "OtherProject"
    branches:
      - main
    filters:
      source-pipeline: "Build*"
      time-window:
        start: "08:00"
        end: "20:00"
      build-reason:
        include: [ResourceTrigger]
---

## Pipeline Filter Agent

Only run when triggered by a Build.* pipeline during working hours.
