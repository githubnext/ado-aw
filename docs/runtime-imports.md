# Runtime Imports

_Part of the [ado-aw documentation](../AGENTS.md)._

Runtime imports let agent prompts pull in snippet files with gh-aw-compatible
`{{#runtime-import ...}}` markers. They are available in the markdown body and
are controlled by the [`inlined-imports:` field](front-matter.md#inlined-imports).
The runtime bundle that expands them is documented in [`ado-script.md`](ado-script.md).

## Marker syntax

Use `{{#runtime-import path}}` for a required import. If the target file is
missing, resolution fails.

```markdown
## Repository policy
{{#runtime-import docs/policy.md}}
```

Use `{{#runtime-import? path}}` for an optional import. If the target file is
missing, the marker is replaced with an empty string.

```markdown
## Local notes
{{#runtime-import? docs/local-notes.md}}
```

## Where markers can appear

Authors can place runtime-import markers anywhere in the agent markdown body.
When `inlined-imports: false` (the default), the compiler also injects an
implicit top-level runtime-import marker that reloads the body itself at
pipeline runtime instead of embedding it into the generated YAML. When
`inlined-imports: true`, that implicit body marker is resolved at compile time
along with any author-written markers.

## Path resolution

- **Author-written markers** must use **relative paths** rooted at the agent
  `.md` file's directory. Absolute paths and `..` segments are rejected. This
  protects the compile host (`ado-aw compile`, which may run on a CI agent
  carrying privileged material like SSH keys and service-connection tokens)
  from untrusted PR branches embedding host files into the compiled YAML —
  e.g. `{{#runtime-import /home/runner/.ssh/id_rsa}}` or
  `{{#runtime-import ../../../../etc/passwd}}` are both compile-time errors.
- **Compiler-generated marker for the agent body** uses a **trigger-repo-relative**
  path resolved against `--base "$(Build.SourcesDirectory)"` — e.g. `agents/foo.md`,
  or `$(Build.Repository.Name)/agents/foo.md` under multi-checkout (ADO expands the
  macro before the resolver runs). It is deliberately relative, not absolute:
  `import.js` rejects absolute paths for everyone. The compile-time `..`/absolute
  restriction is a compile-host guard and does not constrain this tooling-generated
  path beyond the runtime resolver's own absolute/`..` rejection.

## ADO variables in the prompt

A small, fixed set of ADO **path-anchor** variables is substituted into the prompt
by the runtime resolver:

| Variable | Expands to |
|----------|------------|
| `$(Build.SourcesDirectory)` | the checkout root |
| `$(Build.Repository.Name)` | the trigger repo's subfolder name (multi-checkout) |

The compiler passes these to `import.js` as `--var name=$(name)` flags (ADO expands
the macro at runtime); the resolver then replaces every `$(name)` occurrence in the
final prompt — author body and inlined snippets alike. This is a deliberately tiny,
compiler-owned allowlist (defined by `PROMPT_ADO_VARS` in
`src/compile/extensions/ado_script.rs`), **not** a general expression engine:
arbitrary `$(...)` macros and any pipeline/secret variables are **not** expanded and
reach the agent literally. For richer runtime values (PR sha, branch, etc.), use the
[execution-context `aw-context/` files](execution-context.md) rather than
interpolating into the prompt.

> **Mode note.** With `inlined-imports: true` the body is folded into a bash step at
> compile time, so ADO natively expands *any* `$(...)` macro in it. The curated set
> above behaves identically in both modes; other `$(...)` macros only expand in the
> inlined path and should not be relied on.


## Single-pass behavior

Runtime imports are expanded in a single pass. Imported snippets are inserted
verbatim, and any nested `{{#runtime-import ...}}` or
`{{#runtime-import? ...}}` markers inside those snippets are **not** expanded.
This matches gh-aw's runtime-import behavior.

## Resolver ordering

The runtime-import resolver runs first. Any extension supplements that are
appended later with `cat >>` — including SafeOutputs guidance, GitHub MCP
guidance, runtime guidance, and cache-memory guidance — are added after import
resolution and are left untouched.

## Failure modes

| Marker kind | Missing file behavior |
|---|---|
| `{{#runtime-import path}}` | Resolver exits with status 1 and the pipeline fails. |
| `{{#runtime-import? path}}` | Marker is silently replaced with an empty string. |

When `inlined-imports: true`, the same required/optional rules are applied at
compile time instead of on the pipeline runner.

## Implementation notes

- **Runtime**: `import.js` is ncc-bundled into `ado-script.zip`.
  The always-on `AdoScriptExtension` contributes three typed
  `Declarations::agent_prepare_steps` entries to the Agent job:
  `UseNode@1` install, the `ado-script.zip` download/verify/extract,
  and the `node import.js` resolver invocation. All three run on the
  same VM as the agent — ADO jobs are VM-isolated, so the bundle must
  be downloaded inside whichever job consumes it.
- **Compile time**: `resolve_imports_inline()` in
  `src/compile/extensions/ado_script.rs` performs the inline expansion
  when `inlined-imports: true`.

