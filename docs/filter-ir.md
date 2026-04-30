# Filter IR Specification

_Part of the [ado-aw documentation](../AGENTS.md)._

This document specifies the intermediate representation (IR) used by the
ado-aw compiler to translate trigger filter configurations (YAML front matter)
into bash gate steps that run inside Azure DevOps pipelines.

**Source**: `src/compile/filter_ir.rs`

## Overview

When an agent file declares runtime trigger filters under `on.pr.filters` or
`on.pipeline.filters`, the compiler generates a *gate step* — a bash script
injected into the Setup job that evaluates each filter at pipeline runtime and
self-cancels the build if any filter fails.

The IR formalises this compilation as a three-pass pipeline:

```
on.pr.filters / on.pipeline.filters   (YAML front matter)
        │
        ▼
  ┌──────────────┐
  │  1. Lower    │   Filters  →  Vec<FilterCheck>
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │  2. Validate │   Vec<FilterCheck>  →  Vec<Diagnostic>
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │  3. Codegen  │   GateContext + Vec<FilterCheck>  →  bash string
  └──────────────┘
```

## Core Concepts

### Facts

A **Fact** is a typed runtime value that can be acquired during pipeline
execution. Each fact has:

| Property | Type | Purpose |
|----------|------|---------|
| `dependencies()` | `&[Fact]` | Facts that must be acquired first |
| `shell_var()` | `&str` | Shell variable the value is stored in |
| `acquisition_bash()` | `String` | Bash snippet that acquires the value |
| `failure_policy()` | `FailurePolicy` | What happens if acquisition fails |
| `is_pipeline_var()` | `bool` | Whether this is a free ADO pipeline variable |

Facts are organised into four tiers by acquisition cost:

#### Pipeline Variables (free)

These are always available via ADO macro expansion — no I/O required.

| Fact | ADO Variable | Shell Var | Applies To |
|------|-------------|-----------|------------|
| `PrTitle` | `$(System.PullRequest.Title)` | `TITLE` | PR |
| `AuthorEmail` | `$(Build.RequestedForEmail)` | `AUTHOR` | PR |
| `SourceBranch` | `$(System.PullRequest.SourceBranch)` | `SOURCE_BRANCH` | PR |
| `TargetBranch` | `$(System.PullRequest.TargetBranch)` | `TARGET_BRANCH` | PR |
| `CommitMessage` | `$(Build.SourceVersionMessage)` | `COMMIT_MSG` | PR, CI |
| `BuildReason` | `$(Build.Reason)` | `REASON` | All |
| `TriggeredByPipeline` | `$(Build.TriggeredBy.DefinitionName)` | `SOURCE_PIPELINE` | Pipeline |
| `TriggeringBranch` | `$(Build.SourceBranch)` | `TRIGGER_BRANCH` | Pipeline, CI |

#### REST API-Derived

Require a `curl` call to the ADO REST API. `PrIsDraft` and `PrLabels` depend
on `PrMetadata` being acquired first.

| Fact | Source | Shell Var | Depends On |
|------|--------|-----------|------------|
| `PrMetadata` | `GET pullRequests/{id}` | `PR_DATA` | — |
| `PrIsDraft` | `json .isDraft` from `PR_DATA` | `IS_DRAFT` | `PrMetadata` |
| `PrLabels` | `json .labels[].name` from `PR_DATA` | `PR_LABELS` | `PrMetadata` |

#### Iteration API-Derived

Require a separate API call to the PR iterations endpoint.

| Fact | Source | Shell Var | Depends On |
|------|--------|-----------|------------|
| `ChangedFiles` | `GET pullRequests/{id}/iterations/{last}/changes` | `CHANGED_FILES` | — |
| `ChangedFileCount` | `grep -c` on `CHANGED_FILES` | `FILE_COUNT` | — |

#### Computed

Derived from runtime computation (no API calls).

| Fact | Source | Shell Var |
|------|--------|-----------|
| `CurrentUtcMinutes` | `date -u` → minutes since midnight | `CURRENT_MINUTES` |

