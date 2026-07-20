# Safe-output smoke suite

This directory contains the agentic-pipeline fixtures that exercise the
full Stage 1 → Stage 2 → Stage 3 pipeline shape against the
[AgentPlayground](https://dev.azure.com/msazuresphere/AgentPlayground)
ADO sandbox. Each `.md` is compiled by `ado-aw compile` to a sibling
`*.lock.yml`, and each `*.lock.yml` is registered as one Azure DevOps
pipeline.

## Design: canary + infra, not one-per-tool

The original suite had one daily agentic smoke per safe-output tool.
That turned out to be unnecessary: the deterministic
[`tests/executor-e2e/`](../executor-e2e/) suite already exercises every
tool's Stage 3 ADO REST path directly (without an LLM). The agentic
smoke only needs to prove:

1. Stage 1: an LLM agent discovers and emits a safe-output call given
   the MCP tool list.
2. Stage 2: the threat-detection pass clears the NDJSON output.
3. Stage 1 → 2 → 3 handoff: the three-job pipeline shape runs
   end-to-end.

A single successful pipeline run proves all three. The suite is now
five pipelines:

| File | Purpose |
| --- | --- |
| `canary.md` / `canary.lock.yml` | Daily omnibus canary: the agent emits `noop` + `create-work-item` + `add-build-tag` in one run. Proves the full agentic loop with two distinct ADO write paths. |
| `azure-cli.md` / `azure-cli.lock.yml` | Daily: verifies the AWF az CLI extension is mounted, the `az devops` subcommand authenticates via `AZURE_DEVOPS_EXT_PAT`, and the sandbox can reach the ADO control plane. |
| `noop-target.md` / `noop-target.lock.yml` | No-schedule target pipeline queued by the `queue-build` executor-e2e scenario (its ID feeds `E2E_QUEUE_PIPELINE_ID`). |
| `janitor.md` / `janitor.lock.yml` | Weekly: prunes `ado-aw-smoke-*` artifacts (work items, branches, wiki pages, tags, PRs) older than 30 days from AgentPlayground. |
| `smoke-failure-reporter.md` / `smoke-failure-reporter.lock.yml` | Daily ~04:30: queries the canary and azure-cli pipelines for failures and files `[smoke-failure] …` issues on `jamesadevine/ado-aw-issues` while canonical-repo credentials are unavailable. |
| `REGISTERED.md` | Contributor-maintained `fixture → ADO pipeline ID` mapping. |

> **Deterministic complement.** For a flake-free regression check of
> the Stage 3 executor with no LLM in the loop, see
> [`tests/executor-e2e/`](../executor-e2e/). That suite covers all
> 24 ADO-write and signal safe-output tools deterministically.
>
> **Local Copilot CLI contract.** The ignored Rust test
> `tests/copilot_cli_safeoutputs_tests.rs::real_copilot_cli_noop_contract`
> is the customer-focused contract gate for the local agent path.

## Naming convention

Every artifact a smoke creates uses the prefix
`ado-aw-smoke-$(Build.BuildId)-<tool>`. The janitor deletes anything
with that prefix older than 30 days, so cleanup is automatic.

## Adding a new safe output

When you add `src/safe_outputs/<new-tool>.rs`:

1. The compiler's `validate_safe_outputs_keys` (in
   `src/compile/common.rs`) ensures any user-written
   `safe-outputs: <typo>:` block fails at compile time with a
   "did you mean …?" suggestion rather than silently dropping the key.
2. **If the tool has an ADO write path** (it calls any ADO REST API),
   add a scenario in
   [`scripts/ado-script/src/executor-e2e/scenarios/`](../../scripts/ado-script/src/executor-e2e/scenarios/):
   set up preconditions, craft the NDJSON, assert the ADO effect, and
   clean up. Wire it into `index.ts` via the appropriate scenario array.
3. **If the tool is a signal-only tool** (no ADO side effect — like
   `noop`, `missing-tool`, `missing-data`, `report-incomplete`), add a
   scenario in the `signals.ts` file in the same directory instead.
4. Only add a dedicated agentic smoke here if the new tool requires
   a fundamentally new kind of agent prompt or MCP wiring that the
   existing `canary.md` does not exercise.
5. Debug-only tools (currently only `create-issue`) are excluded from
   both suites — exercised by `smoke-failure-reporter.md`.

## Running locally

```bash
# Recompile every fixture in this directory (idempotent):
cargo run -- compile tests/safe-outputs/

# Verify a single fixture compiled correctly:
cargo run -- check tests/safe-outputs/canary.lock.yml
```

## Manual handoff (one-time ADO setup)

In `https://dev.azure.com/msazuresphere/AgentPlayground`:

1. Confirm or create service connections `agent-playground-read` and
   `agent-playground-write`.
2. Bulk-register the smoke pipelines with `ado-aw enable`:

   ```powershell
   cargo run -- enable `
     --org msazuresphere --project AgentPlayground `
     --service-connection github.com_githubnext `
     --also-set-token `
     --folder '\smoke' `
     tests/safe-outputs/
   ```

3. Capture each Pipeline ID and update `REGISTERED.md`.
4. Provision the `ADO_AW_DEBUG_GITHUB_TOKEN` secret (fine-grained PAT,
   Issues: read/write on `jamesadevine/ado-aw-issues`) on the
   `smoke-failure-reporter` pipeline **only**. Confirm the staging repository
   has the `pipeline-failure` and `ado-aw-smoke` labels.
5. Set `EXECUTOR_E2E_ISSUE_REPO=jamesadevine/ado-aw-issues` and
   `E2E_QUEUE_PIPELINE_ID` on the
   executor-e2e pipeline using the `noop-target` pipeline ID from
   step 3.
6. Trigger one manual run per pipeline to seed the schedule.

> **Existing-definition cutover.** `ado-aw enable` matches definitions by YAML
> path and will reuse the four legacy registrations. To create side-by-side
> replacements before deleting the old definitions, register the five YAML
> paths explicitly with `az pipelines create --skip-run true`, configure and
> validate the returned IDs, then retire the legacy IDs.
