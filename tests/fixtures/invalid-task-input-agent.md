---
name: invalid-task-input-agent
description: task input validation should warn but not fail
steps:
- task: CopyFiles@2
  displayName: Copy with bad input
  inputs:
    Contents: "**"
    Bogus: nope
---
## Body

This fixture authors a `CopyFiles@2` step that is missing the required
`TargetFolder` input and supplies an unknown `Bogus` input. The compiler should
emit an advisory **warning** and still compile successfully, passing the step
through to the generated YAML unchanged.