### Failure Policies

Each fact declares what happens if it cannot be acquired at runtime:

| Policy | Behaviour | Used By |
|--------|-----------|---------|
| `FailClosed` | Check fails → `SHOULD_RUN=false` | Pipeline vars, `PrIsDraft`, `CurrentUtcMinutes` |
| `FailOpen` | Check passes → assume OK | `PrLabels`, `ChangedFiles`, `ChangedFileCount` |
| `SkipDependents` | Log warning, skip dependent predicates | `PrMetadata` |

### Predicates

A **Predicate** is a pure boolean test over one or more acquired facts. The IR
supports these predicate types:

| Predicate | Bash Shape | Example |
|-----------|-----------|---------|
| `RegexMatch { fact, pattern }` | `echo "$VAR" \| grep -qE 'pattern'` | Title matches `\[review\]` |
| `Equality { fact, value }` | `[ "$VAR" = "value" ]` | Draft is `false` |
| `ValueInSet { fact, values, case_insensitive }` | `echo "$VAR" \| grep -q[i]E '^(a\|b)$'` | Author in allow-list |
| `ValueNotInSet { fact, values, case_insensitive }` | Inverse of `ValueInSet` | Author not in block-list |
| `NumericRange { fact, min, max }` | `[ "$VAR" -ge N ] && [ "$VAR" -le M ]` | Changed file count in range |
| `TimeWindow { start, end }` | Arithmetic on `CURRENT_MINUTES` | Only during business hours |
| `LabelSetMatch { any_of, all_of, none_of }` | `grep -qiF` per label | PR labels match criteria |
| `FileGlobMatch { include, exclude }` | python3 `fnmatch` | Changed files match globs |
| `And(Vec<Predicate>)` | All must pass | *(reserved for compound filters)* |
| `Or(Vec<Predicate>)` | At least one must pass | *(reserved)* |
| `Not(Box<Predicate>)` | Inner must fail | *(reserved)* |

`And`, `Or`, and `Not` are reserved for future compound filter expressions.
Currently all filter checks at the top level use AND semantics implicitly (all
must pass).

Each predicate can report the set of facts it requires via
`required_facts() -> BTreeSet<Fact>`. This drives fact acquisition planning in
the codegen pass.

### FilterCheck

A **FilterCheck** pairs a predicate with metadata used for diagnostics and bash
codegen:

```rust
struct FilterCheck {
    name: &'static str,            // "title", "author include", "labels", etc.
    predicate: Predicate,          // The boolean test
    build_tag_suffix: &'static str, // "title-mismatch" → "{prefix}:title-mismatch"
}
```

`all_required_facts()` returns the transitive closure of all facts needed by
the check, including dependencies (e.g. a `draft` check needs both `PrIsDraft`
and its dependency `PrMetadata`).

### GateContext

A **GateContext** determines the trigger-type-specific behaviour of the gate step:

| Context | `build_reason()` | `tag_prefix()` | `step_name()` | Bypass Condition |
|---------|-------------------|----------------|----------------|-----------------|
| `PullRequest` | `PullRequest` | `pr-gate` | `prGate` | `Build.Reason != PullRequest` |
| `PipelineCompletion` | `ResourceTrigger` | `pipeline-gate` | `pipelineGate` | `Build.Reason != ResourceTrigger` |

Non-matching builds bypass the gate automatically and set `SHOULD_RUN=true`.

## Pass 1: Lowering

### `lower_pr_filters(filters: &PrFilters) -> Vec<FilterCheck>`

Maps each field of `PrFilters` to a `FilterCheck`:

