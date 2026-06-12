# Native ADO Pipeline IR

> **Status — home stretch** (updated 2026-06-12).
>
> ## What's done
>
> - **Prep PR #957**: ✅ merged. Canonical serde_yaml normalisation pass over every committed lock file. The IR PR's diff is now purely structural.
> - **Draft PR #960** (`native-ado-compiler`): ✅ 21 commits pushed + 1 pending commit (1ES) in working tree.
>
> ### Foundation (6 commits — `src/compile/ir/` + trait surface)
>
> | Commit | Scope |
> |---|---|
> | `080bf10d` `feat(ir): introduce typed pipeline IR` | `ids`, `step`, `job`, `stage`, `env`, `condition`, `output` |
> | `f2b76455` `feat(ir): lower Pipeline to YAML via serde_yaml` | `lower.rs` + `emit.rs` + round-trip tests |
> | `cd3af4d3` `feat(ir): derive job/stage dependsOn from OutputRef graph` | `graph.rs` — Kahn cycle detection, per-stage edge derivation |
> | `ec50b1fa` `feat(ir): lower OutputRefs to per-location ADO reference syntax` | same-job macro / cross-job / cross-stage + Coalesce + auto-isOutput |
> | `87759d2e` `feat(ir): condition codegen with Custom-injection check` | And/Or flattening, Custom-vector rejection |
> | `39bedc62` `feat(extensions): Declarations bundle + Step::RawYaml bridge` | New trait surface; default impl wraps legacy methods |
>
> ### Per-extension ports (all done)
>
> | Commit | Extension | Notes |
> |---|---|---|
> | `d568a493` | `AdoAwMarkerExtension` | Both prepare steps typed; `Condition::Always` on aw_info |
> | `5ec6c25c` | `GitHubExtension` | Trivial — just `copilot_allow_tools` |
> | `6216bd4f` | `SafeOutputsExtension` | mcpg_servers + prompt + allow_tool |
> | `8181b45a` | `AzureCliExtension` | Both prepare steps typed; `Condition::Ne(Variable, Literal(""))` lowers to `ne(variables['AW_AZ_MOUNTS'], '')` |
> | `bb4429ea` | runtimes (Lean/Python/Node/Dotnet) | Typed `Step::Task` + auth `Step::Bash`; `NodeExtension` emits `UseNode@1` |
> | `5cbaa0ad` | tools (AzureDevOps/CacheMemory) | Typed `Step`s; Stage 3 logic (`cache_memory::execute`) untouched |
> | `6c0ac3dc` | `AdoScriptExtension` | The marquee — typed `synthPr` step with `OutputDecl`s; `prGate` consumes via `OutputRef`. Unlocks declarative cross-stage synth-PR propagation. |
> | `996377e9` | `ExecContextExtension` | PR contributor's prepare step uses `EnvValue::Coalesce(vec![Macro(SYS_PR_*), StepOutput(synthPr.*)])` instead of hand-written `$[ coalesce(...) ]` strings |
>
> ### Top-level lowering
>
> | Commit | Scope |
> |---|---|
> | `1253187f` `feat(ir): lower parameters / resources / triggers / variables at top level` | Adds `Parameter` / `Resources` / `Triggers` / `PipelineVar` lowering; `RepositoryResource::SelfRepo`, schedules, PR/CI triggers. |
>
> ### Compile-target migrations (all done — every `*-base.yml` deleted)
>
> | Commit | Target | Notes |
> |---|---|---|
> | `dfba833c` `feat(compile): standalone target builds Pipeline IR; delete base.yml` | `standalone` | First production use of the IR. `src/compile/standalone_ir.rs` (`build_standalone_pipeline`) owns the canonical 5-job graph. |
> | `468359f6` `refactor(compile): extract canonical-jobs builder + extend IR for template targets` | shared infra | `build_pipeline_context` + `build_canonical_jobs` extracted so the template targets reuse the standalone scaffold. `Stage::external_params_wrap` + `Job::template_dependson_wrap` IR fields added for `${{ if eq/ne(length(parameters.X), 0) }}` dual-branch emission. |
> | `9f400732` `feat(compile): stage target builds Pipeline IR; delete stage-base.yml` | `stage` | `src/compile/stage_ir.rs` wraps the canonical jobs in a single prefixed stage with `StageExternalParamsWrap`. |
> | `63b489ee` `feat(compile): job target builds Pipeline IR; delete job-base.yml` | `job` | `src/compile/job_ir.rs` flat-jobs body; Agent job carries `TemplateDependsOnWrap` for dual-branch `dependsOn:` + `condition:`. |
> | `fd8be4dd` `fix(compile): port agent_job_variables hoist to IR` | bugfix | Brings the IR in line with the PR #956 / #972 unified `AW_PR_*` namespace — job-level `variables:` hoist for cross-job step-output references. |
> | **🟢 pending commit** | `1es` | `src/compile/onees_ir.rs` (NEW); `onees.rs` rewritten as ~70-line thin entry point; `src/data/1es-base.yml` deleted (-705 lines). `PipelineShape::OneEs` lowering implemented (was `unimplemented!()`); `Job::template_context` suppresses per-job `pool:` and lifts `Step::Publish` into `templateContext.outputs[]`. Net delta: **−647 lines**. Build clean / 1921 tests pass / clippy clean / shellcheck clean / 11 of 11 `_1es` integration tests pass. |
>
> With 1ES landed, **the IR drives every production compile path** and no template YAML files remain in `src/data/`.
>
> ## Pragmatic deviations from the original plan
>
> 1. **`declarations()` is a default trait impl, not a required method.** The plan asked for "old method names are gone in this commit" but that would have required updating ~150 call sites at once. Instead the default impl wraps every legacy method, with `Step::RawYaml` carrying legacy `Vec<String>` step bodies through the IR unchanged. Every existing call site still works. Per-extension `port-*` commits override `declarations()` one at a time; the final `delete-deprecated-trait-aliases` commit strips the legacy methods + `Step::RawYaml` together.
> 2. **Per-extension ports coexisted with legacy methods during the rollout.** Once `compile-target-{standalone, stage, job, 1es}` all landed, every production target builds from typed `Declarations`. The legacy `prepare_steps` / `setup_steps` / `finalize_steps` etc. methods now have *no production callers* — they're only kept alive by the trait's default-impl bridge until `delete-deprecated-trait-aliases` removes them.
> 3. **Setup / Teardown stay unprefixed even in `target: job|stage|1es`.** The legacy `job-base.yml` / `stage-base.yml` / `1es-base.yml` templates emit a literal `- job: Setup` / `- job: Teardown` regardless of the stage prefix; the IR preserves this via `JobPrefix::id` returning the unprefixed base for `Setup` / `Teardown`. See memory: `stage/job IR migration`.
> 4. **1ES jobs use `templateContext:` instead of per-job `pool:`.** Added `Job::template_context: Option<JobTemplateContext>` so the lowering pass suppresses `pool:` and wraps `steps:` under `templateContext: { type: buildJob, outputs: <publishes>, steps: }`. `Step::Publish` entries in the job are lifted into `templateContext.outputs[]` rather than emitted inline — the 1ES template owns the artifact publish.
>
> ## Remaining work
>
> Four cleanup commits left. Each is mechanical now that the production
> code paths are all IR-driven.
>
> ### Sized — cleanup (0.5 day each)
>
> - **`retire-agentic-depends-on`** — delete `generate_agentic_depends_on`, `generate_setup_job`, `generate_teardown_job`, `generate_prepare_steps`, `generate_finalize_steps`, `format_step_yaml*`, `format_steps_yaml*`, `replace_with_indent`, `generate_parameters`, `generate_repositories`, `generate_checkout_steps`, `generate_checkout_self`, `generate_pipeline_resources`, `generate_pr_trigger`, `generate_ci_trigger`, `generate_schedule`, and friends from `common.rs`. Their behaviour now derives from the typed `Condition` AST + graph pass. Note: `pr_filters.rs` tests still reference `generate_setup_job` — those tests will need updating to use the IR builders directly.
> - **`delete-deprecated-trait-aliases`** — remove `Step::RawYaml`, the 12 legacy trait methods, the `#[allow(dead_code)]` on `Declarations`. Audit grep for `RawYaml` and the old method names must return zero hits outside test fixtures. **Note:** `standalone_ir::build_setup_job` / `build_teardown_job` still use `Step::RawYaml` to carry user-authored setup/teardown YAML — those legitimate use cases survive (the IR doesn't model arbitrary user-authored ADO step shapes), so the audit should account for the standalone_ir use sites.
> - **`lockfile-rebaseline`** — `cargo run -- compile --force` over every fixture; commit the structural diff. Five-fixture spot-check (`synthetic-pr-default`, `pr-mode-policy`, `create-pull-request`, `janitor`, one 1ES).
> - **`docs-update`** — rewrite `docs/extending.md`, replace `docs/template-markers.md` with `docs/ir.md`, refresh `AGENTS.md` and matching `site/src/content/docs/` mdx files.

## Problem

The compiler today emits Azure DevOps pipeline YAML by interpolating
hand-written strings into per-target template files
(`src/data/base.yml`, `1es-base.yml`, `job-base.yml`,
`stage-base.yml` — ~131 KB combined) and concatenating `Vec<String>`
steps from each `CompilerExtension`. Three classes of recurring pain:

1. **Variable-reference correctness is the extension author's problem.**
   ADO has three distinct syntaxes for reading a step output:
   - same-job: `$(stepName.X)` (macro form; the *only* form that
     resolves for runtime expressions in the producing job — see
     `compile_gate_step_external` doc-comment in
     `src/compile/filter_ir.rs:1130-1146`),
   - cross-job same-stage: `dependencies.<job>.outputs['stepName.X']`,
   - cross-stage: `stageDependencies.<stage>.<job>.outputs['stepName.X']`.

   The synthPr bug (memory: `azure devops`) was a textbook symptom —
   `$[ variables['synthPr.X'] ]` was used in `filter_ir.rs:1185` and
   silently resolved to empty. Patched in `filter_ir.rs:1192` and
   `exec_context/pr.rs:173`, but propagation still fails across the
   Setup → Detection → SafeOutputs stages because no compile-time
   invariant forces consumers to use the right form for their
   *location*.

2. **Stage / job `dependsOn` is hand-stitched.**
   `generate_agentic_depends_on` (`src/compile/common.rs:2388-2530`,
   ~140 lines) hard-codes synthPr clauses inside the generic
   Agent-job `condition:` builder. Every future cross-stage signal
   needs more special-case surgery here. The dual-branch
   `${{ if eq(length(parameters.dependsOn), 0) }}` block for
   `target: job` (lines 2484-2529) compounds the complexity.

3. **Templates are opaque to extensions.**
   The four `*-base.yml` files encode the Agent / Detection /
   SafeOutputs / Teardown structure as raw YAML with `{{ marker }}`
   slots (full marker list in `docs/template-markers.md`).
   Extensions can only contribute to the named slots; they cannot
   read, modify, or compose anything else. Cross-stage data flow has
   to be smuggled through `##vso[task.setvariable;isOutput=true]`
   and matching string references baked into multiple templates.

## Approach

Replace YAML-string composition with a typed pipeline IR rooted in
`src/compile/ir/`. The IR is the single source of truth: per-target
compilers build typed `Pipeline` objects, the graph validator derives
`dependsOn` automatically from declared `OutputRef`s, the lowering
pass picks the correct ADO reference syntax per consumer, and a
single serde_yaml emit produces the final lock file. The four
`*-base.yml` template files are deleted — *no YAML survives in
source*.

Decisions agreed with @jamesadevine in plan-mode:

- **Scope (option B)**: Step IR *plus* Job / Stage IR with
  auto-derived `dependsOn`. Retires `generate_agentic_depends_on`.
- **Landing (big-bang)**: single PR ports every extension and rewrites
  every target. Sub-divided into reviewable commits.
- **Synth-PR bug (partial)**: IR must make propagation declarative
  (consumer writes `Var::step_output(synth_pr_step, "AW_SYNTHETIC_PR")`
  and the compiler picks the right reference form); end-to-end bug
  verification ships in a follow-up.
- **Emission**: every `serde_yaml::to_string` call goes through a
  single typed `PipelineYaml` view; no hand-built `replace_with_indent`
  in the final emit path.
- **Migration noise**: a **separate prep PR** lands first that round-trips
  every fixture through `serde_yaml::from_str` → `to_string`. That PR is
  cosmetic only (re-quoting, indentation, key order) and produces no
  semantic change. The big-bang IR PR then diffs against the
  normalised baseline so every line of churn is a real structural
  change.
- **Templates**: the four `*-base.yml` files are deleted. The IR
  composes the pipeline shape per-target programmatically.

## IR specification

### Module layout (new code)

```
src/compile/ir/
├── mod.rs            // pub re-exports + Pipeline root + PipelineShape
├── ids.rs            // StageId / JobId / StepId newtypes (Copy, Hash, Display)
├── step.rs           // Step enum + BashStep / TaskStep / CheckoutStep / DownloadStep / PublishStep
├── job.rs            // Job + Pool + Timeout
├── stage.rs          // Stage
├── env.rs            // EnvValue + Coalesce serialisation
├── condition.rs      // Condition AST + Expr + condition codegen
├── output.rs         // OutputDecl + OutputRef + reference-syntax lowering
├── graph.rs          // dependency graph: cycle detection + dependsOn derivation
├── validate.rs       // post-build validation pass (refs resolve, no orphan jobs, etc.)
├── lower.rs          // IR -> serde_yaml::Value tree
└── emit.rs           // Wrapper around serde_yaml::to_string + canonical normalisation
```

Plus a `tests/` sibling per module for unit-level coverage and a top-level
`src/compile/ir/tests.rs` for integration fixtures.

### Top-level types

```rust
// src/compile/ir/mod.rs
pub struct Pipeline {
    pub name: String,                              // sanitized pipeline_agent_name
    pub parameters: Vec<Parameter>,
    pub resources: Resources,
    pub triggers: Triggers,                        // schedule + pr + ci + pipeline
    pub variables: Vec<PipelineVar>,
    pub body: PipelineBody,
    pub shape: PipelineShape,
}

pub enum PipelineBody {
    Jobs(Vec<Job>),                                // Standalone, JobTemplate
    Stages(Vec<Stage>),                            // OneEs, StageTemplate
}

pub enum PipelineShape {
    Standalone,
    OneEs { sdl: OneEsSdlConfig },                 // captures the `extends: template:` wrapping
    JobTemplate { external_params: TemplateParams },// target: job
    StageTemplate { external_params: TemplateParams }, // target: stage
}
```

### Stage / job / step types

```rust
// src/compile/ir/stage.rs
pub struct Stage {
    pub id: StageId,                               // newtype - graph keys are typed
    pub display_name: String,
    pub jobs: Vec<Job>,
    pub depends_on: Vec<StageId>,                  // *derived*, not user-supplied
    pub condition: Option<Condition>,              // typed AST, see below
}

// src/compile/ir/job.rs
pub struct Job {
    pub id: JobId,
    pub display_name: String,
    pub pool: Pool,
    pub timeout: Option<Duration>,
    pub steps: Vec<Step>,
    pub depends_on: Vec<JobId>,                    // derived
    pub condition: Option<Condition>,
    pub strategy: Option<Strategy>,                // reserved; not used in MVP
}

// src/compile/ir/step.rs
pub enum Step {
    Bash(BashStep),
    Task(TaskStep),
    Checkout(CheckoutStep),
    Download(DownloadStep),
    Publish(PublishStep),
    // additional ADO step kinds added as encountered
}

pub struct BashStep {
    pub id: Option<StepId>,                        // required iff any other step references its outputs
    pub display_name: String,
    pub script: String,                            // raw bash body (no leading "- bash: |")
    pub env: BTreeMap<String, EnvValue>,
    pub outputs: Vec<OutputDecl>,                  // isOutput emitted automatically when needed
    pub condition: Option<Condition>,
    pub timeout: Option<Duration>,
    pub continue_on_error: bool,
    pub working_directory: Option<String>,
}

pub struct TaskStep {
    pub id: Option<StepId>,
    pub task: String,                              // e.g. "NodeTool@0"
    pub display_name: String,
    pub inputs: BTreeMap<String, String>,
    pub env: BTreeMap<String, EnvValue>,
    pub condition: Option<Condition>,
    pub timeout: Option<Duration>,
    pub continue_on_error: bool,
}

pub struct CheckoutStep {
    pub repository: CheckoutRepo,                  // Self | Named(String)
    pub clean: Option<bool>,
    pub submodules: Option<SubmodulesOpt>,
    pub fetch_depth: Option<u32>,
    pub persist_credentials: Option<bool>,
}

// src/compile/ir/output.rs
pub struct OutputDecl { pub name: String, pub is_secret: bool }
pub struct OutputRef { pub step: StepId, pub name: String }
```

### EnvValue + Condition + Expr

```rust
// src/compile/ir/env.rs
pub enum EnvValue {
    Literal(String),                               // "true", "20.x", etc.
    AdoMacro(&'static str),                        // $(Build.SourceBranch) - compile-time validated against an allowlist
    StepOutput(OutputRef),                         // lowered per consumer location
    Coalesce(Vec<EnvValue>),                       // $[ coalesce(a, b, '') ] semantics
    PipelineVar(String),                           // $(MY_VAR) for user-defined ADO vars
    Secret(String),                                // $(MY_SECRET); same lowering as PipelineVar but flagged for audit
}

// src/compile/ir/condition.rs
pub enum Condition {
    Succeeded,
    SucceededOrFailed,                             // ADO `always()` but only after dependsOn complete
    Always,
    Failed,
    And(Vec<Condition>),
    Or(Vec<Condition>),
    Not(Box<Condition>),
    Eq(Expr, Expr),
    Ne(Expr, Expr),
    Custom(String),                                // escape hatch; validated against pipeline-command injection
}

pub enum Expr {
    BuildReason,                                   // variables['Build.Reason']
    BuildVar(&'static str),                        // variables['Build.<X>']
    Variable(String),                              // variables['<user-var>']
    Literal(String),                               // single-quoted scalar
    StepOutput(OutputRef),                         // lowered to dependencies / stageDependencies form
}
```

The `AdoMacro` and `BuildVar` variants accept only known strings,
enforced at compile time by `const ALLOWED_BUILD_VARS: &[&str]`.

### `CompilerExtension` trait shape

```rust
pub trait CompilerExtension {
    fn name(&self) -> &str;
    fn phase(&self) -> ExtensionPhase;
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations>;
}

pub struct Declarations {
    pub agent_prepare_steps: Vec<Step>,
    pub setup_steps: Vec<Step>,
    pub agent_finalize_steps: Vec<Step>,
    pub detection_prepare_steps: Vec<Step>,
    pub safe_outputs_steps: Vec<Step>,
    pub network_hosts: Vec<String>,
    pub bash_commands: Vec<String>,
    pub prompt_supplement: Option<String>,
    pub mcpg_servers: Vec<(String, McpgServerConfig)>,
    pub copilot_allow_tools: Vec<String>,
    pub pipeline_env: Vec<PipelineEnvMapping>,
    pub awf_mounts: Vec<AwfMount>,
    pub awf_path_prepends: Vec<String>,
    pub agent_env_vars: Vec<(String, EnvValue)>,
    pub warnings: Vec<String>,
}
```

The existing 14 methods on `CompilerExtension` collapse into 3.

## Canonical pipeline skeleton (per shape)

The target compilers construct the same canonical job graph; only
the wrapping differs.

### Standalone + OneEs (full 3-stage pipeline)

```
Pipeline { body: Jobs(vec![Setup?, Agent, Detection, SafeOutputs, Teardown?]) }
                                  // OneEs: same jobs nested in a single Stage inside an `extends:` wrapper
```

| JobId        | Slot                          | Source                                   | Edges in          |
|--------------|-------------------------------|------------------------------------------|-------------------|
| `Setup`      | `Declarations::setup_steps`   | extensions + user `setup:` block         | (none)            |
| `Agent`      | `prepare_steps + run + finalize_steps` | extensions + user `steps:`/`post_steps:` | `Setup` if present |
| `Detection`  | static + `detection_prepare_steps` | always-on + extensions             | `Agent`           |
| `SafeOutputs`| static + `safe_outputs_steps` | always-on + extensions                   | `Agent, Detection`|
| `Teardown`   | user `teardown:`              | front-matter only                        | `SafeOutputs`     |

Stage edges (`SafeOutputs.condition`, `Agent.condition`) are emitted
from typed `Condition` nodes built in target compilers — replacing
the hand-stitched strings in `generate_agentic_depends_on`.

### JobTemplate (`target: job`)

```
Pipeline { body: Jobs(...), shape: JobTemplate { external_params } }
```

The IR emits `parameters:` block at the top; the same canonical jobs
follow. The Agent job's `dependsOn` and `condition` are wrapped in
dual-branch `${{ if eq(length(parameters.dependsOn), 0) }}` blocks
*by the lowering pass*, not by extensions. This logic lives in
`src/compile/ir/lower.rs::lower_template_dependson` and replaces the
existing dual-branch code in `common.rs:2484-2529`.

### StageTemplate (`target: stage`)

```
Pipeline { body: Stages(vec![Stage { id: "Main", jobs: <canonical> }]), shape: StageTemplate { ... } }
```

The single stage's `dependsOn` is the external template parameter
slot. Internal Setup/gate `dependsOn` stays within the stage.

## Output-lowering algorithm

For each `OutputRef { step: producer, name }` consumed by a
**consumer** step:

1. Look up `producer`'s containing job and stage.
2. Look up the consumer step's containing job and stage.
3. Pick the syntax:
   - same job (consumer_job == producer_job):
     `$(stepName.name)` — macro form.
   - cross job, same stage (consumer_stage == producer_stage):
     `dependencies.<producer_job>.outputs['stepName.name']`.
   - cross stage:
     `stageDependencies.<producer_stage>.<producer_job>.outputs['stepName.name']`.
4. Mark `OutputDecl { name }` on the producer as needing
   `isOutput=true` (auto-promoted).
5. Add `producer_job` to consumer_job's `depends_on` set; add
   `producer_stage` to consumer_stage's `depends_on` set.
6. After all refs walked, run cycle detection. Error message:
   `IR: cycle in step output references: <stage>.<job>.<step> -> ...`.

Lowering happens once, in `src/compile/ir/output.rs::lower_outputref`,
and is the only place these three syntaxes are produced.

## EnvValue::Coalesce lowering

`EnvValue::Coalesce(vec![a, b, …])` lowers to a single ADO runtime
expression: `"$[ coalesce(<a>, <b>, …, '') ]"`. Each inner value is
lowered recursively. The trailing `''` is added automatically (matches
the pattern used today in `exec_context/pr.rs:198`). Validation
rejects nested `Coalesce` in the same expression (flatten instead).

## Condition lowering

`Condition` lowers to ADO condition syntax with these rules:
- `And(parts)` → `and(<part>, <part>, …)`; flatten nested `And`.
- `Or(parts)`  → `or(...)`; flatten nested `Or`.
- `Not(x)` → `not(<x>)`.
- `Eq(a, b)` → `eq(<a>, <b>)`.
- `Ne(a, b)` → `ne(<a>, <b>)`.
- `Succeeded` → `succeeded()`.
- `Always` → `always()`.
- `Custom(s)` → `s` verbatim, after passing
  `validate::reject_pipeline_injection`.

Top-level conditions taller than 80 columns emit as
`condition: |\n  <pretty-printed>` (matches current style). Single-line
expressions emit inline.

## Validation pass (`src/compile/ir/validate.rs`)

Runs after build, before lowering. Hard errors:

- Every `OutputRef` resolves to a step that exists and has the
  named output in its `OutputDecl` list.
- `OutputRef::step` must point at a step whose `id: Some(...)` is
  set (forces extensions to name producers explicitly).
- No two `Step`s in the same `Job` share a `StepId`.
- No two `Job`s in the same `Stage` (or in the top-level job list)
  share a `JobId`.
- No two `Stage`s share a `StageId`.
- No cycles in the derived `depends_on` graph.
- `Custom(s)` conditions pass
  `validate::reject_pipeline_injection`.
- `BashStep::script` passes the shellcheck pass (run only in
  `cargo test --test bash_lint_tests`, not at compile time —
  matches today).
- `EnvValue::AdoMacro` value is in `ALLOWED_ADO_MACROS`.

## Serde emission contract

`src/compile/ir/emit.rs` exposes:

```rust
pub fn emit(pipeline: &Pipeline) -> anyhow::Result<String>;
```

Internally it lowers to `serde_yaml::Value`, calls
`serde_yaml::to_string`, then runs the **canonical
normalisation wrapper** introduced in the prep PR (round-trip
+ deterministic key order via `serde_yaml::Mapping`). The
header comment from `HEADER_MARKER` (currently `# @ado-aw`,
see `src/compile/common.rs:HEADER_MARKER`) is prepended exactly as
today.

## Out of scope for this PR

- `src/audit/*` — audit reads compiled YAML, not source IR.
- gh-aw-firewall (AWF) and MCPG container code.
- CLI command surface (`secrets`, `enable`, `disable`, `run`,
  `audit`, etc.).
- End-to-end fix for the synth-PR cross-stage propagation bug —
  follow-up PR re-uses the now-declarative `OutputRef`s.
- New compile targets.
- Changes to `safeoutputs/` (Stage 3 executor); only the
  Stage 1 MCP wiring (`SafeOutputsExtension`) is touched.
- Changes to `scripts/ado-script/` TypeScript bundles.

## Per-extension migration table

For each extension, the table below lists where its current emission
lives, the matching slot in `Declarations`, and the key `OutputRef`s
to thread (if any).

| Extension                  | Current emission file                                       | Declarations slot(s)                                                                 | Output producers / refs to thread                                                                |
|----------------------------|-------------------------------------------------------------|--------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------|
| `AdoAwMarkerExtension`     | `extensions/ado_aw_marker.rs:46-142`                        | `agent_prepare_steps` (single bash with JSON marker)                                 | none                                                                                             |
| `GitHubExtension`          | `extensions/github.rs`                                      | `mcpg_servers`, `bash_commands`, `network_hosts`                                     | none                                                                                             |
| `SafeOutputsExtension`     | `extensions/safe_outputs.rs`                                | `mcpg_servers`, `agent_env_vars` (`SAFE_OUTPUTS_PORT`, `_API_KEY`), `agent_prepare_steps` for `SAFE_OUTPUTS_PID` exporter | producer: `safeOutputsLaunch` step exports `SAFE_OUTPUTS_PID` (currently `base.yml:174`); consumer: Agent finalize uses macro form. |
| `AdoScriptExtension`       | `extensions/ado_script.rs`                                  | `setup_steps` (install + download + `synthPr` + gates), `agent_prepare_steps` (install + download + `resolver`) | producer: `synthPr` declares `AW_SYNTHETIC_PR`, `AW_SYNTHETIC_PR_SKIP`, `AW_SYNTHETIC_PR_ID`, `_SOURCEBRANCH`, `_TARGETBRANCH`; consumers: `prGate` (same job, macro), Agent `condition` (cross-job), Detection / SafeOutputs `condition` (cross-job via stage dep). |
| `ExecContextExtension`     | `extensions/exec_context/{mod,contributor,pr}.rs`           | `agent_prepare_steps` (PR contributor)                                               | consumer of `synthPr.*` via `EnvValue::Coalesce(vec![Macro(SYS_PR_*), StepOutput(synthPr.*)])`. |
| `AzureCliExtension`        | `extensions/azure_cli.rs`                                   | `agent_prepare_steps` (mount detection), `network_hosts`, `agent_env_vars` (`AW_AZ_MOUNTS`) | producer: `awAzMounts` step exports `AW_AZ_MOUNTS`; consumer: AWF launch step (same job, macro). |
| `LeanExtension`            | `runtimes/lean/extension.rs`                                | `agent_prepare_steps` (elan install), `awf_mounts`, `awf_path_prepends`, `bash_commands` | none                                                                                       |
| `PythonExtension`          | `runtimes/python/extension.rs`                              | `agent_prepare_steps` (`UsePythonVersion@0` Task), `network_hosts`, `agent_env_vars` | none                                                                                             |
| `NodeExtension`            | `runtimes/node/extension.rs`                                | `agent_prepare_steps` (`UseNode@1` Task — memory: `node install task`), `network_hosts`, `agent_env_vars` | none                                                                                  |
| `DotnetExtension`          | `runtimes/dotnet/extension.rs`                              | `agent_prepare_steps`, `network_hosts`, `agent_env_vars`                             | none                                                                                             |
| `AzureDevOpsExtension`     | `tools/azure_devops/extension.rs`                           | `mcpg_servers`, `pipeline_env` (`AZURE_DEVOPS_EXT_PAT`), `network_hosts`             | none                                                                                             |
| `CacheMemoryExtension`     | `tools/cache_memory/extension.rs`                           | `agent_prepare_steps` (memory mount setup), `awf_mounts`, `agent_env_vars`           | none                                                                                             |

`AdoScriptExtension`'s `synthPr` step is the critical one for the
synth-PR propagation bug — once it returns a `BashStep` with
`id: Some(StepId::new("synthPr"))` and `outputs: vec![OutputDecl
{ name: "AW_SYNTHETIC_PR" }, …]`, the IR enforces correct reference
syntax for every consumer.

## `compile_shared` decomposition

`compile_shared` (`src/compile/common.rs:3199-3650+`, ~450 lines)
becomes a thin builder that:

1. Calls `validate_*` checks (unchanged).
2. Builds a target-specific `Pipeline` IR via
   `target.build_pipeline(front_matter, ctx, &declarations)`.
3. Runs `ir::validate::run(&pipeline)`.
4. Runs `ir::lower::lower(pipeline)` → `Pipeline<Lowered>`.
5. Calls `ir::emit::emit(&lowered)` → final YAML string.
6. Prepends `HEADER_MARKER` if `!skip_header` (unchanged).
7. Atomically writes via `atomic_write` (unchanged).

The following helpers in `common.rs` are deleted as their callers
migrate:

- `generate_setup_job`, `generate_teardown_job`,
  `generate_prepare_steps`, `generate_finalize_steps`,
  `generate_agentic_depends_on`,
- `format_step_yaml`, `format_step_yaml_indented`,
  `format_steps_yaml`, `format_steps_yaml_indented`,
- `replace_with_indent` (last user goes when target compilers stop
  using template strings),
- `generate_parameters`, `generate_repositories`,
  `generate_checkout_steps`, `generate_checkout_self`,
  `generate_pipeline_resources`, `generate_pr_trigger`,
  `generate_ci_trigger`, `generate_schedule` — these become
  IR builders in `ir::triggers` and `ir::resources`.

Helpers that stay (used by IR builders or unrelated subsystems):
`parse_markdown_detailed`, `reconstruct_source`, `atomic_write`,
`sanitize_filename`, `sanitize_pipeline_agent_name`,
`yaml_double_quoted` (still used inside IR lowering),
`validate_*` validators, `compute_effective_workspace`,
`resolve_repos`.

## Test strategy

### New tests (per IR module)

- `ir::ids::tests` — newtype constructors reject empty/invalid names.
- `ir::step::tests` — `BashStep` builder rejects scripts containing
  `##vso[task.setvariable` outside `outputs` declarations.
- `ir::env::tests` — `EnvValue::Coalesce` lowers to the expected
  string; nested `Coalesce` is flattened.
- `ir::condition::tests` — every `Condition` variant lowers to the
  expected string; `Custom(s)` rejects injection.
- `ir::output::tests` — three lowering cases (same-job macro,
  cross-job, cross-stage) each produce the exact expected string.
- `ir::graph::tests` — cycle detection, redundant edges deduped,
  derived `dependsOn` matches hand-built reference graphs.
- `ir::validate::tests` — every hard-error case (missing output,
  unset step id, duplicate ids, cycle, banned macro) has a test.
- `ir::lower::tests` — `lower_template_dependson` produces the same
  dual-branch YAML as `generate_agentic_depends_on` does today
  (snapshot per matrix cell: setup × gate × pr-filters × pipeline-filters
  × synth-active = up to 32 cases; current tests at `common.rs:8400+`
  serve as the spec — port them).

### Extension migration tests

- For each ported extension, add a unit test that calls
  `extension.declarations(&ctx)` on a representative `FrontMatter`
  and asserts the returned `Declarations` matches a hand-written
  fixture (no YAML strings in the assertion — assertions are on the
  IR shape).
- The existing extension integration tests in
  `src/compile/extensions/tests.rs` move from string-matching to
  IR-shape assertions where practical; YAML-level assertions stay
  for end-to-end coverage.

### Target compiler tests

- Standalone fixture round-trip: every file in `tests/fixtures/*.md`
  must compile to a parseable, semantically-equivalent lock file.
  Compare against pre-PR baseline by structural equality on
  `serde_yaml::Value` (key-order-tolerant).
- `tests/compiler_tests.rs` keeps every existing assertion but
  adapts call sites that constructed YAML by hand.
- `tests/bash_lint_tests.rs` must keep passing untouched (the IR
  serialises the same bash bodies).

### Spot-check matrix

Five lock files reviewed by hand for parity:
1. `tests/fixtures/synthetic-pr-default.md.lock.yml` — synth-PR
   propagation (the failing case).
2. `tests/fixtures/pr-mode-policy.md.lock.yml` — policy mode (no
   synth path).
3. `tests/safe-outputs/create-pull-request.lock.yml` — biggest
   SafeOutputs surface.
4. `tests/safe-outputs/janitor.lock.yml` — uses every always-on
   extension.
5. One 1ES fixture (whichever `tests/fixtures/onees-*.md` covers
   the most surface) — exercises `PipelineShape::OneEs`.

## Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Lock-file diff is enormous | Prep PR lands first; semantic-equivalent serde_yaml normalisation makes the IR PR diff purely structural. |
| ADO reference-syntax rules are finicky and under-documented | Port `compile_gate_step_external`, `exec_context/pr.rs`, `generate_agentic_depends_on` first and cross-check against their accumulated comments. Memory `azure devops` is the empirical ground truth. |
| `compile_shared` (~450 lines) has hidden coupling | Decompose in `ir-build-skeleton` commit (todo not in list — break out as part of `compile-target-standalone` if needed). Each helper deleted only when its caller migrates. |
| `target: job` dual-branch dependsOn / condition wrapping is subtle | Port the existing 32-case snapshot tests in `common.rs:8400+` first to lock the behaviour as a contract. |
| Per-target shape differences (1ES `extends:` wrapping, stage template, job template) | Each lives in its own `PipelineShape` variant; lowering branches once at the top, not throughout. |
| `serde_yaml::Mapping` key-order non-determinism | Use `IndexMap` semantics (preserved by serde_yaml's `Mapping`). Canonical key order enforced in `ir::lower`. |
| Bash-lint regressions when emission style changes | `tests/bash_lint_tests.rs` runs on emitted YAML; failures block the commit that introduces them. |

## Todos (tracked in SQL, with explicit acceptance criteria)

A separate **prep PR** comes first; the big-bang IR PR is
everything else. All todo ids match `todos.id` rows; deps live in
`todo_deps`.

### Prep PR (single todo, ships first)

| Todo id | What | Files | Acceptance |
|---------|------|-------|------------|
| `prep-pr` | Round-trip every `tests/**/*.lock.yml` and `tests/fixtures/**/*.lock.yml` through `serde_yaml::from_str → to_string`. Add a `normalize_yaml(&str) -> Result<String>` helper next to `atomic_write` and call it at the end of `compile_shared` before the header is prepended. | `src/compile/common.rs` (add helper + call site), every committed `*.lock.yml`. | `cargo test` passes. Re-running `cargo run -- compile` over every fixture produces zero diff. The diff for the committed lock files is purely cosmetic (re-quoting, key order, indentation). |

### Big-bang IR PR (22 todos)

Each commit must leave the tree green (`cargo build`, `cargo test`,
`cargo clippy --all-targets --all-features`).

| Todo id | Files added / touched | Acceptance |
|---------|-----------------------|------------|
| `ir-types` | `src/compile/ir/{mod,ids,step,job,stage,env,condition,output}.rs` | Types compile; constructor unit tests pass; no callers in the rest of the crate yet. |
| `ir-yaml-emit` | `src/compile/ir/{lower,emit}.rs` (skeleton), `src/compile/ir/tests/round_trip.rs` | Handcrafted `Pipeline { … }` fixtures round-trip through `emit` → `serde_yaml::from_str` → equal `Value`. |
| `ir-graph` | `src/compile/ir/graph.rs`, `src/compile/ir/tests/graph.rs` | Cycle detection produces the documented error message; deriving `dependsOn` for a 5-stage fixture matches the hand-built reference. |
| `ir-output-lowering` | `src/compile/ir/output.rs` (extend with `lower_outputref`), `src/compile/ir/tests/output.rs` | Three lowering cases each produce the exact expected string. Auto-`isOutput=true` is applied iff there is at least one cross-step reader. |
| `ir-condition-codegen` | `src/compile/ir/condition.rs` (extend with codegen), `src/compile/ir/tests/condition.rs` | Every variant lowers to the documented string; `Custom(s)` rejects injection (`reject_pipeline_injection`). |
| `extension-trait-port` | `src/compile/extensions/mod.rs` (trait + `Declarations` + `Extension` enum macro). Old method names removed; the macro now delegates only `name`, `phase`, `declarations`. | Every existing extension still compiles after the trait change because old method bodies are wrapped into a hand-written `declarations()` that returns `Declarations` with `agent_prepare_steps`/`setup_steps` populated from the old `Vec<String>` via a temporary `Step::RawYaml(String)` variant. Tree is green; old method names are gone. |
| `port-ado-aw-marker` | `src/compile/extensions/ado_aw_marker.rs` | Returns typed `Step::Bash(BashStep)` (no `RawYaml`). Existing unit tests pass without YAML-string assertions. |
| `port-github` | `src/compile/extensions/github.rs` | No `RawYaml`. |
| `port-safe-outputs` | `src/compile/extensions/safe_outputs.rs` | No `RawYaml`. The `SAFE_OUTPUTS_PID` exporter has `id: Some(StepId::new("safeOutputsLaunch"))` and declares `AW_SAFE_OUTPUTS_PID` as an `OutputDecl`. |
| `port-azure-cli` | `src/compile/extensions/azure_cli.rs` | The two-branch detect/else step is rebuilt as one `BashStep` whose script uses `if/else` natively. `AW_AZ_MOUNTS` is declared as an `OutputDecl` consumed by the AWF launch step via `OutputRef`. |
| `port-ado-script` | `src/compile/extensions/ado_script.rs`, `src/compile/filter_ir.rs` (replace `compile_gate_step_external`'s string emission with IR construction) | `synthPr` step is `BashStep { id: Some(StepId::new("synthPr")), outputs: [AW_SYNTHETIC_PR{_,_SKIP,_ID,_SOURCEBRANCH,_TARGETBRANCH}], … }`. `prGate` step references those via `OutputRef`. Lowering proves the macro form is used (snapshot regression test). |
| `port-exec-context` | `src/compile/extensions/exec_context/{mod,pr}.rs` | The PR prepare step uses `EnvValue::Coalesce(vec![Macro("System.PullRequest.X"), StepOutput(OutputRef { step: synthPr, name: "AW_SYNTHETIC_PR_X" })])`. Hand-written `$[ coalesce(...) ]` strings are gone. |
| `port-runtimes` | `src/runtimes/{lean,python,node,dotnet}/extension.rs` | All four runtimes return typed `Step`s. `NodeExtension` continues to emit `UseNode@1` (memory: `node install task`). |
| `port-tools` | `src/tools/{azure_devops,cache_memory}/extension.rs` | Both tools return typed `Step`s. Stage 3 logic (`cache_memory::execute`) untouched. |
| `compile-target-standalone` | `src/compile/standalone.rs` (rewritten), **delete `src/data/base.yml`** | `StandaloneCompiler::compile` constructs the canonical `Pipeline { body: Jobs(...), shape: Standalone }` and emits via `ir::emit::emit`. No `include_str!("../data/base.yml")` left. All standalone fixtures recompile identically up to the canonical normalisation baseline. |
| `compile-target-1es` | `src/compile/onees.rs` (rewritten), **delete `src/data/1es-base.yml`** | `PipelineShape::OneEs` wrapping handles `extends: template:` and SDL. All 1ES fixtures recompile identically. |
| `compile-target-job` | `src/compile/job.rs` (rewritten), **delete `src/data/job-base.yml`** | The dual-branch `${{ if eq(length(parameters.dependsOn), 0) }}` wrap is emitted from `lower::lower_template_dependson`; the 32-case snapshot tests at `common.rs:8400+` pass against the new emitter. |
| `compile-target-stage` | `src/compile/stage.rs` (rewritten), **delete `src/data/stage-base.yml`** | All `target: stage` fixtures recompile identically. |
| `retire-agentic-depends-on` | `src/compile/common.rs` (delete `generate_agentic_depends_on`, `generate_setup_job`, `generate_teardown_job`, `generate_prepare_steps`, `generate_finalize_steps`, `format_step_yaml*`, `format_steps_yaml*`, `replace_with_indent`, `generate_parameters`, `generate_repositories`, `generate_checkout_steps`, `generate_checkout_self`, `generate_pipeline_resources`, `generate_pr_trigger`, `generate_ci_trigger`, `generate_schedule`). Replace each with an IR builder. | Helpers removed; no dead code warnings; tree green. |
| `delete-deprecated-trait-aliases` | `src/compile/ir/step.rs` (remove `Step::RawYaml`); audit grep `RawYaml` returns nothing. | No extension uses `RawYaml`; trait is exactly `name`/`phase`/`declarations`. |
| `lockfile-rebaseline` | every committed `*.lock.yml` | `cargo run -- compile` over every fixture produces zero diff after this commit. Five-file spot-check (`synthetic-pr-default`, `pr-mode-policy`, `create-pull-request`, `janitor`, one 1ES) shows the lock files are semantically equivalent to pre-PR (per the prep-PR baseline). |
| `docs-update` | `docs/extending.md`, `docs/template-markers.md` (rewrite as `docs/ir.md`), `docs/filter-ir.md`, `AGENTS.md`, `site/src/content/docs/guides/extending.mdx`, `site/src/content/docs/reference/template-markers.mdx` (rewrite). | New `docs/ir.md` covers IR types, graph rules, output-ref lowering. `docs/template-markers.md` is deleted (no markers survive); the file is redirected to `docs/ir.md`. |

## Validation

Run after every commit:
- `cargo build`
- `cargo test`
- `cargo clippy --all-targets --all-features`
- `cargo test --test bash_lint_tests`

Final validation for the IR PR before merge:
- All four target lock files for the spot-check matrix
  (`synthetic-pr-default`, `pr-mode-policy`, `create-pull-request`,
  `janitor`, one 1ES) compile to byte-identical output as the
  prep-PR baseline (the IR is purely refactoring; semantics unchanged).
- `grep -r "##vso\[task.setvariable" src/` returns only the locations
  where the IR's lowering pass legitimately emits the directive
  (`src/compile/ir/lower.rs` and step-output handling); no remaining
  hand-built `setvariable` strings in extensions or `common.rs`.
- `grep -r "dependencies\." src/` returns only the locations where
  `lower_outputref` emits the cross-job/cross-stage reference; no
  hand-built `dependencies.<job>.outputs[...]` strings in extensions.
- `grep -r "\$(synthPr" src/` returns only the lowering code path;
  no hand-built `$(synthPr.X)` references.
- `find src/data -name "*.yml" -not -name "ecosystem_domains.json"`
  returns only `threat-analysis.md` and `init-agent.md` (the four
  `*-base.yml` files are gone).
