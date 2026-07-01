---
name: invalid-task-input-agent
description: task input validation surfaces via lint but never fails compile
steps:
- task: CopyFiles@2
  displayName: Copy with bad input
  inputs:
    Contents: "**"
    Bogus: nope
---
## Body

This fixture authors a `CopyFiles@2` step that is missing the required
`TargetFolder` input and supplies an unknown `Bogus` input. `compile` succeeds
**silently** and passes the step through to the generated YAML unchanged; the
advisory validation finding is surfaced only through `ado-aw lint` /
the `lint_workflow` MCP tool (as a `task-input-invalid` warning), not on the
compile path.