| Field | Predicate | Fact(s) | Tag Suffix |
|-------|-----------|---------|------------|
| `title` | `RegexMatch` | `PrTitle` | `title-mismatch` |
| `author.include` | `ValueInSet` (case-insensitive) | `AuthorEmail` | `author-mismatch` |
| `author.exclude` | `ValueNotInSet` (case-insensitive) | `AuthorEmail` | `author-excluded` |
| `source_branch` | `RegexMatch` | `SourceBranch` | `source-branch-mismatch` |
| `target_branch` | `RegexMatch` | `TargetBranch` | `target-branch-mismatch` |
| `commit_message` | `RegexMatch` | `CommitMessage` | `commit-message-mismatch` |
| `labels` | `LabelSetMatch` | `PrLabels` (→ `PrMetadata`) | `labels-mismatch` |
| `draft` | `Equality` | `PrIsDraft` (→ `PrMetadata`) | `draft-mismatch` |
| `changed_files` | `FileGlobMatch` | `ChangedFiles` | `changed-files-mismatch` |
| `time_window` | `TimeWindow` | `CurrentUtcMinutes` | `time-window-mismatch` |
| `min/max_changes` | `NumericRange` | `ChangedFileCount` | `changes-mismatch` |
| `build_reason.include` | `ValueInSet` (case-insensitive) | `BuildReason` | `build-reason-mismatch` |
| `build_reason.exclude` | `ValueNotInSet` (case-insensitive) | `BuildReason` | `build-reason-excluded` |

### `lower_pipeline_filters(filters: &PipelineFilters) -> Vec<FilterCheck>`

| Field | Predicate | Fact(s) | Tag Suffix |
|-------|-----------|---------|------------|
| `source_pipeline` | `RegexMatch` | `TriggeredByPipeline` | `source-pipeline-mismatch` |
| `branch` | `RegexMatch` | `TriggeringBranch` | `branch-mismatch` |
| `time_window` | `TimeWindow` | `CurrentUtcMinutes` | `time-window-mismatch` |
| `build_reason.include` | `ValueInSet` | `BuildReason` | `build-reason-mismatch` |
| `build_reason.exclude` | `ValueNotInSet` | `BuildReason` | `build-reason-excluded` |

### The `expression` Escape Hatch

The `expression` field on both `PrFilters` and `PipelineFilters` is **not**
part of the IR. It is a raw ADO condition string applied directly to the Agent
job's `condition:` field (not the bash gate step). It is handled by
`generate_agentic_depends_on()` in `common.rs`.

## Pass 2: Validation

### `validate_pr_filters(filters: &PrFilters) -> Vec<Diagnostic>`

Compile-time checks for impossible or conflicting configurations:

| Check | Severity | Condition |
|-------|----------|-----------|
| Min exceeds max | **Error** | `min_changes > max_changes` |
| Zero-width time window | **Error** | `time_window.start == time_window.end` |
| Author include/exclude overlap | **Error** | `author.include ∩ author.exclude ≠ ∅` (case-insensitive) |
| Build reason include/exclude overlap | **Error** | `build_reason.include ∩ build_reason.exclude ≠ ∅` |
| Labels any-of ∩ none-of overlap | **Error** | `labels.any_of ∩ labels.none_of ≠ ∅` |
| Labels all-of ∩ none-of overlap | **Error** | `labels.all_of ∩ labels.none_of ≠ ∅` |
| Empty labels filter | **Warning** | All of `any_of`, `all_of`, `none_of` are empty |

### `validate_pipeline_filters(filters: &PipelineFilters) -> Vec<Diagnostic>`

| Check | Severity | Condition |
|-------|----------|-----------|
| Zero-width time window | **Error** | `time_window.start == time_window.end` |
| Build reason include/exclude overlap | **Error** | `build_reason.include ∩ build_reason.exclude ≠ ∅` |

**Error** diagnostics cause compilation to fail with an actionable message.
**Warning** diagnostics are emitted to stderr but compilation continues.

Regex and glob pattern overlap is intentionally not validated — it would
require heuristic analysis and could produce false positives.

## Pass 3: Codegen

### `compile_gate_step(ctx: GateContext, checks: &[FilterCheck]) -> String`

