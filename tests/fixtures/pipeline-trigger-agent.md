---
name: Pipeline Trigger Agent
description: Agent triggered by an upstream pipeline
on:
  pipeline:
    name: Build Pipeline
    project: OtherProject
    branches:
    - main
schema-version: 2
---

## Task

Process artifacts from upstream build.
