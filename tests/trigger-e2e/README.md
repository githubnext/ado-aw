# Deterministic trigger-condition (gate / synth-PR) E2E suite

This directory holds a **deterministic**, non-agentic end-to-end test of the
runtime **front-matter trigger conditions managed by ado-script** — the
`gate.js` PR filter evaluator and the `exec-context-pr-synth.js` synthetic-PR
promotion bundle.

## Why this exists

The executor E2E suite in [`tests/executor-e2e/`](../executor-e2e/) covers
**Stage 3 safe-output execution** (`ado-aw execute`). It does **not** cover the
trigger conditions that decide *whether the agent runs at all*: PR label/path/
branch/draft/time-window/change-count filters and the CI→PR synthetic
promotion. Those run at build queue time in the ado-script bundles, emit build
tags + a `SHOULD_RUN` variable, and can self-cancel the build — none of which
the executor suite touches.

This suite exercises them with **no LLM and no agent** in the loop:

1. it creates real ADO PR context deterministically (branch + PR + labels +
   draft) via the ADO REST API,
2. queues the hand-authored **victim pipeline** (`victim-pipeline.yml`) on the
   PR's source branch with a per-scenario base64 `GATE_SPEC` / `PR_SYNTH_SPEC`
   as template parameters,
3. polls the build to completion,
4. asserts the observable gate decision (**build tags + build result**),
5. cleans up every object it created,

and, on any failure, files a GitHub issue on the configured issue repository
and fails the build. AgentPlayground currently uses
`jamesadevine/ado-aw-issues` because a canonical-repository credential is not
available.

## What's here

| File | Purpose |
| --- | --- |
| `victim-pipeline.yml` | Hand-authored, **parameterized** victim. Runs only `exec-context-pr-synth.js` then `gate.js` from a checkout build, emits observable build tags. Registered separately. |
| `azure-pipelines.yml` | Hand-authored **orchestrator** (daily + manual). Builds the harness and runs it against AgentPlayground. |
| `README.md` | This file. |

The harness itself lives in
[`scripts/ado-script/src/trigger-e2e/`](../../scripts/ado-script/src/trigger-e2e/).
It is a **test-only** ado-script bundle: built by `npm run build:trigger-e2e`
to `scripts/ado-script/test-bin/trigger-e2e.js` (a gitignored, non-root path)
and **deliberately excluded** from the released `ado-script.zip` (the release
glob only packages `ado-script/*.js`, and `trigger-e2e` is listed in
`NON_BUNDLE_DIRS` in `src/__tests__/bundle-coverage.test.ts`).

## How one victim covers synth-PR + every gate filter

A build queued via REST has `Build.Reason = Manual`. The victim runs
`exec-context-pr-synth.js` first; given a **real open PR** on the queued source
branch it sets `AW_SYNTHETIC_PR=true` + `AW_PR_ID`. The gate's bypass logic
treats `build_reason == "PullRequest" && AW_SYNTHETIC_PR == "true"` as **not
bypassed**, so the full PR filter evaluation runs against the real PR facts
(labels, changed files, draft, target branch, …) even though the build was
queued via REST — no server-side build-validation policy required.

## Observable build tags (the assertion signal)

| Tag | Meaning |
| --- | --- |
| `trig.synth.promoted` | synth promoted the build to synthetic-PR semantics |
| `trig.synth.skipped` | synth found no matching PR (branch/path mismatch, or none) |
| `trig.synth.real-pr` | a real PR-triggered build (not exercised by this suite) |
| `pr-gate.passed` | gate **bypassed** (not a PR/synth build) |
| `pr-gate.skipped` + `pr-gate.<check>` | a filter failed; the gate **self-cancelled** the build |
| `trig.should-run.true` | the gate let the run proceed (`SHOULD_RUN=true`) |

`skip` outcomes reach a terminal **`canceled`** build result (gate self-cancel);
`pass`/`bypass` outcomes reach `succeeded`.

## Coverage

- **Synthetic-PR promotion:** matching PR (promoted), branch mismatch (skipped),
  path mismatch (skipped).
- **Gate PR filters** (pass + skip): labels, changed-files, draft, target-branch,
  build-reason, change-count, time-window.
- **Bypass:** Manual build with no PR → `pr-gate.passed`.
- **Self-cancel:** a failing filter self-cancels the build (`canceled`).

### Known gaps / future work

- `pr_title` and `author_email` filters source from `System.PullRequest.*`,
  which are empty on synth-promoted Manual builds. The victim accepts
  `prTitle` / `authorEmail` template-parameter overrides so these predicates
  can be tested against controlled values; scenarios for them are not yet
  wired up.
- The synth **two-match refusal** path (two active PRs on one source branch) is
  not tested (hard to set up deterministically).

## Naming / cleanup convention

Every object a scenario creates is prefixed `ado-aw-trig-$(Build.BuildId)-<id>`.
Cleanup (abandon PR + delete branch) runs unconditionally after each scenario;
the smoke-suite janitor (which prunes `ado-aw-*` artifacts) is the backstop.

## Mirror synchronization

The victim's `self` repository must be Azure Repos, while the orchestrator is
registered against GitHub. Before running scenarios, the orchestrator pushes
its checked-out `main` commit to `ado-aw-mirror` and verifies the remote SHA.
The push is authenticated with the `agent-playground-write` AAD token through
an environment-only git bearer header.