Produces a complete ADO pipeline step (`- bash: |`) with a **data-driven
architecture**: bash is a thin ADO-macro shim, all filter logic lives in a
generic Python evaluator that reads a JSON gate spec.

#### Generated Step Structure

```yaml
- bash: |
    # 1. ADO macro exports (fact-specific, minimal set)
    export ADO_BUILD_REASON="$(Build.Reason)"
    export ADO_COLLECTION_URI="$(System.CollectionUri)"
    export ADO_PROJECT="$(System.TeamProject)"
    export ADO_BUILD_ID="$(Build.BuildId)"
    export ADO_PR_TITLE="$(System.PullRequest.Title)"
    # ... only the macros needed by this spec's facts ...

    # 2. Base64-encoded gate spec (safe from ADO macro expansion)
    export GATE_SPEC="eyJjb250ZXh0Ijp7Li4ufX0="

    # 3. Access token passthrough
    export ADO_SYSTEM_ACCESS_TOKEN="$SYSTEM_ACCESSTOKEN"

    # 4. Embedded Python evaluator (heredoc — never modified)
    python3 << 'GATE_EVAL_EOF'
    ...evaluator source...
    GATE_EVAL_EOF
  name: prGate
  displayName: "Evaluate PR filters"
  env:
    SYSTEM_ACCESSTOKEN: $(System.AccessToken)
```

#### Gate Spec Format (JSON)

The spec is base64-encoded to prevent ADO macro expansion and heredoc
quoting issues. Decoded, it contains:

```json
{
  "context": {
    "build_reason": "PullRequest",
    "tag_prefix": "pr-gate",
    "step_name": "prGate",
    "bypass_label": "PR"
  },
  "facts": [
    {"id": "pr_title", "kind": "pr_title", "failure_policy": "fail_closed"},
    {"id": "pr_metadata", "kind": "pr_metadata", "failure_policy": "skip_dependents"},
    {"id": "pr_is_draft", "kind": "pr_is_draft", "failure_policy": "fail_closed"}
  ],
  "checks": [
    {
      "name": "title",
      "predicate": {"type": "regex_match", "fact": "pr_title", "pattern": "\\[review\\]"},
      "tag_suffix": "title-mismatch"
    },
    {
      "name": "draft",
      "predicate": {"type": "equals", "fact": "pr_is_draft", "value": "false"},
      "tag_suffix": "draft-mismatch"
    }
  ]
}
```

The spec is declarative — it uses fact *kinds* (e.g., `"pr_title"`,
`"pr_metadata"`) not raw REST endpoints. The Python evaluator owns
acquisition logic.

#### Python Gate Evaluator (`src/data/gate-eval.py`)

The evaluator is a self-contained Python script embedded via
`include_str!()`. It handles:

1. **Bypass logic** — reads `ADO_BUILD_REASON` and exits early for non-matching
   trigger types
2. **Fact acquisition** — maps fact kinds to acquisition methods:
   - Pipeline variables → `os.environ.get("ADO_*")`
   - PR metadata → `urllib` call to ADO REST API
   - Changed files → iteration API calls
   - UTC time → `datetime.now(timezone.utc)`
3. **Failure policies** — `fail_closed`, `fail_open`, `skip_dependents`
4. **Predicate evaluation** — recursive evaluator supporting all predicate types
5. **Result reporting** — `##vso[...]` logging commands, build tags, self-cancel

The evaluator never changes per-pipeline — all variation is in the spec.

#### ADO Macro Export Strategy

The bash shim exports only the ADO macros needed by the spec's facts:

- **Always exported**: `ADO_BUILD_REASON`, `ADO_COLLECTION_URI`, `ADO_PROJECT`,
  `ADO_BUILD_ID` (needed for bypass and self-cancel)
- **PR API facts**: `ADO_REPO_ID`, `ADO_PR_ID` (only when `pr_metadata`,
  `pr_is_draft`, `pr_labels`, or `changed_files` facts are required)
- **Fact-specific**: each `Fact` variant declares its ADO exports via
  `ado_exports()` (e.g., `PrTitle` → `ADO_PR_TITLE`)

