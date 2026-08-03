# Plan: markdown-only, lane-based smoke suite

> **Status.** Code, tests and docs are complete and verified locally
> (`cargo test`, `npm run typecheck`, `npx vitest run`). What remains is the
> ADO-side work that cannot be done from a checkout: registering the `agentic`
> lane definition, the released orchestrator and the queue target, then a live
> validation run and retiring the ten old definitions. See **Remaining work**.
>
> **Location.** Committed to the repository root as `SMOKE-REDESIGN-PLAN.md` so
> it can be reviewed alongside the change. Delete it once the cutover completes
> and the content has landed in `tests/smoke/README.md`.

## Remaining work (ADO-side, cannot be done from a checkout)

1. Run the setup runbook in `tests/smoke/REGISTERED.md`:
   - create `refs/heads/ado-aw-smoke-candidate-base` on `ado-aw-mirror` with a
     single `.smoke/pipeline.yml` (contents of `inert-child.yml`), deleting the
     five legacy placeholder lock paths in the same commit;
   - register the `agentic` lane definition, the released orchestrator, and the
     executor-e2e queue target;
   - provision `GITHUB_TOKEN` and `ADO_AW_GITHUB_TOKEN` on the `agentic` lane;
     authorize service connections;
   - set `SMOKE_LANE_AGENTIC_DEFINITION_ID` on both orchestrators and
     `E2E_QUEUE_PIPELINE_ID` on definition `2550`.
2. Record the new ids in `tests/smoke/REGISTERED.md` (marked `_TBD_`) and add
   them to `scheduled_only_definition_ids` in `tests/smoke/trigger-policy.json`
   (the `_note` field in that file states this).
3. Manually run **both** orchestrators and check the eight live assertions in
   `tests/smoke/README.md`.
4. Delete definitions `2545`–`2549`, `2554`–`2558` and `2564`–`2565`, removing
   `2545`–`2549` from `scheduled_only_definition_ids` in
   `tests/smoke/trigger-policy.json` in the same commit — the policy audit
   fetches every listed id and a 404 aborts the run.

## Problem

Every new smoke costs a full Azure DevOps **definition registration**, because
both smoke lanes map one test case to one definition. There are ten such
definitions today (five release-backed, five candidate), plus five committed
lock files that must be kept in sync by a dedicated agentic workflow.

Per new smoke, today:

| Cost | Where |
| --- | --- |
| Register definition, default branch = inert base ref | manual REST/UI |
| Provision secret `GITHUB_TOKEN` (ADO definition clone never copies secrets) | manual |
| Authorize service connections on the new definition | manual |
| Apply + audit 6 fork-hardening flags; add to `trigger-policy.json` | manual |
| New `COMPILER_SMOKE_*_DEFINITION_ID` variable on orchestrator `2559` | manual |
| Commit an inert placeholder at the new lock path on the base ref | git |
| Widen `FixtureName` union, `DEFINITION_ID_ENV_BY_FIXTURE`, `fixturePaths()` | code |
| Commit a lock file, kept fresh by `recompile-safe-output-fixtures` | bot PR |
| Write the markdown | code |

Everything except the last row is attached to *the definition* or to *the
committed lock*, not to the test.

## Approach

An ADO definition binds `(repo, yamlFilename)`; the **ref is supplied per
queue**. So if every case compiles to the same path, the branch selects which
pipeline runs. Invert the mapping:

> **The ref carries the test case. The definition carries only the credentials.
> The markdown is the only committed artefact.**

Three ideas compose:

1. **Fixed YAML path** `.smoke/pipeline.yml`, one **ref per case per run**
   (`refs/heads/ado-aw-smoke-candidate/<buildId>/<caseId>`), each a single
   commit parented on `BUILD_SOURCEVERSION` — siblings, so one bulk object push
   plus N tiny deltas.
2. **Lane definitions** — one per credential class, queued N times.
3. **Compiler-source modes** — the same machinery runs against either a
   candidate compiler (built from the PR/nightly commit) or the **latest
   released** compiler, so no committed lock is needed to exercise the release
   path.

Adding a smoke afterwards = **one markdown file + one manifest entry**.

### Before / after

