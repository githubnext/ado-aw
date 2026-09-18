# Runtime Parameters

_Part of the [ado-aw documentation](../AGENTS.md)._

## Runtime Parameters

The `parameters` field defines Azure DevOps [runtime parameters](https://learn.microsoft.com/en-us/azure/devops/pipelines/process/runtime-parameters) that are surfaced in the ADO UI when manually queuing a pipeline run. Parameters are emitted as a top-level `parameters:` block in the generated pipeline YAML.

```yaml
parameters:
  - name: verbose
    displayName: "Verbose output"
    type: boolean
    default: false
  - name: region
    displayName: "Target region"
    type: string
    default: "us-east"
    values:
      - us-east
      - eu-west
      - ap-south
```

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Parameter identifier (valid ADO identifier) |
| `displayName` | string | No | Human-readable label in the ADO UI |
| `type` | string | No | ADO parameter type: `boolean`, `string`, `number`, `object` |
| `default` | any | No | Default value when not specified at queue time |
| `values` | list | No | Allowed values (for `string`/`number` parameters) |

Parameters can be referenced in custom steps using `${{ parameters.paramName }}`.

### Auto-injected `clearMemory` Parameter

When `tools.cache-memory` is configured, the compiler automatically injects a `clearMemory` boolean parameter (default: `false`) at the beginning of the parameters list. This parameter:

- Is surfaced in the ADO UI when manually queuing a run
- When set to `true`, skips downloading the previous agent memory artifact
- Creates an empty memory directory so the agent starts fresh

If you define your own `clearMemory` parameter in the front matter, the auto-injected one is suppressed — your definition takes precedence.

### Reserved names on `target: job` / `target: stage`

On these two targets the compiler auto-injects its own `dependsOn` and
`condition` template parameters (see [`docs/targets.md`](targets.md)) so
callers can wire in external ordering at the `template:` invocation site.
Declaring a front-matter `parameters:` entry named `dependsOn` or `condition`
on `target: job` or `target: stage` is therefore a **compile-time error** —
rename the parameter or switch targets.
