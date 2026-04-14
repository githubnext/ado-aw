---
name: "ADO Work Item Triage"
description: "Triages work items using the Azure DevOps MCP"
schedule: daily around 9:00
engine: claude-sonnet-4.5
tools:
  azure-devops:
    toolsets: [core, work, work-items]
    allowed:
      - core_list_projects
      - wit_my_work_items
      - wit_get_work_item
      - wit_get_work_items_batch_by_ids
      - wit_update_work_item
      - wit_add_work_item_comment
      - search_workitem
permissions:
  read: my-read-arm-connection
  write: my-write-arm-connection
safe-outputs:
  update-work-item:
    status: true
    body: true
    tags: true
    max: 10
    target: "*"
  comment-on-work-item:
    max: 10
    target: "*"
---

## ADO Work Item Triage Agent

You have access to the Azure DevOps MCP server. Use it to:

1. Query open work items assigned to the team
2. Triage items by priority and area path
3. Add comments summarizing your analysis
4. Update tags to reflect triage status

### Guidelines

- Only update work items that need attention
- Add the `triaged` tag to items you've reviewed
- Add a comment explaining your triage decision