Synchronization is fast-forward-only and fail-closed: a divergent mirror stops
the suite before any victim build is queued. No force push is used. When
`TRIGGER_E2E_VICTIM_REPO` is unset, synchronization is skipped and the
existing bypass-only baseline still runs.

## Scenario concurrency

Scenarios run with bounded concurrency rather than waiting for every victim
pipeline to finish before queueing the next one. The default is **4**
concurrent scenarios, which substantially reduces queue-bound wall time while
leaving capacity in the 16-agent pool for other validations.

Set `TRIGGER_E2E_CONCURRENCY` on the orchestrator definition to an integer from
`1` through `8` to tune it. Results remain in declaration order, and each
scenario still owns its unique branch, PR, victim build, assertion, and
unconditional cleanup.

## Running locally

You need a write-capable ADO token (PAT) and a registered victim pipeline id:

```bash
cd scripts/ado-script && npm ci && npm run build:trigger-e2e && cd ../..

export SYSTEM_COLLECTIONURI="https://dev.azure.com/msazuresphere/"
export SYSTEM_TEAMPROJECT="AgentPlayground"
export SYSTEM_ACCESSTOKEN="<write-capable-PAT>"
export TRIGGER_E2E_VICTIM_DEFINITION_ID="<registered victim pipeline id>"
export TRIGGER_E2E_VICTIM_REPO="<ADO Git repo backing the victim's self>"
# Optional:
# export TRIGGER_E2E_GITHUB_TOKEN="<fine-grained PAT: Issues rw on jamesadevine/ado-aw-issues>"
# export TRIGGER_E2E_ISSUE_REPO="jamesadevine/ado-aw-issues"
# export TRIGGER_E2E_SYNC_MIRROR="true" # requires a full main checkout
# export TRIGGER_E2E_CONCURRENCY=4       # 1..8, default 4
# export TRIGGER_E2E_BUILD_TIMEOUT_MS=900000   # per victim build (default 900000)
# export TRIGGER_E2E_BUILD_POLL_MS=10000       # poll interval (default 10000)

node scripts/ado-script/test-bin/trigger-e2e.js
```

When `TRIGGER_E2E_VICTIM_REPO` is unset, the PR/synth/gate scenarios **skip**
and only the bypass baseline runs. The harness exits non-zero if any scenario
fails.

## Manual-handoff checklist (one-time ADO setup)

In `https://dev.azure.com/msazuresphere/AgentPlayground`:

1. **Create and seed `ado-aw-mirror`.** Import or push the merged
   `githubnext/ado-aw` `main` commit into a dedicated Azure Repo. Do not reuse
   `agent-definitions`.
2. **Register the VICTIM pipeline.** New pipeline → Azure Repos →
   `ado-aw-mirror` → existing YAML →
   `tests/trigger-e2e/victim-pipeline.yml`.
   It **must** be an ADO Git repo (not GitHub) so `exec-context-pr-synth` can
   discover open PRs in the pipeline's own `self` repo. Note its **definition
   id** for the next step.
   - **Pool name.** Both `victim-pipeline.yml` and `azure-pipelines.yml`
     hardcode `pool: { name: AZS-1ES-L-Playground-ubuntu-22.04 }`. If your ADO
     project uses a different agent pool, edit both files to point at a
     Linux pool with Node.js 20 available before registering.
3. **Register the ORCHESTRATOR pipeline.** New pipeline → GitHub through
   `github.com_githubnext` → existing YAML →
   `tests/trigger-e2e/azure-pipelines.yml`. Place both definitions in a
   `\trigger-e2e` folder and skip the first run until variables are configured.
4. **Wire the victim id + repo** as non-secret definition variables on the
   orchestrator:
   - `TRIGGER_E2E_VICTIM_DEFINITION_ID=<victim-pipeline-id>`
   - `TRIGGER_E2E_VICTIM_REPO=ado-aw-mirror`
5. **Grant the VICTIM build identity** Code Read on `ado-aw-mirror` for
   checkout and synth PR lookups.
6. **Grant the principal behind `agent-playground-write`** Contribute, Create
   branch, delete refs, and Contribute to PRs on `ado-aw-mirror`, plus Queue
   builds on the victim definition. Grant it project-level **Stop builds**:
   current-build cancellation is checked against a build-instance token and
   does not inherit a definition-scoped Stop permission. Authorize the
   `agent-playground-write` service connection for both the orchestrator and
   victim definitions. This same identity performs mirror sync, creates the
   transient PR context, and supplies the gate's short-lived bearer.
   See [`docs/safe-output-permissions.md`](../../docs/safe-output-permissions.md)
   if either identity hits 401/403.
7. **Set the GitHub PAT secret** on the orchestrator only:
   ```powershell
   ado-aw secrets set TRIGGER_E2E_GITHUB_TOKEN `
     --org msazuresphere --project AgentPlayground `
     --definition-ids <orchestrator-pipeline-id> `
     --value <fine-grained-pat-Issues-rw-on-jamesadevine/ado-aw-issues>
   ```
8. Set `TRIGGER_E2E_ISSUE_REPO=jamesadevine/ado-aw-issues`. Confirm the target
   has `trigger-e2e-failure` and `pipeline-failure` labels.
9. **Trigger one manual orchestrator run from `main`** to seed the schedule.
