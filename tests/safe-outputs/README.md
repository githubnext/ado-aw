# Safe-output smoke sources

Agentic pipeline sources that exercise the full Stage 1 → Stage 2 → Stage 3
shape against the
[AgentPlayground](https://dev.azure.com/msazuresphere/AgentPlayground) ADO
sandbox.

**These are markdown sources only.** There are no committed `*.lock.yml` files
here and no ADO definitions registered against this directory. Each source is
recompiled at run time by the smoke orchestrators — see
[`tests/smoke/`](../smoke/) for the lane model, how cases are declared in
`cases.json`, and how to add one.

## Design: canary + infra, not one-per-tool

The original suite had one daily agentic smoke per safe-output tool. That
turned out to be unnecessary: the deterministic
[`tests/executor-e2e/`](../executor-e2e/) suite already exercises every tool's
Stage 3 ADO REST path directly (without an LLM). The agentic smoke only needs
to prove:

1. Stage 1: an LLM agent discovers and emits a safe-output call given the MCP
   tool list.
2. Stage 2: the threat-detection pass clears the NDJSON output.
3. Stage 1 → 2 → 3 handoff: the three-job pipeline shape runs end-to-end.

A single successful run proves all three.

| Source | Purpose |
| --- | --- |
| `canary.md` | Omnibus canary: the agent emits `noop` + `create-work-item` + `add-build-tag` in one run. Proves the full agentic loop with two distinct ADO write paths. |
| `azure-cli.md` | Verifies the AWF az CLI extension is mounted, `az devops` authenticates via `AZURE_DEVOPS_EXT_PAT`, and the sandbox can reach the ADO control plane. |
| `noop-target.md` | Minimal agentic pipeline. (The executor-e2e `queue-build` target is now the separate, non-agentic [`tests/executor-e2e/queue-target.yml`](../executor-e2e/queue-target.yml).) |
| `janitor.md` | Prunes `ado-aw-smoke-*` artifacts (work items, branches, wiki pages, tags, PRs) older than 30 days from AgentPlayground. Runs in released mode. |
| `smoke-failure-reporter.md` | Queries smoke pipelines for failures and files `[smoke-failure] …` issues on `jamesadevine/ado-aw-issues`. Runs in the isolated `debug` lane because it needs `ADO_AW_DEBUG_GITHUB_TOKEN`. |

Schedules in these sources' front matter are **stripped at staging time** — the
orchestrator owns scheduling, because every case in a lane shares one
definition. Keep or remove `on.schedule` as documentation of intent; it has no
runtime effect in the smoke suite.

## Why there are no lock files here

Committed locks previously existed so five GitHub-backed definitions could run
the exact bytes a customer would commit, using the released compiler. That cost
a bot-maintained recompile workflow, five definitions, and a permanent drift
risk between the checked-in lock and the released compiler.

Released mode replaces it: the orchestrator downloads the **latest released**
`ado-aw`, recompiles these sources with it, and every child still downloads
released assets through its own integrity step. `assertReleaseUrlsPresent`
makes a run that stops exercising release packaging fail closed.

What that trades away, deliberately: pipelines no longer run from a
GitHub-backed definition with real GitHub repository metadata, and the exact
committed bytes are no longer what executes. If a metadata regression ever
escapes, the cheapest mitigation is to re-add a single GitHub-backed canary
with a committed lock.


> **Deterministic complement.** For a flake-free regression check of the
> Stage 3 executor with no LLM in the loop, see
> [`tests/executor-e2e/`](../executor-e2e/), which covers the ADO-write and
> signal safe-output tools deterministically.

## Naming convention

Every artifact a smoke creates uses the prefix
`ado-aw-smoke-$(Build.BuildId)-<tool>`. The janitor deletes anything with that
prefix older than 30 days, so cleanup is automatic.

## Adding a new safe output

When you add `src/safe_outputs/<new-tool>.rs`:

1. The compiler's `validate_safe_outputs_keys` (in `src/compile/common.rs`)
   ensures any user-written `safe-outputs: <typo>:` block fails at compile time
   with a "did you mean ...?" suggestion rather than silently dropping the key.
2. **If the tool has an ADO write path** (it calls any ADO REST API), add a
   scenario in
   [`scripts/ado-script/src/executor-e2e/scenarios/`](../../scripts/ado-script/src/executor-e2e/scenarios/):
   set up preconditions, craft the NDJSON, assert the ADO effect, and clean up.
   Wire it into `index.ts` via the appropriate scenario array.
3. **If the tool is a signal-only tool** (no ADO side effect — like `noop`,
   `missing-tool`, `missing-data`, `report-incomplete`), add a scenario in
   `signals.ts` in the same directory instead.
4. Only add a dedicated agentic smoke here if the new tool requires a
   fundamentally new kind of agent prompt or MCP wiring that the existing
   `canary.md` does not exercise. A smoke case is a markdown file plus one
   entry in [`tests/smoke/cases.json`](../smoke/cases.json).
5. **If the tool writes to GitHub rather than ADO** (`create-github-issue`,
   `set-github-issue-type`), neither suite covers it today — see
   [#1797](https://github.com/githubnext/ado-aw/issues/1797). Executor-e2e is
   the right home; it already files GitHub issues from its own failure
   reporter, so the REST plumbing exists.

## Running locally

These sources carry no committed lock files, so there is nothing to `check`
against a released binary. To compile one with the binary under test:

```bash
cargo run -- compile --force tests/safe-outputs/canary.md
```

Both smoke lanes recompile from source at run time, so a local compile is for
inspection only — delete the generated `.lock.yml` rather than committing it.

For the ADO-side setup runbook, see
[`tests/smoke/REGISTERED.md`](../smoke/REGISTERED.md).
