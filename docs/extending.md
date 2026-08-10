# Extending the Compiler

_Part of the [ado-aw documentation](../AGENTS.md)._

ado-aw compiles agent markdown into Azure DevOps YAML through the typed pipeline IR in `src/compile/ir/`. New features should add typed declarations and IR nodes, not YAML string fragments.

## Adding New Features

When extending the compiler:

1. **New CLI commands**: add variants to the `Commands` enum in `src/main.rs`, implement dispatch, and add parsing/behavior tests.
2. **New compile targets**: build a typed `Pipeline` IR in a target wrapper module under `src/compile/` (use existing `standalone_ir.rs`, `onees_ir.rs`, `job_ir.rs`, and `stage_ir.rs` as references). The canonical Setup → Agent → Detection → SafeOutputs → Teardown shape, plus the optional Conclusion job, lives in `src/compile/agentic_pipeline.rs` and is reused by every target — wrappers only set the per-target `PipelineShape` and lift the shared `BuiltPipelineContext` into the right envelope.
3. **New front matter fields**: add fields to `FrontMatter` or nested config types in `src/compile/types.rs`. Breaking changes require a codemod under `src/compile/codemods/`; see [`docs/codemods.md`](codemods.md).
4. **New compiler extensions**: implement `name()` and `phase()`; override `declarations()` (which defaults to `Ok(Declarations::default())`) when the extension contributes steps, hosts, tools, or other signals.
5. **New safe-output tools**: add to `src/safe_outputs/`, implement the safe-output data model and executor, and register it in MCP and Stage 3 execution wiring.
6. **New first-class tools**: create `src/tools/<name>/` with `mod.rs` and `extension.rs` (`CompilerExtension` impl). Add `execute.rs` if the tool has Stage 3 runtime logic. Extend `ToolsConfig` in `types.rs` and collection in `collect_extensions()`.
7. **New runtimes**: create `src/runtimes/<name>/` with `mod.rs` (config types/helpers) and `extension.rs` (`CompilerExtension` impl). Extend `RuntimesConfig` in `types.rs` and collection in `collect_extensions()`.
8. **Validation**: add compile-time validation for front matter, safe outputs, permissions, and any IR invariants your feature introduces.

## Code organization principles

The codebase follows a colocation principle:

- **Tools** (`tools:` front matter) live in `src/tools/<name>/` — one directory per tool, containing compile-time (`extension.rs`) and optional runtime (`execute.rs`) code.
- **Runtimes** (`runtimes:` front matter) live in `src/runtimes/<name>/` — config and helpers in `mod.rs`, compiler integration in `extension.rs`.
- **Infrastructure extensions** live in `src/compile/extensions/`. These are always-on compiler plumbing, not user-facing tools.
- **Safe outputs** (`safe-outputs:` front matter) live in `src/safe_outputs/`. They follow the Stage 1 NDJSON proposal → Detection → Stage 3 execution lifecycle and are not `CompilerExtension` implementations.

`src/compile/extensions/mod.rs` owns the `CompilerExtension` trait, the `Extension` enum, `Declarations`, and `collect_extensions()`. It re-exports runtime/tool extension types from their colocated modules so target compilers can import extension machinery from one place.

## `CompilerExtension` trait

Runtimes, first-class tools, and always-on compiler infrastructure declare compile-time contributions through `CompilerExtension`:

```rust
pub trait CompilerExtension {
    fn name(&self) -> &str;
    fn phase(&self) -> ExtensionPhase;
    /// Default returns `Ok(Declarations::default())` — override when the
    /// extension contributes steps, hosts, tools, or other signals.
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations> {
        let _ = ctx;
        Ok(Declarations::default())
    }
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
    pub agent_prepare_steps: Vec<Step>,   // steps before the agent invocation
    pub setup_steps: Vec<Step>,           // steps in the Setup job
    // Reserved for future use — present in the struct but no compile-target reads these yet.
    // Do not populate them; steps placed here will be silently dropped.
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
    /// Clauses AND-ed into the Agent job's `condition:` by `fold_agent_conditions`.
    pub agent_conditions: Vec<Condition>,
}
```

Return `Declarations::default()` and fill only the fields your feature owns. The three fields marked "reserved for future use" (`agent_finalize_steps`, `detection_prepare_steps`, `safe_outputs_steps`) exist in the struct but are not currently read by any compile target — steps placed in them will be silently dropped. Do not add target-specific special cases when the same information can be declared here.

## Building typed steps

Compiler-owned steps should be `Step` variants from `src/compile/ir/step.rs`.

### Bash steps

