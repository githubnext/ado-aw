# Pipeline IR

_Part of the [ado-aw documentation](../AGENTS.md)._

ado-aw no longer compiles pipelines by substituting strings into YAML template files. Every production target builds a typed Azure DevOps pipeline IR, resolves graph-level facts, lowers that IR to `serde_yaml::Value`, and serializes once with `serde_yaml::to_string`.

The implementation lives under `src/compile/ir/`. The canonical agentic-pipeline shape (Setup → Agent → Detection → SafeOutputs → Teardown, plus an optional always-running Conclusion job when `conclusion:` is configured) lives in `src/compile/agentic_pipeline.rs` and is shared by every target. Per-target wrappers handle only the envelope:

- `src/compile/standalone_ir.rs`
- `src/compile/onees_ir.rs`
- `src/compile/job_ir.rs`
- `src/compile/stage_ir.rs`

Those wrappers are the only place per-target shape (top-level `PipelineShape`, template parameters, 1ES `extends:`) should be assembled. Shared canonical-shape logic belongs in `agentic_pipeline.rs`. Shared target logic should be typed IR construction helpers, not string fragments.

## Module layout

`src/compile/ir/` is split by responsibility:

- `ids.rs` — typed `StageId`, `JobId`, and `StepId` newtypes. Constructors validate the ADO identifier grammar (`^[A-Za-z_][A-Za-z0-9_]*$`) so invalid names fail at compile time.
- `step.rs` — `Step` and concrete step structs: `BashStep`, `TaskStep`, `CheckoutStep`, `DownloadStep`, and `PublishStep`.
- `tasks.rs` — typed factory helpers for built-in ADO tasks that return preconfigured `TaskStep` values with required inputs set. Prefer extending these helpers for compiler-generated tasks rather than open-coding `TaskStep::new(...)` with raw string keys at each call site.
- `job.rs` — `Job`, `Pool`, job variables, 1ES `templateContext` support, and target-job external `dependsOn` / `condition` wrapping.
- `stage.rs` — `Stage` plus target-stage external `dependsOn` / `condition` wrapping.
- `env.rs` — typed environment values (`EnvValue`) including ADO macros, pipeline variables, secrets, `OutputRef`s, `Coalesce`, macro-form `Concat`, and `RuntimeExpression` (a `$[ ... ]` ADO runtime expression that the lowering pass auto-hoists to a job-level `variables:` entry — ADO does not evaluate `$[ ... ]` inside step `env:`). `RuntimeExpression` is only valid at the top level of a step `env:` value: nesting it inside `Concat` or `Coalesce` is rejected at lower time (the hoist pass walks only the top level, so a nested occurrence would emit a dangling `$(AwRtExpr_…)` macro). A `Literal` or `RawYamlScalar(String)` smuggling a raw `$[ ... ]` into a step `env:` value (whether at the top level or nested inside a `Concat`) is likewise rejected at lower time — ADO passes such scalars verbatim, so the typed `RuntimeExpression` variant must be used instead.
- `condition.rs` — the `Condition` / `Expr` AST and code generation to ADO condition syntax.
- `output.rs` — `OutputDecl`, `OutputRef`, and the output-reference lowering rules.
- `graph.rs` — graph construction, `dependsOn` derivation, output validation, `isOutput=true` promotion, and cycle detection.
- `validate` pass — there is no separate `validate.rs` module in the current tree; graph invariants live in `graph.rs`, shape checks live near the relevant lowering code in `lower.rs`, and target-specific validation stays in the target builder.
- `lower.rs` — converts typed IR to a `serde_yaml::Value` tree.
- `emit.rs` — calls `lower::lower()` and `serde_yaml::to_string()` for canonical YAML output.

## Top-level pipeline types

The root type is `Pipeline` in `src/compile/ir/mod.rs`:

```rust
pub struct Pipeline {
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub resources: Resources,
    pub triggers: Triggers,
    pub variables: Vec<PipelineVar>,
    pub body: PipelineBody,
    pub shape: PipelineShape,
}
```

`PipelineBody` captures whether the emitted document has a top-level `jobs:` block or a top-level `stages:` block:

```rust
pub enum PipelineBody {
    Jobs(Vec<Job>),
    Stages(Vec<Stage>),
}
```

`PipelineShape` captures the wrapping rules that used to be split across template files:

