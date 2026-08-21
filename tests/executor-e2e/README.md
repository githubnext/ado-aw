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
- **Work items:** `create-work-item`, `assign-work-item`, `update-work-item`,
  `comment-on-work-item`, `link-work-items`, `upload-workitem-attachment`, plus
  two rendering-fidelity scenarios (see [Rendering
  fidelity](#rendering-fidelity) below)
- **Wiki:** `create-wiki-page`, `update-wiki-page`
- **PR:** `add-pr-comment`, `reply-to-pr-comment`, `resolve-pr-thread`,
  `submit-pr-review`, `update-pr`
- **Git:** `create-branch`, `create-git-tag`
- **Build:** `add-build-tag`, `queue-build`, `upload-build-attachment`,
  `upload-pipeline-artifact`
- **Flagship:** `create-pull-request` covers both a named additional checkout at
  `<BUILD_SOURCESDIRECTORY>/<alias>` and `repository: self` beneath a non-Git
  multi-checkout root, with compiler-owned self repository identity taking
  precedence over trigger-scoped `BUILD_REPOSITORY_*` values. The `self`
  scenario supplies only `ADO_AW_SELF_REPOSITORY_NAME` — matching what the
  compiler emits — so it also proves the executor resolves a repository from
  its name alone.
- **GitHub issues:** `create-github-issue`, `set-github-issue-type`, and the
  same-run `temporary_id` handoff between them. These are the only scenarios
  that assert against **GitHub** rather than ADO — see
  [GitHub issue scenarios](#github-issue-scenarios) below.

Excluded (out of scope): none of the currently shipped safe outputs.

The standalone `create-work-item` scenario writes the deterministic internal
ID `#aw_wicreate`; the `assign-work-item-temporary-id-handoff` scenario stages
another `create-work-item` ahead of `assign-work-item` in one executor
invocation and resolves it through `#aw_wiassign`. Both explicit IDs emulate
the values generated and persisted by the MCP because this harness writes
internal executor NDJSON directly rather than agent-supplied parameters. The
handoff verifies `System.AssignedTo` and deletes the scratch item. It uses
`E2E_WORK_ITEM_ASSIGNEE` when configured,
otherwise `BUILD_REQUESTEDFOREMAIL`; it skips when neither provides an
assignable identity.

### Rendering fidelity

`create-work-item` is the only safe output whose body goes through the Markdown
sanitizer (`src/sanitize/markdown.rs`), and its stored description is what a
human actually reads in the work item. Two scenarios pin that rendering:

| Scenario id | Work item type | Field |
| --- | --- | --- |
| `create-work-item-rendering` | `Task` | `System.Description` |
| `create-work-item-rendering-bug` | `Bug` | `Microsoft.VSTS.TCM.ReproSteps` |

Both propose the same **unsanitized** corpus and assert, in order, that the
stored field:

1. contains no denied construct (`<script`, `onerror=`, `<iframe`, a
   `javascript:` URL) outside fenced code, while the fenced-code copies of those
   same strings survive verbatim;
2. is recorded with `multilineFieldsFormat: Markdown` — when the organization
   does not surface that on read, the scenario logs a note and the patch stays
   pinned by the executor unit tests in `src/safe_outputs/create_work_item.rs`;
3. equals the sanitized golden **byte for byte**.

The corpus and its golden live in one place —
[`scripts/ado-script/src/executor-e2e/scenarios/markdown-rendering-corpus.json`](../../scripts/ado-script/src/executor-e2e/scenarios/markdown-rendering-corpus.json)
— imported by the harness and `include_str!`d by the Rust golden test in
`src/sanitize/markdown.rs`, so the sub-second local test and the against-ADO
test cannot drift. A deliberate rendering change means updating `expected` in
that one file; an accidental one fails `cargo test` before it ever reaches ADO.

The Bug scenario skips (rather than fails) when the project does not define the
`Bug` work item type.

The checked-in pipeline resolves `E2E_WORK_ITEM_ASSIGNEE` from a same-named
definition/queue-time variable first, then falls back to
`Build.RequestedForEmail`.

> **Coverage note.** The signal scenarios (`noop`, `missing-tool`,
> `missing-data`, `report-incomplete`) were previously exercised only by
> now-deleted per-tool agentic smoke pipelines. Adding them here closes
> the coverage gap while keeping the test deterministic.

## GitHub issue scenarios

`create-github-issue` and `set-github-issue-type` had **zero runtime
coverage** before these scenarios: their only proof was a wiremock unit test,
and the last thing exercising `create-github-issue` end to end
(`smoke-failure-reporter`) was removed by the smoke-suite rework.

| Scenario id | Tool | What it proves |
| --- | --- | --- |
| `create-github-issue` | `create-github-issue` | final title is `title-prefix` + the agent title; the body carries the agent text and the `<!-- ado-aw -->` traceability footer; config-injected static labels merge with allowed agent labels |
| `create-github-issue-label-denied` | `create-github-issue` | `allowed-labels` is **default-deny** — an agent label outside the allowlist is rejected and no issue is filed |
| `set-github-issue-type` | `set-github-issue-type` | a native issue type is applied to an existing issue |
| `set-github-issue-type-clear` | `set-github-issue-type` | the documented `issue_type: ""` clear operation |
| `create-github-issue-temporary-id-handoff` | `set-github-issue-type` (with `create-github-issue` staged ahead of it) | the same-run `temporary_id` handoff |

### Why the handoff scenario is shaped differently

`create-github-issue` may mint a `temporary_id`, and
`set-github-issue-type.issue_number` accepts either a real number or that id.
The registry backing this (`ExecutionContext::resolved_github_issues`) is an
in-process `Arc<Mutex<HashMap<…>>>` that is never persisted, so the handoff is
only observable inside a **single `ado-aw execute` invocation**.

That matches production — a SafeOutputs job runs one `ado-aw execute` over the
whole `safe_outputs.ndjson` — but every other scenario here runs one entry per
invocation. The handoff scenario therefore uses the harness's `priorEntries`
hook (see `Scenario.priorEntries` in `scripts/ado-script/src/executor-e2e/scenario.ts`)
to stage `create-github-issue` as an extra NDJSON line ahead of its own, in the
same executor process. The assertion is that the `set-github-issue-type` record
reports the issue number `create-github-issue` actually filed.

> **Why this can't be split across jobs.** Because the registry is per-process,
> putting `require-approval` on only one of the two tools would split Stage 3
> into two `ado-aw execute` processes (`SafeOutputs` and `SafeOutputs_Reviewed`),
> and a `temporary_id` minted in one could not resolve in the other. The
> compiler rejects that configuration up front —
> `validate_github_issue_outputs_config` in `src/compile/common.rs` requires both
> tools to have the same *effective* `require-approval` setting, so the
> section-level default and a per-tool override are both accounted for.

### A product bug these scenarios caught

Adding this coverage immediately found a real defect in Stage 3 config
handling, now fixed in `ExecutionContext::get_tool_config`
(`src/safe_outputs/result.rs`). It is recorded here because it is exactly the
class of bug the wiremock unit tests structurally could not see.

Stage 3 injects synthetic `staged` and `require-approval` keys into **every**
tool config (`src/main.rs` for `--source`; `src/compile/custom_tools.rs` for the
compiler-generated `--resolved-config` that production uses).
`CreateGithubIssueConfig` and `SetGithubIssueTypeConfig` are the only
safe-output configs declared `#[serde(deny_unknown_fields)]`, and neither
declares a `staged` field — so deserialization failed, `get_tool_config`
swallowed the error via `.ok().unwrap_or_default()`, and **the operator config
was silently replaced with `Default::default()`**.

Observable effects: `target-repo` ignored (Stage 3 failed outright on
non-GitHub-backed ADO builds), `title-prefix` never applied, static
`labels`/`assignees` dropped, `allowed-labels` emptied so default-deny rejected
*every* agent label, `require-temporary-id` unenforced, and
`set-github-issue-type.allowed` never gating anything — the last of which
failed **open**.

The unit tests missed it because they build an `ExecutionContext` directly with
a config map that has no `staged` key, i.e. a shape that never occurs in
production. The fix strips both orchestration keys and logs a warning instead of
silently defaulting; `result.rs` carries regression tests asserting an operator
config survives the injected keys.

Note that `create-github-issue-label-denied` deliberately matches only the
`labels not in allowed-labels` message. The alternative message
(`no allowed-labels configured`) is precisely what the executor emitted when the
config was dropped, so accepting both would have let the scenario pass either
way — the failure mode this suite exists to prevent.

### Close, don't delete

GitHub has **no REST endpoint to delete an issue**. These scenarios are the only
ones in the suite that cannot tear down completely: `cleanup()` closes each
issue as `not_planned` instead.

Every scratch issue title embeds the standard `ado-aw-det-$(Build.BuildId)-<id>`
marker, so anything a cleanup misses is findable with a single search on the
scratch repository. Cleanup also does **not** depend solely on state captured in
`assert()` — when the executor filed an issue but the record came back
non-`succeeded`, `assert()` never runs, so cleanup falls back to an exact-title
search on that marker.

Because issues accumulate (closed, never deleted), point these scenarios at a
scratch repository, not a canonical one.

### Environment

| Variable | Meaning |
| --- | --- |
| `EXECUTOR_E2E_GITHUB_TOKEN` | Reused from failure-issue filing. It must now also carry **Issues: write** on the scratch repository, because these scenarios create, mutate, and close issues. |
| `EXECUTOR_E2E_SCENARIO_ISSUE_REPO` | Optional. `owner/repo` for scratch issues; falls back to `EXECUTOR_E2E_ISSUE_REPO`. Set it to keep scenario issues away from the failure-report repository. |
| `E2E_GITHUB_ISSUE_TYPE` | Optional. Forces a native issue-type name for environments where the token cannot read org metadata but the type is known to exist. |

There is deliberately **no default repo** for these scenarios: when neither
variable is set they skip rather than filing scratch issues onto
`githubnext/ado-aw`.

### Scenarios that skip when a precondition is missing

Some scenarios need optional infrastructure and **skip** (rather than fail)
when it is not available:

- `queue-build` — needs a target pipeline id in `E2E_QUEUE_PIPELINE_ID`.
- `create-wiki-page` / `update-wiki-page` — need a wiki in the project. The
  harness auto-discovers the first wiki; set `E2E_WIKI_NAME` to force one. When
  no wiki exists, both skip.
- `add-build-tag`, `upload-build-attachment`, `upload-pipeline-artifact` — need
  a real current build (`BUILD_BUILDID`); they skip when run outside a pipeline.
- **All five GitHub issue scenarios** — need `EXECUTOR_E2E_GITHUB_TOKEN` and a
  scratch repo (`EXECUTOR_E2E_SCENARIO_ISSUE_REPO` or `EXECUTOR_E2E_ISSUE_REPO`).
  They also skip when the token authenticates but cannot write issues on that
  repo, with the harness's auth diagnosis attached to the skip reason.
- `set-github-issue-type` and, on the named-type path, the handoff — need a
  native issue type to exist. Issue types are an **organisation-level**
  construct (`GET /orgs/{org}/issue-types`) with no user-account equivalent, so
  a **user-owned scratch repo can never expose one** and these skip
  permanently there. The handoff scenario stays runnable by falling back to the
  documented `issue_type: ""` clear operation; `set-github-issue-type-clear`
  skips only if GitHub rejects the clear outright.

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
# Optional: keep GitHub issue scenario scratch issues out of the failure-report repo
# export EXECUTOR_E2E_SCENARIO_ISSUE_REPO="<owner>/<scratch-repo>"
# Optional: force a native issue-type name (org-owned repos only)
# export E2E_GITHUB_ISSUE_TYPE="Bug"
# export E2E_QUEUE_PIPELINE_ID="<noop-target pipeline id>"
# export E2E_WORK_ITEM_ASSIGNEE="<ADO user email, UPN, or display name>"
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
> `E2E_QUEUE_PIPELINE_ID` pointing at the `queue-target` definition
> registered from [`queue-target.yml`](queue-target.yml).

1. **Register the pipeline.** New pipeline → GitHub through the
   `githubnext` service connection → existing YAML →
   `tests/executor-e2e/azure-pipelines.yml`. Place it in a `\executor-e2e`
   folder and skip the first run until variables are configured.
   In the live pull-request trigger settings, disable builds from forks and
   disable fork access to secrets/full tokens. Definition `2550` is audited by
   `tests/smoke/trigger-policy.json`.
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

   > The GitHub issue **scenarios** reuse this same token, so it needs
   > **Issues: write** on the scratch repository — not just enough to file a
   > failure report. When it can authenticate but not write, those scenarios
   > skip with a diagnosis rather than failing the build.
4. Set `EXECUTOR_E2E_ISSUE_REPO=jamesadevine/ado-aw-issues`.
   Confirm the target repository has `executor-e2e-failure` and
   `pipeline-failure` labels.
   *(Optional)* Set `EXECUTOR_E2E_SCENARIO_ISSUE_REPO` to a separate scratch
   repository so the GitHub issue scenarios do not accumulate closed issues
   alongside the failure reports.

   > `set-github-issue-type` and the named-type half of the handoff need a
   > native issue type, which is an **organisation-level** construct. On a
   > user-owned repo such as `jamesadevine/ado-aw-issues` they will skip on
   > every run; point `EXECUTOR_E2E_SCENARIO_ISSUE_REPO` at an org-owned repo
   > with issue types defined to enable them.
5. Set `E2E_QUEUE_PIPELINE_ID` to the `queue-target` definition ID (register
   [`queue-target.yml`](queue-target.yml) if it does not exist yet). It is a
   permanent, trigger-free, non-agentic pipeline that exists only to be
   queued, so this scenario no longer depends on the smoke suite's
   registration lifecycle.
   *(Optional)* Set `E2E_WIKI_NAME` to enable the wiki scenarios.
6. **Trigger one manual run** to seed the schedule.
