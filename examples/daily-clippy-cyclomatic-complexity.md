---
name: "Daily Clippy Complexity Reducer"
description: "Runs Clippy daily, targets high cyclomatic complexity hotspots, and opens a PR with refactors"
schedule: daily
tools:
  bash:
    - cargo
    - python3
    - rg
    - git
    - cat
    - sed
    - awk
    - sort
    - head
permissions:
  write: my-write-arm-connection
network:
  allowed:
    - rust
safe-outputs:
  create-pull-request:
    target-branch: main
    auto-complete: true
    delete-source-branch: true
    squash-merge: true
    labels:
      - automated
      - clippy
      - complexity
---

## Daily Clippy Complexity Reducer

Reduce Rust function complexity by addressing the highest-impact Clippy complexity lints, then submit a pull request when improvements are made.

### Scope

- Work only in checked-out repository files.
- Focus on complexity findings from Clippy JSON output.
- Prioritize changes that reduce complexity without changing external behavior.

### Process

1. Run Clippy in JSON mode:
   - `cargo clippy --all-targets --all-features --message-format=json`
2. Parse JSON diagnostics and keep only complexity lints:
   - Primary lint code: `clippy::cyclomatic_complexity`
   - Also accept `clippy::cognitive_complexity` if emitted by the current Clippy version.
3. Rank findings by reported complexity score (highest first). If score extraction fails, rank by diagnostic order.
4. For top findings, use spans to identify file and line range, inspect the function, and refactor with safe, behavior-preserving changes:
   - extract helper functions
   - flatten nested conditional logic
   - simplify boolean expressions
   - replace complex branching with clearer structure
5. Re-run `cargo clippy --all-targets --all-features --message-format=json` and confirm complexity findings are reduced.
6. Run `cargo test` to verify no regressions.

### Output Rules

- If no complexity issues are found, or no safe improvement can be made, use `noop` with a short summary.
- If changes reduce complexity and tests pass, use `create-pull-request`:
  - Title: `refactor: reduce clippy complexity hotspots`
  - Description must include:
    - the diagnostics addressed (file/function)
    - before/after complexity values when available
    - a short summary of refactors performed
    - test and clippy verification results
- If blocked (for example, diagnostics cannot be parsed or refactor cannot be made safely), use `report-incomplete` with concrete blocker details.
