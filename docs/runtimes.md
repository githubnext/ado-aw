# Runtimes Configuration

_Part of the [ado-aw documentation](../AGENTS.md)._

## Runtimes Configuration

The `runtimes` field configures language environments that are installed before the agent runs. Unlike tools (which are agent capabilities like edit, bash, memory), runtimes are execution environments that the compiler auto-installs via pipeline steps.

Aligned with [gh-aw's `runtimes:` front matter field](https://github.github.com/gh-aw/reference/frontmatter/#runtimes-runtimes).

### Node.js (`node:`)

Node.js runtime. Installs Node.js via `NodeTool@0` (the same ADO task used internally by the `gate.js` and `prompt.js` ado-script bundles), adds npm registry domains to the network allowlist, extends the bash command allow-list, and appends a prompt supplement informing the agent that Node.js is available.

Optionally configures npm to use a private/internal registry (e.g., an Azure Artifacts feed) with bearer-token authentication.

```yaml
# Simple enablement (installs Node.js 20.x LTS)
runtimes:
  node: true

# Pin to a specific LTS major version
runtimes:
  node:
    version: "22.x"

# With an internal npm feed (Azure Artifacts)
runtimes:
  node:
    version: "20.x"
    internal-feed:
      registry: "https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/"
      auth-token-var: "SC_READ_TOKEN"
```

When enabled, the compiler:
- Injects a `NodeTool@0` step into `{{ prepare_steps }}` (runs before the agent)
- Defaults to Node.js `20.x` (current LTS); accepts any `NodeTool@0` version spec (e.g., `"22.x"`)
- Auto-adds `node`, `npm`, and `npx` to the bash command allow-list
- Adds npm registry domains to the network allowlist (expands the `"node"` ecosystem identifier)
- Appends a prompt supplement informing the agent about Node.js availability and basic commands
- Emits a compile-time warning if `tools.bash` is empty (Node.js requires bash access)

When `internal-feed` is configured, the compiler also injects a `bash` step that:
1. Runs `npm config set registry <registry-url>` to redirect all npm commands to the private registry.
2. If `auth-token-var` is set: runs `npm config set //<registry-path>/:_authToken "$TOKEN"` to authenticate. The token is read from the named pipeline variable at runtime — it is never embedded in the compiled YAML.

#### `internal-feed` options

| Field | Type | Description |
|-------|------|-------------|
| `registry` | string (required) | Full npm registry URL, e.g., `https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/` |
| `auth-token-var` | string (optional) | Pipeline variable name holding the auth token (e.g., `"SC_READ_TOKEN"`). When set, `npm config` is updated with the per-registry `_authToken` so authenticated feeds work without a pre-existing `.npmrc`. |

**Note:** In the 1ES target, the bash command allow-list is updated but the `NodeTool@0` installation step must be done manually via `steps:` front matter. The 1ES target handles network isolation separately.

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
