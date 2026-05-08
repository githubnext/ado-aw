---
name: Comment on Work Item Agent
description: Agent that comments on work items with area path scoping
permissions:
  write: my-write-sc
safe-outputs:
  comment-on-work-item:
    target: MyProject\Backend
    max: 3
schema-version: 2
---

## Comment on Work Item Agent

Review work items and add comments with findings.