Compiler-generated shell must go through `ShellScript` — see [Generated shell scripts](#generated-shell-scripts) below. Construct `BashStep::new` directly only for a body with no substitution at all.

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

### Steps that run an ado-script bundle

If your step invokes an ado-script Node bundle (`/tmp/ado-aw-scripts/ado-script/*.js`),
model its env contract via [`src/compile/ado_bundle.rs`](../src/compile/ado_bundle.rs)
rather than hand-writing the auth env:

- Add a `Bundle` variant with its `path()` and `auth()` (`BundleAuth::Bearer`
  if the bundle reads `SYSTEM_ACCESSTOKEN` — for ADO REST via `getWebApi()`
  and/or git bearer auth — else `BundleAuth::None`).
- Project the bearer with `apply_bundle_auth(step, Bundle::X, token)` — never
  `.with_env("SYSTEM_ACCESSTOKEN", …)` by hand. Use `token_source_for(write_sc)`
  to pick the token source.
- Do **not** re-project ADO predefined variables (`System.*` / `Build.*`): ADO
  auto-injects them into every script step's env under their SCREAMING_SNAKE
  names, so the bundle reads them directly. Only genuinely computed inputs
  (base64 specs, `AW_*` config, `PARAM_*`) belong in `.with_env`. The contract
  tests and the churn guard in `tests/compiler_tests.rs` enforce this.



```rust
use crate::compile::ir::step::Step;
use crate::compile::ir::tasks::publish_test_results::{PublishTestResults, TestResultsFormat};

let step = Step::Task(
    PublishTestResults::new(TestResultsFormat::JUnit, "**/TEST-*.xml")
        .test_run_title("Unit Tests")
        .into_step(),
);
```

Use `TaskStep` for Azure DevOps built-in tasks. When the task is compiler-generated, add or reuse a typed builder struct in `src/compile/ir/tasks/` (one file per task) so required inputs are positional, optional inputs are typed setters, and call sites do not hand-construct `TaskStep::new(...)` with raw task/input strings. Each builder exposes `new(<required>)`, typed chained setters, and `into_step() -> TaskStep`; constrained values are typed enums and bool inputs take `bool`. Command/mode-dispatch tasks (e.g. `Docker@2`) use a command enum with per-variant data — model new ones on `src/compile/ir/tasks/docker.rs`.

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

A compile target should build a complete typed `Pipeline` and then use the shared IR emit path. Follow the existing target wrapper modules — they delegate the heavy lifting to `src/compile/agentic_pipeline.rs::build_pipeline_context` and only handle the per-target envelope:

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

Safe-output tools live in `src/safe_outputs/`. Use them when the agent should propose a write action that Detection can inspect and Stage 3 can apply with a write-capable token.

Typical steps:

1. Add `src/safe_outputs/<tool>.rs` with the tool input type, sanitization/validation, `ToolResult`, and `Executor` implementation.
2. Register the module in `src/safe_outputs/mod.rs`.
3. Expose the MCP tool in `src/mcp.rs`.
4. Wire Stage 3 execution in `src/execute.rs` if the executor dispatch table needs an update.
5. Add front-matter configuration if the tool is configurable under `safe-outputs:`.
6. Add tests for validation, NDJSON parsing, MCP handling, and executor behavior.

> **Type path/identifier `Params` fields with validated newtypes.** If your tool's input holds a file path, git ref, commit SHA, artifact name, or similar identifier, use a newtype from [`src/secure.rs`](../src/secure.rs) (`RelativeSafePath`, `StrictRelativePath`, `PathSegment`, `GitRefName`, `BranchName`, `CommitSha`, `ArtifactName`, `Identifier`, `HostName`, `Version`) instead of a raw `String`. These wrap the canonical primitives in [`src/validate.rs`](../src/validate.rs) and run them at deserialization time, so the path-traversal / injection / format checks are applied automatically and cannot be silently omitted. Reserve the manual `validate()` method for cross-field and semantic rules (e.g. positive IDs, length minimums).

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

## Generated shell scripts

Every shell script the compiler emits lives in `src/compile/shell/` as a
registered raw-string constant, not as a `format!` template. `format!` forced
three layers of escaping onto a script at once — `\n\` continuations, doubled
braces to survive `format!` itself, and an escaped quote for every quoted word
— which made a long body impossible to review as shell. Reviewing it as shell
is the only way to know it is correct.

### Declaring a script

```rust
use crate::shell_script;
use crate::compile::shell::{Binding, ShellScript};

shell_script! {
    /// One line on why this script exists.
    STOP_ADO_PROXY {
        interpreter: Bash,           // or Sh
        bindings: [PROXY_CONTAINER], // the compiler supplies these
        externals: [SOME_ENV_VAR],   // the runtime supplies these
        fragments: [],               // shell composed in from elsewhere
        body: r#"
docker rm -f "$PROXY_CONTAINER" 2>/dev/null || true
"#,
    }
}

ShellScript::new(&STOP_ADO_PROXY)
    .text("PROXY_CONTAINER", ADO_PROXY_CONTAINER_NAME)
    .into_step("Stop ado-proxy")
```

The `body:` is the shell **exactly as it will run**. Nothing is escaped: a
Docker Go-template is written literally as `{{.State.Status}}`.

### Bindings

A value can only ever be the right-hand side of an assignment in the generated
prelude. That is the one position where a value's own quoting fully determines
its meaning, so it cannot alter the structure of the script no matter what it
contains. Pick the constructor that matches the shape:

| Constructor | Renders as | Use for |
|---|---|---|
| `Binding::text(s)` | `'s'` | any single-line literal |
| `Binding::number(n)` | `n` | ports, counts, timeouts |
| `Binding::boolean(b)` | `true` / `false` | flags read as `[ "$V" = true ]` |
| `Binding::words([…])` | `'a b c'` | a list the body expands unquoted in `for` |
| `Binding::ado_macro("Agent.TempDirectory")` | `'$(Agent.TempDirectory)'` | ADO predefined variables |
| `Binding::ado_path(p)` | `'$(Pipeline.Workspace)/x'` | a path built around one |
| `Binding::document(text)` | quoted heredoc | JSON, prompts, certificates |

Each validates its own shape: `words` rejects an entry containing whitespace
or a glob (the consumer expands it unquoted, so that would silently change the
list), `ado_macro` accepts only a well-formed dotted name, and `ado_path`
checks every embedded `$(…)` is such a name — so the value can only ever
expand to a variable Azure DevOps substitutes, never to a command the runner
executes.

### Declaring the variable surface

Every variable the body reads must be declared as a `binding` (the compiler
supplies it) or an `external` (the runtime does — step `env:`, an ADO
`##vso[task.setvariable]` from an earlier step, or a fragment). Anything the
body assigns itself needs no declaration.

This is enforced two ways: `ShellScript::render` refuses to render with a
declared binding unbound, and a registry-wide test fails on a body that reads
an undeclared variable.

### Secrets

A credential must never become a binding. The prelude is written verbatim into
the `*.lock.yml` committed to the repository. `Binding` rejects values naming a
known credential; credentials arrive through `env:` as `EnvValue::secret`,
which Azure DevOps masks in logs.

```rust
ShellScript::new(&START_ADO_PROXY)
    .text("PROXY_CONTAINER", ADO_PROXY_CONTAINER_NAME)
    .into_step("Start ado-proxy policy engine")
    .with_env("ADO_PROXY_BEARER", EnvValue::secret("SC_READ_TOKEN"))
```

### Composing a long script from phases

A script too long to review whole is assembled from registered phases spliced
at markers:

```rust
body: r#"
set -euo pipefail
# ado-aw:fragment resolve_org
echo "$ADO_PROXY_ORGANIZATION"
"#,
```

```rust
.fragment("resolve_org", common::resolve_ado_organization_bash())
```

A marker is an ordinary shell comment, so the outline body stays valid,
shellcheck-able shell whether or not the fragment is spliced. Any variable a
fragment defines must be declared in the consumer's `externals:`, which forces
the inter-phase contract somewhere a reviewer can see it. Declaring a fragment
without marking it (or vice versa) is a test failure, not a silent no-op.

### Reviewing the scripts as files

```bash
cargo run -- export-bash-scripts --out /tmp/ado-aw-shell
cargo run -- export-bash-scripts --out /tmp/ado-aw-shell --format json
```

Writes one `.sh` per registered script with a provenance header naming the
producing Rust source, for review with ordinary shell tooling.

## Filter IR (`src/compile/filter_ir.rs`)

Trigger filter expressions still use the separate filter IR. It lowers `PrFilters` / `PipelineFilters` into typed checks, validates conflicts, and emits bash consumed by `AdoScriptExtension` declarations. The generated gate steps are now returned as typed IR steps instead of being spliced into YAML templates.

To add a new filter type:

1. Add a `Fact` variant if the filter needs a new data source.
2. Add a `Predicate` variant if it needs a new test shape.
3. Extend lowering from `PrFilters` or `PipelineFilters` in `filter_ir.rs`.
4. Add validation rules for impossible or redundant combinations.
5. Add lowering, validation, and codegen tests.

## Bash step linting

Generated shell is linted at two levels, and both matter.

**Every registered script, in isolation** (`src/compile/shell/lint.rs`). Reads
the registry directly, so it reaches every script whether or not any pipeline
emits it. Declared bindings and externals are stub-assigned so SC2154 still
fires for a variable the body reads without declaring. This closes a real gap:
lint coverage used to be a function of fixture reachability, so a generator no
fixture happened to exercise — including several hundred lines of `ado-proxy`
and `az` wrapper shell — was linted by nothing.

**Every bash body that reaches the emitted YAML** (`tests/bash_lint_tests.rs`).
Compiles representative fixtures and shellchecks what actually ships. This
proves scripts are *emitted*, where the registry lint proves they are *correct*.

When adding or modifying shell:

1. Run `ENFORCE_BASH_LINT=1 cargo test --test bash_lint_tests` and
   `cargo test --bin ado-aw compile::shell` if `shellcheck` is available locally.
2. Fix findings such as unquoted variables, `cd` without failure handling,
   masked exit codes, and tilde-in-double-quotes.
3. If a finding is intentional, add a `# shellcheck disable=SCxxxx` comment
   immediately above the line in the body.

Do not add blanket `set -eo pipefail` to every step just to satisfy lint. Use
targeted fail-fast behavior only when the step requires it.
