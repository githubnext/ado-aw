---
name: "Daily Code Review"
description: "Reviews code changes and creates summary reports"
schedule: daily
repositories:
  - repository: azure-devops-agentic-pipelines
    type: git
    name: azure-devops-agentic-pipelines
workspace: repo
---

## Code Review Agent

Review the latest code changes in the repository and provide feedback.

### Tasks

1. Analyze recent commits
2. Check for code quality issues
3. Generate a summary report
