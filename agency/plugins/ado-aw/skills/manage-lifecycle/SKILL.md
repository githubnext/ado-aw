---
name: manage-lifecycle
description: Operate compiled ado-aw pipelines in Azure DevOps. Use when the user wants to register/enable/disable/remove build definitions, queue runs, set pipeline secrets, or report pipeline status.
allowed-tools: Bash, Read, Glob, Grep
---

# Manage pipeline lifecycle

Operate compiled ado-aw pipelines against Azure DevOps. These are **mutating CLI
actions** — they are deliberately not exposed via the read-only MCP server.

1. Confirm the `ado-aw` compiler is installed (`ado-aw --version`) and that ADO
   auth is available (`gh`/`az` login or an ADO PAT, per ado-aw's auth resolution).

2. Use the `ado-aw` CLI (via Bash):
   - `ado-aw enable` — register ADO build definitions for compiled pipelines and
     ensure they are enabled.
   - `ado-aw disable` — set matched definitions to disabled (default) or paused.
   - `ado-aw remove` — delete matched build definitions (honors `--yes` / tty prompt).
   - `ado-aw run` — queue builds for matched definitions, optionally polling to
     completion.
   - `ado-aw list` — render matched definitions with their latest-run state
     (text or JSON).
   - `ado-aw status` — denser per-pipeline status block.
   - `ado-aw secrets set/list/delete` — manage pipeline variables. `list` never
     prints values; never echo secret values into the conversation.

## Guardrails

- These commands change real ADO state. Confirm scope (`--source` / `--all-repos`
  and the matched set via `ado-aw list`) **before** running enable/disable/remove/run.
- Never fabricate or hardcode a PAT; rely on ado-aw's auth resolution.
- Surface ADO auth/permission errors verbatim; for Stage 3 401/403 see
  `docs/safe-output-permissions.md`.

The user's request: $ARGUMENTS
