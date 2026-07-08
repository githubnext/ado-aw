# Target Platforms

_Part of the [ado-aw documentation](../AGENTS.md)._

## Target Platforms

The `target` field in the front matter determines the output format and execution environment for the compiled pipeline.

### `standalone` (default)

Generates a self-contained Azure DevOps pipeline with:
- Full 3-job pipeline: `Agent` → `Detection` → `SafeOutputs`
- AWF (Agentic Workflow Firewall) L7 domain whitelisting via Squid proxy + Docker
- MCP Gateway (MCPG) for MCP routing with SafeOutputs HTTP backend
- Setup/teardown job support
- All safe output features (create-pull-request, create-work-item, etc.)

This is the recommended target for maximum flexibility and security controls.

### `1es`

Generates a pipeline that extends the 1ES Unofficial Pipeline Template:
- Uses `templateContext.type: buildJob` with Copilot CLI + AWF + MCPG (same execution model as standalone)
- Integrates with 1ES SDL scanning and compliance tools
- Full 3-job pipeline: Agent → Detection → SafeOutputs
- Requires 1ES Pipeline Templates repository access

Example:
```yaml
target: 1es
```

When using `target: 1es`, the pipeline will extend `1es/1ES.Unofficial.PipelineTemplate.yml@1ESPipelinesTemplates`.

### `job`

Generates a **job-level ADO YAML template** with `jobs:` at root. This is a
reusable template that can be included in an existing pipeline — it does not
generate a complete pipeline.

The output contains the same 3-job chain (Agent → Detection → SafeOutputs) as
`standalone`, with:
- Job names prefixed with the agent name for uniqueness (e.g., `DailyReview_Agent`)
- No triggers, pipeline name, or resource declarations (the parent pipeline owns those)
- Pool baked in from the front matter `pool:` field (`vmImage` or `name`; defaults to `vmImage: ubuntu-22.04`)

> **Variable groups are not supported here.** ADO `job` / `stage` templates
> cannot declare pipeline-level `variables:` — the parent pipeline owns them.
> Declaring [`variable-groups:`](front-matter.md#variable-groups-variable-groups)
> on `target: job` or `target: stage` is a compile-time error; import the group
> in the parent pipeline that includes this template instead.

Example front matter:
```yaml
target: job
```

#### Usage in a flat pipeline

```yaml
jobs:
  - job: Build
    steps: ...
  - template: agents/review.lock.yml
    parameters:
      dependsOn: [Build]              # list of upstream job names; omit for implicit dep on previous job
      condition: succeeded('Build')   # optional; ANDed into the agent job's internal condition
```

#### Usage inside a user-defined stage

```yaml
stages:
  - stage: Build
    jobs: ...
  - stage: AgenticReview
    dependsOn: Build
    jobs:
      - template: agents/review.lock.yml
```

#### Notes

- ADO's [`jobs.template`](https://learn.microsoft.com/azure/devops/pipelines/yaml-schema/jobs-template)
  schema only allows `template:` and `parameters:` at the call site. `dependsOn:`
  and `condition:` as bare keys on the `- template:` line are rejected; the
  compiled template surfaces them as parameters and applies them to the
  agent job internally.
- When the agent has a Setup job (e.g. PR or pipeline filters), the
  `dependsOn` parameter MUST be a YAML list — the template uses
  `${{ each }}` to merge `Setup` with the caller's deps, and `${{ each }}`
  requires an iterable. For agents without a Setup job, either a string or a
  list works.
- The `condition` parameter is ANDed into the agent job's existing internal
  condition (PR gate, pipeline gate, etc.). Empty default preserves ADO's
  native `succeeded()` behaviour.
- Triggers (`on:`) are ignored with a warning (the parent pipeline controls triggers).
- If the agent declares additional repositories via `repos:`, add them to the
  parent pipeline's `resources:` block (documented in the generated file header).

### `stage`

Generates a **stage-level ADO YAML template** with `stages:` at root. This
wraps the 3-job chain inside a stage block for direct inclusion in multi-stage
pipelines.

Example front matter:
```yaml
target: stage
```

#### Usage

```yaml
stages:
  - stage: Build
    jobs: ...
  - template: agents/review.lock.yml
    parameters:
      dependsOn: Build              # or [Build, Test]; omit for implicit dep on previous stage
      condition: succeeded('Build') # optional; omit for ADO's default succeeded()
```

#### Notes

- ADO's [`stages.template`](https://learn.microsoft.com/azure/devops/pipelines/yaml-schema/stages-template)
  schema only allows `template:` and `parameters:` at the call site —
  `dependsOn:` and `condition:` as bare keys on the `- template:` line are
  rejected by the YAML parser. The compiled template surfaces them as the
  `dependsOn` and `condition` parameters and applies them to the inner
  stage block via ADO conditional template expressions, so empty defaults
  preserve ADO's implicit "depends on previous stage" and `succeeded()`
  behaviour.
- The `dependsOn` parameter is typed `object`, matching ADO's native
  `dependsOn:` semantics (accepts a single string or a list).
- Same 3-job chain, job-name prefixing, and pool handling as `target: job`.
- Triggers (`on:`) are ignored with a warning.
- If the agent declares additional repositories via `repos:`, add them to the
  parent pipeline's `resources:` block.
