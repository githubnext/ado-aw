# `ado-script`: bundled TypeScript runtime helpers

`ado-script` is the umbrella name for the TypeScript workspace at
[`scripts/ado-script/`](../scripts/ado-script/). It produces small,
ncc-bundled Node programs that the **compiler injects into every emitted
pipeline** as runtime helpers. The current bundles are:

- **`gate.js`** — the trigger-filter gate evaluator (Setup job).
- **`prompt.js`** — the agent prompt renderer (Agent job). Reads the
  agent `.md` from the workspace at runtime, strips its front matter,
  runs single-pass variable substitution, and writes the rendered
  prompt for the AWF sandbox. See *What `prompt.js` does* below.

> **Internal-only.** `ado-script` is not a user-facing front-matter
> feature. Authors never write an `ado-script:` block in their agent
> markdown. The compiler decides when an `ado-script` bundle is needed
> and how to wire it. See [`docs/tools.md`](tools.md) for what *is*
> user-facing. The one user-visible knob is
> [`inlined-imports: true`](front-matter.md) which opts back into the
> legacy compile-time prompt-embedding behaviour and skips
> `prompt.js`.

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
│   │   ├── types.gen.ts         # AUTO-GENERATED from GateSpec  — do not edit
│   │   ├── types-prompt.gen.ts  # AUTO-GENERATED from PromptSpec — do not edit
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
│   └── prompt/                  # prompt.js entry point + per-concern modules
│       ├── index.ts             # main(): decode → strip FM → assemble → substitute → write
│       ├── frontmatter.ts       # stripFrontMatter (mirrors parse_markdown_detailed in Rust)
│       └── substitute.ts        # single-pass substitution engine (block-the-chain attack)
├── test/                        # End-to-end smoke tests for built bundles
└── dist/<bundle>/index.js       # ncc bundle output per bundle (gitignored)
```

The release workflow (`.github/workflows/release.yml`) runs
`npm ci && npm run build`, then **flattens** each `dist/<bundle>/index.js`
into a top-level `<bundle>.js` inside `ado-script.zip` (e.g. `gate.js`,
`prompt.js`). Pipelines download that asset at runtime by URL pinned to
the compiler's `CARGO_PKG_VERSION`, verify its SHA-256 against the
`checksums.txt` asset, then extract directly into `/tmp/ado-aw-scripts/`,
where each bundle is referenced by `/tmp/ado-aw-scripts/<bundle>.js`.

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

`npm run codegen` runs both schemas: `codegen:gate` regenerates
`types.gen.ts` from `GateSpec`, and `codegen:prompt` regenerates
`types-prompt.gen.ts` from `PromptSpec`. The CI workflow
(`.github/workflows/ado-script.yml`) regenerates **both** files and
runs `git diff --exit-code` to fail on drift, on both PRs and pushes
to `main`. If you change either IR shape in Rust, run
`cd scripts/ado-script && npm run codegen` and commit the regenerated
type files.

The Rust subcommands that emit the schemas are intentionally hidden:

```sh
cargo run -- export-gate-schema   --output schema/gate-spec.schema.json
cargo run -- export-prompt-schema --output schema/prompt-spec.schema.json
```

## How the gate bundle is wired into emitted pipelines

`TriggerFiltersExtension`
(`src/compile/extensions/trigger_filters.rs`) declares
`needs_scripts_bundle() == true` when any `filters:` block produces
checks. The compiler emits the shared install pair (NodeTool@0 +
checksum-verified `ado-script.zip` download) **once per job**:

- **Setup job** — the install pair is hoisted out of the extension via
  `compile/extensions/mod.rs::scripts_install_steps_if_needed`. The
  trigger-filters extension then contributes only the gate step.
- **Agent job** — the runtime prompt path (when
  `inlined-imports: false`, the default) emits its own copy of the
  install pair via `generate_prepare_agent_prompt`. The Setup job's
  download is on a different ADO agent VM, so the Agent VM must
  re-download.

The wiring for trigger filters specifically:

1. **`NodeTool@0`** — installs Node 20.x LTS, capped at
   `timeoutInMinutes: 5`.
2. **`curl` download + verify + extract** — fetches `checksums.txt`
   and `ado-script.zip` from the `githubnext/ado-aw` release matching
   `CARGO_PKG_VERSION`, verifies the zip's SHA-256, then
   `unzip -o /tmp/ado-aw-scripts/ado-script.zip -d /tmp/ado-aw-scripts/`.
   Also capped at `timeoutInMinutes: 5`.
3. **`bash: node '/tmp/ado-aw-scripts/gate.js'`** —
   runs the gate with `GATE_SPEC` and the env-var contract above.

The IR-to-bash codegen that produces step 3 is
`compile_gate_step_external` in `src/compile/filter_ir.rs`.

## What `prompt.js` does

`prompt.js` is a single-shot Node program that runs in the **Agent
job**, before the agent is launched. It:

1. Decodes the base64-encoded [`PromptSpec`](../src/compile/prompt_ir.rs)
   from `ADO_AW_PROMPT_SPEC` and refuses to run on a mismatched
   schema version.
2. Reads the agent `.md` source from the workspace at the absolute
   path baked into the spec (already resolved from
   `{{ trigger_repo_directory }}` at compile time, so the spec carries
   a literal `$(Build.SourcesDirectory)/path/to/agent.md`).
3. Strips the YAML front-matter block (mirroring
   `parse_markdown_detailed` in Rust).
4. Joins the body with any `PromptSpec.supplements` contributed by
   extensions, in the same order
   [`generate_prepare_steps`](../src/compile/common.rs) would have
   emitted them in `inlined-imports: true` mode (Runtimes phase first,
   then Tools).
5. Runs **single-pass** substitution over the joined content. The
   single regex pass recognises four token shapes, with replacement
   values returned verbatim and **never re-scanned**:

   | Token                          | Resolved via                                    | Notes                                                |
   |--------------------------------|-------------------------------------------------|------------------------------------------------------|
   | `\$(VAR)` / `\$(VAR.SUB)`      | escape                                          | Backslash stripped; `$(VAR)` stays literal.          |
   | `${{ parameters.NAME }}`       | `ADO_AW_PARAM_<NAME upper, hyphen→underscore>`  | Only parameters listed in the spec substitute.       |
   | `$(VAR)` / `$(VAR.SUB)`        | `<NAME upper, dot→underscore>` (process env)    | Unset vars left verbatim with a warning.             |
   | `$[ ... ]`                     | not substituted                                 | Left verbatim with one warning per render.           |

   **Single-pass is load-bearing.** It blocks the
   "queue-with-malicious-parameter-value" chaining attack: if a caller
   queues with `target = "$(System.AccessToken)"`, the substituted
   value lands in the rendered prompt as the literal string
   `$(System.AccessToken)` — not the access token itself. Same applies
   in reverse: a `$(VAR)` value containing `${{ parameters.* }}` is
   never re-expanded.

6. Writes the rendered prompt to `/tmp/awf-tools/agent-prompt.md` for
   the AWF sandbox.

Like `gate.js`, `prompt.js` is a data interpreter, not a code
evaluator — there is no `eval`, no `Function`, no `vm`. A compromised
compiler cannot use the spec to execute arbitrary code on the agent
runner.

### Opt-out: `inlined-imports: true`

Set `inlined-imports: true` in front matter to skip `prompt.js`
entirely and restore the legacy compile-time behaviour: the body is
embedded verbatim in a heredoc step at compile time, and extension
supplements are emitted as per-extension `cat >>` steps. Use this
when:

- The agent `.md` source path will not be resolvable inside
  `$(Build.SourcesDirectory)` at runtime (e.g., compile happens
  outside the trigger repo).
- The Agent pool cannot reach `github.com` for the release-asset
  download.
- You need a fully self-contained compiled YAML for offline review or
  archival.

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
   invokes `node /tmp/ado-aw-scripts/poll.js`
   as a runtime step.
5. Extend the release workflow's package step in
   `.github/workflows/release.yml` — the flatten loop iterates over
   every `dist/*/index.js`, so a new bundle is picked up automatically
   as long as its build step writes to `dist/<name>/index.js`.

### Local development loop

From `scripts/ado-script/`:

```sh
npm ci                 # one-time
npm run codegen        # regenerate types.gen.ts (compiles ado-aw first)
npm test               # vitest unit tests
npm run typecheck      # strict tsc --noEmit
npm run build          # ncc-bundle each src/<bundle>/index.ts to dist/<bundle>/index.js
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
