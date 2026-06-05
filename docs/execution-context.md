# Execution Context

_Part of the [ado-aw documentation](../AGENTS.md)._

The **execution-context plugin** stages per-run context (changed files,
diffs, base/head SHAs, file snapshots, metadata) on disk in a stable
layout under `aw-context/` *before* the agent starts. The agent then
reads these files instead of running its own `git fetch` / `git diff`
plumbing.

This is an always-on compiler extension. There is no `tools:` entry to
enable it; per-trigger contributors gate themselves based on the
agent's `on:` configuration.

> Background and motivation: this feature was tracked in
> [issue #860](https://github.com/githubnext/ado-aw/issues/860).

## Why this exists

PR-reviewer agents almost always need the same precondition: a fully
fetched target branch, resolved base / head SHAs, a unified diff, and
optionally pre / post snapshots of touched files. ADO's default
`checkout: self` is shallow (`fetchDepth: 1`), doesn't fetch the PR
target branch, and (deliberately) does not persist credentials into
`.git/config` for OAuth bearer reuse. Every PR-reviewer agent has
historically rebuilt the same ~120 lines of bash to work around this.

The execution-context plugin owns that step centrally:

- One canonical implementation that evolves with the framework.
- Driven by ADO's predefined `System.PullRequest.*` variables — no
  manual ref discovery.
- Inside the trust boundary: the bearer token used to fetch is
  scoped to the precompute step's process env and never reaches the
  agent container or `.git/config`.

## v1 contributors

| Contributor | Trigger        | Output layout            |
|-------------|----------------|--------------------------|
| `pr`        | `on.pr`        | `aw-context/pr/*`        |

Future trigger contributors (pipeline-completion, schedule, manual)
plug in via the same internal `ContextContributor` trait without
breaking the agent-facing layout.

## Front-matter surface

```yaml
execution-context:
  enabled: true                  # master switch; defaults to true
  pr:                            # PR contributor configuration
    enabled: true                # defaults to true when `on.pr` is configured
    scope:                       # pathspecs scoping diff + snapshots
      - "src/**"
      - "docs/**"
      - ":(top,glob)*.yml"
    unified: 3                   # `-U` lines of context for diff.patch
    max-diff-bytes: 524288       # truncate diff.patch beyond this many bytes
    snapshots: true              # write head-files/ and base-files/
```

All keys are optional. When the `execution-context:` block is omitted
entirely, defaults are *"on for the triggers configured in `on:`"*.

### Fields

- **`enabled`** (`bool`, default `true`) — master switch. When `false`,
  no contributor runs and no `aw-context/` is staged.
- **`pr.enabled`** (`bool`, default `true` when `on.pr` is set) —
  whether to activate the PR contributor. Set `false` to opt out on
  huge monorepos where the targeted fetch + diff cost is unacceptable
  (the agent then has to roll its own equivalent).
- **`pr.scope`** (`list[string]`, default `[]` = all paths) — pathspecs
  passed to `git diff -- <scope>` for both `changed-files-in-scope.txt`
  and `diff.patch`. Sanitised at compile time.
- **`pr.unified`** (`u32`, default `3`) — `-U` lines of context for
  `diff.patch`.
- **`pr.max-diff-bytes`** (`u64`, default `524288` / 512 KiB) — cap on
  `diff.patch` size. When exceeded, the file ends with a literal
  marker line `--- TRUNCATED at <N> bytes; full diff suppressed ---`
  so the agent knows it is reading a partial diff.
- **`pr.snapshots`** (`bool`, default `true`) — whether to write per-file
  pre / post snapshots under `head-files/` and `base-files/`. Disable on
  large changes if you only need the diff.

## Agent-visible layout

For PR-triggered builds, the precompute step stages files under
`$(Build.SourcesDirectory)/aw-context/` (i.e. relative to the agent's
working directory):

```
aw-context/
  status.txt                       # OK | (errors propagate to per-contributor files)
  trigger.txt                      # pr (today; future: pipeline / schedule / manual)
  metadata.txt                     # build_id, build_reason, repository, source_branch
  pr/
    status.txt                     # OK | NO_PR_CONTEXT | DIFF_RESOLUTION_FAILED
    metadata.txt                   # pr_id, source_branch, target_branch, base_sha, head_sha
    changed-files.txt              # full `git diff --name-status`
    changed-files-in-scope.txt     # name-status restricted to `scope`
    diff.patch                     # unified diff, scoped, capped, may end with TRUNCATED marker
    head-files/<path>              # post-PR snapshots of A/M/T/R*/C* files in scope
    base-files/<path>              # pre-PR snapshots of D files in scope
    error.txt                      # only present when pr/status.txt != OK
```

**Agents MUST read `aw-context/pr/status.txt` first** and act on its
value:

- `OK` — `aw-context/pr/*` is fully populated. Prefer reading those
  files over running `git fetch` / `git diff` yourself.
- `NO_PR_CONTEXT` — the build is not a PR (e.g. manual queue of a
  PR-triggered pipeline). Skip PR-specific logic.
- `DIFF_RESOLUTION_FAILED` — the precompute step ran but could not
  resolve the base / head SHAs. See `aw-context/pr/error.txt` for the
  reason. Surface this in your output rather than silently producing
  an empty review.
- `CONTEXT_GENERATION_FAILED` — base / head SHAs resolved, but at
  least one of the `git diff` commands that populates the staged
  files failed. The `metadata.txt` file is still trustworthy, but
  `changed-files.txt`, `changed-files-in-scope.txt`, or `diff.patch`
  may be empty or partial. See `aw-context/pr/error.txt`.

If `aw-context/pr/status.txt` does not exist at all (e.g. when the
extension is disabled), treat it as `NO_PR_CONTEXT`.

## What the precompute step does

The PR contributor's generated bash step:

1. **Reads `System.PullRequest.*` from the environment.** No manual ref
   discovery — ADO already populates `SourceBranch`, `TargetBranch`,
   and `PullRequestId`. If they are missing, writes `NO_PR_CONTEXT`
   and exits 0.
2. **Detects merge-commit shape first.** If `HEAD` has two parents
   (the synthetic merge commit ADO checks out for PR builds), uses
   `HEAD^1` / `HEAD^2` as base / head and skips the target-branch
   fetch entirely. Otherwise:
3. **Fetches the PR target branch with progressive deepening** —
   `--depth=200`, then `500`, then `2000`, then finally `--unshallow`.
   **After each successful fetch, attempts `git merge-base
   origin/<target> HEAD`** and continues to the next depth if it
   cannot resolve yet. Bounded bandwidth on the common case; covers
   the long-tail PR-against-old-base case. On exhaustion writes
   `DIFF_RESOLUTION_FAILED`.
4. **Writes `metadata.txt`, `changed-files.txt`,
   `changed-files-in-scope.txt`, `diff.patch`.** The diff is scoped to
   `pr.scope` (or all paths if empty) and truncated at `pr.max-diff-bytes`
   with a literal marker. If any of these `git diff` invocations fails,
   the status becomes `CONTEXT_GENERATION_FAILED` rather than `OK`.
5. **Snapshots** (when `pr.snapshots: true`) — for each in-scope file:
   `head-files/<path>` for `A`/`M`/`T`/`R*`/`C*` entries,
   `base-files/<path>` for `D` entries.
6. **Writes the final status** to `pr/status.txt` and `status.txt`.

The step is gated by `condition: eq(variables['Build.Reason'],
'PullRequest')` so it is a no-op on manual or scheduled queues of a
PR-triggered pipeline.

## Trust boundary

The PR contributor must fetch the PR target branch (which the default
checkout does not), but doing so requires an OAuth bearer. ado-aw
preserves the Stage 1 read-only invariant with these design choices:

| Mechanism                                   | Decision |
|---------------------------------------------|----------|
| Override `checkout: self` with `persistCredentials: true` | **Rejected.** It would write the build identity's bearer into `.git/config` inside the workspace, which is then mounted into the AWF sandbox where the agent could read and exfiltrate it. |
| Override `checkout: self` with `fetchDepth: 0` | **Rejected.** Unnecessary — the precompute fetches exactly the two refs it needs. |
| In-step `SYSTEM_ACCESSTOKEN` + bash bearer wrapper | **Adopted.** `SYSTEM_ACCESSTOKEN` is mapped from `$(System.AccessToken)` only into the precompute step's process env. A `git_fetch` wrapper injects `git -c http.extraheader="Authorization: bearer ${SYSTEM_ACCESSTOKEN}" fetch …`. The token lives only in the bash step's process memory and is never written to disk. |

After the precompute step exits, the bearer is gone from the runtime
environment the agent inherits, `.git/config` contains no
`http.extraheader` line, and the agent container is started by AWF
with its own (read-only) MI from the ARM service connection.

The compile-time test `test_execution_context_pr_does_not_leak_system_accesstoken`
asserts that generated YAML never contains `persistCredentials: true`,
never writes to `.git/config`, and that `SYSTEM_ACCESSTOKEN` appears
only in the execution-context prepare step.

## Migrating from a hand-rolled precompute

If you have an existing PR-reviewer agent with a `steps:` block that
manually fetches the target branch, resolves merge-base, and emits a
diff: delete that block, ensure `on.pr` is configured, and read from
`aw-context/pr/*` in your agent prompt. The prompt supplement is
appended automatically — you do not need to mention the layout in your
own markdown body.

## Notes and edge cases

- **`AW_PR_*` env vars are not surfaced.** ado-aw's agent-env-var
  channel rejects ADO `$(...)` expressions for injection-defence
  reasons, and bouncing values through pipeline output variables
  introduces a second source of truth. Agents read everything from
  `aw-context/pr/metadata.txt`.
- **No `git` / `cat` / `ls` is added to the agent's bash allow-list.**
  The agent reads `aw-context/*` using its normal file-reading
  mechanism (the `edit` tool, native copilot reads, etc.), not via
  shell. This avoids silently widening the bash capability surface
  when the user has restricted bash.
- **Non-`self` checkouts in `repos:`.** v1 only diffs the `self`
  checkout. The PR contributor does not currently produce contexts
  for additional repository checkouts.
- **Workspace alias.** When `workspace:` points to a non-`self` alias,
  `aw-context/` is still relative to `$(Build.SourcesDirectory)` —
  i.e. the pipeline's working directory, not the workspace alias's
  directory.
- **Ordering.** The precompute step runs after the standard
  `- checkout: self` and before any user `steps:`, so user `steps:`
  can also read `aw-context/` if needed.

## Compiler internals

- Always-on `ExecContextExtension` in
  `src/compile/extensions/exec_context/mod.rs` (`ExtensionPhase::Tool`).
- Internal `ContextContributor` trait in `contributor.rs`. v1 ships one
  contributor: `PrContextContributor` in `pr.rs`.
- Front-matter types: `ExecutionContextConfig` and `PrContextConfig` in
  `src/compile/types.rs`.
- Compile tests live in `tests/compiler_tests.rs` (search for
  `test_execution_context_pr_*`).
- The generated bash is shellchecked by `tests/bash_lint_tests.rs` via
  the `execution-context-agent.md` fixture.
