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
- Contributes an elan installation step to `Declarations::agent_prepare_steps` (runs before AWF network isolation)
- Defaults to the `stable` toolchain; if a `lean-toolchain` file exists in the repo, elan overrides to that version automatically
- Auto-adds `lean`, `lake`, and `elan` to the bash command allow-list
- Adds Lean-specific domains to the network allowlist: `elan.lean-lang.org`, `leanprover.github.io`, `lean-lang.org`
- Mounts `$HOME/.elan` into the AWF container via `--mount` flag so the elan toolchain is accessible inside the chroot (AWF replaces `$HOME` with an empty overlay for security)
- Appends a prompt supplement informing the agent about Lean 4 availability and basic commands
- Emits a compile-time warning if `tools.bash` is empty (Lean requires bash access)

**Note:** In the 1ES target, the bash command allow-list is updated but elan installation must be done manually via `steps:` front matter. The 1ES target handles network isolation separately.

### Python (`python:`)

Python runtime. Auto-installs Python via `UsePythonVersion@0`, adds Python ecosystem domains to the AWF network allowlist, extends the bash command allow-list, and optionally injects feed URL env vars for pip and uv.

```yaml
# Simple enablement (installs default Python 3.x)
runtimes:
  python: true

# With options (pin version, configure feed)
runtimes:
  python:
    version: "3.12"
    feed: internal          # a permissions.packages feed granting pypi
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Python version to install (e.g., `"3.12"`, `"3.11"`). Passed to `UsePythonVersion@0` `versionSpec`. Defaults to latest 3.x. |
| `feed` | string | Handle of a `permissions.packages` feed that grants `pypi`. Sets `PIP_INDEX_URL` and `UV_DEFAULT_INDEX` to the feed's canonical `pypi/simple/` URL; the credential stays in the package proxy (see [`package-feeds.md`](package-feeds.md)). Mutually exclusive with `feed-url` and `config`. |
| `public-registry` | `allow` \| `block` | `block` removes the public PyPI hosts from the AWF allowlist. Defaults to `allow`. |
| `feed-url` | string | PyPI-compatible feed URL for a feed that needs no credential. Injects `PIP_INDEX_URL` and `UV_DEFAULT_INDEX` env vars into the agent environment. Azure Artifacts URLs are migrated to `feed` by codemod 0009 (see [Package-feed credentials](#package-feed-credentials)). |
| `config` | string | Path to a pip/uv config file. Accepted with a warning — the file will not be available inside the AWF agent environment until proxy-auth support lands. |

When enabled, the compiler:
- Contributes a `UsePythonVersion@0` task to `Declarations::agent_prepare_steps` (runs before AWF)
- Auto-adds `python`, `python3`, `pip`, `pip3`, `uv` to the bash command allow-list
- Adds Python ecosystem domains to the network allowlist (pypi.org, pythonhosted.org, etc.)
- If `feed` or `feed-url` is set, injects `PIP_INDEX_URL` and `UV_DEFAULT_INDEX` env vars into the agent environment; `feed-url` also warns that the agent holds no feed credential
- Appends a prompt supplement informing the agent about Python availability
- No AWF mounts or PATH prepends needed — `UsePythonVersion@0` installs to `/opt/hostedtoolcache` (auto-mounted by AWF) and publishes PATH entries that AWF merges via `$GITHUB_PATH`

### Node.js (`node:`)

Node.js runtime. Auto-installs Node.js via `UseNode@1`, adds Node ecosystem domains to the AWF network allowlist, extends the bash command allow-list, and optionally injects feed URL env vars for npm.

```yaml
# Simple enablement (installs default Node LTS)
runtimes:
  node: true

