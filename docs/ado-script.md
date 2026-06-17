# `ado-script`: bundled TypeScript runtime helpers

`ado-script` is the umbrella name for the TypeScript workspace at
[`scripts/ado-script/`](../scripts/ado-script/). It produces small,
ncc-bundled Node programs that the **compiler injects into every emitted
pipeline** as runtime helpers. Today it produces seven bundles:

- `gate.js` — trigger-filter gate evaluator (Setup job).
- `import.js` — runtime prompt resolver described in
  [`runtime-imports.md`](runtime-imports.md) (Agent job).
- `exec-context-pr.js` — PR-context precompute that resolves the
  merge-base, writes `aw-context/pr/{base,head}.sha`, and appends a
  prompt fragment to the agent prompt (Agent job, before the agent
  runs). See [`execution-context.md`](execution-context.md).
- `exec-context-pr-synth.js` — Setup-job precompute that normalises
  PR-identifier variables into the stable `AW_PR_*` namespace,
  promoting CI builds with an open PR to PR semantics (Setup job,
  before any gate step).
- `exec-context-manual.js` — Manual-context precompute that stages
  `aw-context/manual/{requested-for, parameters.json}` for
  manually-queued builds and appends a `## Manual run context`
  fragment to the agent prompt (Agent job; see
  [`execution-context.md`](execution-context.md)).
- `exec-context-pipeline.js` — Pipeline-completion precompute that
  fetches upstream-build metadata via the Build REST API and stages
  `aw-context/pipeline/upstream-*` files plus a `## Pipeline-completion
  context` prompt fragment (Agent job; see
  [`execution-context.md`](execution-context.md)).
- `conclusion.js` — Conclusion job work-item reporter: reads the
  safe-outputs execution manifest and upstream job results,
  files/comments ADO work items for pipeline failures and diagnostic
  signals (Conclusion job).

> **Internal-only.** `ado-script` is not a user-facing front-matter
> feature. Authors never write an `ado-script:` block in their agent
> markdown. The compiler decides when an `ado-script` bundle is needed
> and how to wire it. See [`docs/tools.md`](tools.md) for what *is*
> user-facing.

## What `gate.js` does

`gate.js` is a single-shot Node program that runs as a step in the
pipeline's **Setup** job and decides whether the downstream Agent /
SafeOutputs jobs should execute. It evaluates a declarative `GateSpec`
against runtime facts (PR title, labels, changed files, build reason,
etc.) and emits exactly one `##vso[task.setvariable]` line:

```
##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]true   (or false)
```

Downstream jobs gate themselves on that variable via a `condition:`
clause emitted by the compiler.

The gate is a *data interpreter*, not a code evaluator. The `GateSpec`
is a typed JSON document; predicates are dispatched via a `switch` on a
discriminated union. There is no `eval`, no `Function`, no `vm` — a
compromised compiler cannot use the spec to run arbitrary code on the
pipeline runner.

## What `import.js` does

`import.js` is a single-shot Node program. It reads the prompt file path
from `argv[2]` and resolves `{{#runtime-import path}}` markers in place.
The compiler runs it as a post-prepare-prompt step when
[`inlined-imports: false`](front-matter.md#inlined-imports). See
[`runtime-imports.md`](runtime-imports.md) for the author-facing marker
syntax.

### Env-var contract

`import.js` takes no environment variables. Relative-path markers
resolve against `dirname(argv[2])`; in pipeline use this is irrelevant
because the compiler always embeds an absolute marker path and
`import.js` is single-pass (nested markers inside the inlined body are
not re-expanded).

The bundle lives at `import.js` and ships in the same
`ado-script.zip` release asset as `gate.js`, `exec-context-pr.js`,
`exec-context-pr-synth.js`, `exec-context-manual.js`, and
`exec-context-pipeline.js`, so pipelines download it through the
same Agent-job asset flow.
`import.js` uses only the Node standard library, so the ncc bundle is
small (~1.5 KB) and carries no SDK dependency.

The Stage-2 threat-analysis prompt is **not** runtime-imported.
`src/data/threat-analysis.md` is `include_str!`'d into the `ado-aw`
binary and inlined into the emitted YAML at compile time, matching
gh-aw's pattern (their `threat_detection.md` ships with the setup
action and is read directly from disk — no marker, no resolver).

