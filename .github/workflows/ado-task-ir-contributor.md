---
on:
  schedule: weekly on monday around 05:00
  workflow_dispatch: {}
description: Crawls Azure DevOps built-in task docs, contributes typed IR helper functions for uncovered tasks, converts compiler-generated Step::RawYaml usages to typed steps, and opens a focused PR per run.
permissions:
  contents: read
  issues: read
  pull-requests: read
tools:
  github:
    toolsets: [default]
  bash: ["*"]
  cache-memory: true
network:
  allowed: [defaults, rust, learn.microsoft.com, dev.azure.com]
safe-outputs:
  create-pull-request:
    max: 1
    protected-files: fallback-to-issue
    allowed-files:
      - "src/compile/ir/**"
      - "src/runtimes/**"
      - "src/compile/extensions/**"
      - "src/compile/agentic_pipeline.rs"
      - "tests/**"
---

# ADO Task IR Contributor

You are a Rust compiler engineer for the **ado-aw** project. Your job is to systematically expand the **typed IR** for Azure DevOps built-in pipeline tasks, one focused PR at a time.

## Background

The `ado-aw` compiler transforms agent markdown into Azure DevOps YAML through a typed pipeline IR in `src/compile/ir/`. Steps are represented by the `Step` enum in `src/compile/ir/step.rs`:

- `Step::Bash` — bash scripts
- `Step::Task` — generic ADO task steps (e.g. `UseNode@1`, `UseDotNet@2`)
- `Step::Checkout` / `Step::Download` / `Step::Publish` — special-purpose steps
- `Step::RawYaml` — **escape hatch** for user-authored YAML that the IR cannot model

The `TaskStep` struct is already fully generic — it accepts any `task: String` and an `inputs: IndexMap`. The gap is:
1. **No typed factory functions** for most ADO built-in tasks — every place that uses a known ADO task must hand-craft `TaskStep::new("Foo@1", "display").with_input(...)`.
2. **Compiler-generated `Step::RawYaml`** — occasionally, extension/runtime code emits `Step::RawYaml` for steps that could be expressed as `Step::Task` with a typed helper; these should be migrated.

## Step 1 — Load Previous State

```bash
cat /tmp/gh-aw/cache-memory/ado-task-ir-state.json 2>/dev/null || echo '{"completed_tasks":[],"deferred_tasks":[],"history":[]}'
```

Track:
- `completed_tasks` — ADO task identifiers already contributed (`["UseNode@1", "UseDotNet@2", ...]`)
- `deferred_tasks` — tasks skipped in prior runs with a reason
- `history` — last 30 run records (`date`, `outcome`, `pr_title`, `pr_open`, `pr_number`)

If the most recent history entry has `pr_open: true`, look up that PR in GitHub:

```bash
gh pr list --search "is:open in:title ado-task-ir OR \"typed IR\"" --limit 10
```

If the prior PR is still open, emit `noop` with "Waiting on open PR #N before adding more tasks." and stop.

## Step 2 — Audit Existing Typed Coverage

Examine what typed factory functions already exist:

```bash
# Existing TaskStep factory functions in runtimes and extensions
grep -rn "TaskStep::new\|fn.*task_step" src/runtimes src/compile/extensions --include="*.rs"
```

Also look for any `Step::RawYaml` in **compiler-generated** code (as opposed to user-YAML passthrough paths):

```bash
# Find RawYaml usages that are NOT in user-YAML passthrough functions
grep -n "Step::RawYaml\|RawYaml(" src/compile/agentic_pipeline.rs src/runtimes/**/*.rs src/compile/extensions/**/*.rs 2>/dev/null
```

Focus on:
- `src/runtimes/**` — runtime install/auth steps
- `src/compile/extensions/**` — always-on compiler infrastructure steps

Ignore `push_raw_yaml_if_nonempty`, `step_to_raw_yaml_string`, and any function that explicitly handles user-authored YAML (these are legitimately `Step::RawYaml`).

Build a set of **already-typed tasks**: task identifiers that already have typed factory functions (e.g. `UseNode@1`, `UseDotNet@2`, `UsePythonVersion@0`, `NpmAuthenticate@0`, `PipAuthenticate@1`, `NuGetAuthenticate@1`).

## Step 3 — Fetch ADO Built-In Task Catalog

