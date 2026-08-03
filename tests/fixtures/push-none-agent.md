---
name: "Push None Agent"
description: "Fixture exercising on.push: none alongside a synthetic-mode on.pr"
on:
  push: none
  pr:
    branches:
      include: [main]
---

## Push None Agent

`on.push: none` is the only way to say "never start this pipeline on a push".
Azure DevOps reads a *missing* top-level `trigger:` key as "run CI on every
branch", so the compiled YAML must emit `trigger: none` explicitly.

An explicit `on.push` always wins. Here it overrides the all-branches trigger
that `on.pr`'s default `mode: synthetic` would otherwise emit as its delivery
mechanism, even though that defeats synthetic PR resolution — the author asked
for it, so the compiler must not second-guess them.
