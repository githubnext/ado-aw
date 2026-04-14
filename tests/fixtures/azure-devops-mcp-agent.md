---
name: "Azure DevOps MCP Agent"
description: "Agent with Azure DevOps MCP via containerized stdio transport"
mcp-servers:
  azure-devops:
    container: "node:20-slim"
    entrypoint: "npx"
    entrypoint-args: ["-y", "@azure-devops/mcp", "myorg", "-d", "core", "work-items"]
    env:
      AZURE_DEVOPS_EXT_PAT: ""
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
network:
  allow:
    - "dev.azure.com"
    - "*.dev.azure.com"
---

## Azure DevOps MCP Integration Test

Review work items and create tasks as needed using the Azure DevOps MCP server.
