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

and, on any failure, files a GitHub issue on `githubnext/ado-aw` and fails the
build.

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
# export TRIGGER_E2E_GITHUB_TOKEN="<fine-grained PAT: Issues rw on githubnext/ado-aw>"
# export TRIGGER_E2E_BUILD_TIMEOUT_MS=900000   # per victim build (default 900000)
# export TRIGGER_E2E_BUILD_POLL_MS=10000       # poll interval (default 10000)

node scripts/ado-script/test-bin/trigger-e2e.js
```

When `TRIGGER_E2E_VICTIM_REPO` is unset, the PR/synth/gate scenarios **skip**
and only the bypass baseline runs. The harness exits non-zero if any scenario
fails.

## Manual-handoff checklist (one-time ADO setup)

In `https://dev.azure.com/msazuresphere/AgentPlayground`:

1. **Register the VICTIM pipeline.** New pipeline → Azure Repos → the ado-aw
   Azure Repos mirror → existing YAML → `tests/trigger-e2e/victim-pipeline.yml`.
   It **must** be an ADO Git repo (not GitHub) so `exec-context-pr-synth` can
   discover open PRs in the pipeline's own `self` repo. Note its **definition
   id** for the next step.
2. **Register the ORCHESTRATOR pipeline.** New pipeline → existing YAML →
   `tests/trigger-e2e/azure-pipelines.yml`. Place both in a `\trigger-e2e`
   folder.
3. **Wire the victim id + repo** as pipeline/definition variables on the
   orchestrator:
   ```powershell
   ado-aw secrets set TRIGGER_E2E_VICTIM_DEFINITION_ID `
     --org msazuresphere --project AgentPlayground `
     --definition-ids <orchestrator-pipeline-id> --value <victim-pipeline-id>
   ado-aw secrets set TRIGGER_E2E_VICTIM_REPO `
     --org msazuresphere --project AgentPlayground `
     --definition-ids <orchestrator-pipeline-id> --value <ado-aw-mirror-repo-name>
   ```
   (These are not secrets; `secrets set` is just the variable-management path.)
4. **Grant the VICTIM build identity** Code (read) on the repo, Build (add
   tags / edit build quality), and permission to cancel builds (gate
   self-cancel). See [`docs/safe-output-permissions.md`](../../docs/safe-output-permissions.md)
   if it hits 401/403.
5. **Grant the ORCHESTRATOR build identity** Contribute / Create branch /
   Contribute to PRs on the victim repo (it creates transient PRs) and Queue
   builds on the victim definition.
6. **Set the GitHub PAT secret** on the orchestrator only:
   ```powershell
   ado-aw secrets set TRIGGER_E2E_GITHUB_TOKEN `
     --org msazuresphere --project AgentPlayground `
     --definition-ids <orchestrator-pipeline-id> `
     --value <fine-grained-pat-Issues-rw-on-githubnext/ado-aw>
   ```
7. **Trigger one manual orchestrator run** to seed the schedule.
