# Daily safe-output smoke suite

This directory contains one daily-scheduled agentic-pipeline fixture per
production safe-output tool plus three infra fixtures. Each `.md` is
compiled by `ado-aw compile` to a sibling `*.lock.yml`, and each
`*.lock.yml` is registered as one Azure DevOps pipeline in the
[AgentPlayground](https://dev.azure.com/msazuresphere/AgentPlayground)
sandbox project.

The suite exists so that every safe output we ship is exercised
end-to-end against a real ADO project at least once a day. A green
pipeline = Stage 1 (agent) → Stage 2 (threat detection) → Stage 3
(executor) → ADO REST round-trip all succeeded.

> **See also — deterministic complement.** This suite depends on an LLM
> (Stage 1) emitting each safe output, so it validates the full agentic
> flow but is inherently non-deterministic. For a flake-free regression
> check of the Stage 3 executor alone — crafting the executor's NDJSON
> directly, with no agent in the loop — see the deterministic suite in
> [`tests/executor-e2e/`](../executor-e2e/).

## What's here

| File | Purpose |
| --- | --- |
| `<tool>.md` / `<tool>.lock.yml` | One per safe output, scheduled `daily around 03:00`. The agent calls exactly one safe-output tool with predictable literal values. |
| `noop-target.md` / `noop-target.lock.yml` | Trivial `bash: ["echo ok"]` pipeline targeted by the `queue-build` smoke. |
| `janitor.md` / `janitor.lock.yml` | Weekly-scheduled pipeline that prunes `ado-aw-smoke-*` artifacts older than 30 days. |
| `smoke-failure-reporter.md` / `smoke-failure-reporter.lock.yml` | Daily ~04:30 pipeline that queries ADO REST, finds failed smoke runs, and files `[smoke-failure] ...` issues against `githubnext/ado-aw` via `ado-aw-debug.create-issue` (PR #492). |
| `REGISTERED.md` | Contributor-maintained mapping `fixture → ADO pipeline ID`. Filled in during manual handoff. |

## Naming convention

Every artifact a smoke creates uses the prefix
`ado-aw-smoke-$(Build.BuildId)-<tool>`. The janitor deletes anything
with that prefix older than 30 days, so cleanup is automatic.

## Fixture template

Every Wave 1–4 fixture has the same shape:

```markdown
---
name: "Daily safe-output smoke: <tool-name>"
description: "Exercises the <tool-name> safe output once a day"
on:
  schedule: daily around 03:00
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: gpt-5-mini
  timeout-minutes: 15
permissions:
  read: agent-playground-read
  write: agent-playground-write
safe-outputs:
  <tool-name>:
    # per-tool config from docs/safe-outputs.md
setup:
  - bash: |
      set -euo pipefail
      echo "Setup for <tool-name>"
teardown:
  - bash: |
      set -euo pipefail
      # Best-effort prefix cleanup; always exit 0.
---

## Daily smoke for <tool-name>

You are a smoke test. Call exactly one safe-output tool: `<tool-name>`.
Use these literal values (no improvisation):

- title: "ado-aw-smoke-$(Build.BuildId)-<tool-name>"
- body: "ok"

Do not call any other tool. After the safe output is emitted, stop.
```

The prompt must end with the sentence "Do not call any other tool." —
the `tests/safe_output_coverage_tests.rs` Rust integration test enforces
this so flake stays low.

## Adding a new safe output

When you add `src/safeoutputs/<new-tool>.rs`:

1. The compiler's `validate_safe_outputs_keys` (in
   `src/compile/common.rs`) ensures any user-written
   `safe-outputs: <typo>:` block fails at compile time with a
   "did you mean …?" suggestion rather than silently dropping the key.
2. By convention, add a matching daily smoke fixture here at
   `tests/safe-outputs/<yaml-key>.md` so the new tool gets exercised
   end-to-end every day. The fixture filename matches the YAML key
   under `safe-outputs:` (the kebab-case name declared by the
   `tool_result! { name = "..." }` macro), not the Rust filename.
3. Compile it (`cargo run -- compile tests/safe-outputs/<yaml-key>.md`)
   and commit the resulting `.lock.yml` alongside.
4. Register the new pipeline in AgentPlayground (manual handoff).

Debug-only tools (currently only `create-issue`) are excluded from the
smoke suite — they're exercised by `smoke-failure-reporter.md`.

## Running locally

```bash
# Recompile every fixture in this directory (idempotent):
cargo run -- compile tests/safe-outputs/

# Verify a single fixture compiled correctly:
cargo run -- check tests/safe-outputs/noop.lock.yml
```

## Manual handoff (one-time ADO setup)

See the
[implementation plan](https://github.com/githubnext/ado-aw/issues?q=label%3Aado-aw-smoke)
issue (or the originating session plan) for the manual handoff that
provisions the AgentPlayground sandbox: service connections, perma-PR,
variable group `ado-aw-daily-smoke`, the `ADO_AW_DEBUG_GITHUB_TOKEN`
secret on the failure-reporter pipeline, and ADO pipeline registrations.