```
BEFORE   10 definitions + 5 committed locks
  release lane  (GitHub-backed, scheduled)      candidate lane (mirror-backed)
    2545 canary        <- canary.lock.yml         2554 canary
    2546 azure-cli     <- azure-cli.lock.yml      2555 azure-cli
    2547 noop-target   <- noop-target.lock.yml    2556 noop-target
    2548 janitor       <- janitor.lock.yml        2558 smoke-failure-reporter
    2549 reporter      <- reporter.lock.yml       2564 custom-safe-output

AFTER    3 lane definitions + 1 queue target, zero committed locks
  lane agentic  <- .smoke/pipeline.yml  <- refs .../<buildId>/{canary,azure-cli,
                                                  noop-target,custom-safe-output,
                                                  multi-repo,smoke-failure-reporter,
                                                  janitor}
  lane infra    <- .smoke/pipeline.yml  <- (ready for AWF / ado-proxy)
  queue-target  <- static YAML, permanent, not a smoke (executor-e2e dependency)

  driven by two orchestrators over one shared steps template:
    candidate mode  - PR (comment-gated) + nightly     - builds the compiler
    released  mode  - scheduled daily                  - downloads latest release
```

### Confirmed decisions

1. **Two lanes** — `agentic` (every current case; holds `GITHUB_TOKEN`,
   `ADO_AW_GITHUB_TOKEN` and the `agent-playground-*` service connections),
   `infra` (no credentials at all; reserved for AWF and ado-proxy).

   An earlier revision split `smoke-failure-reporter` into its own `debug`
   lane for `ADO_AW_GITHUB_TOKEN`. That was dropped once GitHub issue filing
   became the public `create-github-issue` safe output rather than a
   debug-only capability: a lane per credential fragments as more cases adopt
   it, and the isolation is enforced where it cannot drift — the compiler
   confines the token to the Stage 3 executor, and `assertAdoTokenIsolation`
   fails the run if it reaches Agent or Detection. That prevents the leak
   rather than bounding its blast radius.
2. **Big-bang cutover** — all cases move in one PR. Mitigated by a manual
   pre-merge live run in both modes, and by *disabling* rather than deleting
   old definitions for one release cycle.
3. **Infra lane infrastructure only** — build `kind: raw` support and register
   the `infra` lane, but ship no AWF/proxy cases here.
4. **Eliminate committed locks entirely** — delete all five
   `tests/safe-outputs/*.lock.yml`, retire definitions 2545–2549, and recompile
   from markdown at run time in both modes. Coverage consequences are accepted
   and enumerated below.

## Design

### Compiler-source modes

One variable, `COMPILER_SMOKE_COMPILER_SOURCE` ∈ `candidate | released`, drives
every mode-dependent behaviour:

| | `candidate` | `released` |
| --- | --- | --- |
| Compiler binary | built from `BUILD_SOURCEVERSION`, published as `ado-aw-candidate` | latest GitHub Release asset, downloaded by the orchestrator |
| `supply-chain` transform | inject `pipeline-artifact` pinned to this run | **none** — compiled output keeps its release-URL integrity step |
| Release-URL assertion | `assertNoForbiddenReleaseUrls` (must be absent) | **inverted**: release URLs must be **present** |
| `assertPipelineArtifactValues` | required | skipped |
| Rust build in orchestrator | yes | no (download only) |
| Trigger | PR (comment-gated) + nightly `main` | scheduled daily |

Released mode preserves the release-packaging signal that the committed locks
provided: the orchestrator downloads a released asset to compile with, and every
child then downloads released assets again via its own integrity step. A broken
or missing release asset fails the run in both places.

### Case manifest — `tests/smoke/cases.json`

```jsonc
{
  "schema": "ado-aw/smoke-cases/1",
  "yamlPath": ".smoke/pipeline.yml",
  "lanes": {
    "agentic": { "definitionIdEnv": "SMOKE_LANE_AGENTIC_DEFINITION_ID" },
    "infra":   { "definitionIdEnv": "SMOKE_LANE_INFRA_DEFINITION_ID" }
  },
  "cases": [
    { "id": "canary",      "lane": "agentic", "kind": "compiled",
      "modes": ["candidate", "released"],
      "source": "tests/safe-outputs/canary.md" },
    { "id": "azure-cli",   "lane": "agentic", "kind": "compiled",
      "modes": ["candidate", "released"],
      "source": "tests/safe-outputs/azure-cli.md",
      "assertions": {
        "agentCommand": {
          "required":  ["shell(az", "shell(head"],
          "forbidden": ["--allow-all-tools", "--allow-all-paths"]
        }
      } },
    { "id": "noop-target", "lane": "agentic", "kind": "compiled",
      "modes": ["candidate", "released"],
      "source": "tests/safe-outputs/noop-target.md" },
    { "id": "custom-safe-output", "lane": "agentic", "kind": "compiled",
      "modes": ["candidate"],
      "source": "tests/smoke/custom-safe-output.md",
      "assertions": { "requiredBuildTags": ["ado-aw-custom-job-{buildId}"] } },
    { "id": "smoke-failure-reporter", "lane": "agentic", "kind": "compiled",
      "modes": ["released"],
      "source": "tests/safe-outputs/smoke-failure-reporter.md" },
    { "id": "janitor",     "lane": "agentic", "kind": "compiled",
      "modes": ["released"],
      "source": "tests/safe-outputs/janitor.md" }
  ]
}
```

