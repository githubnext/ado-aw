# Extending the Compiler

_Part of the [ado-aw documentation](../AGENTS.md)._

ado-aw compiles agent markdown into Azure DevOps YAML through the typed pipeline IR in `src/compile/ir/`. New features should add typed declarations and IR nodes, not YAML string fragments.

## Adding New Features

When extending the compiler:

1. **New CLI commands**: add variants to the `Commands` enum in `src/main.rs`, implement dispatch, and add parsing/behavior tests.
2. **New compile targets**: build a typed `Pipeline` IR in a target module under `src/compile/`; use existing `standalone_ir.rs`, `onees_ir.rs`, `job_ir.rs`, and `stage_ir.rs` as references.
3. **New front matter fields**: add fields to `FrontMatter` or nested config types in `src/compile/types.rs`. Breaking changes require a codemod under `src/compile/codemods/`; see [`docs/codemods.md`](codemods.md).
4. **New compiler extensions**: implement the `CompilerExtension` `name` / `phase` / `declarations` trio and return typed `Declarations`.
5. **New safe-output tools**: add to `src/safeoutputs/`, implement the safe-output data model and executor, and register it in MCP and Stage 3 execution wiring.
6. **New first-class tools**: create `src/tools/<name>/` with `mod.rs` and `extension.rs` (`CompilerExtension` impl). Add `execute.rs` if the tool has Stage 3 runtime logic. Extend `ToolsConfig` in `types.rs` and collection in `collect_extensions()`.
7. **New runtimes**: create `src/runtimes/<name>/` with `mod.rs` (config types/helpers) and `extension.rs` (`CompilerExtension` impl). Extend `RuntimesConfig` in `types.rs` and collection in `collect_extensions()`.
8. **Validation**: add compile-time validation for front matter, safe outputs, permissions, and any IR invariants your feature introduces.

## Code organization principles

The codebase follows a colocation principle:

- **Tools** (`tools:` front matter) live in `src/tools/<name>/` — one directory per tool, containing compile-time (`extension.rs`) and optional runtime (`execute.rs`) code.
- **Runtimes** (`runtimes:` front matter) live in `src/runtimes/<name>/` — config and helpers in `mod.rs`, compiler integration in `extension.rs`.
- **Infrastructure extensions** live in `src/compile/extensions/`. These are always-on compiler plumbing, not user-facing tools.
- **Safe outputs** (`safe-outputs:` front matter) live in `src/safeoutputs/`. They follow the Stage 1 NDJSON proposal → Detection → Stage 3 execution lifecycle and are not `CompilerExtension` implementations.

`src/compile/extensions/mod.rs` owns the `CompilerExtension` trait, the `Extension` enum, `Declarations`, and `collect_extensions()`. It re-exports runtime/tool extension types from their colocated modules so target compilers can import extension machinery from one place.

## `CompilerExtension` trait

Runtimes, first-class tools, and always-on compiler infrastructure declare compile-time contributions through `CompilerExtension`:

```rust
pub trait CompilerExtension {
    fn name(&self) -> &str;
    fn phase(&self) -> ExtensionPhase;
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations>;
}
```

`name()` is for diagnostics. `phase()` controls ordering. `declarations()` returns a typed aggregate of everything the extension contributes.

### Phase ordering

Extensions are sorted by `ExtensionPhase` before the compiler merges declarations:

- `System` — compiler-internal infrastructure that later phases depend on (for example `AdoScriptExtension`).
- `Runtime` — language/toolchain installation (`LeanExtension`, `PythonExtension`, `NodeExtension`, `DotnetExtension`).
- `Tool` — first-party tools (`AzureDevOpsExtension`, `CacheMemoryExtension`, `AzureCliExtension`).

System extensions run first, runtimes run before tools, and definition order is preserved within each phase.

### Always-on extensions

`collect_extensions()` always includes:

- `AdoAwMarkerExtension` — embeds ado-aw metadata in compiled YAML.
- `GitHubExtension` — GitHub MCP plumbing.
- `SafeOutputsExtension` — SafeOutputs MCP plumbing.
- `AdoScriptExtension` — gate evaluator, runtime-import resolver, and synthetic PR helpers.
- `ExecContextExtension` — `aw-context/` precompute contributors.
- `AzureCliExtension` — Azure CLI mounts, allowlist entries, and PATH setup.

