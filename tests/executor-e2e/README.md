# Deterministic executor (Stage 3) safe-output E2E suite

This directory holds a **deterministic**, non-agentic end-to-end test of the
`ado-aw execute` (Stage 3) safe-output executor.

## Why this exists

The daily smoke suite in [`tests/safe-outputs/`](../safe-outputs/) exercises
each safe output end-to-end, but it drives Stage 3 by having an **LLM agent
(Stage 1) emit the safe output** — so a failure can be the model's fault and the
suite is inherently flaky.

This suite removes the LLM from the loop. For every ADO-write safe output it:

1. sets up preconditions **deterministically** via the ADO REST API,
2. crafts the executor's `safe_outputs.ndjson` input directly (fixed literal
   values),
3. runs the real `ado-aw execute` binary (built from the checkout),
4. asserts the effect via the ADO REST API,
5. cleans up every object it created,

and, on any failure, files a GitHub issue on `githubnext/ado-aw` and fails the
build.

## What's here

| File | Purpose |
| --- | --- |
| `azure-pipelines.yml` | Hand-authored ADO pipeline (daily + path-filtered CI + manual). Builds `ado-aw`, builds the harness, runs it against AgentPlayground. |
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
`create-pull-request`:

- **Work items:** `create-work-item`, `update-work-item`,
  `comment-on-work-item`, `link-work-items`, `upload-workitem-attachment`
- **Wiki:** `create-wiki-page`, `update-wiki-page`
- **PR:** `add-pr-comment`, `reply-to-pr-comment`, `resolve-pr-thread`,
  `submit-pr-review`, `update-pr`
- **Git:** `create-branch`, `create-git-tag`
- **Build:** `add-build-tag`, `queue-build`, `upload-build-attachment`,
  `upload-pipeline-artifact`
- **Flagship:** `create-pull-request`

Excluded (nothing to assert or out of scope): the NDJSON-only tools (`noop`,
`missing-data`, `missing-tool`, `report-incomplete`) and the GitHub-only
`create-issue`.

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
# export EXECUTOR_E2E_GITHUB_TOKEN="<fine-grained PAT: Issues rw on githubnext/ado-aw>"
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

1. **Register the pipeline.** New pipeline → Azure Repos/GitHub → existing YAML
   → `tests/executor-e2e/azure-pipelines.yml`. Place it in a `\executor-e2e`
   folder.
2. **Grant the build identity write access** on the `agent-definitions` repo
   (Contribute, Create branch, Contribute to PRs) and on Build (add tags) — the
   pipeline uses `$(System.AccessToken)`. See
   [`docs/safe-output-permissions.md`](../../docs/safe-output-permissions.md) if
   Stage 3 hits 401/403.
3. **Set the GitHub PAT secret** on this pipeline only:
   ```powershell
   ado-aw secrets set EXECUTOR_E2E_GITHUB_TOKEN `
     --org msazuresphere --project AgentPlayground `
     --definition-ids <executor-e2e-pipeline-id> `
     --value <fine-grained-pat-Issues-rw-on-githubnext/ado-aw>
   ```
   Do **not** place this token in a shared variable group.
4. *(Optional)* Set `E2E_QUEUE_PIPELINE_ID` (the `noop-target` pipeline id) and
   `E2E_WIKI_NAME` to enable the queue-build and wiki scenarios.
5. **Trigger one manual run** to seed the schedule.
