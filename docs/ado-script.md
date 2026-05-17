# ado-script: Bundled TypeScript scripts for ado-aw

`ado-script` is the umbrella name for **internal**, compiler-targeted
TypeScript bundles that ado-aw emits into compiled pipelines as runtime
helpers. The first (and currently only) bundle is **`gate.js`**, the
trigger-filter gate evaluator.

> Internal-only: `ado-script` is not a user-facing front-matter feature.
> Authors do **not** write `ado-script:` blocks in their agent markdown.
> The compiler decides when an `ado-script` bundle is needed and how to
> wire it.

## Decision: Bundled Node, not a Rust subcommand (Variant A2)

We chose to ship gate evaluation logic as a **bundled Node.js artifact
built from a TypeScript workspace** rather than:

- **A1: an `ado-aw` subcommand** (`ado-aw gate-eval --spec=…`). This
  was rejected because:
  - The `ado-aw` binary's role is the compiler. Folding pipeline-runtime
    logic into the compiler binary expands its blast radius and forces
    every pipeline runner to download the full compiler.
  - We want to use the mature
    [`azure-devops-node-api`](https://www.npmjs.com/package/azure-devops-node-api)
    SDK for ADO REST calls. Re-implementing equivalent Rust clients (or
    embedding a Node interpreter inside the Rust binary) is a worse
    cost/benefit trade.
  - Per-use-site Node bundles compose cleanly: each emitted helper
    (`gate.js` today, possibly `poll.js` or `stats.js` tomorrow) is a
    self-contained `dist/` artifact with no shared runtime state.

- **B: a user-facing `ado-script:` front-matter block** that lets agent
  authors run arbitrary TypeScript at pipeline runtime. Out of scope —
  separate RFC if ever pursued. Allowing user-supplied scripts would
  bypass our safe-output policy and require sandboxing we don't yet
  have.

The full design walkthrough that produced this decision lives at
[`ado-script-design.md`](../ado-script-design.md).

## Architecture

```
scripts/ado-script/                # TS workspace
├── package.json                   # type:module, deps: azure-devops-node-api
├── tsconfig.json                  # NodeNext, ESNext target
├── src/
│   ├── shared/                    # Reusable across all bundles
│   │   ├── types.gen.ts           # AUTO-GENERATED from Rust IR
│   │   ├── auth.ts                # ADO token / collection URI plumbing
│   │   ├── ado-client.ts          # azure-devops-node-api wrapper + retries
│   │   ├── env-facts.ts           # Pipeline-variable readers
│   │   ├── policy.ts              # Failure-policy state machine
│   │   └── vso-logger.ts          # ##vso[…] command emitters
│   └── gate/                      # gate.js entry point
│       ├── index.ts               # main()
│       ├── bypass.ts              # build-reason auto-pass
│       ├── facts.ts               # fact acquisition (env + REST)
│       ├── predicates.ts          # 11 predicate evaluators
│       └── selfcancel.ts          # best-effort build cancellation
├── test/                          # End-to-end smoke tests
└── dist/gate/index.js             # ncc-bundled output (gitignored)
```

The release workflow (`.github/workflows/release.yml`) runs `npm ci &&
npm run build`, then packages `scripts/ado-script/dist/` as the
`ado-script.zip` release asset that pipelines download at runtime.

## Schema codegen — preventing drift

The TypeScript `GateSpec` types are **not** hand-written. They are
derived from the Rust IR in `src/compile/filter_ir.rs` using the
[`schemars`](https://crates.io/crates/schemars) crate, then converted to
TypeScript via
[`json-schema-to-typescript`](https://www.npmjs.com/package/json-schema-to-typescript).

The pipeline:

```
┌───────────────────────────┐    JsonSchema    ┌─────────────────────┐
│ src/compile/filter_ir.rs  │  ───────────►   │  schema/gate-spec.  │
│ (Rust IR types with       │   schemars       │      schema.json    │
│  #[derive(JsonSchema)])   │                  └─────────────────────┘
└───────────────────────────┘                           │
                                              json-schema-to-typescript
                                                        ▼
                                        ┌──────────────────────────────┐
                                        │ src/shared/types.gen.ts      │
                                        │ (consumed by gate/*.ts)      │
                                        └──────────────────────────────┘
```

`npm run codegen` runs both stages. The `ado-script` CI workflow
(`.github/workflows/ado-script.yml`) regenerates the file and runs
`git diff --exit-code` to fail on drift. If you change the IR shape in
Rust, you must run `cd scripts/ado-script && npm run codegen` and
commit the regenerated `types.gen.ts`.

The Rust subcommand that emits the schema is intentionally hidden:

```sh
cargo run -- export-gate-schema --output schema/gate-spec.schema.json
```

## How the gate bundle is wired into emitted pipelines

The `TriggerFiltersExtension`
(`src/compile/extensions/trigger_filters.rs`) injects three Setup-job
steps when any `filters:` block is active:

1. **`NodeTool@0`** — installs Node 20.x LTS (preinstalled on
   Microsoft-hosted images; pinned for reproducibility on others).
2. **`curl` download** — fetches `ado-script.zip` from the
   `githubnext/ado-aw` release matching the compiler's version and
   extracts `ado-script/dist/gate/index.js` to
   `/tmp/ado-aw-scripts/ado-script/dist/gate/index.js`.
3. **`bash: node '/tmp/ado-aw-scripts/ado-script/dist/gate/index.js'`** — runs the gate with
   `GATE_SPEC` (base64 JSON) plus required pipeline env vars.

The IR-to-bash codegen lives in `compile_gate_step_external`
(`src/compile/filter_ir.rs:~1100`).

### Runtime env-var contract

The gate reads the following at runtime (in addition to the
predicate-specific `ADO_*` facts emitted by `collect_ado_exports`):

| Env var | Source | Purpose |
|---|---|---|
| `GATE_SPEC` | compiled inline | Base64-encoded `GateSpec` JSON |
| `SYSTEM_ACCESSTOKEN` | `$(System.AccessToken)` | ADO REST auth |
| `ADO_COLLECTION_URI` | `$(System.CollectionUri)` | ADO org base URL |
| `ADO_PROJECT` / `ADO_REPO_ID` / `ADO_PR_ID` | compiler-injected | PR-derived facts |
| `ADO_API_TIMEOUT_MS` | optional override | Per-attempt timeout in ms for every ADO REST call. Defaults to 30 000 (30 s). On timeout, the call is retried once; if the retry also times out, the gate falls back to the per-fact `FailurePolicy`. |

## Adding a new internal use site

Suppose we want a `poll.js` bundle (e.g. for polling external systems):

1. Create `src/poll/index.ts` and supporting modules in
   `scripts/ado-script/src/poll/`. Reuse anything in `src/shared/`.
2. Add a build script to `package.json`:
   ```json
   "build:poll": "ncc build src/poll/index.ts -o dist/poll -m -t",
   ```
   and extend `build` to also run it and copy `dist/poll/index.js` to
   `../poll.js`.
3. Add tests under `src/poll/__tests__/`.
4. Wire from a new `CompilerExtension` (or extend an existing one) that
   downloads and invokes `poll.js` as a runtime step.
5. Update `.github/workflows/release.yml` if the zip exclusion list
   needs to include the new `dist/poll` directory.

## Bundle-size budget

Each bundled artifact must stay **under 5 MB**. The current `gate.js` is
~1.1 MB, dominated by `azure-devops-node-api`. If a future bundle blows
the budget:

- First, check ncc's `--minify` and `--target` flags.
- If still too large, weigh dropping the SDK in favor of hand-rolled
  `fetch` for the hot endpoints we use. The retry/error helpers in
  `src/shared/ado-client.ts` are written so they could wrap either
  approach.

## Out of scope (explicitly)

- A user-facing `ado-script:` front-matter block. Letting authors run
  arbitrary TypeScript at pipeline runtime is a separate RFC.
- Migrating the safe-output executors (`src/safeoutputs/*.rs`) to Node.
  Stage 3 keeps a Rust-only execution path.
- Migrating the agent-stats parser. It runs in-pipeline as part of
  Stage 1 wrap-up and has no TypeScript dependency need.
- Bundling Node itself. Pipelines install Node via `NodeTool@0`.

## See also

- [`filter-ir.md`](filter-ir.md) — the IR consumed by `gate.js`.
- [`extending.md`](extending.md) — generic compiler-extension guide.
- [`../ado-script-design.md`](../ado-script-design.md) — original design
  doc that produced the A2 decision recorded here.
