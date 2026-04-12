---
name: "ADO Work Item Triage"
description: "Triages work items using the Azure DevOps MCP"
schedule: daily around 9:00
engine: claude-sonnet-4.5
mcp-servers:
  azure-devops:
    container: "node:20-slim"
    entrypoint: "npx"
    entrypoint-args: ["-y", "@azure-devops/mcp", "myorg", "-d", "core", "work", "work-items"]
    env:
      AZURE_DEVOPS_EXT_PAT: ""
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
network:
  allow:
    - "dev.azure.com"
    - "*.dev.azure.com"
    - "*.visualstudio.com"
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
