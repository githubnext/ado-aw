# Deterministic executor (Stage 3) safe-output E2E suite

This directory holds a **deterministic**, non-agentic end-to-end test of the
`ado-aw execute` (Stage 3) safe-output executor.

## Why this exists

The agentic smoke suite in [`tests/safe-outputs/`](../safe-outputs/) exercises
the full Stage 1 → 2 → 3 pipeline shape, but it drives Stage 3 by having an
**LLM agent (Stage 1) emit the safe output** — so a failure can be the model's
fault and the suite is inherently flaky. It also only runs a small number of
omnibus pipelines rather than one per tool.

This suite removes the LLM from the loop. For every ADO-write safe output it:

1. sets up preconditions **deterministically** via the ADO REST API,
2. crafts the executor's `safe_outputs.ndjson` input directly (fixed literal
   values),
3. runs the real `ado-aw execute` binary (built from the checkout),
4. asserts the effect via the ADO REST API,
5. cleans up every object it created,

and, on any failure, files a GitHub issue on the configured issue repository
and fails the build. AgentPlayground currently uses
`jamesadevine/ado-aw-issues` because a canonical-repository credential is not
available.

## What's here

| File | Purpose |
| --- | --- |
| `azure-pipelines.yml` | Hand-authored ADO pipeline (daily schedule on `main` + path-filtered PR validation + manual). Builds `ado-aw`, builds the harness, runs it against AgentPlayground. |
| `README.md` | This file. |

The harness itself lives in
[`scripts/ado-script/src/executor-e2e/`](../../scripts/ado-script/src/executor-e2e/).
It is a **test-only** ado-script bundle: it is built by `npm run
build:executor-e2e` to `scripts/ado-script/test-bin/executor-e2e.js` (a
gitignored, non-root path) and is **deliberately excluded** from the released
`ado-script.zip` (the release glob only packages `ado-script/*.js`, and the
`executor-e2e` dir is listed in `NON_BUNDLE_DIRS` in
`src/__tests__/bundle-coverage.test.ts`).

## Coverage

All deterministically-assertable ADO-write safe outputs plus the flagship
`create-pull-request`, and the four signal-only tools:

- **Signals:** `noop`, `missing-tool`, `missing-data`, `report-incomplete`
  (no ADO write path; assert that the executor emits the expected status)
- **Work items:** `create-work-item`, `update-work-item`,
  `comment-on-work-item`, `link-work-items`, `upload-workitem-attachment`
- **Wiki:** `create-wiki-page`, `update-wiki-page`
- **PR:** `add-pr-comment`, `reply-to-pr-comment`, `resolve-pr-thread`,
  `submit-pr-review`, `update-pr`
- **Git:** `create-branch`, `create-git-tag`
- **Build:** `add-build-tag`, `queue-build`, `upload-build-attachment`,
  `upload-pipeline-artifact`
- **Flagship:** `create-pull-request`

Excluded (out of scope or GitHub-only): the GitHub-only `create-issue`.

> **Coverage note.** The signal scenarios (`noop`, `missing-tool`,
> `missing-data`, `report-incomplete`) were previously exercised only by
> now-deleted per-tool agentic smoke pipelines. Adding them here closes
> the coverage gap while keeping the test deterministic.

### Scenarios that skip when a precondition is missing

Some scenarios need optional infrastructure and **skip** (rather than fail)
when it is not available:

- `queue-build` — needs a target pipeline id in `E2E_QUEUE_PIPELINE_ID`.
- `create-wiki-page` / `update-wiki-page` — need a wiki in the project. The
  harness auto-discovers the first wiki; set `E2E_WIKI_NAME` to force one. When
  no wiki exists, both skip.
- `add-build-tag`, `upload-build-attachment`, `upload-pipeline-artifact` — need
  a real current build (`BUILD_BUILDID`); they skip when run outside a pipeline.

## Naming / cleanup convention

