# Auditing Pipelines with `ado-aw audit`

_Part of the [ado-aw documentation](../AGENTS.md)._

## Overview

`ado-aw audit` audits one Azure DevOps build at a time. It downloads the selected build artifacts, runs the built-in analyzers (firewall, MCP gateway, OTel, safe outputs, detection verdict, build timeline, and missing-tool / missing-data / noop extraction), and renders a structured console report or the raw `AuditData` JSON. The MVP is single-run only; diff mode and cross-run trend reporting are follow-ups.

## Usage

`ado-aw audit <build-id-or-url> [options]`

## Accepted input formats

| Input | Example |
|---|---|
| Numeric build ID | `12345` |
| dev.azure.com URL | `https://dev.azure.com/my-org/My%20Project/_build/results?buildId=12345` |
| dev.azure.com URL with job/step anchors | `...?buildId=12345&j=<guid>&t=<guid>` (accepted; the MVP audits the parent build) |
| Legacy visualstudio.com URL | `https://my-org.visualstudio.com/proj/_build/results?buildId=12345` |
| On-prem Azure DevOps Server URL | `https://onprem.example.com/DefaultCollection/MyProject/_build/results?buildId=12345` |

URL-encoded project segments are decoded before the ADO context is resolved. `t=` and `s=` are both accepted as step anchors.

## Flags

| Flag | Default | Behavior |
| --- | --- | --- |
| `-o, --output <dir>` | `./logs` | Directory under which `<dir>/build-<id>/` is written. Non-CLI entry points (`ado-aw trace`, the mcp-author tools) instead default to the shared `${TEMP}/ado-aw/audit` cache root so they do not scatter `./logs/` directories under arbitrary working directories. |
| `--json` | off | Emit the full `AuditData` as JSON to stdout (suppresses the trailing `Audit complete` stderr line). |
| `--org <url>` | auto | Azure DevOps organization override for bare build IDs. Full build URLs provide the host / org directly. |
| `--project <name>` | auto | Azure DevOps project override for bare build IDs. Full build URLs provide the project directly. |
| `--pat <token>` | env | Personal Access Token. Also reads `AZURE_DEVOPS_EXT_PAT`. Falls back to the existing Azure CLI auth chain when omitted. |
| `--artifacts <set,...>` | all | Restrict download + analysis to a subset of artifact sets. Valid values: `agent`, `detection`, `safe-outputs` (`safe_outputs` alias also accepted). |
| `--no-cache` | off | Force re-processing even if `<dir>/build-<id>/run-summary.json` already exists. |

## Behavior

- The command resolves `<build-id-or-url>` first. Bare IDs use `--org` / `--project` or git-remote auto-detection; full build URLs contribute host, org, and project, and those URL-derived values win.
- Only the three audit artifact families are in scope: `agent_outputs*`, `analyzed_outputs*`, and `safe_outputs*`. Other published build artifacts are ignored.
- Artifact refresh is cache-preserving. If a matching local artifact directory already exists, it is renamed aside before re-download and restored if the download fails.
- Analyzer failures are soft. The command records a warning, keeps any successfully-derived sections, and still renders the report.
- When multiple local directories share one recognized prefix, the lexicographically last match is used.

## Output layout

```text
<output>/build-<id>/
├── run-summary.json                  # Cached AuditData, CLI-version-keyed
├── agent_outputs[_<BuildId>]/        # Downloaded artifact (Agent stage)
│   ├── staging/
│   │   ├── safe_outputs.ndjson       # Agent's safe-output proposals
│   │   ├── aw_info.json              # Runtime engine / agent / source metadata
│   │   └── otel.jsonl                # Copilot OTel (when emitted)
│   └── logs/
│       ├── firewall/                 # AWF Squid proxy logs
│       ├── mcpg/                     # MCP Gateway logs
│       ├── safeoutputs.log           # SafeOutputs HTTP server log
│       └── agent-output.txt          # Filtered agent stdout
├── analyzed_outputs[_<BuildId>]/     # Downloaded artifact (Detection stage)
│   ├── threat-analysis.json          # Aggregate verdict + reasons
│   └── threat-analysis-output.txt
└── safe_outputs[_<BuildId>]/         # Downloaded artifact (SafeOutputs stage)
    └── safe-outputs-executed.ndjson  # Per-item execution log
```

`aw_info.json`, `otel.jsonl`, and `safe_outputs.ndjson` are searched in `staging/` first and then at the artifact top level so older layouts still audit cleanly.

## Report shape (`AuditData`)

Current top-level keys include the following. Optional sections are omitted from `--json` when empty.

