---
name: "Azure DevOps MCP Agent"
description: "Agent with Azure DevOps MCP via first-class tool integration"
repos:
  - name: LocalProject/implicit-api
    checkout: false
  - name: owner/github-only
    type: github
    endpoint: github-templates
    checkout: false
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
  read:
    service-connection: my-read-arm-connection
    capabilities: [core, repos]
    allow:
      - organization: fabrikam
        projects:
          - project: Shared
            project-id: 33333333-3333-3333-3333-333333333333
            repositories: [shared-api]
  write: my-write-arm-connection
safe-outputs:
  create-work-item:
    work-item-type: Task
---

## Azure DevOps MCP Integration Test

Review work items and create tasks as needed using the Azure DevOps MCP server.
