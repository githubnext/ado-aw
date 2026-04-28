# Runtimes Configuration

_Part of the [ado-aw documentation](../AGENTS.md)._

## Runtimes Configuration

The `runtimes` field configures language environments that are installed before the agent runs. Unlike tools (which are agent capabilities like edit, bash, memory), runtimes are execution environments that the compiler auto-installs via pipeline steps.

Aligned with [gh-aw's `runtimes:` front matter field](https://github.github.com/gh-aw/reference/frontmatter/#runtimes-runtimes).

### Lean 4 (`lean:`)

Lean 4 theorem prover runtime. Auto-installs the Lean toolchain via elan, extends the bash command allow-list, adds Lean-specific domains to the network allowlist, and appends a prompt supplement informing the agent that Lean is available.

```yaml
# Simple enablement (installs latest stable toolchain)
runtimes:
  lean: true

# With options (pin specific toolchain version)
runtimes:
  lean:
    toolchain: "leanprover/lean4:v4.29.1"
```

When enabled, the compiler:
- Injects an elan installation step into `{{ prepare_steps }}` (runs before AWF network isolation)
- Defaults to the `stable` toolchain; if a `lean-toolchain` file exists in the repo, elan overrides to that version automatically
- Auto-adds `lean`, `lake`, and `elan` to the bash command allow-list
- Adds Lean-specific domains to the network allowlist: `elan.lean-lang.org`, `leanprover.github.io`, `lean-lang.org`
- Installs the toolchain under `/tmp/awf-tools/elan/` (via `ELAN_HOME`) so the wrappers, toolchain binaries, and shared libraries are reachable inside the AWF container, which auto-mounts `/tmp` but not `$HOME`
- Appends a prompt supplement informing the agent that the binaries live at `/tmp/awf-tools/elan/bin/` and showing how to put them on `PATH`
- Emits a compile-time warning if `tools.bash` is empty (Lean requires bash access)

**Note:** In the 1ES target, the bash command allow-list is updated but elan installation must be done manually via `steps:` front matter. The 1ES target handles network isolation separately.
