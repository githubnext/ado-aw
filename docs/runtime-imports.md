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

- **Absolute paths** are used as-is.
- **Relative paths at runtime** are resolved against the `ADO_AW_IMPORT_BASE`
  environment variable. For user-facing imports, the compiler sets this to
  `{{ trigger_repo_directory }}`.
- **Relative paths at compile time** (`inlined-imports: true`) are resolved
  against the source `.md` file's directory.

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

- **Runtime**: `dist/import/index.js` is ncc-bundled into `ado-script.zip`.
  The always-on `AdoScriptExtension`'s `prepare_steps()` injects three
  steps into the Agent job's existing `{{ prepare_steps }}` block:
  `NodeTool@0` install, the `ado-script.zip` download/verify/extract,
  and the `node import.js` resolver invocation. All three run on the
  same VM as the agent — ADO jobs are VM-isolated, so the bundle must
  be downloaded inside whichever job consumes it.
- **Compile time**: `resolve_imports_inline()` in
  `src/compile/extensions/ado_script.rs` performs the inline expansion
  when `inlined-imports: true`.

