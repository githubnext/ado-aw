---
name: "PR Filter Tier 2 Agent"
description: "Agent with Tier 2/3 PR filters (requires evaluator extension)"
on:
  pr:
    branches:
      include: [main]
    filters:
      title:
        match: "\\[review\\]"
      labels:
        any-of: ["run-agent", "needs-review"]
        none-of: ["do-not-run"]
      draft: false
      time-window:
        start: "09:00"
        end: "17:00"
      min-changes: 1
      max-changes: 500
---

## Tier 2 Filter Agent

Run agent only during business hours, on non-draft PRs with the right
labels, with a reasonable number of changed files.