`modes` replaces today's implicit "the candidate lane compiles four of the five"
rule with an explicit, reviewable declaration.

Validation is strict / fail-closed:

- `id` matches `^[a-z0-9][a-z0-9-]{0,48}$` — **security-relevant**, the id is
  interpolated into a git ref name.
- `id` unique; `lane` must exist; `modes` non-empty and drawn from the known set.
- `source` repo-relative, normalised, no `..`, must exist in the worktree.
- `kind: compiled` sources end `.md`; `kind: raw` sources end `.yml`/`.yaml`.
- `{buildId}` is the only supported tag placeholder.
- Lane definition ids come from env: required positive integers, distinct.

Read **from the detached worktree** (the exact `BUILD_SOURCEVERSION` tree), so
`loadConfig()` splits into `loadConfig()` (env only) and
`loadCases(worktreeDir, env, mode)` called after `createDetachedWorktree`.

### Staging loop

```
for case of cases matching mode:            # sequential, deterministic order
  kind=compiled:
    candidate mode: inject supply-chain.pipeline-artifact
    both modes:     strip the entire `on:` block
    `ado-aw compile --force` + `check`      (ADO_AW_COMPILE_REMOTE_URL=<mirrorUrl>)
    assertions: ADO token isolation, NO TRIGGERS, manifest agentCommand,
                + mode-specific release-URL / pipeline-artifact assertions
    cp <case>.lock.yml -> .smoke/pipeline.yml
  kind=raw:
    cp <source> -> .smoke/pipeline.yml
    assertions: NO TRIGGERS
  changed-paths allowlist guard (per case)
  commitAll -> push HEAD:refs/heads/ado-aw-smoke-candidate/<buildId>/<caseId>
  verifyRemoteRef
  git reset --hard <BUILD_SOURCEVERSION>
```

Per-case allowlist: `.smoke/pipeline.yml`, `.gitattributes`,
`.ado-aw/imports/**`, plus (compiled only) that case's own `.md` and the
generated `.lock.yml`. Strictly tighter than today's union-of-five allowlist.

Note the generated `.lock.yml` is now purely an intermediate that dies with the
ref — nothing is ever committed back to GitHub.

### Trigger hardening (load-bearing under a shared path)

Verified: the compiler already emits `trigger: none` / `pr: none` by default,
and emits `schedules:` only from `on.schedule`. But a case declaring
`on: pr:`/`on: push:` would compile a real trigger — and because every case in a
lane shares one definition and one path, pushing that case's ref would
CI-trigger the lane *in addition to* the API-queued run.

1. `injectPipelineArtifact` (renamed `prepareCaseSource`) strips the entire
   `on:` block in **both** modes. This also removes each case's schedule, which
   is now owned by the orchestrator rather than by the child.
2. New `assertNoTriggers(yamlText, label)`: require top-level `trigger: none`
   and `pr: none`; reject any `schedules:` key or `resources.pipelines[].trigger`.
   Runs for `compiled` and `raw` cases alike, before push.

### Ref model

- `candidateRef(buildId, caseId)` → `refs/heads/ado-aw-smoke-candidate/<buildId>/<caseId>`.
- `parseCandidateRef(ref)` → `{ buildId, caseId } | undefined`, replacing
  `parseCandidateBuildId`'s `^[0-9]+$` suffix test with
  `^([0-9]+)/([a-z0-9][a-z0-9-]{0,48})$`. Anything else stays `ambiguous` and is
  never deleted — the fail-closed posture is preserved.
