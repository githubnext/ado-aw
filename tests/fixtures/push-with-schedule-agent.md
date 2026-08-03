---
name: "Push With Schedule Agent"
description: "Fixture proving an explicit on.push survives schedule trigger suppression"
on:
  schedule: daily around 03:00
  push:
    branches:
      include: [main]
---

## Push With Schedule Agent

"Run nightly, and also whenever `main` moves" is a legitimate pipeline shape.
A schedule on its own suppresses CI and PR triggers, but an explicit `on.push`
must survive that suppression rather than being silently ignored.

The compiled YAML must therefore carry both a `schedules:` block and a
`trigger:` block filtered to `main`. `pr:` stays `none`, because `on.push`
controls only the `trigger:` half — the PR half is driven by `on.pr`.