```rust
pub enum PipelineShape {
    Standalone,
    OneEs { sdl, top_level_pool, stage_id, stage_display_name },
    JobTemplate { external_params },
    StageTemplate { external_params },
}
```

Shape is intentionally separate from body. For example, the 1ES target still builds the canonical job graph as `PipelineBody::Jobs`; the lowering pass wraps those jobs under the 1ES `extends.parameters.stages[0].jobs` shape.

## Steps

All generated pipeline steps should use typed variants from `src/compile/ir/step.rs`:

```rust
pub enum Step {
    Bash(BashStep),
    Task(TaskStep),
    Checkout(CheckoutStep),
    Download(DownloadStep),
    Publish(PublishStep),
    RawYaml(String),
}
```

Use the typed structs whenever the compiler owns the step:

- `Step::Bash` for inline bash (`BashStep::script` is the raw body, not a YAML block).
- `Step::Task` for ADO task invocations such as `UseNode@1`, `UsePythonVersion@0`, or `UseDotNet@2`. For compiler-generated built-in tasks, prefer `src/compile/ir/tasks.rs` factory helpers over ad-hoc `TaskStep::new(...)` calls.
- `Step::Checkout` for `checkout:` steps.
- `Step::Download` for pipeline-artifact downloads.
- `Step::Publish` for pipeline-artifact publishes. Under 1ES, lowering moves publish steps into `templateContext.outputs` so artifacts are published by the 1ES template machinery exactly once.
- `Step::RawYaml` is reserved for user-authored setup/teardown YAML that the IR does not model. Do not use it for compiler-generated steps that need output refs, conditions, env rewriting, or graph-derived dependencies.

`BashStep` and `TaskStep` carry common compiler-owned fields:

- `id: Option<StepId>` — emitted as ADO step `name:`; required when another step consumes an output from this step.
- `display_name: String` — emitted as `displayName:`.
- `env: IndexMap<String, EnvValue>` — typed environment values.
- `condition: Option<Condition>` — typed ADO condition AST.
- `timeout: Option<Duration>` and `continue_on_error: bool`.
- `outputs: Vec<OutputDecl>` on `BashStep`.

Example:

```rust
let synth = Step::Bash(
    BashStep::new("Resolve synthetic PR", script)
        .with_id(StepId::new("synthPr")?)
        .with_output(OutputDecl::new("AW_SYNTHETIC_PR_ID"))
        .with_env("BUILD_REASON", EnvValue::ado_macro("Build.Reason")?),
);
```

## Output declarations and references

A producer declares a step output with `OutputDecl`:

```rust
OutputDecl::new("AW_SYNTHETIC_PR_ID")
OutputDecl::secret("MCP_GATEWAY_API_KEY")
```

A consumer references it with `OutputRef`:

```rust
let r = OutputRef::new(StepId::new("synthPr")?, "AW_SYNTHETIC_PR_ID");
EnvValue::step_output(r)
```

The consumer does not choose the ADO expression syntax. `output.rs::lower_outputref()` chooses the correct syntax from the consumer and producer locations:

| Consumer vs. producer | Lowered syntax |
| --- | --- |
| Same job | `$(stepName.X)` |
| Sibling job in the same stage, or both jobs are stage-less | `dependencies.<job>.outputs['stepName.X']` |
| Different stage | `stageDependencies.<stage>.<job>.outputs['stepName.X']` |

This rule exists because Azure DevOps output variables are context-sensitive. The historical `synthPr` failures came from hand-written code using the wrong reference form for the consumer location. The IR centralizes that choice so new compiler code declares what it needs (`OutputRef`) rather than guessing how ADO will expose it.

`graph.rs` also sets `OutputDecl::auto_is_output = true` when any consumer reads the declaration. The producer can then emit `##vso[task.setvariable ...;isOutput=true]` only when cross-step visibility is actually needed.

## Graph pass

`graph.rs::resolve()` is the all-in-one pass for dependency derivation:

1. Index every named step and its declared outputs.
2. Walk every `EnvValue::StepOutput`, every output nested inside `EnvValue::Coalesce` / `EnvValue::Concat`, and every `Expr::StepOutput` inside conditions.
3. Validate that each reference names an existing step with a matching `OutputDecl`.
4. Lift step-output edges into job-level and stage-level dependencies.
5. Detect cycles in the derived job and stage graphs.
6. Merge the derived edges into `Job::depends_on` and `Stage::depends_on` while preserving any explicit values a target builder supplied.
7. Mark producer outputs that need `isOutput=true`.

