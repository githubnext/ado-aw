---
name: "Complete Test Agent"
description: "A complete test agent with all features enabled"
schedule: daily around 14:00
repositories:
  - repository: test-repo-1
    type: git
    name: test-org/test-repo-1
    ref: refs/heads/main
  - repository: test-repo-2
    type: git
    name: test-org/test-repo-2
    ref: refs/heads/develop
checkout:
  - test-repo-1
mcp-servers:
  ado: true
  es-chat: true
  bluebird: true
  icm:
    allowed:
      - create_incident
      - get_incident
  kusto:
    allowed:
      - query
  custom-tool:
    command: "node"
    args: ["server.js"]
    allowed:
      - custom_function_1
      - custom_function_2
    env:
      NODE_ENV: "test"
steps:
  - bash: echo "Preparing context"
    displayName: "Prepare context"
post-steps:
  - bash: echo "Finalizing"
    displayName: "Finalize"
setup:
  - bash: echo "Setup step"
    displayName: "Setup"
teardown:
  - bash: echo "Teardown step"
    displayName: "Teardown"
---

## Complete Test Agent

This agent demonstrates all available configuration options.

### Features

- Multiple repositories
- Daily schedule around 2 PM (fuzzy ±60 min)
- Mix of built-in and custom MCPs
- Permission configuration
- Environment variables

### Instructions

1. Review the configuration
2. Execute the task
3. Report results
