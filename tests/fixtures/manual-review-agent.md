---
name: "Manual Review Agent"
description: "Agent whose high-impact outputs require manual approval"
on:
  schedule: "daily around 14:00"
safe-outputs:
  require-approval: true
  create-pull-request: {}
  add-pr-comment:
    require-approval: false
---

## Task

Propose changes that a human approves before they are applied.
