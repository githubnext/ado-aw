# Author MCP Server

_Part of the [ado-aw documentation](../AGENTS.md)._

`ado-aw mcp-author` runs a local, author/debug-facing MCP server over stdio for
IDE and Copilot Chat integrations. It exposes read-only workflow inspection,
graph, step dependency traversal, output mapping, lint, what-if, trace, and audit tools.

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
| `catalog` | List safe-outputs, runtimes, tools, engines, and models. | `{ "kind": "safe-outputs" }` |
| `audit_build` | Download and analyze a build; same shape as `ado-aw audit --json`. | `{ "build_id_or_url": "123", "org": null, "project": null, "pat": null, "artifacts": null, "no_cache": false }` |

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
