# `ado-aw-debug:` - Debug-only front-matter section

_Part of the [ado-aw documentation](../AGENTS.md)._

> This section is for compiler dogfood and local diagnostics. It is not a
> regular safe-output surface.

The compiler accepts:

```yaml
ado-aw-debug:
  skip-integrity: true
```

Unrecognized keys fail compilation (`#[serde(deny_unknown_fields)]`).

## `skip-integrity`

Equivalent to passing `--skip-integrity` to `ado-aw compile`. Setting either
omits the generated **Verify pipeline integrity** step.

The integrity step downloads the same ado-aw version used at compile time and
runs `ado-aw check` against the committed pipeline. Without it, a modified
compiled YAML file is not detected at runtime. Use this option only for
short-lived dogfood pipelines.

## Retired: `ado-aw-debug.create-issue`

GitHub issue filing is now a regular safe output:

```yaml
safe-outputs:
  create-github-issue:
    target-repo: githubnext/ado-aw
```

The front-matter codemod moves legacy `ado-aw-debug.create-issue` configuration
to `safe-outputs.create-github-issue`. When no public GitHub auth is already
configured, it also adds:

```yaml
safe-outputs:
  github-token: "$(ADO_AW_DEBUG_GITHUB_TOKEN)"
```

That explicit token reference preserves the existing ADO secret during
migration; it is not a debug runtime path. New workflows should use the
`ADO_AW_GITHUB_TOKEN` default or GitHub App auth. See
[`docs/safe-outputs.md`](safe-outputs.md#github-issue-safe-outputs).