Every object a scenario creates is prefixed
`ado-aw-det-$(Build.BuildId)-<tool>`. Cleanup runs unconditionally after each
scenario; the smoke-suite janitor (which prunes `ado-aw-*` artifacts) is the
backstop for anything a cleanup misses.

## Running locally

You need a write-capable ADO token (PAT) and a checkout-built binary:

```bash
cargo build --release --bin ado-aw
cd scripts/ado-script && npm ci && npm run build:executor-e2e && cd ../..

export SYSTEM_COLLECTIONURI="https://dev.azure.com/msazuresphere/"
export SYSTEM_TEAMPROJECT="AgentPlayground"
export SYSTEM_ACCESSTOKEN="<write-capable-PAT>"
export EXECUTOR_E2E_ADO_AW_BIN="$PWD/target/release/ado-aw"
export EXECUTOR_E2E_ADO_REPO="agent-definitions"
# Optional:
# export EXECUTOR_E2E_GITHUB_TOKEN="<fine-grained PAT: Issues rw on jamesadevine/ado-aw-issues>"
# export EXECUTOR_E2E_ISSUE_REPO="jamesadevine/ado-aw-issues"
# export E2E_QUEUE_PIPELINE_ID="<noop-target pipeline id>"
# Optional timeout tuning (milliseconds) for slow environments:
# export EXECUTOR_E2E_REST_TIMEOUT_MS=30000     # per ADO REST call (default 30000)
# export EXECUTOR_E2E_EXECUTE_TIMEOUT_MS=600000 # per `ado-aw execute` run (default 600000)
# export EXECUTOR_E2E_GIT_TIMEOUT_MS=300000     # per git subprocess call (default 300000)

node scripts/ado-script/test-bin/executor-e2e.js
```

Build-scoped scenarios (`add-build-tag`, uploads) skip locally because there is
no current build. The harness exits non-zero if any scenario fails.

## Manual-handoff checklist (one-time ADO setup)

In `https://dev.azure.com/msazuresphere/AgentPlayground`:

> Current registration: definition `2550` in `\executor-e2e`, with
> `E2E_QUEUE_PIPELINE_ID=2547`.

1. **Register the pipeline.** New pipeline → GitHub through the
   `githubnext` service connection → existing YAML →
   `tests/executor-e2e/azure-pipelines.yml`. Place it in a `\executor-e2e`
   folder and skip the first run until variables are configured.
   In the live pull-request trigger settings, disable builds from forks and
   disable fork access to secrets/full tokens. Definition `2550` is audited by
   `tests/compiler-smoke-e2e/credentialed-pr-definitions.json`.
2. **Grant the principal behind `agent-playground-write` write access** on the
   `agent-definitions` repo (Contribute, Create branch, Contribute to PRs) and
   on Build (add tags). The YAML maps its AAD token to
   `SYSTEM_ACCESSTOKEN` through `SC_WRITE_TOKEN`. See
   [`docs/safe-output-permissions.md`](../../docs/safe-output-permissions.md) if
   Stage 3 hits 401/403.
3. **Set the GitHub PAT secret** on this pipeline only:
   ```powershell
   ado-aw secrets set EXECUTOR_E2E_GITHUB_TOKEN `
     --org msazuresphere --project AgentPlayground `
     --definition-ids <executor-e2e-pipeline-id> `
     --value <fine-grained-pat-Issues-rw-on-jamesadevine/ado-aw-issues>
   ```
   Do **not** place this token in a shared variable group.
4. Set `EXECUTOR_E2E_ISSUE_REPO=jamesadevine/ado-aw-issues`.
   Confirm the target repository has `executor-e2e-failure` and
   `pipeline-failure` labels.
5. Set `E2E_QUEUE_PIPELINE_ID` to the replacement `noop-target` definition ID.
   *(Optional)* Set `E2E_WIKI_NAME` to enable the wiki scenarios.
6. **Trigger one manual run** to seed the schedule.