- `listCandidateRefs` glob widened to `refs/heads/<prefix>/**`; the existing
  client-side `startsWith(prefix)` guard remains the real filter.
- `deleteRemoteRefs(refs[])` batches into one `git push --delete url ref1 ref2 …`
  with per-ref fallback.
- **Granularity win:** each case's ref is deleted iff *that case's*
  `terminalProven` is true. Today one unproven child retains the single shared
  ref for everything.

### Lane queueing

`FixtureBuildRequest` becomes `{ caseId, lane, definitionId, sourceBranch,
sourceVersion }`, where `definitionId` is the lane id — so several requests
legitimately share it. `runner.ts`'s `describeMismatch` identity check still
works and is strengthened in practice: `sourceBranch` is now unique per case, so
it alone disambiguates.

`stale.ts` `childDefinitionIds` becomes the registered lane definition ids.

### Assertions become declarative

`index.ts` hardcodes `if (fixture.name === "azure-cli")` and `fixtures.ts`
hardcodes `requiredBuildTags` for `custom-safe-output`. Both move into the
manifest, so a new case with either need touches JSON, not TypeScript.

### Orchestrators

Shared steps template `tests/smoke/orchestrator-steps.yml`, consumed by two thin
root pipelines so their trigger blocks can't leak into one another:

| Root YAML | Definition | Triggers | Mode |
| --- | --- | --- | --- |
| `tests/smoke/azure-pipelines-candidate.yml` | `2559` (existing) | PR (comment-gated) + nightly 01:00 UTC | `candidate` |
| `tests/smoke/azure-pipelines-release.yml` | new | scheduled daily; `trigger: none`, `pr: none` | `released` |

The Rust build steps are `condition:`-gated on the mode variable, so released
mode skips the toolchain install and compiler build entirely.

**Janitor scheduling:** rather than reproduce per-case cron semantics, `janitor`
runs on every released-mode run (daily instead of weekly). Its prune window is
"older than 30 days", so it is idempotent and running it more often is
strictly safer than running it less. This avoids needing per-schedule template
parameters.

### ADO target state

| Definition | Repo | `yamlFilename` | Default branch | Secrets | Service connections |
| --- | --- | --- | --- | --- | --- |
| smoke lane `agentic` | `ado-aw-mirror` | `/.smoke/pipeline.yml` | `refs/heads/ado-aw-smoke-candidate-base` | `GITHUB_TOKEN`, `ADO_AW_GITHUB_TOKEN` | `agent-playground-read/write` |
| smoke lane `infra` | `ado-aw-mirror` | `/.smoke/pipeline.yml` | same | none | none |
| release orchestrator | `githubnext/ado-aw` | `tests/smoke/azure-pipelines-release.yml` | `main` | none | `githubnext`, `agent-playground-write` |
| queue target | `githubnext/ado-aw` | `tests/executor-e2e/queue-target.yml` | `main` | none | `githubnext` |

All lane definitions: no CI trigger, no PR trigger, no schedule — API-queued
only, and added to `trigger-policy.json`'s `scheduled_only_definition_ids`.

Base ref `refs/heads/ado-aw-smoke-candidate-base` gains `.smoke/pipeline.yml`
(the existing `inert-child.yml` content); the five old placeholder lock paths are
removed in the same commit.

Run readability under a shared definition is already solved — each compiled lock
carries its own `name:` (e.g. `Daily safe-output smoke canary-$(BuildID)`).
`smoke-case:<caseId>` / `smoke-candidate:<buildId>` build tags are added at queue
time for filtering and scanner correlation.

## Dependencies discovered — must not break

1. **`E2E_QUEUE_PIPELINE_ID=2547`.** The executor-e2e `queue-build` scenario
   (definition `2550`) queues the `noop-target` definition. Retiring 2545–2549
   would silently break it — and a lane definition is not a valid substitute,
   because on its default branch it hits the inert placeholder, which fails by
   design.
   **Resolution:** add a permanent, static, non-agentic
   `tests/executor-e2e/queue-target.yml` (`trigger: none`, one echo step),
   register it, and repoint `E2E_QUEUE_PIPELINE_ID`. `noop-target` remains a
   smoke *case* for behavioural coverage; the queue *target* becomes a separate
   trivial fixture. This is a simplification — the scenario only ever needed a
   queueable definition, not an agentic pipeline.

