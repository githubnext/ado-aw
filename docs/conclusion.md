# Conclusion Job

_Part of the [ado-aw documentation](../AGENTS.md)._

The Conclusion job is an always-running housekeeping job that reports
pipeline failures and diagnostic signals (`noop`, `missing-tool`,
`missing-data`) to Azure DevOps work items.

## When it runs

The compiler emits the Conclusion job only when `conclusion:` is present
in front matter. The job runs with `condition: always()`, so it still
executes regardless of upstream job outcomes.

## Pipeline shape

```text
Setup → Agent → Detection → SafeOutputs → Teardown → Conclusion
                                                        ↑
                                              condition: always()
```

## Configuration (`conclusion:`)

| Field | Type | Default | Notes |
|---|---|---|---|
| `report-failure-as-work-item` | bool | `true` | Enables work-item filing/commenting. |
| `work-item-title` | string | _built-in per signal_ | Optional title override; supports the `{pipeline_name}` placeholder. |
| `work-item-type` | string | `"Bug"` | Work item type to create when no open title match exists. |
| `area-path` | string | _none_ | Optional Azure DevOps area path. |
| `iteration-path` | string | _none_ | Optional Azure DevOps iteration path. |
| `tags` | list of strings | `[]` | Static tags applied to created work items. |
| `include-stats` | bool | `true` | Appends build/job stats to the report body. |

### Example

```yaml
conclusion:
  work-item-type: Bug
  area-path: "MyProject\\MyTeam"
  tags:
    - pipeline-failure
    - automated
```

## What gets reported

- **Pipeline failure** — when the Agent, Detection, or SafeOutputs job
  fails.
- **Noop** — when the agent produced noop safe outputs.
- **Missing tool** — when the agent reported missing tools.
- **Missing data** — when the agent reported missing data.

## How it works

The job downloads the `safe_outputs` artifact, reads
`safe-outputs-executed.ndjson`, checks upstream Agent / Detection /
SafeOutputs job results, and then files or comments on Azure DevOps
work items using `SYSTEM_ACCESSTOKEN`.

## Deduplication

Conclusion reports deduplicate by rendered work-item title. The job
searches for an existing open work item with the same title; if it finds
one, it appends a comment. Otherwise it creates a new work item.

## Relationship to gh-aw

This mirrors gh-aw's conclusion-job pattern: a single always-running
post-pipeline job handles housekeeping after the main agentic flow.

## Security

The Conclusion job uses `SYSTEM_ACCESSTOKEN` only inside the
post-pipeline reporter. It works from compiler-controlled `conclusion:`
configuration plus the sanitized `safe-outputs-executed.ndjson`
execution manifest rather than giving raw agent prompt content direct
work-item API access.
