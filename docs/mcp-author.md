# Author MCP Server

_Part of the [ado-aw documentation](../AGENTS.md)._

`ado-aw mcp-author` runs a local, author/debug-facing MCP server over stdio for
IDE and Copilot Chat integrations. It exposes read-only workflow inspection,
graph, lint, what-if, trace, and audit tools.

It is **not** the SafeOutputs MCP server embedded in compiled pipelines. The
pipeline SafeOutputs server records proposed mutations for Stage 3 execution;
`mcp-author` is a local helper for humans and agents authoring or debugging
workflows.

## Tool surface

| Tool | Description | Input shape |
| --- | --- | --- |
| `inspect_workflow` | Build and return the public `PipelineSummary`. | `{ "source_path": "agents/example.md" }` |
| `graph_summary` | Return the resolved `GraphSummary`. | `{ "source_path": "agents/example.md" }` |
| `graph_dump` | Render the graph as text or Graphviz DOT. | `{ "source_path": "...", "format": "text" \| "dot" }` |
| `step_dependencies` | Traverse dependencies for a step or job id. | `{ "source_path": "...", "step_id": "Agent", "direction": "upstream" \| "downstream" }` |
| `step_outputs` | List declared outputs and consumers. | `{ "source_path": "...", "producer": null, "consumer": null }` |
| `trace_failure` | Trace a build's failed-job chain using audit data plus any local IR graph. | `{ "build_id_or_url": "123", "step": null, "org": null, "project": null, "pat": null }` |
| `whatif` | Classify downstream jobs if a step or job fails. | `{ "source_path": "...", "failing_id": "Agent" }` |
| `lint_workflow` | Run structural lint checks. | `{ "source_path": "agents/example.md" }` |
| `validate_steps` | IR-validate a proposed front-matter step block. | `{ "steps": [...], "allow_list": "full" \| "curated" }` |
| `catalog` | List safe-outputs, runtimes, tools, engines, and models. | `{ "kind": "safe-outputs" }` |
| `audit_build` | Download and analyze a build; same shape as `ado-aw audit --json`. | `{ "build_id_or_url": "123", "org": null, "project": null, "pat": null, "artifacts": null, "no_cache": false }` |

## `validate_steps`

`validate_steps` lets an authoring agent run the shared IR step-block validator
against a proposed `steps:` or `post-steps:` block before writing it into the
source `.md` file.

Input schema:

```jsonc
{
  "steps": [/* JSON array of ADO step entries */],
  "allow_list": "full" | "curated" // optional, default "full"
}
```

Modes:

- `"full"` accepts every step kind the IR models (`bash`, `task`, `checkout`,
  `download`, `publish`) and any valid task identifier. Use it when an author is
  writing their own steps.
- `"curated"` restricts `task:` steps to the typed-factory set in
  `src/compile/ir/tasks.rs` (`CURATED_TASK_IDS`). Use it when tooling
  double-checks an untrusted agent-proposed block.

Success response:

```json
{
  "ok": "true",
  "kinds": [
    { "kind": "bash" }
  ]
}
```

Error response:

```json
{
  "ok": "false",
  "errors": [
    {
      "step_index": 0,
      "path": "steps[0].task",
      "message": "task \"AzureCLI@2\" is not in the curated allow-list (Curated mode only permits tasks with a typed factory in src/compile/ir/tasks.rs)"
    },
    {
      "step_index": 1,
      "path": "steps[1].bash",
      "message": "bash: value must be a string (the script body)"
    }
  ]
}
```

Validation collects errors instead of short-circuiting, so one call returns the
full picture for the proposed block.

## Trust model

`mcp-author` runs as the invoking local user. It has no bounding directory,
sandbox, or pipeline-style filesystem restrictions. ADO-facing calls (`audit`,
`trace`) use the same `resolve_auth()` path as `ado-aw audit`: explicit PAT,
environment, or Azure CLI fallback depending on local configuration.

## IDE configuration

### VS Code MCP

```json
{
  "mcp": {
    "servers": {
      "ado-aw-author": {
        "command": "ado-aw",
        "args": ["mcp-author"]
      }
    }
  }
}
```

### Claude Desktop

Add this to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "ado-aw-author": {
      "command": "ado-aw",
      "args": ["mcp-author"]
    }
  }
}
```

## Related references

- [`docs/ir.md#public-json-summary-irsummary`](ir.md#public-json-summary-irsummary) — public summary schema contract.
- [`docs/audit.md`](audit.md) — `audit_build` and `trace_failure` build reference and report details.
- [`docs/cli.md`](cli.md) — CLI counterparts for every MCP tool.
