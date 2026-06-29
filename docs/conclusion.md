# Conclusion Job

_Part of the [ado-aw documentation](../AGENTS.md)._

The Conclusion job is an always-running housekeeping job that reports
pipeline failures and diagnostic signals (`noop`, `missing-tool`,
`missing-data`) to Azure DevOps work items.

## When it runs

The compiler emits the Conclusion job whenever `safe-outputs:` is
configured in front matter (noop is always on, so the conclusion job
runs for every pipeline that has safe outputs). The job runs with
`condition: always()`, regardless of upstream job outcomes.

## Pipeline shape

```text
Setup → Agent → Detection → SafeOutputs → Teardown → Conclusion
                                                        ↑
                                              condition: always()
```

## Configuration

All configuration lives under `safe-outputs:` in front matter.

### Global toggle

`report-failure-as-work-item` is a **master kill-switch**. When set to
`false` the Conclusion job files **no** work items at all — this suppresses
*every* signal, including the `noop`, `missing-tool`, and `missing-data`
diagnostics, not just pipeline failures. To suppress an individual diagnostic
while keeping the others, use its per-tool `report-as-work-item: false`
(see below) instead of the global toggle.

```yaml
safe-outputs:
  report-failure-as-work-item: false   # master kill-switch: disable ALL work-item filing
```

### Per-tool configuration

Each diagnostic tool (`noop`, `missing-tool`, `missing-data`) supports
these fields under its `safe-outputs:` entry:

| Field | Type | Default | Notes |
|---|---|---|---|
| `report-as-work-item` | bool | `true` | Per-tool opt-out for work-item filing. |
| `title-prefix` | string | _built-in per signal_ | Prefix for the work-item title. |
| `work-item-type` | string | `"Task"` | Work item type to create. |
| `area-path` | string | _none_ | Azure DevOps area path. |
| `iteration-path` | string | _none_ | Azure DevOps iteration path. |
| `tags` | list of strings | `[]` | Static tags applied to created work items. |

### Example

```yaml
safe-outputs:
  noop:
    title-prefix: "[ado-aw] Agent noop"
    work-item-type: Task
    area-path: "MyProject\\MyTeam"
    tags:
      - agent-noop
  missing-tool:
    report-as-work-item: false          # don't file WIs for missing tools
  missing-data: {}                      # use defaults
```

### Disabling a tool entirely

Setting a tool to `false` prevents the agent from calling it and
disables work-item filing:

```yaml
safe-outputs:
  noop: false
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

Per-tool config is passed from the compiler to `conclusion.js` as
individual flat env vars per field (e.g. `AW_NOOP_TITLE_PREFIX`,
`AW_NOOP_AREA_PATH`), matching gh-aw's pattern.

## Deduplication

Conclusion reports deduplicate by rendered work-item title. The job
searches for an existing open work item with the same title; if it finds
one, it appends a comment. Otherwise it creates a new work item.

## Relationship to gh-aw

This mirrors gh-aw's conclusion-job pattern: a single always-running
post-pipeline job handles housekeeping after the main agentic flow.

## Security

The Conclusion job uses `SYSTEM_ACCESSTOKEN` (or `SC_WRITE_TOKEN` when
a write service connection is configured) only inside the post-pipeline
reporter. It works from compiler-controlled `safe-outputs:`
configuration plus the sanitized `safe-outputs-executed.ndjson`
execution manifest rather than giving raw agent prompt content direct
work-item API access.
