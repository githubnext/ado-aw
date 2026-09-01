---
name: "ADO proxy read-only az agent"
description: "permissions.read enables wrapped az without the Azure DevOps MCP"
tools:
  bash: [head]
  azure-devops: false
permissions:
  read: my-read-arm-connection
safe-outputs:
  noop: {}
---

Use wrapped Azure CLI reads, then emit noop.