User-configured runtimes and tools are appended after those always-on extensions, then sorted by phase.

### Declarations

`Declarations` contains typed IR steps plus non-step signals:

```rust
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
    pub agent_env_vars: Vec<(String, String)>,
    pub warnings: Vec<String>,
}
```

Return `Declarations::default()` and fill only the fields your feature owns. Do not add target-specific special cases when the same information can be declared here.

## Building typed steps

Compiler-owned steps should be `Step` variants from `src/compile/ir/step.rs`.

### Bash steps

```rust
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::ids::StepId;
use crate::compile::ir::output::OutputDecl;
use crate::compile::ir::step::{BashStep, Step};

let step = Step::Bash(
    BashStep::new("Prepare tool", "echo preparing")
        .with_id(StepId::new("prepareTool")?)
        .with_env("BUILD_REASON", EnvValue::ado_macro("Build.Reason")?)
        .with_output(OutputDecl::new("TOOL_READY")),
);
```

`BashStep::script` is the raw bash body. Do not include `- bash: |` or YAML indentation; the lowerer and serializer own YAML formatting.

### Task steps

```rust
use crate::compile::ir::step::{Step, TaskStep};

let step = Step::Task(
    TaskStep::new("NodeTool@0", "Install Node.js")
        .with_input("versionSpec", "20.x"),
);
```

Use `TaskStep` for Azure DevOps built-in tasks such as `NodeTool@0`, `UsePythonVersion@0`, and `UseDotNet@2`.

### Download and publish steps

```rust
use crate::compile::ir::step::{DownloadStep, PublishStep, Step};

let download = Step::Download(DownloadStep {
    source: "current".into(),
    artifact: "agent_outputs_$(Build.BuildId)".into(),
    condition: None,
});

let publish = Step::Publish(PublishStep {
    path: "$(Agent.TempDirectory)/agent_outputs".into(),
    artifact: "agent_outputs_$(Build.BuildId)".into(),
    condition: Some(Condition::Always),
});
```

`Step::Publish` lowers differently for 1ES: the 1ES shape collects publishes into `templateContext.outputs` and removes the inline publish step.

### Raw YAML

`Step::RawYaml` is an escape hatch for user-authored setup/teardown YAML that the IR does not model. Prefer typed steps for generated compiler behavior, especially when a step needs env values, conditions, outputs, or graph-derived dependencies.

## Declaring and consuming outputs

A producer declares outputs on `BashStep`:

```rust
let producer = BashStep::new("Resolve PR", script)
    .with_id(StepId::new("synthPr")?)
    .with_output(OutputDecl::new("AW_SYNTHETIC_PR_ID"));
```

A consumer references an output through `OutputRef`:

```rust
let pr_id = OutputRef::new(StepId::new("synthPr")?, "AW_SYNTHETIC_PR_ID");
let step = BashStep::new("Use PR", "echo using PR")
    .with_env("PR_ID", EnvValue::step_output(pr_id));
```

The graph and lowering passes choose the correct Azure DevOps syntax for same-job, cross-job, or cross-stage consumers. Do not hand-code `$(step.var)`, `dependencies.*`, or `stageDependencies.*` unless you are adding a new lowering rule.

The graph pass also derives `dependsOn` edges from these refs, validates that producers and output names exist, detects cycles, and marks producer declarations that need `isOutput=true`.

## Conditions

Use `Condition` and `Expr` from `src/compile/ir/condition.rs`:

```rust
use crate::compile::ir::condition::{Condition, Expr};

let only_pr = Condition::Eq(
    Expr::Variable("Build.Reason".into()),
    Expr::Literal("PullRequest".into()),
);

let condition = Condition::and([
    Condition::Succeeded,
    only_pr,
]);
```

Available forms include `Succeeded`, `Always`, `Failed`, `SucceededOrFailed`, `And`, `Or`, `Not`, `Eq`, `Ne`, and `Custom`. Prefer the AST. Use `Condition::Custom` only for ADO expressions the AST cannot yet model; codegen rejects embedded newlines and pipeline-command markers before emitting custom strings.

`Expr::StepOutput(OutputRef)` participates in the same graph and output-ref lowering path as `EnvValue::StepOutput`.

## Adding a compile target

A compile target should build a complete typed `Pipeline` and then use the shared IR emit path. Follow the existing target builders:

