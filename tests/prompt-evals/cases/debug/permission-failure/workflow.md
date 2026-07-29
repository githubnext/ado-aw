---
name: "Synthetic Work Item Commenter"
description: "Comments on stale synthetic work items"
permissions:
  read: synthetic-read-sc
  write: synthetic-write-sc
safe-outputs:
  comment-on-work-item:
    max: 3
    target: "SyntheticProject\\Platform"
---

Review stale work items and propose a concise status-request comment.