| Key | Source |
| --- | --- |
| `overview` | ADO build metadata + `aw_info.json` (engine, model, agent name, source, target). |
| `task_domain` | Audit heuristics over the run's prompts and outputs. |
| `behavior_fingerprint` | Higher-level audit heuristics over the run's behavior. |
| `agentic_assessments` | Higher-level audit assessments emitted by the analyzers. |
| `metrics` | OTel JSONL (`otel.jsonl`) plus audit-time warning / error counts. |
| `key_findings` | Heuristic rules + analyzer-emitted findings (for example aggregate-gate rejection). |
| `recommendations` | Follow-up actions derived from findings. |
| `performance_metrics` | Derived from `metrics`, runtime duration, tool usage, and firewall counts. |
| `engine_config` | Runtime engine configuration derived from `aw_info.json`. |
| `safe_output_summary` | Counts of proposed / executed / rejected / not processed items. |
| `safe_output_execution` | Per-item trace joining proposal + detection + execution. |
| `rejected_safe_outputs` | Rollup of rejections by reason / threat flag. |
| `detection_analysis` | `threat-analysis.json`. |
| `mcp_server_health` | MCPG logs aggregated per server. |
| `pipeline_graph` | Optional typed-IR `PipelineSummary` rebuilt from local source metadata (`aw_info.json.source`) for graph correlation. |
| `mcp_tool_usage` | MCPG logs aggregated per `(server, tool)`. |
| `mcp_failures` | MCPG `tool_error` / `server_error` events. |
| `jobs` | ADO `/timeline` records filtered to `type: Job`; when `pipeline_graph` is available, each entry may include `upstream_jobs` and `downstream_jobs` from IR job edges. |
| `firewall_analysis` | AWF Squid proxy logs aggregated by domain. |
| `policy_analysis` | AWF policy artifacts aggregated into allow / deny summaries. |
| `missing_tools` / `missing_data` / `noops` | NDJSON entries from the corresponding SafeOutputs MCP tools. |
| `downloaded_files` | One entry per file under `<output>/build-<id>/`. |
| `errors` / `warnings` | Run-level error / warning aggregates. |
| `tool_usage` | High-level runtime tool-usage rollups derived from telemetry. |
| `created_items` | Successful `executed` items with extracted id / url / title. |

## Rejected safe-output trace

When `threat-analysis.json` reports any threat flag, the audit treats the SafeOutputs batch as rejected by the aggregate gate and records each proposal with:

- `status: not_processed_due_to_aggregate_gate`
- `applies_to_whole_batch: true`
- `rejection_reason`: the aggregate `reasons[]` from `threat-analysis.json`, joined with `; `

Additionally, exactly one severity-`high` finding is emitted summarizing the gate decision: which threat flags fired, how many proposals were dropped, and the full aggregate reasons.

Per-item detection verdicts are not currently available. `threat-analysis.md` emits an aggregate verdict only; per-item verdicts are a follow-up that should stay aligned with gh-aw.

## Pipeline graph correlation

After the standard analyzers run, `audit` looks for
`agent_outputs[_<BuildId>]/staging/aw_info.json` (falling back to the artifact
top level) and resolves its `source` path relative to the current working
directory. If that markdown source exists locally, the command rebuilds the
typed IR with the same public summary shape emitted by `ado-aw inspect --json`
and stores it under `pipeline_graph.summary`. The audit embeds the full
`PipelineSummary` rather than a reduced subset so audit, inspect, graph, and
trace consumers share one schema.

When graph correlation succeeds, `jobs[]` entries also gain optional
`upstream_jobs` and `downstream_jobs` arrays. These are omitted when empty or
when the source markdown is unavailable locally. Failed jobs with downstream
edges emit a medium-severity finding summarizing the downstream runtime
classifications.

## Cache behavior

`<output>/build-<id>/run-summary.json` is written after a successful run. On subsequent invocations against the same build:

- If the cached `ado_aw_version` matches the current CLI version, the report is rendered from cache and download / analysis is skipped. The cache-hit info line is printed only in console mode.
- If the cached file is missing, cannot be parsed, or was written by a different `ado-aw` version, it is ignored and the build is processed again.
- `--no-cache` always re-processes.

## Permission failures

- The initial build-metadata fetch is live ADO-only. A 401 / 403 at that step is fatal.
- If artifact listing or artifact download returns 401 / 403 and the run directory already contains at least one recognized artifact family, the audit continues from local cache and records a warning.
- If artifact listing or download returns 401 / 403 and no local artifact cache exists, the command emits a structured error pointing at `az pipelines runs artifact download --run-id <id> --path <dir>` as the manual escape hatch.

## Out-of-scope (planned follow-ups)

- **Diff mode** (`ado-aw audit <a> <b>`) — domain / MCP / metrics diffs.
- **Cross-run trends** (`ado-aw audit --last N`) — trend report over recent builds.
- **`--parse`** — Rust-native `log.md` / `firewall.md` renderers.
- **Job / step audit** — pin to a specific timeline record.
- **MCP-exposed audit** — `agentic-pipelines` MCP tool for in-pipeline self-audit.
- **Per-item detection verdict** — coordinated upstream with gh-aw.
- **Additional pipeline inventory artifacts** — graceful-degradation gaps such as richer AWF policy / firewall inventories.

## Related Documentation

- [CLI Commands](cli.md) — full CLI reference, including `trace`
- [Front Matter](front-matter.md) — agent file format
- [Safe Outputs](safe-outputs.md) — what proposals look like
- [Network](network.md) — AWF firewall configuration