Same-job refs do not produce `dependsOn` entries because ADO orders steps by position. Cross-job refs add `Job::depends_on`; cross-stage refs add `Stage::depends_on`. The lowering pass reads those fields and emits canonical `dependsOn:` blocks.

## Conditions

`condition.rs` defines a small AST for ADO conditions:

```rust
pub enum Condition {
    Succeeded,
    Always,
    Failed,
    SucceededOrFailed,
    And(Vec<Condition>),
    Or(Vec<Condition>),
    Not(Box<Condition>),
    Eq(Expr, Expr),
    Ne(Expr, Expr),
    Custom(String),
}

pub enum Expr {
    Literal(String),
    Variable(String),
    StepOutput(OutputRef),
}
```

Use constructors such as `Condition::and([...])`, `Condition::or([...])`, and `Condition::not(...)` when composing nested expressions. Codegen flattens nested `And` / `Or` nodes and quotes string literals for ADO expression syntax:

```rust
Condition::Eq(
    Expr::Variable("Build.Reason".into()),
    Expr::Literal("PullRequest".into()),
)
```

lowers to:

```text
eq(variables['Build.Reason'], 'PullRequest')
```

`Expr::StepOutput` uses the same location-aware output-ref lowering as `EnvValue::StepOutput`. `Condition::Custom` is an escape hatch for expressions not yet modeled by the AST; codegen rejects embedded newlines and ADO pipeline-command markers (`##vso[`, `##[`) before emitting it.

## Extension declarations

The extension trait lives in `src/compile/extensions/mod.rs` and now has exactly three surface methods:

```rust
pub trait CompilerExtension {
    fn name(&self) -> &str;
    fn phase(&self) -> ExtensionPhase;
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations>;
}
```

`Declarations` is the typed aggregate for every signal an extension contributes:

- `agent_prepare_steps: Vec<Step>`
- `setup_steps: Vec<Step>`
- `agent_finalize_steps: Vec<Step>`
- `detection_prepare_steps: Vec<Step>`
- `safe_outputs_steps: Vec<Step>`
- `network_hosts: Vec<String>`
- `bash_commands: Vec<String>`
- `prompt_supplement: Option<String>`
- `mcpg_servers: Vec<(String, McpgServerConfig)>`
- `copilot_allow_tools: Vec<String>`
- `pipeline_env: Vec<PipelineEnvMapping>`
- `awf_mounts: Vec<AwfMount>`
- `awf_path_prepends: Vec<String>`
- `agent_env_vars: Vec<(String, String)>`
- `warnings: Vec<String>`

Extension phases are `System`, `Runtime`, and `Tool`. The compiler sorts extensions by phase before merging declarations, so internal system plumbing lands first, runtime installs land before user tools, and tool extensions can assume requested runtimes are available.

Always-on extensions are collected in `collect_extensions()` before user-configured runtimes/tools:

- `AdoAwMarkerExtension`
- `GitHubExtension`
- `SafeOutputsExtension`
- `AdoScriptExtension`
- `ExecContextExtension`
- `AzureCliExtension`

## Lowering and emission

`lower.rs::lower()` builds and validates a `Graph`, then converts the typed `Pipeline` into a `serde_yaml::Value` tree. The lowerer owns ADO wire shapes and canonical ordering: top-level identity and configuration keys first, then `jobs:` / `stages:`, with target-specific wrapping based on `PipelineShape`.

`emit.rs::emit()` is intentionally thin:

```rust
pub fn emit(pipeline: &Pipeline) -> Result<String> {
    let value = super::lower::lower(pipeline)?;
    serde_yaml::to_string(&value)
}
```

This gives all targets one serialization path and one canonical YAML style. Target compilers should return a complete typed `Pipeline`; they should not format YAML directly.

## Per-target compilers

The production target wrappers are:

- `standalone_ir.rs` — wraps the canonical shape in a top-level standalone pipeline.
- `onees_ir.rs` — wraps the same canonical shape with `PipelineShape::OneEs`, causing the lowerer to emit the 1ES `extends:` wrapper and `templateContext` outputs.
- `job_ir.rs` — wraps the canonical shape as a target-job template with external `dependsOn` / `condition` template parameters.
- `stage_ir.rs` — wraps the canonical shape as a target-stage template with the stage-level external-parameter wrapper.