# With options (pin version, configure feed)
runtimes:
  node:
    version: "22.x"
    feed: internal          # a permissions.packages feed granting npm
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Node.js version to install (e.g., `"22.x"`, `"20.x"`). Passed to `UseNode@1` `version`. Defaults to `"22.x"`. |
| `feed` | string | Handle of a `permissions.packages` feed that grants `npm`. Sets `NPM_CONFIG_REGISTRY` to the feed's canonical `npm/registry/` URL; the credential stays in the package proxy (see [`package-feeds.md`](package-feeds.md)). Mutually exclusive with `feed-url` and `config`. |
| `public-registry` | `allow` \| `block` | `block` removes the public npm registry hosts from the AWF allowlist. Defaults to `allow`. |
| `feed-url` | string | npm registry URL for a registry that needs no credential. Injects `NPM_CONFIG_REGISTRY` env var into the agent environment. Azure Artifacts URLs are migrated to `feed` by codemod 0009 (see [Package-feed credentials](#package-feed-credentials)). |
| `config` | string | Path to an .npmrc config file. Accepted with a warning — the file will not be available inside the AWF agent environment until proxy-auth support lands. |

When enabled, the compiler:
- Contributes a `UseNode@1` task to `Declarations::agent_prepare_steps` (runs before AWF)
- Auto-adds `node`, `npm`, `npx` to the bash command allow-list
- Adds Node ecosystem domains to the network allowlist (npmjs.org, nodejs.org, etc.)
- If `feed` or `feed-url` is set, injects `NPM_CONFIG_REGISTRY` env var into the agent environment; `feed-url` also warns that the agent holds no feed credential
- Appends a prompt supplement informing the agent about Node.js availability
- No AWF mounts or PATH prepends needed — `UseNode@1` installs to `/opt/hostedtoolcache` (auto-mounted by AWF) and publishes PATH entries that AWF merges via `$GITHUB_PATH`
- Note: AWF overlays `~/.npmrc` with `/dev/null` for credential security — the `NPM_CONFIG_REGISTRY` env var approach avoids conflicting with this overlay

### .NET (`dotnet:`)
.NET runtime. Auto-installs the .NET SDK via `UseDotNet@2`, adds .NET ecosystem domains to the AWF network allowlist, and extends the bash command allow-list with `dotnet`.

```yaml
# Simple enablement (installs default .NET SDK, currently 8.0.x)
runtimes:
  dotnet: true

# With options (pin version, configure internal feed)
runtimes:
  dotnet:
    version: "8.0.x"
    feed: internal          # a permissions.packages feed granting nuget

# Or point at a checked-in nuget.config
runtimes:
  dotnet:
    version: "8.0.x"
    config: "nuget.config"

# Pin SDK from the repo's global.json (UseDotNet@2 useGlobalJson mode)
runtimes:
  dotnet:
    version: "global.json"
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | .NET SDK version to install (e.g., `"8.0.x"`, `"9.0.x"`). Passed to `UseDotNet@2` `version` with `packageType: 'sdk'`. Defaults to `"8.0.x"`. The special value `"global.json"` (case-insensitive) emits `useGlobalJson: true` instead, which discovers and installs every SDK referenced by `global.json` files in the workspace. |
| `feed` | string | Handle of a `permissions.packages` feed that grants `nuget`. Writes a minimal `nuget.config` pointing at the feed's canonical `nuget/v3/index.json` when none exists; the credential stays in the package proxy (see [`package-feeds.md`](package-feeds.md)). Mutually exclusive with `feed-url` and `config`. |
| `public-registry` | `allow` \| `block` | `block` removes the public nuget.org hosts from the AWF allowlist. Defaults to `allow`. |
| `feed-url` | string | NuGet feed URL for a feed that needs no credential. When set, the compiler creates a minimal `nuget.config` if none exists. Azure Artifacts URLs are migrated to `feed` by codemod 0009 (see [Package-feed credentials](#package-feed-credentials)). |
| `config` | string | Path to a checked-in `nuget.config` in the repo. Mutually exclusive with `feed-url`. Azure Artifacts sources in it work when `permissions.packages` grants them. |

**`global.json` precedence.** A `global.json` file in the repo is the canonical
way to pin the .NET SDK. The compiler enforces a single source of truth:

- If a `global.json` exists at the agent's compile directory **and** the front
  matter sets a concrete `version`, compilation **errors out**. Either remove
  the front-matter version or set it to the literal string `"global.json"` to
  opt into `UseDotNet@2`'s `useGlobalJson: true` mode.
- If `version: "global.json"` is set, the compiler emits
  `useGlobalJson: true` (no explicit `version:` input) so the install task
  walks the workspace for `global.json` files itself.
- If no `version` is set and a `global.json` exists, the compiler does not
  auto-promote — the default `"8.0.x"` is used. Opt in explicitly with the
  sentinel.

When enabled, the compiler:
- Contributes a `UseDotNet@2` task to `Declarations::agent_prepare_steps` (runs before AWF)
- If `feed` or `feed-url` is set, injects an ensure-`nuget.config` step (writes a minimal `nuget.config` referencing the feed only when one doesn't already exist)
- If `feed-url` or `config` is set, warns that the agent holds no feed credential
- Auto-adds `dotnet` to the bash command allow-list
- Adds .NET ecosystem domains to the network allowlist (nuget.org, dotnet.microsoft.com, pkgs.dev.azure.com, etc.)
- Appends a prompt supplement informing the agent about .NET availability
- No AWF mounts or PATH prepends needed — `UseDotNet@2` installs to `/opt/hostedtoolcache` (auto-mounted by AWF) and publishes PATH entries that AWF merges via `$GITHUB_PATH`

**Differences from the Python and Node runtimes** (called out for clarity, since this runtime intentionally diverges):
- **No agent env var is injected for `feed-url`.** Unlike `pip` (`PIP_INDEX_URL`) and `npm` (`NPM_CONFIG_REGISTRY`), NuGet has no first-class environment-variable equivalent for selecting a package source. Feed configuration always goes through a `nuget.config` file.
- **`config:` is visible to the agent.** AWF only overlays files in `$HOME` (e.g., `~/.npmrc` → `/dev/null`); workspace files such as `nuget.config` are preserved inside the agent sandbox, so a checked-in `nuget.config` selects sources today.

### Package-feed credentials

The compiler never emits `PipAuthenticate`, `npmAuthenticate`, or
`NuGetAuthenticate` in a job that runs the AWF sandbox. These tasks must
export their credential as a **non-secret** job variable or write it to a
file so their client tools can read it — `PIP_EXTRA_INDEX_URL` with an
embedded token, `VSS_NUGET_ACCESSTOKEN`, or `_authToken` lines appended to
the workspace `.npmrc`. Azure Pipelines maps non-secret variables into every
later step's environment, and AWF starts the agent with `--env-all`, so any
of them would hand the job's build-identity token to the agent.

Consequences:

- Azure Artifacts feeds are reached through the credential-isolated package
  proxy instead: grant them under `permissions.packages` and select them with
  `runtimes.<x>.feed` (see [`package-feeds.md`](package-feeds.md)). Codemod
  0009 migrates Azure Artifacts `feed-url` values automatically.
- `feed-url` and `config` select a package source but carry no credential.
  Anonymous feeds work; other feeds that require authentication reject the
  agent's requests, and the compiler warns.
- `steps:` and `safe-outputs.threat-detection.steps` — operator steps that
  run before AWF in the same job — may not use `NuGetAuthenticate`,
  `npmAuthenticate`, `PipAuthenticate`, `TwineAuthenticate`,
  `CargoAuthenticate`, or `MavenAuthenticate`; compilation fails. `setup:`,
  `post-steps:`, and `teardown:` are unaffected.
- Both AWF invocations exclude `VSS_NUGET_ACCESSTOKEN`,
  `VSS_NUGET_EXTERNAL_FEED_ENDPOINTS`, `PIP_EXTRA_INDEX_URL`, and
  `CARGO_REGISTRY_TOKEN` from `--env-all` as defense in depth.

### Combining Runtimes

Multiple runtimes can be enabled simultaneously:

```yaml
runtimes:
  python:
    version: "3.12"
  node:
    version: "22.x"
  dotnet:
    version: "8.0.x"
  lean: true
```

All runtime extensions are sorted into `ExtensionPhase::Runtime` and execute before tool extensions (`ExtensionPhase::Tool`), ensuring language toolchains are available before any tools that depend on them.