#### Predicate Types in Spec

| `type` | Fields | Description |
|--------|--------|-------------|
| `regex_match` | `fact`, `pattern` | Python `re.search()` |
| `equals` | `fact`, `value` | Exact string equality |
| `value_in_set` | `fact`, `values`, `case_insensitive` | Value membership |
| `value_not_in_set` | `fact`, `values`, `case_insensitive` | Inverse membership |
| `numeric_range` | `fact`, `min?`, `max?` | Integer range check |
| `time_window` | `start`, `end` | UTC HH:MM window (overnight-aware) |
| `label_set_match` | `fact`, `any_of?`, `all_of?`, `none_of?` | Label set predicates |
| `file_glob_match` | `fact`, `include?`, `exclude?` | Python `fnmatch` globs |
| `and` | `operands` | All must pass |
| `or` | `operands` | At least one must pass |
| `not` | `operand` | Inner must fail |

## Integration Points

### TriggerFiltersExtension

When Tier 2/3 filters are configured, the `TriggerFiltersExtension`
(`src/compile/extensions/trigger_filters.rs`) activates via
`collect_extensions()`. It implements `CompilerExtension` and controls:

1. **Download step** — fetches `gate-eval.py` from the ado-aw release
   artifacts to `/tmp/ado-aw-scripts/gate-eval.py`
2. **Gate step** — calls `compile_gate_step_external()` to generate a step
   that references the downloaded script (no inline heredoc)
3. **Validation** — runs `validate_pr_filters()` / `validate_pipeline_filters()`
   during compilation via the `validate()` trait method

The extension uses the `setup_steps()` trait method (not `prepare_steps()`)
because the gate must run in the **Setup job** (before the Execution job).

### Tier 1 Inline Path

When only Tier 1 filters are configured (pipeline variables — title, author,
branch, commit-message, build-reason), the extension is NOT activated.
`generate_pr_gate_step()` generates an inline bash gate step directly, with
no Python evaluator and no download step.

### Gate Step Injection

Gate steps are injected into the Setup job by `generate_setup_job()` in
`common.rs`. When the `TriggerFiltersExtension` is active, its
`setup_steps()` are collected and injected first (download + gate). When
only Tier 1 filters are present, the inline gate step is injected directly.

User setup steps are conditioned on the gate output:
`condition: eq(variables['{stepName}.SHOULD_RUN'], 'true')`

### Agent Job Condition

`generate_agentic_depends_on()` in `common.rs` generates the Agent job's
`dependsOn` and `condition` clauses:

```yaml
dependsOn: Setup
condition: |
  and(
    succeeded(),
    or(
      ne(variables['Build.Reason'], 'PullRequest'),
      eq(dependencies.Setup.outputs['prGate.SHOULD_RUN'], 'true')
    )
  )
```

When both PR and pipeline filters are active, both `or()` clauses are ANDed.
The `expression` escape hatch is also ANDed if present.

### Scripts Distribution

`gate-eval.py` lives at `scripts/gate-eval.py` in the repository and is
shipped as a release artifact alongside the ado-aw binary. The download URL
is deterministic based on the ado-aw version:
`https://github.com/githubnext/ado-aw/releases/download/v{VERSION}/gate-eval.py`

## Adding New Filter Types

See [extending.md](extending.md#filter-ir-srccompilefilter_irrs) for the
step-by-step guide. In summary:

1. Add a `Fact` variant if a new data source is needed (with `kind()`,
   `ado_exports()`, `dependencies()`, `failure_policy()`)
2. Add a `Predicate` variant if a new test shape is needed
3. Add a `PredicateSpec` variant for serialization
4. Add an evaluator handler in `scripts/gate-eval.py` for the new predicate
   type
5. Extend the lowering function (`lower_pr_filters` or
   `lower_pipeline_filters`)
6. Add validation rules if the new filter can conflict with existing ones
7. Write tests: lowering, validation, spec serialization, and evaluator