The canonical Setup → Agent → Detection → SafeOutputs → Teardown shape, plus the optional Conclusion job, lives in `agentic_pipeline.rs` and is reused unchanged by every wrapper above; extensions plug into it via `Declarations` (steps, env, hosts, MCPG entries, and Agent-job condition clauses — see `Declarations::agent_conditions`).

When adding a target, follow the same pattern: parse and validate front matter, collect extension `Declarations`, build typed jobs/stages/steps, set the correct `PipelineShape`, and call the shared emit path.

## Public JSON summary (`ir::summary`)

The internal IR types (`Pipeline`, `Job`, `Step`, `Graph`, …) are
intentionally tied to the compiler's lowering needs and are **not**
public API. To give agent-facing tooling a stable view of a compiled
pipeline, `src/compile/ir/summary.rs` defines a parallel
**summary tree** with `#[derive(Serialize)]` that is consumed by:

- `ado-aw inspect <source> [--json]` — top-level pipeline summary.
- `ado-aw graph dump <source> [--format text|json|dot]` — resolved
  dependency graph (subset of the summary).
- `ado-aw graph deps <source> <step-id>` and `ado-aw graph outputs
  <source>` — focused graph queries over step dependencies and output
  declaration/reference edges.
- `ado-aw whatif <source> --fail <step-id-or-job-id>` — static
  downstream skip classification from graph reachability and rendered
  conditions.
- The `ado-aw audit` JSON (`AuditData.pipeline_graph`) and the
  author-MCP server.

### Stability contract

`PipelineSummary::schema_version` (currently `1`) is the public schema
version. **Bump** it when the JSON shape changes in a way a downstream
consumer would notice (renamed field, removed variant, changed
semantics). Additive changes like new optional fields do not require a
bump. New enum variants currently do require a schema-version bump
because the serialized enums do not have catch-all `Unknown` variants.

The summary is the public schema. Internal IR types may change freely
without bumping the summary version, as long as the summary lowering
keeps the existing field set populated correctly.

### Shape

```jsonc
{
  "schema_version": 1,
  "name": "<pipeline name>",
  "shape": "standalone" | "1es" | "job-template" | "stage-template",
  "body": { "kind": "jobs", "jobs": [...] }
              // OR
           { "kind": "stages", "stages": [...] },
  "graph": {
    "step_locations": [{ "step", "stage?", "job", "outputs": [...] }],
    "job_edges":      [{ "consumer", "producer" }],  // consumer dependsOn producer
    "stage_edges":    [{ "consumer", "producer" }],
    "outputs_needing_is_output": [{ "step", "outputs": [...] }]
  }
}
```

Per-`JobSummary`: `id`, `stage?`, `display_name`, `depends_on`,
`condition?` (lowered ADO condition string), `pool`, `steps`.

Per-`StepSummary`: `id?`, `kind` (`bash` / `task` / `checkout` /
`download` / `publish` / `raw_yaml`), `display_name?`, `task?`,
`condition?`, `outputs[]` (`{name, is_secret, auto_is_output}`),
`env_refs[]` (`{step, name}`), `condition_refs[]` (`{step, name}`).

`condition?` is the lowered ADO condition string (e.g.
`"eq(dependencies.Detection.outputs['threatAnalysis.SafeToProcess'], 'true')"`),
not the typed AST — consumers don't need the AST to reason about
"would this run if X failed?".

### Construction

```rust
let (front_matter, pipeline) = ado_aw::compile::build_pipeline_ir(&source).await?;
let summary = ado_aw::compile::ir::summary::PipelineSummary::from_pipeline(&pipeline)?;
let json = serde_json::to_string_pretty(&summary)?;
```

`build_pipeline_ir` is the public read-only entry point: it parses
and sanitises front matter, runs the same target dispatch as
`compile_pipeline`, and returns the typed `Pipeline` without writing
any YAML. `PipelineSummary::from_pipeline` runs the graph pass
(reusing `graph::build_graph` for validation + edge derivation) and
populates `auto_is_output` for any output that has at least one
cross-step consumer — without mutating the input pipeline.