2. **The weekly janitor.** Definition `2548` prunes `ado-aw-smoke-*` artifacts
   from AgentPlayground. Retiring it without replacement lets the sandbox fill
   up indefinitely.
   **Resolution:** `janitor` becomes a released-mode case (see above).

## Coverage delta (accepted)

| Property | Before | After |
| --- | --- | --- |
| Release assets exist / are downloadable | committed lock's integrity step | **retained** — orchestrator downloads a released asset, and every released-mode child downloads again |
| Released compiler output runs end to end | committed lock | **retained** — recompiled at stage time by the released binary |
| Runtime integrity check passes | committed lock | **retained** — the child still recompiles from the staged `.md` and compares |
| Committed lock matches released compiler (`ado-aw check` drift) | `recompile-safe-output-fixtures` | **dissolved** — nothing committed, so no drift is possible |
| **Pipelines run from a GitHub-backed definition with real GitHub metadata** | 2545–2549 | **LOST** — every smoke now runs from `ado-aw-mirror` with mirror metadata |
| **The exact committed bytes a customer would commit are executed** | 2545–2549 | **LOST** |
| Repo dogfoods the commit-the-lock customer workflow | `tests/safe-outputs/` | **LOST** here; still exercised by `.github/workflows/*.lock.yml` (gh-aw) |

The two genuine losses are both about GitHub-backed, committed-artifact
execution. Cheapest future mitigation if a metadata regression ever escapes:
re-add a *single* GitHub-backed canary with a committed lock. Deliberately not
done now.

## Todos

Every code/docs todo below is **done**; items 22–25 are the ADO-side work
summarised under *Remaining work* at the top.

1. **manifest-schema** — ✅ `tests/smoke/cases.json` (two lanes, seven cases,
   `modes`, declarative assertions).
2. **manifest-loader** — ✅ `cases.ts`: strict fail-closed validation and
   `loadCases(worktreeDir, env, mode)`.
3. **config-split** — ✅ `config.ts` reduced to env-only;
   `candidateRef(buildId, caseId)`; `SMOKE_COMPILER_SOURCE` parsing.
4. **mode-plumbing** — ✅ artifact injection skipped in released mode;
   release-URL assertion inverted; artifact assertion skipped.
5. **strip-on-block** — ✅ whole `on:` block stripped;
   `injectPipelineArtifact` → `prepareCaseSource`.
6. **assert-no-triggers** — ✅ `assertNoTriggers` + `assertReleaseUrlsPresent`.
7. **declarative-assertions** — ✅ `agentCommand` / `requiredBuildTags` driven
   from the manifest; `fixture.name ===` branches deleted.
8. **fixed-path-staging** — ✅ per-case stage/commit/push/reset;
   `fixtures.ts` retired.
9. **raw-kind** — ✅ verbatim copy path, still trigger-asserted.
10. **ref-model** — ✅ `parseCandidateRef`, widened glob, batched
    `deleteRemoteRefs`.
11. **per-case-cleanup** — ✅ refs deleted per case on proven-terminal.
12. **lane-queueing** — ✅ `runner.ts` keyed by `caseId` + `lane`.
13. **stale-scanner** — ✅ new ref pattern; `laneDefinitionIds`.
14. **queue-tags** — ✅ `smoke-case:` / `smoke-candidate:` via `addBuildTags`.
15. **orchestrator-split** — ✅ `orchestrator-steps.yml` +
    `orchestrator-variables.yml` + two mode-specific roots.
16. **queue-target** — ✅ `tests/executor-e2e/queue-target.yml` + README.
17. **delete-locks** — ✅ five locks deleted; smoke dir moved to `tests/smoke/`.
18. **retire-recompile-workflow** — ✅ workflow and its release dispatch job
    removed.
19. **trigger-policy** — ✅ path fixed, `_note` documents the pending ids.
20. **unit-tests** — ✅ 270 harness tests (15 files), incl. new `cases.test.ts`.
21. **docs** — ✅ both smoke READMEs, `REGISTERED.md`, `AGENTS.md`,
    `docs/ado-script.md`, executor-e2e README, and the two review workflows.
22. **ado-runbook** — ✅ written (`tests/smoke/REGISTERED.md`).
23. **ado-apply** — ⏳ requires AgentPlayground access.
24. **live-validation** — ⏳ requires registered definitions.
25. **retire-old-defs** — ⏳ after live validation.

### Dependencies

