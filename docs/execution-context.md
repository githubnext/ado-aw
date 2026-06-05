# Execution Context

_Part of the [ado-aw documentation](../AGENTS.md)._

The **execution-context plugin** stages a small, focused set of per-run
context signals on disk and appends a tailored fragment to the agent
prompt *before* the agent starts. The agent then runs `git diff`,
`git show`, `git log` itself against the precomputed SHAs (the
workspace's `.git/objects/` are already populated by the precompute
fetch) and calls Azure DevOps MCP tools with pre-filled identifiers
already embedded in its prompt.

This is an always-on compiler extension. There is no `tools:` entry to
enable it; per-trigger contributors gate themselves based on the
agent's `on:` configuration.

> Background and motivation: this feature was tracked in
> [issue #860](https://github.com/githubnext/ado-aw/issues/860).

## Why this exists

PR-reviewer agents almost always need the same precondition: a fully
fetched target branch and resolved base / head SHAs. ADO's default
`checkout: self` is shallow (`fetchDepth: 1`), doesn't fetch the PR
target branch, and (deliberately) does not persist credentials into
`.git/config` for OAuth bearer reuse. Every PR-reviewer agent has
historically rebuilt the same ~120 lines of bash to work around this.

The execution-context plugin owns that step centrally — but does
*only* the part the agent cannot do for itself:

- Fetches the PR target branch with progressive deepening until
  `git merge-base` resolves (requires the bearer; cannot happen
  inside the agent's sandbox).
- Writes the resolved `base.sha` and `head.sha` so the agent can
  reuse them across many `git diff` invocations.
- Appends a prompt fragment listing the right `git` commands and
  ADO MCP tool calls (with literal PR id / project / repo
  interpolated) for the agent to use.

The agent does its own diff/show/log/stat work — it has the objects
locally and `git` is added to its bash allow-list automatically.

## v1 contributors

| Contributor | Trigger | Output layout            |
|-------------|---------|--------------------------|
| `pr`        | `on.pr` | `aw-context/pr/*`        |

Future trigger contributors (pipeline-completion, schedule, manual)
plug in via the same internal `ContextContributor` trait without
breaking the agent-facing layout.

## Front-matter surface

```yaml
execution-context:
  enabled: true       # master switch; defaults to true
  pr:
    enabled: true     # defaults to true when `on.pr` is configured
```

All keys are optional. When the `execution-context:` block is omitted
entirely, defaults are *"on for the triggers configured in `on:`"*.

### Fields

- **`enabled`** (`bool`, default `true`) — master switch. When `false`,
  no contributor runs and no `aw-context/` is staged.
- **`pr.enabled`** (`bool`, default `true` when `on.pr` is set) —
  whether to activate the PR contributor. Set `false` to opt out
  (e.g. when an agent already does its own precompute or doesn't need
  PR context). **`on.pr` must be configured** for the contributor to
  activate at all — `pr.enabled: true` without an `on.pr` trigger has
  no effect (the prepare step would be dead code, and silently widening
  the agent's bash allow-list with git commands for a non-PR agent
  would be a footgun).

`pr.enabled: false` also suppresses the auto-extension of the agent's
bash allow-list with git commands described below.

## Agent-visible layout

For PR-triggered builds, the precompute step stages files under
`$(Build.SourcesDirectory)/aw-context/` (i.e. relative to the agent's
working directory):

### Success case (2 files)

```
aw-context/
  pr/
    base.sha          # PR merge-base SHA (40-char hex, no trailing newline)
    head.sha          # PR head SHA (40-char hex, no trailing newline)
```

`base.sha` is the common ancestor of the PR head and the PR target
branch — `git merge-base` in both the synthetic-merge-commit path and
the progressive-deepening path. This makes `git diff $BASE..$HEAD`
produce the SAME change set regardless of whether ADO checked out a
real branch tip or a synthetic merge commit (i.e. the diff is "what
the PR introduces since branch-point", not "what the PR introduces
versus the current target tip").

### Failure case (1 file)

```
aw-context/
  pr/
    error.txt         # one-line failure reason
```

(`base.sha` / `head.sha` are not written on failure.)

Short identifiers — PR id, ADO project name, ADO repository name —
are **not** staged as files. They are interpolated directly into the
agent prompt fragment ("This is PR #4242 in project 'OneBranch' /
repository 'my-repo'…"), so the agent sees them as natural English
and as literal arguments in example ADO MCP tool calls. Files are
reserved for the opaque 40-char SHAs the agent reuses across many
commands.

## Agent prompt fragment

The precompute step appends one of two fragments directly to
`/tmp/awf-tools/agent-prompt.md` (the file built by the
"Prepare agent prompt" step in `base.yml`). This mirrors how gh-aw
injects its own built-in prompt sections.

### Success fragment

The fragment shows how to set `$BASE` / `$HEAD` from the staged files,
lists six common `git` invocations (`diff --stat`, `diff
--name-status`, `diff`, `diff -- <path>`, `show $HEAD:<path>`, `log`),
and shows three example ADO MCP tool calls
(`repo_get_pull_request_by_id`, `repo_list_pull_request_threads`,
`repo_create_pull_request_thread`) with `project`, `repositoryId`,
and `pullRequestId` pre-filled to the actual values.

### Failure fragment

When the precompute fails (identifier validation or merge-base
resolution exhausts the depth budget), the failure fragment is
appended instead. It states the reason from `aw-context/pr/error.txt`
and tells the agent:

- Local `git diff` is unavailable for this run.
- ADO MCP tool calls remain possible (the PR id / project / repo are
  still embedded in the fragment).
- Do NOT produce an empty review or pretend the PR has no changes —
  surface the failure (e.g. via `report_incomplete`) or fall back to
  the API.

If neither fragment is appended (Build.Reason ≠ PullRequest), the
agent prompt is silent on PR context.

## Bash allow-list auto-extension

When the PR contributor activates, these read-only `git` commands
are added to the agent's bash allow-list:

```
git, git diff, git log, git show, git status, git rev-parse, git symbolic-ref
```

The extension uses the same `required_bash_commands()` plumbing as
the runtime extensions (Python, Node, .NET, Lean). When the agent has:

| `tools.bash` setting             | Behaviour |
|----------------------------------|-----------|
| `bash:` (omitted or wildcard)    | Allow-all mode — extension is a no-op (commands are already permitted). |
| `bash: ["..."]` (explicit list)  | The 7 git commands are appended to the user's list. |
| `pr.enabled: false`              | The 7 git commands are NOT added (matches the contributor's overall inactive state). |

This keeps the agent's bash surface intentional: opting out of the
PR contributor opts out of the corresponding git capability.

## What the precompute step does

The PR contributor's generated bash step:

1. **Reads `System.PullRequest.*` and `System.TeamProject` /
   `Build.Repository.Name` from the environment.** No manual ref
   discovery — ADO already populates these.
2. **Validates identifiers** with strict allowlist regexes
   (`PR_ID` ⊆ digits, `PROJECT`/`REPO` ⊆ alphanumeric + `._-`,
   `PROJECT` additionally allows space, `PR_TARGET_BRANCH` ⊆
   alphanumeric + `._/-`). Failure writes `error.txt` and appends
   the failure prompt fragment.
3. **Detects merge-commit shape.** If `HEAD` has two parents (the
   synthetic merge commit ADO checks out for PR builds), uses
   `HEAD^2` as the PR head and computes `git merge-base HEAD^1 HEAD^2`
   as the base — same semantics as the deepening path, no
   target-branch fetch needed. Otherwise:
4. **Fetches the PR target branch with progressive deepening** —
   `--depth=200`, then `500`, then `2000`, then finally `--unshallow`.
   After each successful fetch, attempts `git merge-base
   origin/<target> HEAD` and continues to the next depth if it
   cannot resolve yet.
5. **Writes `base.sha` and `head.sha`** on success and appends the
   success prompt fragment to `/tmp/awf-tools/agent-prompt.md`.
6. **On failure**, writes `error.txt` and appends the failure prompt
   fragment.

The step exits 0 in both success and failure paths so the build
proceeds — the agent surfaces failures via the prompt fragment, not
via a build break.

The whole step is gated by `condition: eq(variables['Build.Reason'],
'PullRequest')` so it is a no-op on manual or scheduled queues of a
PR-triggered pipeline.

## Trust boundary

The PR contributor must fetch the PR target branch (which the default
checkout does not), but doing so requires an OAuth bearer. ado-aw
preserves the Stage 1 read-only invariant with these design choices:

| Mechanism                                                 | Decision |
|-----------------------------------------------------------|----------|
| Override `checkout: self` with `persistCredentials: true` | **Rejected.** It would write the build identity's bearer into `.git/config` inside the workspace, which is then mounted into the AWF sandbox where the agent could read and exfiltrate it. |
| Override `checkout: self` with `fetchDepth: 0`            | **Rejected.** Unnecessary — the precompute fetches exactly the refs it needs. |
| In-step `SYSTEM_ACCESSTOKEN` + `GIT_CONFIG_*` bearer env  | **Adopted.** `SYSTEM_ACCESSTOKEN` is mapped from `$(System.AccessToken)` only into the precompute step's process env. `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_0` / `GIT_CONFIG_VALUE_0` inject `http.extraheader: Authorization: bearer …` into `git fetch` *via env vars* (not via `git -c` on argv) so the token never appears in process listings. The token lives only in the bash step's process memory and is never written to disk. |

After the precompute step exits, the bearer is gone from the runtime
environment the agent inherits, `.git/config` contains no
`http.extraheader` line, and the agent container is started by AWF
with its own (read-only) MI from the ARM service connection.

The compile-time test
`test_execution_context_pr_does_not_leak_system_accesstoken` walks
the generated YAML and asserts that `SYSTEM_ACCESSTOKEN` appears
only in the execution-context prepare step's `env:` block, never
the agent step's.

## Migrating from a hand-rolled precompute

If you have an existing PR-reviewer agent with a `steps:` block that
manually fetches the target branch and resolves merge-base: delete
that block, ensure `on.pr` is configured, and let the agent read
`aw-context/pr/{base,head}.sha` directly. The prompt fragment is
appended automatically — you do not need to mention the layout in
your own markdown body.

## Notes and edge cases

- **Identifiers in the prompt, SHAs on disk.** Short values (PR id,
  project, repo) are interpolated into the prompt heredoc; long
  opaque 40-char SHAs stay as files where shell ergonomics actually
  win (`BASE=$(cat aw-context/pr/base.sha)` is the natural pattern).
- **Non-`self` checkouts in `repos:`.** v1 only diffs the `self`
  checkout. The PR contributor does not currently produce contexts
  for additional repository checkouts.
- **Workspace alias.** When `workspace:` points to a non-`self`
  alias, `aw-context/` is still relative to `$(Build.SourcesDirectory)`
  — i.e. the pipeline's working directory, not the workspace alias's
  directory.
- **Ordering.** The precompute step runs after `{{ checkout_self }}`
  in the Agent job's prepare phase, after the "Prepare agent prompt"
  step (so it can append) and before the agent runs (so the agent
  sees the appended prompt).

## Compiler internals

- Always-on `ExecContextExtension` in
  `src/compile/extensions/exec_context/mod.rs`
  (`ExtensionPhase::Tool`).
- Internal `ContextContributor` trait in `contributor.rs`. v1 ships
  one contributor: `PrContextContributor` in `pr.rs`.
- Front-matter types: `ExecutionContextConfig` and `PrContextConfig`
  in `src/compile/types.rs` (`PrContextConfig` is just
  `{ enabled: Option<bool> }`).
- Compile tests live in `tests/compiler_tests.rs` (search for
  `test_execution_context_pr_*`).
- The generated bash is shellchecked by `tests/bash_lint_tests.rs`
  via the `execution-context-agent.md` fixture.