- `src/compile/standalone_ir.rs`
- `src/compile/onees_ir.rs`
- `src/compile/job_ir.rs`
- `src/compile/stage_ir.rs`

Recommended workflow:

1. Parse and validate front matter in `src/compile/types.rs`.
2. Build `CompileContext` and call `collect_extensions()`.
3. Merge extension `Declarations` in phase order.
4. Construct typed `Job`s, `Stage`s, and `Step`s.
5. Choose `PipelineBody::Jobs` or `PipelineBody::Stages`.
6. Choose the appropriate `PipelineShape` or add a new shape if the output wrapper is structurally new.
7. Let `ir::emit` lower through `serde_yaml::Value` and serialize.
8. Add fixture tests for the target's emitted YAML.

Do not create new template files or marker replacement systems for new targets.

## Adding a safe-output tool

Safe-output tools live in `src/safeoutputs/`. Use them when the agent should propose a write action that Detection can inspect and Stage 3 can apply with a write-capable token.

Typical steps:

1. Add `src/safeoutputs/<tool>.rs` with the tool input type, sanitization/validation, `ToolResult`, and `Executor` implementation.
2. Register the module in `src/safeoutputs/mod.rs`.
3. Expose the MCP tool in `src/mcp.rs`.
4. Wire Stage 3 execution in `src/execute.rs` if the executor dispatch table needs an update.
5. Add front-matter configuration if the tool is configurable under `safe-outputs:`.
6. Add tests for validation, NDJSON parsing, MCP handling, and executor behavior.

Safe-output tools are not `CompilerExtension`s. If a safe output also needs compile-time MCP configuration, add that through the always-on `SafeOutputsExtension` declarations.

## Adding a runtime

Runtimes live under `src/runtimes/<name>/`.

1. Add config types and helpers in `mod.rs`.
2. Implement `CompilerExtension` in `extension.rs`.
3. Return installation steps as typed `Step::Task` or `Step::Bash` in `Declarations::agent_prepare_steps`.
4. Return network hosts, bash commands, prompt supplements, env vars, mounts, and warnings through `Declarations` as needed.
5. Extend `RuntimesConfig` in `src/compile/types.rs`.
6. Re-export and collect the extension in `src/compile/extensions/mod.rs`.
7. Add tests for front-matter parsing and generated pipeline IR/YAML.

## Adding a first-class tool

First-class tools live under `src/tools/<name>/`.

1. Add config and helper code in `mod.rs`.
2. Implement `CompilerExtension` in `extension.rs`.
3. Return typed setup, prepare, finalize, detection, or SafeOutputs steps through `Declarations`.
4. Return MCPG servers, allowed Copilot tools, pipeline env mappings, AWF mounts/PATH entries, network hosts, and prompt supplements through the corresponding declaration fields.
5. Add `execute.rs` if the tool also runs in Stage 3.
6. Extend `ToolsConfig` in `src/compile/types.rs` and `collect_extensions()`.
7. Add tests for config parsing, declarations, and emitted pipeline behavior.

## Filter IR (`src/compile/filter_ir.rs`)

Trigger filter expressions still use the separate filter IR. It lowers `PrFilters` / `PipelineFilters` into typed checks, validates conflicts, and emits bash consumed by `AdoScriptExtension` declarations. The generated gate steps are now returned as typed IR steps instead of being spliced into YAML templates.

To add a new filter type:

1. Add a `Fact` variant if the filter needs a new data source.
2. Add a `Predicate` variant if it needs a new test shape.
3. Extend lowering from `PrFilters` or `PipelineFilters` in `filter_ir.rs`.
4. Add validation rules for impossible or redundant combinations.
5. Add lowering, validation, and codegen tests.

## Bash step linting

`tests/bash_lint_tests.rs` compiles representative fixtures and runs `shellcheck` against every literal `bash:` body in generated YAML. When adding or modifying bash:

1. Run `cargo test --test bash_lint_tests` if `shellcheck` is available locally.
2. Fix findings such as unquoted variables, `cd` without failure handling, masked exit codes, and tilde-in-double-quotes.
3. If a finding is intentional, add a `# shellcheck disable=SCxxxx` comment immediately above the line in the bash body.

Do not add blanket `set -eo pipefail` to every step just to satisfy lint. Use targeted fail-fast behavior only when the step requires it.