```
manifest-schema ──> manifest-loader ──> config-split ──┬─> mode-plumbing ──┐
                                                       ├─> fixed-path-staging ──> raw-kind ──┐
strip-on-block ──> assert-no-triggers ─────────────────┤                                     │
declarative-assertions ────────────────────────────────┤                                     │
ref-model ──> per-case-cleanup ────────────────────────┤                                     │
lane-queueing ──> stale-scanner ───────────────────────┤                                     │
queue-tags ────────────────────────────────────────────┴──> orchestrator-split ──────────────┤
                                                                                             │
delete-locks ──> retire-recompile-workflow                                                   │
queue-target ────────────────────────────────────────────────────────────────────────────────┤
                                                                     trigger-policy ─────────┤
                                                                     unit-tests ─────────────┤
                                                                     docs ────────────────────┤
                                                     ado-runbook ──> ado-apply ──────────────┤
                                                                                             ▼
                                                                                     live-validation
                                                                                             ▼
                                                                                     retire-old-defs
```

## Validation

**Local (deterministic, no ADO):**

```bash
cd scripts/ado-script
npm ci
npm run typecheck
npx vitest run src/compiler-smoke-e2e
npm run build:compiler-smoke-e2e
```

Plus `cargo test` — several Rust tests reference `tests/safe-outputs/` paths and
must be checked after `delete-locks`.

**Live contract:**

| # | Assertion | Mode |
| --- | --- | --- |
| 1 | Producer remains in progress after publishing its artifact | candidate |
| 2 | Every child downloads the exact producer `run-id` | candidate |
| 3 | Every child downloads released assets from GitHub Releases | released |
| 4 | All in-mode cases succeed | both |
| 5 | `custom-safe-output` carries `ado-aw-custom-job-<child-build-id>` | candidate |
| 6 | Exactly one ref per case created; every ref deleted | both |
| 7 | Each build ran the lane definition on that case's own ref | both |
| 8 | Queued build count == case count (no ref push CI-triggered a lane) | both |
| 9 | `queue-build` executor-e2e scenario passes against the new queue target | n/a |

## Risks

| Risk | Mitigation |
| --- | --- |
| Big-bang cutover removes **both** existing smoke signals at once | Manual live run of both orchestrators before the old definitions are deleted; the new lanes must be proven green first, since deletion is not reversible |
| Loss of GitHub-backed / committed-artifact execution | Accepted (decision 4). Re-add a single GitHub-backed canary if a metadata regression escapes |
| `E2E_QUEUE_PIPELINE_ID` breakage | Explicit `queue-target` todo; live assertion #9 |
| Janitor stops pruning; AgentPlayground fills up | Janitor becomes a daily released-mode case; idempotent 30-day window |
| GitHub PAT reaches the agent | Compiler confines it to the Stage 3 executor; `assertAdoTokenIsolation` fails the run on freshly compiled YAML if it appears in Agent or Detection |
| Credential creep into the credential-free lane | `infra` holds nothing, and a manifest test asserts it carries no cases |
| A case with a stray trigger double-queues its whole lane | `assertNoTriggers` + full `on:` strip, both fail-closed before push |
| Malicious/typo `caseId` injected into a git ref name | Strict `^[a-z0-9][a-z0-9-]{0,48}$` at manifest load, before any git call |
| Released mode silently degrades to candidate behaviour | Inverted release-URL assertion makes a missing release URL a hard failure |
| Parallel-job exhaustion as case count grows | `COMPILER_SMOKE_CONCURRENCY` retained; raise deliberately |
| Sequential per-case compile lengthens staging | Measure during live validation; parallelise the compile phase only if material |

## Notes

- `ado-aw-mirror` is **not** a mirror of GitHub — nothing syncs into it. It
  holds only the permanent inert base ref plus ephemeral per-run refs.
  Conceptually a *staging repo*; renaming it is out of scope.
- Candidate commits are built from the **local GitHub checkout** at
  `BUILD_SOURCEVERSION` and pushed only to ADO. `verifyLocalCommit`'s
  no-mirror-fetch rule (PR merge refs don't exist on the mirror) is unchanged.
- After this change `tests/safe-outputs/` is markdown-only; consolidating it
  into `tests/smoke/` is a natural follow-up but is kept out of scope to limit
  path churn in one PR.
- Resolving lane ids by name over REST instead of env vars was considered and
  dropped: with three stable lanes it is no longer a per-case cost.
