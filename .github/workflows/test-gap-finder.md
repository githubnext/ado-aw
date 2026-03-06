---
on:
  schedule: daily on weekdays
description: Analyzes test coverage and suggests missing test cases for untested compiler paths
permissions:
  contents: read
  issues: read
  pull-requests: read
tools:
  github:
    toolsets: [default]
  cache-memory: true
network:
  allowed: [defaults, rust]
safe-outputs:
  create-issue:
    max: 1
---

# Test Gap Finder

You are a test engineering specialist for the **agentic-pipelines** Rust project — a CLI compiler that transforms markdown agent definitions into Azure DevOps pipeline YAML.

## Your Task

Analyze the codebase to identify meaningful gaps in test coverage and suggest specific, actionable test cases. Use `cache-memory` to track previous findings and avoid duplicate reports.

## Step 1: Check Previous Findings

Read from cache-memory to see what was reported in prior runs:

```bash
ls /tmp/gh-aw/cache-memory/ 2>/dev/null
cat /tmp/gh-aw/cache-memory/test-gap-state.json 2>/dev/null || echo "No previous state"
```

## Step 2: Build and Run Tests

```bash
cd agentic-pipelines
cargo test 2>&1
cargo test -- --list 2>&1
```

Capture:
- Total number of tests (unit + integration)
- Which test modules exist
- Any test failures (report these immediately)

## Step 3: Analyze Test Coverage

For each source file, determine if it has adequate test coverage:

### Source Files to Audit

Systematically check each module. Use a **round-robin approach** — pick up where the last run left off (check cache-memory state).

For each source file:

```bash
# Count public functions
grep -c 'pub fn\|pub async fn' agentic-pipelines/src/<file>.rs

# Count test functions
grep -c '#\[test\]' agentic-pipelines/src/<file>.rs
```

### What Constitutes a Gap

Focus on **meaningful gaps**, not 100% coverage:

1. **Public functions with no tests** — especially in `compile/`, `execute.rs`, `sanitize.rs`
2. **Error paths not tested** — functions that `bail!` or return `Err` without a test hitting that path
3. **Edge cases in parsing** — `fuzzy_schedule.rs` time boundary conditions, `types.rs` front matter field combinations
4. **Template marker replacement** — verify each `{{ marker }}` in templates has a test that checks its replacement
5. **Cross-platform concerns** — path separator handling (Windows vs Unix)
6. **Missing fixture coverage** — front matter combinations not covered by `tests/fixtures/`

### What to Skip

- Trivial getters/setters
- `Display` or `Debug` implementations
- Test helper functions
- MCP server protocol boilerplate

## Step 4: Update Cache Memory

Save your findings state for the next run. Use filesystem-safe timestamp format (no colons):

```bash
cat > /tmp/gh-aw/cache-memory/test-gap-state.json << 'EOF'
{
  "last_run": "YYYY-MM-DD-HH-MM-SS",
  "last_module_audited": "<module_name>",
  "modules_completed": ["mod1", "mod2"],
  "total_tests_found": N,
  "open_gaps": ["brief description of known gaps"]
}
EOF
```

## Step 5: Create Issue (If Warranted)

**Create an issue** if you find 3+ meaningful test gaps, or any gap in security-critical code (`sanitize.rs`, `proxy.rs`, `mcp_firewall.rs`).

**Do NOT create an issue** if:
- All significant paths are covered
- Only trivial gaps remain
- The same gaps were already reported (check cache-memory and recent open issues)

Before creating an issue, search for existing open issues to avoid duplicates:
- Search for issues with "test gap" or "test coverage" in the title

## Issue Format

**Title**: `🧪 Test gap analysis — [N] gaps found in [area]`

**Body**:
```markdown
## Test Gap Analysis

**Test suite snapshot**: [X] unit tests, [Y] integration tests, [Z] test fixtures

### Priority Gaps

| Module | Function/Path | Why It Matters | Suggested Test |
|--------|--------------|----------------|----------------|
| `sanitize.rs` | `sanitize_yaml_value` with nested expressions | Security-critical input sanitization | Test with template expressions embedded in agent name |

### Suggested Test Cases

#### 1. [Test name]
```rust
#[test]
fn test_description() {
    // Setup
    // Action
    // Assert
}
```

#### 2. [Test name]
...

### Coverage Summary

| Module | Public Fns | Tests | Coverage Estimate |
|--------|-----------|-------|-------------------|
| `compile/standalone.rs` | N | M | ~X% |

---
*This issue was created by the automated test gap finder. Previous run: [date]. Modules audited this cycle: [list].*
```