Retrieve the Azure DevOps built-in task reference page:

```bash
curl -fsSL "https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/?view=azure-devops" \
  -H "Accept: text/html" \
  --max-time 30 \
  -o /tmp/gh-aw/agent/ado-tasks-reference.html 2>&1 | head -5
wc -l /tmp/gh-aw/agent/ado-tasks-reference.html
```

Extract the task list (task identifier + version + category):

```bash
grep -oP '(?<=href=")[^"]*task-reference[^"]*(?=")' /tmp/gh-aw/agent/ado-tasks-reference.html | sort -u | head -50
# Also extract task names visible in the page
grep -oP '[A-Za-z][A-Za-z0-9\-]+@[0-9]+' /tmp/gh-aw/agent/ado-tasks-reference.html | sort -u
```

If the HTML download fails, fall back to a curated list of high-value tasks from the compiler's own documentation and test fixtures:

```bash
# Fallback: tasks already referenced in tests/fixtures and source
grep -rh "task:\|TaskStep::new" src tests --include="*.rs" --include="*.yml" --include="*.yaml" 2>/dev/null \
  | grep -oP '[A-Za-z][A-Za-z0-9\-]+@[0-9]+' | sort -u
```

## Step 4 — Choose One Task to Contribute

From the catalog, select **one** task that:
1. Is **not** already in `completed_tasks` from state
2. Is **not** already typed (no existing factory function)
3. Has a stable, widely-used task identifier (prefer tasks in the `Utility`, `Build`, or `Test` categories)
4. Has clear documentation on `learn.microsoft.com`

Priority order (highest first):
1. A `Step::RawYaml` in compiler-generated code that maps to a known ADO task — converting it to `Step::Task` removes tech debt directly.
2. A task used frequently in the `tests/` fixtures as raw YAML strings.
3. A task from the ADO catalog that the project is likely to use (e.g. `CopyFiles@2`, `PublishTestResults@2`, `DotNetCoreCLI@2`, `ArchiveFiles@2`, `ExtractFiles@1`, `PowerShell@2`, `CmdLine@2`, `DownloadBuildArtifacts@1`, `PublishPipelineArtifact@1`).

Fetch the selected task's detail page to understand its inputs:

```bash
TASK_ID="<chosen-task-id>"  # e.g. CopyFiles@2
TASK_SLUG=$(echo "$TASK_ID" | tr '@' '-' | tr '[:upper:]' '[:lower:]')
curl -fsSL "https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/${TASK_SLUG}-task?view=azure-devops" \
  --max-time 30 \
  -o /tmp/gh-aw/agent/ado-task-detail.html
# Extract key inputs
grep -A2 -B2 -i "input\|required\|default" /tmp/gh-aw/agent/ado-task-detail.html | head -100
```

Document the task:
- Task identifier (e.g. `CopyFiles@2`)
- Display name convention
- Required inputs
- Optional inputs with defaults
- Relevant use cases in ado-aw

## Step 5 — Implement the Typed Helper

Decide where to place the factory function:

- **If the task is tied to a specific runtime** (language install, package auth): add to `src/runtimes/<name>/mod.rs`.
- **If the task is a general-purpose ADO built-in**: add to `src/compile/ir/step.rs` as a standalone constructor function alongside the existing step types, or create a new `src/compile/ir/tasks.rs` module if multiple new tasks are being added.

For a new `tasks.rs` module, create it at `src/compile/ir/tasks.rs` and add `pub mod tasks;` to `src/compile/ir/mod.rs`.

### Helper function shape

Model after existing patterns in `src/runtimes/`:

```rust
/// Returns a [`TaskStep`] for `CopyFiles@2`.
///
/// Copies files matching `contents` from `source_folder` to `target_folder`.
/// All parameters map directly to the ADO task inputs.
pub fn copy_files_step(
    source_folder: impl Into<String>,
    contents: impl Into<String>,
    target_folder: impl Into<String>,
) -> TaskStep {
    TaskStep::new("CopyFiles@2", "Copy Files")
        .with_input("SourceFolder", source_folder)
        .with_input("Contents", contents)
        .with_input("TargetFolder", target_folder)
}
```

