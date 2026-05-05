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
- Mounts `$HOME/.elan` into the AWF container via `--mount` flag so the elan toolchain is accessible inside the chroot (AWF replaces `$HOME` with an empty overlay for security)
- Appends a prompt supplement informing the agent about Lean 4 availability and basic commands
- Emits a compile-time warning if `tools.bash` is empty (Lean requires bash access)

**Note:** In the 1ES target, the bash command allow-list is updated but elan installation must be done manually via `steps:` front matter. The 1ES target handles network isolation separately.

### Python (`python:`)

Python runtime. Optionally installs a specific Python version via the `UsePythonVersion@0` ADO task, adds PyPI domains to the network allowlist, extends the bash command allow-list (`python`, `python3`, `pip`, `pip3`), and optionally injects package feed environment variables to override the default PyPI registry with an internal feed.

```yaml
# Simple enablement (uses system Python, no install step emitted)
runtimes:
  python: true

# Install a specific Python version
runtimes:
  python:
    version: "3.12"

# Install a version and redirect pip/uv to an internal ADO Artifacts feed
runtimes:
  python:
    version: "3.12"
    index-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"

# Internal primary feed with a public fallback
runtimes:
  python:
    version: "3.x"
    index-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"
    extra-index-url: "https://pypi.org/simple/"
```

When enabled, the compiler:
- Injects a `UsePythonVersion@0` task step into `{{ prepare_steps }}` when `version:` is specified (runs before AWF network isolation); when only `true` is used, relies on the system Python
- Auto-adds `python`, `python3`, `pip`, `pip3` to the bash command allow-list
- Adds the `python` ecosystem identifier to the AWF network allowlist (expands to PyPI domains — `pypi.org`, `files.pythonhosted.org`, etc.)
- Appends a prompt supplement informing the agent about Python availability
- Emits a compile-time warning if `tools.bash` is empty (Python requires bash access)

#### Internal feed configuration

When `index-url:` is specified, the compiler injects the following environment variables into the AWF agent step:

| Environment variable | Tool | Purpose |
|---|---|---|
| `PIP_INDEX_URL` | pip | Overrides the primary pip package index |
| `UV_DEFAULT_INDEX` | uv | Overrides the primary uv package index |

When `extra-index-url:` is specified additionally:

| Environment variable | Tool | Purpose |
|---|---|---|
| `PIP_EXTRA_INDEX_URL` | pip | Adds a secondary fallback pip package index |

These variables are injected into the `env:` block of the AWF step so they are visible to the agent process inside the network-isolated sandbox. This allows `pip install` and `uv` commands run by the agent to resolve packages from the internal feed.

**Tip:** If you want to prevent the agent from falling back to public PyPI entirely, set `network.blocked` to block `pypi.org` and `files.pythonhosted.org` after pointing the index URL to your internal feed.

