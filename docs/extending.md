# Extending the Compiler

_Part of the [ado-aw documentation](../AGENTS.md)._

## Adding New Features

When extending the compiler:

1. **New CLI commands**: Add variants to the `Commands` enum in `main.rs`
2. **New compile targets**: Implement the `Compiler` trait in a new file under `src/compile/`
3. **New front matter fields**: Add fields to `FrontMatter` in `src/compile/types.rs`
4. **New template markers**: Handle replacements in the target-specific compiler (e.g., `standalone.rs` or `onees.rs`)
5. **New safe-output tools**: Add to `src/safeoutputs/` — implement `ToolResult`, `Executor`, register in `mod.rs`, `mcp.rs`, `execute.rs`
6. **New first-class tools**: Create `src/tools/<name>/` with `mod.rs` and `extension.rs` (CompilerExtension impl). Add `execute.rs` if the tool has Stage 3 runtime logic. Extend `ToolsConfig` in `types.rs`, add collection in `collect_extensions()`
7. **New runtimes**: Create `src/runtimes/<name>/` with `mod.rs` (config types) and `extension.rs` (CompilerExtension impl). Extend `RuntimesConfig` in `types.rs`, add collection in `collect_extensions()`
8. **Validation**: Add compile-time validation for safe outputs and permissions

### Code Organization Principles

The codebase follows a **colocation** principle for tools and runtimes:

- **Tools** (`tools:` front matter) live in `src/tools/<name>/` — one directory per tool, containing both compile-time (`extension.rs`) and runtime (`execute.rs`) code. This means you can look at a single directory to understand everything a tool does.
- **Runtimes** (`runtimes:` front matter) live in `src/runtimes/<name>/` — one directory per runtime, with config types in `mod.rs` and the `CompilerExtension` impl in `extension.rs`.
- **Infrastructure extensions** (GitHub MCP, SafeOutputs MCP) that are always-on and not user-configured stay in `src/compile/extensions/`. These are internal plumbing, not user-facing tools.
- **Safe outputs** (`safe-outputs:` front matter) stay in `src/safeoutputs/` — they follow a different lifecycle (Stage 1 NDJSON → Stage 3 execution) and are not `CompilerExtension` implementations.

The `src/compile/extensions/mod.rs` file owns the `CompilerExtension` trait, the `Extension` enum, and `collect_extensions()`. It re-exports tool/runtime extension types from their colocated homes so the rest of the compiler can import them from a single path.

### `CompilerExtension` Trait

Runtimes and first-party tools declare their compilation requirements via the `CompilerExtension` trait (`src/compile/extensions/mod.rs`). Instead of scattering special-case `if` blocks across the compiler, each runtime/tool implements this trait and the compiler collects requirements generically:

```rust
pub trait CompilerExtension: Send {
    fn name(&self) -> &str;                                    // Display name
    fn phase(&self) -> ExtensionPhase;                         // Runtime (0) < Tool (1)
    fn required_hosts(&self) -> Vec<String>;                   // AWF network allowlist
    fn required_bash_commands(&self) -> Vec<String>;           // Agent bash allow-list
    fn prompt_supplement(&self) -> Option<String>;              // Agent prompt markdown
    fn prepare_steps(&self) -> Vec<String>;                    // Execution job steps (install, etc.)
    fn setup_steps(&self, ctx: &CompileContext) -> Vec<String>; // Setup job steps (gates, pre-checks)
    fn mcpg_servers(&self, ctx: &CompileContext) -> Result<Vec<(String, McpgServerConfig)>>; // MCPG entries
    fn allowed_copilot_tools(&self) -> Vec<String>;            // --allow-tool values
    fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>>; // Compile-time warnings/errors
    fn required_pipeline_vars(&self) -> Vec<PipelineEnvMapping>; // Container env var mappings
    fn required_awf_mounts(&self) -> Vec<AwfMount>;            // AWF Docker volume mounts
    fn awf_path_prepends(&self) -> Vec<String>;                // Directories to add to chroot PATH
}
```

**`prepare_steps()` vs `setup_steps()`**: `prepare_steps()` injects into the
Execution job (before the agent runs). `setup_steps()` injects into the Setup
job (before the Execution job starts). Use `setup_steps()` for pre-activation
gates or checks that must complete before the agent is launched.

**Phase ordering**: Extensions are sorted by phase — runtimes
(`ExtensionPhase::Runtime`) execute before tools (`ExtensionPhase::Tool`).
This guarantees runtime install steps run before tool steps that may depend
on them.

To add a new runtime or tool: (1) create a directory under `src/tools/` or `src/runtimes/`, (2) implement `CompilerExtension` in `extension.rs`, (3) add a variant to the `Extension` enum and a collection check in `collect_extensions()` in `src/compile/extensions/mod.rs`.

### Filter IR (`src/compile/filter_ir.rs`)

Trigger filter expressions (PR filters, pipeline filters) are compiled to bash
gate steps via a three-pass IR pipeline:

1. **Lower** — `PrFilters` / `PipelineFilters` → `Vec<FilterCheck>` (typed
   predicates over typed facts)
2. **Validate** — detect conflicts at compile time (impossible combinations,
   redundant checks)
3. **Codegen** — dependency-ordered fact acquisition + predicate evaluation →
   bash gate step

To add a new filter type:

1. **Add a `Fact` variant** (if the filter needs a new data source) — implement
   `dependencies()`, `kind()`, `ado_exports()`, and
   `failure_policy()` on the new variant
2. **Add a `Predicate` variant** (if the filter needs a new test shape) —
   implement the codegen match arm in `emit_predicate_check()`
3. **Extend lowering** — add the filter field to `PrFilters` or
   `PipelineFilters` in `types.rs`, then add the lowering logic in
   `lower_pr_filters()` or `lower_pipeline_filters()` in `filter_ir.rs`
4. **Add validation rules** — check for conflicts with other filters in
   `validate_pr_filters()` or `validate_pipeline_filters()`
5. **Write tests** — lowering test, validation test, and codegen test in
   `filter_ir.rs`