Guidelines:
- Function name: snake_case, derived from the task display name, without the version suffix (e.g. `copy_files_step`, `publish_test_results_step`).
- Required inputs: positional parameters.
- Optional inputs with common defaults: keyword-style builder on the returned `TaskStep` (use `.with_input` after the initial construction).
- Include a doc comment with the task identifier and a one-line description.
- Do NOT add a new `Step` enum variant for standard tasks — `Step::Task(TaskStep)` is the correct representation.

### Convert RawYaml if applicable

If Step 4 identified a `Step::RawYaml` in compiler code that this task covers, replace it now:

```rust
// Before:
steps.push(Step::RawYaml(format!("- task: CopyFiles@2\n  inputs:\n    SourceFolder: {src}\n    TargetFolder: {dst}")));

// After:
use crate::compile::ir::tasks::copy_files_step;
steps.push(Step::Task(copy_files_step(&src, "**", &dst)));
```

## Step 6 — Add Tests

Add at least one unit test to the same file (or `tests/compiler_tests.rs` for integration tests):

```rust
#[test]
fn copy_files_step_builder_sets_inputs() {
    let t = copy_files_step("$(Build.SourcesDirectory)", "**/*.rs", "$(Build.ArtifactStagingDirectory)");
    assert_eq!(t.task, "CopyFiles@2");
    assert_eq!(t.inputs.get("SourceFolder").map(|s| s.as_str()), Some("$(Build.SourcesDirectory)"));
    assert_eq!(t.inputs.get("TargetFolder").map(|s| s.as_str()), Some("$(Build.ArtifactStagingDirectory)"));
}
```

## Step 7 — Validate

Run the full validation suite:

```bash
cargo build --all-targets 2>&1 | tail -20
cargo test 2>&1 | tail -30
cargo clippy --all-targets --all-features --workspace -- -D warnings 2>&1 | tail -20
```

All three must pass. If anything fails:
- Compilation errors: fix them before continuing.
- Test failures: investigate whether the new code broke an existing test or the test itself is wrong.
- Clippy warnings: apply the canonical fix.

Do not open a PR with a failing CI baseline.

## Step 8 — Save State

Update cache memory:

```bash
cat > /tmp/gh-aw/cache-memory/ado-task-ir-state.json << 'EOF'
{
  "completed_tasks": [...previous list..., "<new-task-id>"],
  "deferred_tasks": [...],
  "history": [
    {
      "date": "<today>",
      "outcome": "contributed|no-action|deferred",
      "task": "<task-id>",
      "pr_title": "<title or null>",
      "pr_number": null,
      "pr_open": true
    },
    ...previous entries (keep last 30)...
  ]
}
EOF
```

## Step 9 — Open the PR

If changes were made, open a PR with:

**Title** — conventional commits format:
- `feat(ir): add typed helper for <TaskName@version>` — for new factory functions
- `refactor(ir): replace RawYaml with typed TaskStep for <TaskName@version>` — for RawYaml conversions
- `feat(ir): add tasks module with typed helpers for <TaskA> and <TaskB>` — if a new module is created

**Body**:

```markdown
## Summary

Adds a typed factory function for `<TaskName@version>` to the ado-aw IR.

## Motivation

Previously, any code that needed to emit this ADO task step had to hand-craft
`TaskStep::new(...)` with raw string inputs. This PR introduces a well-typed
helper that validates required inputs at the call site and provides a clear API.

## Changes

- `src/compile/ir/tasks.rs` (or relevant file): `<fn_name>()` factory function
- `tests/...`: unit tests for the new helper
- (if applicable) `src/compile/...`: converted `Step::RawYaml` → `Step::Task`

## ADO Task Reference

- Task: `<TaskName@version>`
- Docs: `https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/...`
- Required inputs: ...
- Optional inputs: ...

## Validation

- [x] `cargo build --all-targets`
- [x] `cargo test`
- [x] `cargo clippy --all-targets --all-features --workspace -- -D warnings`

---
*Created by the ado-task-ir-contributor workflow.*
```

## When NOT to Open a PR

- An open PR from a previous run exists — emit `noop`.
- The catalog fetch fails AND no `Step::RawYaml` conversion candidates exist — emit `noop` with "Cannot fetch ADO task catalog; no local candidates found."
- All interesting tasks are already typed — emit `noop` with a count.
- Validation fails and no safe scope exists — emit `report-incomplete` with the failure details.

One run. One task. One PR.
