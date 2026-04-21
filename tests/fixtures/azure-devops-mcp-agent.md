---
name: "Azure DevOps MCP Agent"
description: "Agent with Azure DevOps MCP via first-class tool integration"
tools:
  azure-devops:
    org: myorg
    toolsets: [core, work-items]
    allowed:
      - core_list_projects
      - wit_get_work_item
      - wit_create_work_item
      - wit_my_work_items
permissions:
  read: my-read-arm-connection
  write: my-write-arm-connection
safe-outputs:
  create-work-item:
    work-item-type: Task
---

## Azure DevOps MCP Integration Test

Review work items and create tasks as needed using the Azure DevOps MCP server.
