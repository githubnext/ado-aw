---
name: "Execution Context Agent"
description: "Agent exercising the execution-context PR contributor"
on:
  pr:
    branches:
      include: [main]
---

## Execution Context Agent

This fixture exercises the always-on `ExecContextExtension` with the PR
contributor in its default configuration.

When triggered by a pull request, the precompute step stages
`aw-context/pr/base.sha` and `aw-context/pr/head.sha` under
`$(Build.SourcesDirectory)/aw-context/`, and appends a tailored prompt
fragment to the agent prompt with literal PR id / project / repo values
plus example `git diff $BASE..$HEAD` and Azure DevOps MCP tool calls.

On preparation failure the step writes `aw-context/pr/error.txt` and a
failure-mode prompt fragment instead.

