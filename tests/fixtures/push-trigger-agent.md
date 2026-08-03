---
name: "Push Trigger Agent"
description: "Fixture exercising on.push with native ADO branch and path filters"
on:
  push:
    branches:
      include: [main, "release/*"]
      exclude: ["wip/*"]
    paths:
      include: ["src/**"]
      exclude: ["docs/**"]
---

## Push Trigger Agent

This agent declares an explicit `on.push`, so the compiled YAML must contain
a top-level `trigger:` block carrying the authored branch and path filters
verbatim — never the `none` scalar, and never the all-branches form.

`on.push` controls only `trigger:`. With no `on.pr` present, the compiler
still emits `pr: none`, because Azure DevOps reads a missing `pr:` key as
"build every pull request".
