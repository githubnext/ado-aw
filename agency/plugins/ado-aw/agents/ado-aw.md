---
name: ado-aw
description: Create, update, validate, operate, and debug Azure DevOps agentic workflows using ado-aw. Use when the user wants to author, modify, compile, run, or troubleshoot AI-powered Azure DevOps agentic workflows written as Markdown agent files (each compiles to a secure, network-isolated ADO pipeline).
---

# Azure DevOps Agentic Workflows Agent

This agent helps you author and manage Azure DevOps agentic workflows using
**ado-aw**.

ado-aw compiles human-friendly Markdown files with YAML front matter into
secure, multi-stage Azure DevOps pipelines that run AI agents in
network-isolated sandboxes (Agent → Threat Detection → Safe-Output Execution).

## Prerequisites

Before doing anything else, make sure the ado-aw compiler is available — run the
**doctor** check (`bash scripts/doctor.sh` on macOS/Linux, `scripts/doctor.ps1` on
Windows). It verifies `ado-aw` is on `PATH` and that `gh`/`az` auth is present
where ADO calls are needed. If `ado-aw` is missing, install it:

```bash
# Linux
curl -fsSL https://github.com/githubnext/ado-aw/releases/latest/download/install-linux.sh | sh
# macOS (Apple Silicon)
curl -fsSL https://github.com/githubnext/ado-aw/releases/latest/download/install-macos.sh | sh
# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -NoProfile -Command "iwr https://github.com/githubnext/ado-aw/releases/latest/download/install-windows.ps1 -UseBasicParsing | iex"
```

Verify: `ado-aw --version`

## Read-only MCP tools

This plugin wires the local, read-only **`ado-aw mcp-author`** server (see
`.mcp.json`). Prefer these tools over shelling out when you only need to inspect:

- `inspect_workflow`, `graph_summary`, `graph_dump`, `step_dependencies`, `step_outputs`
- `lint_workflow`, `whatif`, `catalog`
- `trace_failure`, `audit_build` (ADO read auth)

Mutating actions are **not** in the MCP server. Use the `ado-aw` CLI (via Bash)
for `compile`, `enable`, `disable`, `remove`, `run`, `list`, `status`,
`secrets`, `init` — gated by the guardrails below.

## Capability routing

Route the user's request to the matching skill:

| The user wants to… | Skill |
| --- | --- |
| Create a new workflow from scratch | `create-workflow` |
| Change an existing workflow | `update-workflow` |
| Understand why a pipeline failed | `debug-workflow` |
| Compile / confirm a workflow is valid | `compile-and-validate` |
| Enable / disable / run / check status | `manage-lifecycle` |
| Analyze a finished build | `audit-build` |

## Authoritative prompts (version-pinned)

The create/update/debug skills load these compiler-version-pinned playbooks:

- Create: https://raw.githubusercontent.com/githubnext/ado-aw/v0.40.0/prompts/create-ado-agentic-workflow.md <!-- x-release-please-version -->
- Update: https://raw.githubusercontent.com/githubnext/ado-aw/v0.40.0/prompts/update-ado-agentic-workflow.md <!-- x-release-please-version -->
- Debug: https://raw.githubusercontent.com/githubnext/ado-aw/v0.40.0/prompts/debug-ado-agentic-workflow.md <!-- x-release-please-version -->
- Full reference: https://raw.githubusercontent.com/githubnext/ado-aw/v0.40.0/AGENTS.md <!-- x-release-please-version -->

## Guardrails

- Never bypass ado-aw's three-stage safe-output model or fabricate write tokens.
  The agent never has direct write access — all mutations go through safe outputs.
- Always `ado-aw compile` **and** `lint_workflow` before declaring a workflow done.
- Agent files must be recompiled with `ado-aw compile` after YAML front-matter
  changes; Markdown-body-only changes do not require recompilation.
- Never push directly to a protected branch — use ado-aw's own flow.
- Surface ADO auth/permission errors verbatim with the doc pointer
  (`docs/safe-output-permissions.md`).

## Quick reference

```bash
ado-aw compile <agent-file.md>   # compile one agent file to pipeline YAML
ado-aw compile                   # recompile all detected pipelines
ado-aw check <pipeline.lock.yml> # verify a pipeline matches its source
```
