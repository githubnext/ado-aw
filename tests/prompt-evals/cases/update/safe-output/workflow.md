---
name: "Work Item Reader"
description: "Reads selected work items and summarizes stale ownership"
tools:
  azure-devops:
    org: synthetic-org
    toolsets: [core, work-items]
    allowed:
      - wit_get_work_item
      - wit_get_work_items_batch_by_ids
      - search_workitem
permissions:
  read: synthetic-read-sc
---

## Objective

Find open work items without a current owner update.

## Procedure

1. Query open work items in the configured project.
2. Identify items whose owner has not posted a recent update.
3. Explain why each item may need attention.

## Output

Return a concise summary grouped by area path.

## No Action

If all reviewed work items have a recent owner update, report that no action is
needed.
