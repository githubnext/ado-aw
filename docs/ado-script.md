# `ado-script`: bundled TypeScript runtime helpers

`ado-script` is the umbrella name for the TypeScript workspace at
[`scripts/ado-script/`](../scripts/ado-script/). It produces small,
ncc-bundled Node programs that the **compiler injects into every emitted
pipeline** as runtime helpers. Today it produces `gate.js`, the
trigger-filter gate evaluator, and `import.js`, the runtime prompt
resolver described in [`runtime-imports.md`](runtime-imports.md).

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

The bundle lives at `dist/import/index.js` and ships in the same
`ado-script.zip` release asset as `gate.js`, so pipelines download it
through the same Setup-job asset flow. `import.js` uses only the Node
standard library, so the ncc bundle is small (~1.5 KB) and carries no
SDK dependency.

The Stage-2 threat-analysis prompt is **not** runtime-imported.
`src/data/threat-analysis.md` is `include_str!`'d into the `ado-aw`
binary and inlined into the emitted YAML at compile time, matching
gh-aw's pattern (their `threat_detection.md` ships with the setup
action and is read directly from disk — no marker, no resolver).

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
       │   1. NodeTool@0      │
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
`bash: node gate/index.js` step. `gate.js` reads them via
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
│   │   └── vso-logger.ts        # ##vso[…] emitters with property/message escaping; complete() is idempotent
│   ├── gate/                    # gate.js entry point + per-concern modules
│   │   ├── index.ts             # main(): decode → preflight → bypass → facts → eval → emit
│   │   ├── bypass.ts            # build-reason auto-pass
│   │   ├── facts.ts             # fact acquisition (env + REST)
│   │   ├── predicates.ts        # 11 predicate evaluators + validatePredicateTree + glob ReDoS hardening
│   │   └── selfcancel.ts        # best-effort build cancellation
│   └── import/                  # import.js entry point + runtime prompt resolver
│       ├── index.ts             # main(): expand runtime-import markers in place
│       └── __tests__/           # marker, path-resolution, and single-pass coverage
├── test/                        # End-to-end smoke tests
└── dist/                        # ncc bundle output (gitignored)
    ├── gate/index.js
    └── import/index.js
```

The release workflow (`.github/workflows/release.yml`) runs
`npm ci && npm run build`, then zips `scripts/ado-script/dist/` into
the `ado-script.zip` release asset. Pipelines download that asset at
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

When `filters:` lowers to non-empty checks, `setup_steps()` returns
three step strings into the Setup job:

1. **`NodeTool@0`** — installs Node 20.x LTS, capped at
   `timeoutInMinutes: 5`.
2. **`curl` download + verify + extract** — fetches `checksums.txt`
   and `ado-script.zip` from the `githubnext/ado-aw` release matching
   `CARGO_PKG_VERSION`, verifies the zip's SHA-256, then
   `unzip -o /tmp/ado-aw-scripts/ado-script.zip -d /tmp/ado-aw-scripts/`.
   Also capped at `timeoutInMinutes: 5`.
3. **`bash: node '/tmp/ado-aw-scripts/ado-script/dist/gate/index.js'`** —
   runs the gate with `GATE_SPEC` and the env-var contract documented
   above.

### Agent job (runtime-import resolver)

When `inlined-imports: false` (the default), `prepare_steps()` returns
the same install + download pair plus the resolver invocation, into
the Agent job's existing `{{ prepare_steps }}` block:

1. **`NodeTool@0`** — same shape as above.
2. **`curl` download + verify + extract** — same artefact, same
   verification.
3. **`bash: node '/tmp/ado-aw-scripts/ado-script/dist/import/index.js'`** —
   expands `{{#runtime-import …}}` markers in
   `/tmp/awf-tools/agent-prompt.md` in place. See
   [`runtime-imports.md`](runtime-imports.md) for marker syntax.

### Per-job download (NOT a duplication bug)

ADO jobs use **isolated VMs** — `/tmp` is not shared between jobs.
The `ado-script.zip` bundle therefore has to be downloaded once per
job that consumes it. When both features are active (a pipeline with
both `filters:` and `inlined-imports: false`), install + download
steps appear in **both** Setup and Agent. That's correct architecture
given ADO's topology, not waste.

### What gets emitted, by case

| `filters:` | `inlined-imports` | Setup-job steps | Agent-job extra steps |
|---|---|---|---|
| inactive   | `true`  | (none)                              | (none)                              |
| inactive   | `false` | (no Setup job)                      | install + download + resolver       |
| active     | `true`  | install + download + gate           | (none)                              |
| active     | `false` | install + download + gate           | install + download + resolver       |

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
   "build:poll": "ncc build src/poll/index.ts -o dist/poll -m -t"
   ```
   and extend `build` to also run it.
3. Add vitest tests under `src/poll/__tests__/`.
4. Wire from a new `CompilerExtension` (or extend an existing one)
   that downloads `ado-script.zip` (already a release asset) and
   invokes `node /tmp/ado-aw-scripts/ado-script/dist/poll/index.js`
   as a runtime step.
5. No release-workflow change is needed — `zip -r ado-script/dist`
   picks up the new bundle automatically.

### Local development loop

From `scripts/ado-script/`:

```sh
npm ci                 # one-time
npm run codegen        # regenerate types.gen.ts (compiles ado-aw first)
npm test               # vitest unit tests
npm run typecheck      # strict tsc --noEmit
npm run build          # ncc-bundle to dist/gate/index.js
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
- Bundling Node itself. Pipelines install Node via `NodeTool@0`.

## See also

- [`filter-ir.md`](filter-ir.md) — the IR consumed by `gate.js`.
- [`extending.md`](extending.md) — generic compiler-extension guide.