## What `exec-context-pr.js` does

`exec-context-pr.js` is a single-shot Node program that runs as the
**precompute step** of the PR contributor of the execution-context
extension. It runs in the Agent job *before* the agent step, inside
the AWF network-isolated sandbox's prepare phase.

It performs the work that used to live as ~190 lines of bash heredoc
inside `src/compile/extensions/exec_context/pr.rs`:

1. **Validate identifiers** — `PR_ID`, `SYSTEM_TEAMPROJECT`,
   `BUILD_REPOSITORY_NAME`, and `SYSTEM_PULLREQUEST_TARGETBRANCH` are
   each matched against a strict allowlist regex (`validate.ts`)
   before any of them are interpolated into a git refspec or the
   agent prompt. On any failure the program writes
   `aw-context/pr/error.txt` and a `### PR context (unavailable)`
   fragment to the agent prompt, then exits 0 (soft fail: the agent
   still runs, but is told the context is missing).
2. **Resolve merge-base** — if the checkout is a synthetic
   merge-commit (parent count ≥ 3 per ADO's PR-validation flow),
   `merge-base.ts::resolveMergeBase` computes `git merge-base` over
   the two parents. Otherwise it fetches the target branch with
   progressive deepening (`--depth=200/500/2000/--unshallow`) and
   then `git merge-base` against `HEAD`. Same `BASE_SHA` semantics
   in both paths (git's true common ancestor).
3. **Stage artefacts** — writes `aw-context/pr/base.sha` and
   `aw-context/pr/head.sha` so the agent can `git diff $(cat
   .../base.sha)..$(cat .../head.sha)` itself.
4. **Append prompt fragment** — appends a `## PR context` section to
   `/tmp/awf-tools/agent-prompt.md` (path overridable via
   `AW_AGENT_PROMPT_FILE` for tests).

### Trust boundary

The bearer (`SYSTEM_ACCESSTOKEN`) is mapped into the Node process's
env by the wrapper bash step, but is **only** propagated into the
spawned `git` child process via `GIT_CONFIG_COUNT=1 / KEY_0 /
VALUE_0` env vars (see `git.ts::bearerEnv` + `runGit` in
`merge-base.ts`). It never appears in argv, is never written to
`.git/config`, and is never visible to the agent process (which is
spawned later, in a separate AWF child). The
`test_execution_context_pr_does_not_leak_system_accesstoken` Rust
test walks the emitted YAML and asserts this scoping.

### Env-var contract

| Env var | Source | Purpose |
|---|---|---|
| `SYSTEM_ACCESSTOKEN` | `$(System.AccessToken)` | ADO REST / git fetch bearer |
| `SYSTEM_PULLREQUEST_PULLREQUESTID` | `$(System.PullRequest.PullRequestId)` | PR identifier (validated numeric) |
| `SYSTEM_PULLREQUEST_TARGETBRANCH` | `$(System.PullRequest.TargetBranch)` | PR target branch for the fetch |
| `SYSTEM_TEAMPROJECT` | `$(System.TeamProject)` | ADO project name (validated) |
| `BUILD_REPOSITORY_NAME` | `$(Build.Repository.Name)` | Repository name (validated) |
| `BUILD_SOURCESDIRECTORY` | `$(Build.SourcesDirectory)` | Workspace root for `aw-context/` |
| `AW_AGENT_PROMPT_FILE` | (test override) | Override default `/tmp/awf-tools/agent-prompt.md` |

The bundle uses only `node:child_process` / `node:fs` / `node:path`
— no `azure-devops-node-api`, no `fetch`. The ncc'd bundle is ~8 KB.

## What `exec-context-pr-synth.js` does

`exec-context-pr-synth.js` is a single-shot Node program that runs as
the first step of the pipeline's **Setup** job, *before* any gate
step. It normalises the PR-identifier variables into the stable
`AW_PR_*` namespace so that every downstream consumer (the gate step
in the same job and the Agent job) can read a single set of names
regardless of whether the build is a real PR build or a CI build that
was *synth-promoted* to PR semantics.

### Why it exists

Azure DevOps Services ignores the YAML `pr:` block unless a
per-branch Build Validation policy is registered server-side. Without
that policy, a push to a feature branch fires the pipeline as
`Build.Reason = IndividualCI` even when an open PR exists. The synth
path closes that gap: it looks up the active PR for the build's source
branch and, if exactly one matches the agent's `on.pr` branch/path
filters, promotes the CI build to PR semantics.

Doing the real-vs-synth merge here (in TypeScript) — rather than
coalescing `$(System.PullRequest.X)` with `$(AW_SYNTHETIC_PR_X)` inside
step `env:` — is deliberate: ADO only evaluates `$[ ... ]` runtime
expressions inside the `variables:` block and `condition:` fields, NOT
inside step `env:` values, so the coalesce form silently passed the
literal expression string to downstream steps. Every consumer now reads
plain `$(AW_PR_*)` macros instead.

### Variables emitted

Each variable is emitted as **both** a `setOutput` (`isOutput=true`,
for cross-job consumption via
`dependencies.Setup.outputs['synthPr.<NAME>']`) and a regular `setVar`
(for same-job consumption via `$(<NAME>)` macros). Both forms are
required because `isOutput=true` does not register the variable in the
producing job's regular namespace — see the `setVar` doc-comment in
`scripts/ado-script/src/shared/vso-logger.ts`.

| Variable | Meaning |
|---|---|
| `AW_PR_ID` | Resolved PR id (real or synth); empty if not a PR build |
| `AW_PR_TARGETBRANCH` | Resolved target ref (`refs/heads/<name>`) |
| `AW_PR_SOURCEBRANCH` | Resolved source ref |
| `AW_PR_IS_DRAFT` | `"true"` / `"false"` / `""` (only meaningful on the synth path) |
| `AW_SYNTHETIC_PR` | `"true"` iff this build was synth-promoted (CI build + matched open PR); empty on real PR builds and non-promoted CI |
| `AW_SYNTHETIC_PR_SKIP` | `"true"` iff synth was attempted but no match was found (gates the Agent job to skip) |

### Runtime logic

All soft skips exit 0; only spec-decode and infrastructure errors exit
non-zero:

1. **Real PR build** — if `SYSTEM_PULLREQUEST_PULLREQUESTID` is
   non-empty (after stripping unsubstituted `$(name)` macro literals),
   copy the `SYSTEM_PULLREQUEST_*` env into `AW_PR_*` and return. No
   API call needed.
2. **GitHub-typed repo** — if `BUILD_REPOSITORY_PROVIDER` is `GitHub`,
   emit empty `AW_PR_*` plus `AW_SYNTHETIC_PR_SKIP=true` (ADO routes
   GitHub PR webhooks natively, so a CI build on a GitHub repo has no
   associated PR).
3. **Decode `PR_SYNTH_SPEC`** — base64-decode the compiler-emitted
   filter spec (`build_pr_synth_spec` in `src/compile/filter_ir.rs`).
   Corruption is a hard failure (exit 1).
4. **Fetch active PRs** whose `sourceRefName == BUILD_SOURCEBRANCH`.
5. **Branch filter** — keep PRs whose `targetRefName` matches
   `spec.branches.include` / `spec.branches.exclude`.
6. **Exactly-one rule** — a count other than 1 emits empty `AW_PR_*` +
   skip.
7. **Path filter** — if the agent declared `on.pr.paths`, fetch the
   latest PR iteration's changed files and skip unless at least one
   matches.
8. **Match** — emit the resolved `AW_PR_*` plus `AW_SYNTHETIC_PR=true`.

### Env-var contract

The compiler injects these on the `node exec-context-pr-synth.js`
step; the predefined `SYSTEM_PULLREQUEST_*` variables are auto-mapped
into the process env by ADO at runtime:

| Env var | Source | Purpose |
|---|---|---|
| `PR_SYNTH_SPEC` | compiled inline (base64) | The branch/path filter spec |
| `SYSTEM_ACCESSTOKEN` | `$(System.AccessToken)` | ADO REST auth |
| `ADO_COLLECTION_URI` | `$(System.CollectionUri)` | ADO org base URL |
| `ADO_PROJECT` | `$(System.TeamProject)` | ADO project for the PR lookup |
| `ADO_REPO_ID` | `$(Build.Repository.ID)` | Repository id for the PR lookup |
| `BUILD_REASON` | `$(Build.Reason)` | Distinguishes CI from PR builds |
| `BUILD_REPOSITORY_PROVIDER` | `$(Build.Repository.Provider)` | Detects GitHub-typed repos |
| `BUILD_SOURCEBRANCH` | `$(Build.SourceBranch)` | Source ref matched against active PRs |
| `SYSTEM_PULLREQUEST_*` | ADO-injected | Real-PR identifiers propagated verbatim |

The bundle lazy-imports `azure-devops-node-api` only when it needs to
call the PR REST endpoints (steps 4 and 7); real PR builds and
GitHub-typed repos return before any SDK load.

## End-to-end data flow

```
       ┌──────────────────────┐
       │  Rust compiler       │
       │  (filter_ir.rs)      │
       └──────────┬───────────┘
                  │ build_gate_spec(...)  →  GateSpec  (JSON, base64)
                  ▼
       ┌──────────────────────┐
       │  Generated pipeline  │
       │  Setup job:          │
       │   1. UseNode@1       │
       │   2. curl + sha256   │     downloads ado-script.zip
       │      + unzip         │     from the matching ado-aw release
       │   3. node gate/index │     reads GATE_SPEC env var
       │      .js             │
       └──────────┬───────────┘
                  │ ##vso[task.setvariable variable=SHOULD_RUN;…]
                  ▼
       ┌──────────────────────┐
       │  Agent / SafeOutputs │  conditioned on SHOULD_RUN=true
       │  jobs                │
       └──────────────────────┘
```

The same `GateSpec` shape is generated as a JSON Schema by
`cargo run -- export-gate-schema` and converted to TypeScript by
`json-schema-to-typescript` into `src/shared/types.gen.ts`. The TS
gate evaluator imports from `types.gen.ts`, never from a hand-written
mirror of the IR — so the spec contract cannot drift between compiler
and evaluator. CI enforces this with a `git diff --exit-code` step on
the codegen output.

## Runtime stages inside `gate.js`

`gate.js`'s entry point is `src/gate/index.ts`. It runs five stages,
all single-shot, all fail-closed on error:

1. **Decode + size-cap** — base64-decode `GATE_SPEC`, reject if the
   decoded JSON exceeds `MAX_SPEC_DECODED_BYTES` (256 KiB), then
   `JSON.parse`.
2. **Pre-flight validation** — walk the predicate tree and throw on
   any unknown `type` discriminant. This catches version drift between
   a newer compiler and an older bundled `gate.js` before fact
   acquisition runs, so the failure mode is "loud" rather than "silent
   skip when the dependent fact is unavailable". Deliberately runs
   **before** `runBypass` so a malformed spec fails fast regardless of
   build reason.
3. **Bypass** — if `ADO_BUILD_REASON` does not match
   `spec.context.build_reason` (e.g. spec is for `PullRequest` but the
   build is `Manual`), auto-pass: emit `SHOULD_RUN=true`, tag the
   build, complete `Succeeded`, exit.
4. **Fact acquisition** — for every `FactSpec` in the spec, either
   read a pipeline env var (`isPipelineVarFact`) or call the ADO REST
   API (`pr_metadata`, `pr_labels`, `changed_files`, …). Each per-fact
   failure is recorded in the `PolicyTracker` and dispatched via that
   fact's `failure_policy` (`fail_closed` / `fail_open` /
   `skip_dependents`).
5. **Predicate evaluation** — for each `CheckSpec`, the
   `PolicyTracker` decides whether the check is `evaluate`, `pass`,
   `skip`, or `fail` based on which referenced facts are still
   available. Evaluator dispatches the predicate via the `switch` in
   `evaluatePredicate`. Failing checks emit `addBuildTag` and the
   overall `SHOULD_RUN` is `true` iff every check is `pass` or `skip`.

If `SHOULD_RUN` ends up `false`, `selfCancelIfRequested` issues a
best-effort `BuildStatus.Cancelling` PATCH so the pipeline run is
visibly cancelled in the ADO UI rather than just paused on a gated
job.

## Runtime env-var contract

The compiler injects these environment variables on the
`bash: node gate.js` step. `gate.js` reads them via
`process.env`:

| Env var | Source | Purpose |
|---|---|---|
| `GATE_SPEC` | compiled inline (base64) | The full `GateSpec` JSON |
| `SYSTEM_ACCESSTOKEN` | `$(System.AccessToken)` | ADO REST auth |
| `ADO_COLLECTION_URI` | `$(System.CollectionUri)` | ADO org base URL |
| `ADO_BUILD_REASON` | `$(Build.Reason)` | Used by the bypass branch |
| `ADO_BUILD_ID` | `$(Build.BuildId)` | Used for `selfCancelIfRequested` |
| `ADO_PROJECT` / `ADO_REPO_ID` / `ADO_PR_ID` | compiler-injected | PR-derived facts |
| `ADO_*` (fact-specific) | `Fact::ado_exports()` in Rust | Per-fact pipeline-variable readers (e.g. `ADO_PR_TITLE`, `ADO_SOURCE_BRANCH`) |
| `ADO_API_TIMEOUT_MS` | optional override | Per-attempt timeout for every ADO REST call. Default 30 000. On timeout, the call is retried once; if the retry also times out, the gate falls back to the per-fact `FailurePolicy`. |

The exact contract for pipeline-variable facts (which env var maps to
which `FactKind`) lives in **two places** that must stay in lockstep:

- Rust: `Fact::ado_exports()` in `src/compile/filter_ir.rs`
- TS: `ENV_BY_FACT` plus the `FactKind` union in
  `scripts/ado-script/src/shared/env-facts.ts`

The codegen drift check only mirrors the `GateSpec` *shape*, not the
env-var mapping, so when adding a new pipeline-variable fact you must
update both sides by hand. `Fact::ado_exports()` carries a docstring
pointing at the TS mirror as a reminder.

## Workspace layout

```
scripts/ado-script/
├── package.json                 # type:module; dep: azure-devops-node-api (lazy-imported)
├── tsconfig.json                # strict; noUncheckedIndexedAccess; NodeNext
├── src/
│   ├── shared/                  # Reusable across all bundles
│   │   ├── types.gen.ts         # AUTO-GENERATED from Rust IR — do not edit
│   │   ├── auth.ts              # WebApi factory; SDK is dynamic-imported here
│   │   ├── ado-client.ts        # azure-devops-node-api wrapper + retry + timeout + pagination
│   │   ├── env-facts.ts         # Pipeline-variable readers + ENV_BY_FACT + BRANCH_FACTS + ref-prefix stripping
│   │   ├── policy.ts            # PolicyTracker state machine
│   │   ├── vso-logger.ts        # ##vso[…] emitters with property/message escaping; complete() is idempotent
│   │   ├── git.ts               # execFile wrappers + bearerEnv helper (promoted from exec-context-pr/ in Stage 0)
│   │   ├── merge-base.ts        # synthetic-merge detection + progressive-deepening fetch (promoted from exec-context-pr/)
│   │   ├── validate.ts          # identifier regex guards (promoted from exec-context-pr/)
│   │   ├── prompt.ts            # agent-prompt-file append helpers (promoted from exec-context-pr/)
│   │   └── build.ts             # Build REST helpers (added in Stage 2; used by pipeline / ci-push / pr.checks)
│   ├── gate/                    # gate.js entry point + per-concern modules
│   │   ├── index.ts             # main(): decode → preflight → bypass → facts → eval → emit
│   │   ├── bypass.ts            # build-reason auto-pass
│   │   ├── facts.ts             # fact acquisition (env + REST)
│   │   ├── predicates.ts        # 11 predicate evaluators + validatePredicateTree + glob ReDoS hardening
│   │   └── selfcancel.ts        # best-effort build cancellation
│   ├── import/                  # import.js entry point + runtime prompt resolver
│   │   ├── index.ts             # main(): expand runtime-import markers in place
│   │   └── __tests__/           # marker, path-resolution, and single-pass coverage
│   ├── exec-context-pr/         # exec-context-pr.js entry point + PR precompute
│   │   ├── index.ts             # main(): validate → resolve merge-base → stage SHAs → append prompt
│   │   │                        # (imports validate/git/merge-base/prompt from ../shared/)
│   │   └── __tests__/           # end-to-end / integration tests live here; the
│   │                            # per-module unit tests moved with their modules
│   │                            # into ../shared/__tests__/
│   ├── exec-context-pr-synth/   # exec-context-pr-synth.js entry point + synthetic-PR resolver
│   │   ├── index.ts             # main(): real-PR / GitHub / synth-promote branch resolution → emit AW_PR_*
│   │   ├── match.ts             # branch/path include-exclude glob matching
│   │   ├── spec.ts              # PR_SYNTH_SPEC base64 decode + validation
│   │   └── __tests__/           # unit tests across the three modules
│   ├── exec-context-manual/     # exec-context-manual.js entry point + manual-context precompute
│   │   ├── index.ts             # main(): collect PARAM_* env vars → JSON snapshot → prompt fragment
│   │   └── __tests__/           # unit tests for success / failure / sanitisation paths
│   ├── exec-context-pipeline/   # exec-context-pipeline.js entry point + pipeline-completion precompute
│   │   ├── index.ts             # main(): validate TriggeredBy ids → fetch upstream Build via REST → stage + prompt
│   │   └── __tests__/           # unit tests for validate / success / failure / sanitisation paths
│   └── conclusion/              # conclusion.js entry point + Conclusion-job reporter
│       ├── index.ts             # main(): inspect upstream results + safe-outputs manifest → file/append work items
│       └── __tests__/           # unit tests for signal detection and work-item filing behaviour
├── test/                        # End-to-end smoke tests (gate, import, exec-context-pr)
├── gate.js                      # ncc bundle output (gitignored)
├── import.js                    # ncc bundle output (gitignored)
├── exec-context-pr.js           # ncc bundle output (gitignored)
├── exec-context-pr-synth.js     # ncc bundle output (gitignored)
├── exec-context-manual.js       # ncc bundle output (gitignored)
├── exec-context-pipeline.js     # ncc bundle output (gitignored)
└── conclusion.js                # ncc bundle output (gitignored)
```

The release workflow (`.github/workflows/release.yml`) runs
`npm ci && npm run build`, then zips `scripts/ado-script/gate.js`,
`scripts/ado-script/import.js`,
`scripts/ado-script/exec-context-pr.js`,
`scripts/ado-script/exec-context-pr-synth.js`,
`scripts/ado-script/exec-context-manual.js`, and
`scripts/ado-script/exec-context-pipeline.js`, and
`scripts/ado-script/conclusion.js` into the
`ado-script.zip` release asset. Pipelines download that asset at
runtime by URL pinned to the compiler's `CARGO_PKG_VERSION`, verify
its SHA-256 against the `checksums.txt` asset, then extract.

## Schema codegen

`types.gen.ts` is derived from the Rust IR via
[`schemars`](https://crates.io/crates/schemars) →
[`json-schema-to-typescript`](https://www.npmjs.com/package/json-schema-to-typescript):

```
┌──────────────────────────┐   schemars   ┌──────────────────────────┐
│ src/compile/filter_ir.rs │ ───────────► │ schema/gate-spec.schema  │
│ #[derive(JsonSchema)]    │              │     .json                │
└──────────────────────────┘              └────────────┬─────────────┘
                                                       │ json2ts
                                                       ▼
                                       ┌──────────────────────────────┐
                                       │ src/shared/types.gen.ts      │
                                       │ (consumed by gate/*.ts)      │
                                       └──────────────────────────────┘
```

`npm run codegen` runs both stages. The CI workflow
(`.github/workflows/ado-script.yml`) regenerates the file and runs
`git diff --exit-code` to fail on drift, on both PRs and pushes to
`main`. If you change the IR shape in Rust, run
`cd scripts/ado-script && npm run codegen` and commit the regenerated
`types.gen.ts`.

The Rust subcommand that emits the schema is intentionally hidden:

```sh
cargo run -- export-gate-schema --output schema/gate-spec.schema.json
```

## How the bundles are wired into emitted pipelines

`AdoScriptExtension`
(`src/compile/extensions/ado_script.rs`) is the always-on single
extension that owns all `ado-script` wiring. It has two independent
features, each emitted **into the job that actually consumes the
bundle**:

### Setup job (gate evaluator)

When `filters:` lowers to non-empty checks, `AdoScriptExtension::declarations()`
returns three typed `Declarations::setup_steps` entries for the Setup job:

1. **`UseNode@1`** — installs Node 22.x LTS, capped at
   `timeoutInMinutes: 5`.
2. **`curl` download + verify + extract** — fetches `checksums.txt`
   and `ado-script.zip` from the `githubnext/ado-aw` release matching
   `CARGO_PKG_VERSION`, verifies the zip's SHA-256, then
   `unzip -o /tmp/ado-aw-scripts/ado-script.zip -d /tmp/ado-aw-scripts/`.
   Also capped at `timeoutInMinutes: 5`.
3. **`bash: node '/tmp/ado-aw-scripts/ado-script/gate.js'`** —
   runs the gate with `GATE_SPEC` and the env-var contract documented
   above.

### Agent job (runtime-import resolver + PR-context precompute)

When `inlined-imports: false` (the default) OR the execution-context
PR contributor activates (`on.pr` configured and not disabled),
`AdoScriptExtension::declarations()` returns the install + download pair in
`Declarations::agent_prepare_steps` for the Agent job:

1. **`UseNode@1`** — same shape as above.
2. **`curl` download + verify + extract** — same artefact, same
   verification.
3. **`bash: node '/tmp/ado-aw-scripts/ado-script/import.js'`** —
   expands `{{#runtime-import …}}` markers in
   `/tmp/awf-tools/agent-prompt.md` in place. See
   [`runtime-imports.md`](runtime-imports.md) for marker syntax.
   **Only emitted when `inlined-imports: false`.**

The PR-context precompute step (`node exec-context-pr.js`) is owned
by `ExecContextExtension` (not `AdoScriptExtension`) and emitted through
its own Tool-phase `Declarations::agent_prepare_steps`. Phase ordering
(`AdoScriptExtension::phase() == System` < `ExecContextExtension::phase() == Tool`)
guarantees the bundle is installed and on disk before the
exec-context invocation runs.

### Per-job download (NOT a duplication bug)

ADO jobs use **isolated VMs** — `/tmp` is not shared between jobs.
The `ado-script.zip` bundle therefore has to be downloaded once per
job that consumes it. When both Setup and Agent need it, install +
download steps appear in **both**. That's correct architecture given
ADO's topology, not waste.

### What gets emitted, by case

The rows below assume the synthetic-PR resolver is **not** active
(`pr_trigger_for_synth = None`):

| Setup consumer | Agent consumer | Setup-job steps | Agent-job extra steps |
|---|---|---|---|
| no gate    | none                                   | (none)                              | (none)                              |
| no gate    | `inlined-imports: false` only          | (no Setup job)                      | install + download + resolver       |
| no gate    | `on.pr` execution-context only         | (no Setup job)                      | install + download + exec-context-pr |
| no gate    | both                                   | (no Setup job)                      | install + download + resolver + exec-context-pr |
| gate       | none                                   | install + download + gate           | (none)                              |
| gate       | any combination of resolver / exec-pr  | install + download + gate           | install + download + (resolver?) + (exec-context-pr?) |

When the synthetic-PR resolver **is** active
(`pr_trigger_for_synth = Some(_)`, i.e. `synthetic_pr_active()` is
true) the Setup job gains the `synthPr` step (`node
exec-context-pr-synth.js`) before any gate step — and the Setup job is
emitted even with no gate:

| Setup consumer | Setup-job steps | Agent-job extra steps |
|---|---|---|
| synth-PR (no gate) | install + download + synth-PR | (per Agent consumer above) |
| gate (no synth-PR) | install + download + gate | (per Agent consumer above) |
| synth-PR + gate    | install + download + synth-PR + gate | (per Agent consumer above) |

The "Setup consumer" column is gated on `filters:` lowering to non-empty
checks **or** `synthetic_pr_active()` being true. The "Agent consumer"
columns are gated on `inlined-imports: false` (resolver) and the PR
contributor's activation predicate (exec-context-pr; see
`pr_contributor_will_activate` in
`src/compile/extensions/exec_context/mod.rs`).

The IR-to-bash codegen that produces the gate step is
`compile_gate_step_external` in `src/compile/filter_ir.rs`.

## Modifying `ado-script`

### Add a new predicate

1. Add a `Predicate` + `PredicateSpec` variant in
   `src/compile/filter_ir.rs`. Run `cargo test` and update spec tests.
2. In `scripts/ado-script/`, run `npm run codegen` so `types.gen.ts`
   picks up the new variant.
3. Add a `case` to the `switch` in
   `src/gate/predicates.ts::evaluatePredicate`.
4. Add the new type name to `KNOWN_PREDICATE_TYPES` (right above the
   `validatePredicateTree` function). **Both updates are required** —
   the drift test
   `KNOWN_PREDICATE_TYPES stays in sync with evaluatePredicate switch`
   in `predicates.test.ts` will fail if you forget either.
5. Add a vitest case under
   `src/gate/__tests__/ports/<new-predicate>.test.ts`.

### Add a new pipeline-variable fact

1. Add a `Fact` variant in `src/compile/filter_ir.rs` and update
   `Fact::ado_exports()`. (Its docstring reminds you about step 3.)
2. `npm run codegen` to regenerate types.
3. Add an entry to `ENV_BY_FACT` and extend the `FactKind` union in
   `scripts/ado-script/src/shared/env-facts.ts`. Without this step the
   gate silently treats the fact as missing.
4. If the fact value is ref-shaped (e.g. a branch name), add it to
   the exported `BRANCH_FACTS` set so the read-time strip is applied.

### Add a new bundle (e.g. `poll.js`)

1. Create `src/poll/index.ts` and supporting modules under
   `scripts/ado-script/src/poll/`. Reuse anything in `src/shared/`.
2. Add a build script to `package.json`:
   ```json
   "build:poll": "ncc build src/poll/index.ts -o .ado-build/poll -m -t && node -e \"const fs=require('node:fs'); fs.copyFileSync('.ado-build/poll/index.js','poll.js'); fs.rmSync('.ado-build/poll',{recursive:true,force:true});\""
   ```
   and extend `build` to also run it.
3. Add vitest tests under `src/poll/__tests__/`.
4. Wire from a new `CompilerExtension` (or extend an existing one)
   that downloads `ado-script.zip` (already a release asset) and
   invokes `node /tmp/ado-aw-scripts/ado-script/poll.js`
   as a runtime step.
5. Update release packaging to include `scripts/ado-script/poll.js`
   in `ado-script.zip` alongside other bundles.

### Local development loop

From `scripts/ado-script/`:

```sh
npm ci                 # one-time
npm run codegen        # regenerate types.gen.ts (compiles ado-aw first)
npm test               # vitest unit tests
npm run typecheck      # strict tsc --noEmit
npm run build          # ncc-bundle to gate.js
npm run test:smoke     # build + smoke test the bundle end-to-end
```

The Rust-side E2E gate test compiles a real agent, extracts the
emitted `GATE_SPEC`, and shells out to the bundled `gate.js`:

```sh
cargo test --test gate_e2e -- --ignored --nocapture
```

## Bundle-size budget

Each bundled artifact must stay **under 5 MB**. The entry-point
chunk for `gate.js` is ~78 KB; the lazy-imported
`azure-devops-node-api` SDK lives in a separate ~2.7 MB chunk loaded
only when an ADO REST call is needed. Pipelines that bypass or rely
only on pipeline-variable facts never load the SDK.

If a future bundle blows the budget:

- First, check ncc's `--minify` and `--target` flags.
- If still too large, weigh dropping `azure-devops-node-api` in favor
  of hand-rolled `fetch` for the hot endpoints. The retry / timeout /
  pagination helpers in `src/shared/ado-client.ts` are written so
  they could wrap either approach.

## Out of scope (explicitly)

- A user-facing `ado-script:` front-matter block. Letting authors run
  arbitrary TypeScript at pipeline runtime would bypass the
  safe-output trust boundary and require sandboxing the project does
  not have.
- Migrating the safe-output executors (`src/safeoutputs/*.rs`) to
  Node. Stage 3 keeps a Rust-only execution path.
- Migrating the agent-stats parser. It runs in-pipeline as part of
  Stage 1 wrap-up and has no TypeScript dependency need.
- Bundling Node itself. Pipelines install Node via `UseNode@1`.

## See also

- [`filter-ir.md`](filter-ir.md) — the IR consumed by `gate.js`.
- [`extending.md`](extending.md) — generic compiler-extension guide.
