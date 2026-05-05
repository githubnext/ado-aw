# ado-script: Bundled TypeScript scripts for ado-aw

`ado-script` is the umbrella name for **internal**, compiler-targeted
TypeScript bundles that ado-aw emits into compiled pipelines as runtime
helpers. There are currently two bundles:

- **`gate.js`** — the trigger-filter gate evaluator (Setup job).
- **`prompt.js`** — the runtime prompt renderer that reads the agent
  `.md` from the workspace, strips front matter, applies variable
  substitution, and assembles the prompt with extension supplements
  (Agent job; default behaviour as of v0.22).

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
│   │   ├── types.gen.ts           # AUTO-GENERATED from Rust IR (gate)
│   │   ├── types-prompt.gen.ts    # AUTO-GENERATED from Rust IR (prompt)
│   │   ├── auth.ts                # ADO token / collection URI plumbing
│   │   ├── ado-client.ts          # azure-devops-node-api wrapper + retries
│   │   ├── env-facts.ts           # Pipeline-variable readers
│   │   ├── policy.ts              # Failure-policy state machine
│   │   └── vso-logger.ts          # ##vso[…] command emitters
│   ├── gate/                      # gate.js entry point
│   │   ├── index.ts               # main()
│   │   ├── bypass.ts              # build-reason auto-pass
│   │   ├── facts.ts               # fact acquisition (env + REST)
│   │   ├── predicates.ts          # 11 predicate evaluators
│   │   └── selfcancel.ts          # best-effort build cancellation
│   └── prompt/                    # prompt.js entry point
│       ├── index.ts               # main()
│       ├── frontmatter.ts         # YAML front-matter stripper
│       └── substitute.ts          # ${{ parameters.* }} / $(VAR) substitution
├── test/                          # End-to-end smoke tests (gate + prompt)
└── dist/                          # ncc-bundled output (gitignored)
    ├── gate/index.js
    └── prompt/index.js
```

The release workflow (`.github/workflows/release.yml`) runs `npm ci &&
npm run build` and copies `dist/gate/index.js` to `scripts/gate.js` and
`dist/prompt/index.js` to `scripts/prompt.js`, both of which are then
included in the `scripts.zip` release asset that pipelines download at
runtime.

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

`npm run codegen` runs the full pipeline: it exports both the
`gate-spec.schema.json` and `prompt-spec.schema.json` from Rust, then
runs `json2ts` to regenerate `src/shared/types.gen.ts` (gate) and
`src/shared/types-prompt.gen.ts` (prompt). The `ado-script` CI workflow
(`.github/workflows/ado-script.yml`) regenerates both files and runs
`git diff --exit-code` to fail on drift. If you change the IR shape in
Rust, you must run `cd scripts/ado-script && npm run codegen` and
commit the regenerated `*.gen.ts` files.

The Rust subcommands that emit the schemas are intentionally hidden:

```sh
cargo run -- export-gate-schema   --output schema/gate-spec.schema.json
cargo run -- export-prompt-schema --output schema/prompt-spec.schema.json
```

## How the gate bundle is wired into emitted pipelines

The `TriggerFiltersExtension`
(`src/compile/extensions/trigger_filters.rs`) injects three Setup-job
steps when any `filters:` block is active:

1. **`NodeTool@0`** — installs Node 20.x LTS (preinstalled on
   Microsoft-hosted images; pinned for reproducibility on others).
2. **`curl` download** — fetches `scripts.zip` from the
   `githubnext/ado-aw` release matching the compiler's version and
   extracts `gate.js` to `/tmp/ado-aw-scripts/gate.js`.
3. **`bash: node '/tmp/ado-aw-scripts/gate.js'`** — runs the gate with
   `GATE_SPEC` (base64 JSON) plus required pipeline env vars.

The IR-to-bash codegen lives in `compile_gate_step_external`
(`src/compile/filter_ir.rs:~1100`).

## How the prompt bundle is wired into emitted pipelines

`generate_prepare_agent_prompt` in `src/compile/common.rs` injects a
parallel three-step bundle into the **Agent job** when
`inlined-imports: false` (the default):

1. **`NodeTool@0`** — same Node 20.x install as for `gate.js`.
   Idempotent across multiple invocations in the same job.
2. **`curl` download** — fetches `scripts.zip` and extracts
   `prompt.js` to `/tmp/ado-aw-scripts/prompt.js`. Each pool agent
   downloads its own copy; the Setup and Agent jobs run on independent
   agents so the download isn't shared.
3. **`bash: node '/tmp/ado-aw-scripts/prompt.js'`** — runs the renderer
   with `ADO_AW_PROMPT_SPEC` (base64 JSON of the `PromptSpec`) plus one
   `ADO_AW_PARAM_<NAME>: ${{ parameters.<NAME> }}` env per declared
   parameter.

When `inlined-imports: true`, the same helper instead emits the legacy
heredoc step that embeds the prompt body and supplements directly into
the YAML; `prompt.js` is not invoked.

Both download steps share the helper `scripts_download_step()` in
`src/compile/extensions/mod.rs` so URL/version stay in lockstep.

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
3. Add tests under `src/poll/__tests__/` (unit) and `test/` (smoke,
   gated on `dist/poll/index.js` existing).
4. Wire from a new `CompilerExtension` (or extend an existing one) that
   downloads and invokes `poll.js` as a runtime step. Use the shared
   `scripts_download_step()` and `node_tool_step()` helpers.
5. If the contract is non-trivial, follow the `gate.js` /
   `prompt.js` pattern: define a `Spec` struct in Rust with
   `#[derive(Serialize, JsonSchema)]`, add a hidden
   `export-poll-schema` CLI, and extend `npm run codegen` to regenerate
   `types-poll.gen.ts`. For trivial contracts (a couple of env vars),
   hand-written types are fine.
6. Update `.github/workflows/release.yml` if the zip exclusion list
   needs to include the new `dist/poll` directory.

## Bundle-size budget

Each bundled artifact must stay **under 5 MB**. Current sizes:

- `gate.js` is ~1.1 MB, dominated by `azure-devops-node-api`.
- `prompt.js` is ~5 KB (no SDK dependency).

If a future bundle blows the budget:

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
