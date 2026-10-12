---
name: invalid-command-dispatch-task-input-agent
description: task input validation surfaces command-dispatch task errors via lint
steps:
- task: DotNetCoreCLI@2
  displayName: Push input on a test command
  inputs:
    command: test
    packagesToPush: foo.nupkg
- task: NuGetCommand@2
  displayName: Unknown NuGet command
  inputs:
    command: teleport
---
## Body

This fixture authors two command-dispatch task steps with invalid inputs:

1. A `DotNetCoreCLI@2` step with `command: test` that supplies
   `packagesToPush`, an input that only belongs to `command: push`.
2. A `NuGetCommand@2` step with an unrecognized `command: teleport`.

`compile` succeeds silently and passes both steps through to the generated
YAML unchanged; the advisory validation findings are surfaced only through
`ado-aw lint` / the `lint_workflow` MCP tool (as `task-input-invalid`
warnings), not on the compile path. This covers the command-dispatch
validators (`dotnet_core_cli::validate_inputs`, `nuget_command::validate_inputs`)
which previously had no test coverage anywhere in the repository (unlike the
`Docker@2` command-dispatch validator, which is covered both in-module and via
`invalid-task-input-agent.md`).
