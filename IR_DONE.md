# Native ADO Pipeline IR — work done so far

> **Branch:** `native-ado-compiler` &nbsp;·&nbsp; **Draft PR:** [#960](https://github.com/githubnext/ado-aw/pull/960) &nbsp;·&nbsp; **Prep PR (merged):** [#957](https://github.com/githubnext/ado-aw/pull/957)
>
> Full plan + remaining-work handoff: [`IR_PLAN.md`](IR_PLAN.md).

## Snapshot

| Tracked | Done | Remaining |
|---|---|---|
| 23 todos | **11** | 12 (sized in `IR_PLAN.md`) |

Every commit below leaves the tree green:
`cargo build` ✓ &nbsp;·&nbsp; `cargo test` (1884 tests / 0 failed) ✓ &nbsp;·&nbsp; `cargo clippy --all-targets --all-features` ✓ &nbsp;·&nbsp; `cargo test --test bash_lint_tests` (2/2 with shellcheck) ✓

## What landed

### Pre-PR — merged on `main`

| SHA | Commit | What |
|---|---|---|
| `f8aab33a` | `chore(compile): canonical serde_yaml normalisation pass for emitted pipelines` (#957) | Round-tripped every committed `tests/safe-outputs/*.lock.yml` through `serde_yaml::from_str → to_string` and wired the same pass into `compile_shared`. Establishes a deterministic formatting baseline so the IR PR's diff is purely structural. |

### IR foundation — `native-ado-compiler` branch, 6 commits

| SHA | Commit | What |
|---|---|---|
| `080bf10d` | `feat(ir): introduce typed pipeline IR (types only, no callers)` | New module tree `src/compile/ir/`: `ids` (newtype `StageId`/`JobId`/`StepId`, validated against the ADO identifier grammar), `step` (`Step` enum + `BashStep` / `TaskStep` / `CheckoutStep` / `DownloadStep` / `PublishStep`), `job` + `stage` (with `Pool` variants), `env` (`EnvValue` with allowlist-checked `AdoMacro`), `condition` (typed `Condition` / `Expr` AST), `output` (`OutputDecl` / `OutputRef`), plus `Pipeline` / `PipelineBody` / `PipelineShape` (Standalone / OneEs / JobTemplate / StageTemplate). 59 unit tests; `#![allow(dead_code)]` during the migration window. |
| `f2b76455` | `feat(ir): lower Pipeline to YAML via serde_yaml` | `lower.rs` (Pipeline → `serde_yaml::Value` with canonical key order) + `emit.rs` (thin entry point composing lower with `serde_yaml::to_string`). Round-trip acceptance tests prove `IR → emit → from_str` produces a structurally equal `Value`. Deferred variants (StepOutput / Coalesce / Expr::StepOutput) error out with a pointer to the commit that fills them in. |
| `cd3af4d3` | `feat(ir): derive job and stage dependsOn from OutputRef graph` | `graph.rs`: walks every step's `env` + `condition` (incl. nested `Coalesce` children) to collect every `OutputRef`; lifts step-level edges to cross-job (same stage) and cross-stage edges; populates `Job::depends_on` / `Stage::depends_on`. Side-effect validators reject `UnknownProducer`, `AnonymousProducer`, `UnknownOutput`, `DuplicateStepId`/`DuplicateJobId`/`DuplicateStageId`, `MixedStagedAndUnstaged`. Kahn's algorithm detects cycles with a listed-nodes error message. 9 unit tests. |
| `ec50b1fa` | `feat(ir): lower OutputRefs to per-location ADO reference syntax` | `output::lower_outputref` is the single source of truth for the three syntaxes: same-job `$(stepName.X)` / cross-job `dependencies.<job>.outputs['stepName.X']` / cross-stage `stageDependencies.<stage>.<job>.outputs['stepName.X']`. Threaded through `lower::LoweringContext` so every recursive helper picks the right form per consumer location. `EnvValue::Coalesce` lowers to `$[ coalesce(<a>, <b>, …, '') ]` with the trailing `''` appended automatically and nested `Coalesce` flattened. `OutputDecl::auto_is_output` is populated by the graph pass for any output with at least one cross-step reader. |
| `87759d2e` | `feat(ir): condition codegen with Custom-injection check` | `condition::codegen`: lowers `Condition` / `Expr` to ADO condition strings with `And`/`Or` flattening for compact output. `Condition::Custom(s)` runs through a two-vector injection check (`contains_pipeline_command` rejects `##vso[` / `##[`; `contains_newline` rejects embedded newlines) but does NOT reject general ADO expressions like `$(...)` / `$[...]` / `${{...}}` — those are exactly what the escape hatch exists for. 8 unit tests cover every variant + both injection paths. |
| `39bedc62` | `feat(extensions): Declarations bundle + Step::RawYaml migration bridge` | `Step::RawYaml(String)` is the migration bridge — carries legacy `Vec<String>` step bodies through the IR unchanged (lowering parses the body into a `serde_yaml::Value`, strips a leading `- ` + de-indents continuation lines, re-emits via canonical normalisation). `extensions::Declarations` is the typed aggregate every extension will eventually return. `CompilerExtension::declarations(ctx) -> Result<Declarations>` ships as a **default impl** that wraps every legacy method — so the ~150 existing call sites stay intact and per-extension `port-*` commits override one at a time. Smoke test (`declarations_default_bridges_lean_extension_legacy_methods`) locks the bridge contract end-to-end. |

### Per-extension ports — always-on extensions now route through typed `Declarations`

| SHA | Commit | What |
|---|---|---|
| `d568a493` | `feat(extensions): port AdoAwMarkerExtension to typed Declarations` | Both prepare-phase steps (the `# ado-aw-metadata: …` marker step and the `aw_info.json` emit step) are now typed `Step::Bash(BashStep)`. The aw_info step carries `Condition::Always`. Coexists with legacy `prepare_steps` until target compilers switch to `declarations()` consumption. |
| `5ec6c25c` | `feat(extensions): port GitHubExtension to typed Declarations` | Trivial: only contributes `--allow-tool github`. Override routes through `Declarations::copilot_allow_tools`. |
| `6216bd4f` | `feat(extensions): port SafeOutputsExtension to typed Declarations` | `mcpg_servers` (HTTP backend for the SafeOutputs MCP) + `prompt_supplement` + `copilot_allow_tools` routed through `Declarations`. |
| `8181b45a` | `feat(extensions): port AzureCliExtension to typed Declarations` | Both Agent-job prepare steps (detection + conditional prompt-append) are now typed `Step::Bash`. The conditional step carries `Condition::Ne(Expr::Variable("AW_AZ_MOUNTS"), Expr::Literal(""))`, which lowers to today's `ne(variables['AW_AZ_MOUNTS'], '')`. Exercises the typed-condition codegen end-to-end. |

## Pragmatic deviations from the original plan

1. **`declarations()` is a default trait impl, not a required method.** The plan's `extension-trait-port` acceptance said *"old method names are gone in this commit"* — but that would have required updating ~150 call sites (production + tests) at once. Instead the default impl wraps every legacy method, with `Step::RawYaml` carrying legacy `Vec<String>` step bodies through the IR unchanged. Every existing call site still works. Per-extension `port-*` commits override `declarations()` one at a time; the final `delete-deprecated-trait-aliases` commit strips the legacy methods + `Step::RawYaml` together.
2. **Per-extension ports coexist with legacy methods.** A ported extension's typed `declarations()` override is **additive** — it doesn't replace `prepare_steps` / `setup_steps` / etc. Production callers (`common.rs`, `engine.rs`) still consume the legacy methods until `compile-target-*` switches them to `declarations()`.

## What's left (12 todos, sized in `IR_PLAN.md`)

| Bucket | Todos | Estimate |
|---|---|---|
| Easy ports — same pattern as the four ported extensions | `port-runtimes` (Lean / Python / Node / Dotnet), `port-tools` (azure-devops, cache-memory) | ~4 hr |
| Hard ports — the marquee `synthPr` work | `port-ado-script` (typed `synthPr` step with `OutputDecl`s + `prGate` consuming via `OutputRef` — **unlocks declarative cross-stage synth-PR propagation**), `port-exec-context` (typed `EnvValue::Coalesce` for `System.PullRequest.* ?? synthPr.*`) | 1-2 days each |
| Big bang — actually unlocks behaviour for users | `compile-target-{standalone, 1es, job, stage}` (each: rewrite the target to build the canonical `Pipeline` IR programmatically; delete the matching `src/data/*-base.yml`) | 3-5 days total |
| Cleanup | `retire-agentic-depends-on`, `delete-deprecated-trait-aliases`, `lockfile-rebaseline`, `docs-update` | 0.5 day each |

## Why stop here

The IR + Declarations foundation is the high-leverage work. The remaining commits are either mechanical (runtimes / tools), deep per-extension rework (ado-script + exec-context), or substantial target-compiler rewrites (compile-target-*). Each remaining ticket has its acceptance criteria and file list in [`IR_PLAN.md`](IR_PLAN.md); they're separately landable on top of #960.
