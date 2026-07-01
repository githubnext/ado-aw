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

Python runtime. Auto-installs Python via `UsePythonVersion@0`, emits `PipAuthenticate@1` for internal feed access, adds Python ecosystem domains to the AWF network allowlist, extends the bash command allow-list, and optionally injects feed URL env vars for pip and uv.

```yaml
# Simple enablement (installs default Python 3.x)
runtimes:
  python: true

# With options (pin version, configure feed)
runtimes:
  python:
    version: "3.12"
    feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Python version to install (e.g., `"3.12"`, `"3.11"`). Passed to `UsePythonVersion@0` `versionSpec`. Defaults to latest 3.x. |
| `feed-url` | string | Internal PyPI feed URL. Injects `PIP_INDEX_URL` and `UV_DEFAULT_INDEX` env vars into the agent environment. |
| `config` | string | Path to a pip/uv config file. Accepted with a warning — the file will not be available inside the AWF agent environment until proxy-auth support lands. |

When enabled, the compiler:
- Contributes a `UsePythonVersion@0` task to `Declarations::agent_prepare_steps` (runs before AWF)
- If `feed-url` is set, also injects `PipAuthenticate@1` to authenticate the ADO build service identity for internal feeds
- Auto-adds `python`, `python3`, `pip`, `pip3`, `uv` to the bash command allow-list
- Adds Python ecosystem domains to the network allowlist (pypi.org, pythonhosted.org, etc.)
- If `feed-url` is set, injects `PIP_INDEX_URL` and `UV_DEFAULT_INDEX` env vars into the agent environment
- Appends a prompt supplement informing the agent about Python availability
- No AWF mounts or PATH prepends needed — `UsePythonVersion@0` installs to `/opt/hostedtoolcache` (auto-mounted by AWF) and publishes PATH entries that AWF merges via `$GITHUB_PATH`

**Note:** `PipAuthenticate@1` is currently emitted with an empty `artifactFeeds` input, which configures credentials for all feeds accessible to the build service identity. If your internal feed requires scoped authentication to a specific Azure Artifacts feed, this may need future refinement.

### Node.js (`node:`)

Node.js runtime. Auto-installs Node.js via `UseNode@1`, emits `npmAuthenticate@0` for internal feed access, adds Node ecosystem domains to the AWF network allowlist, extends the bash command allow-list, and optionally injects feed URL env vars for npm.

```yaml
# Simple enablement (installs default Node LTS)
runtimes:
  node: true

# With options (pin version, configure feed)
runtimes:
  node:
    version: "22.x"
    feed-url: "https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/"
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Node.js version to install (e.g., `"22.x"`, `"20.x"`). Passed to `UseNode@1` `version`. Defaults to `"22.x"`. |
| `feed-url` | string | Internal npm registry URL. Injects `NPM_CONFIG_REGISTRY` env var into the agent environment. |
| `config` | string | Path to an .npmrc config file. Accepted with a warning — the file will not be available inside the AWF agent environment until proxy-auth support lands. |

When enabled, the compiler:
- Contributes a `UseNode@1` task to `Declarations::agent_prepare_steps` (runs before AWF)
- If `feed-url` or `config` is set, also injects `npmAuthenticate@0` (and an ensure-`.npmrc` step) to authenticate the ADO build service identity for internal feeds
- Auto-adds `node`, `npm`, `npx` to the bash command allow-list
- Adds Node ecosystem domains to the network allowlist (npmjs.org, nodejs.org, etc.)
- If `feed-url` is set, injects `NPM_CONFIG_REGISTRY` env var into the agent environment
- Appends a prompt supplement informing the agent about Node.js availability
- No AWF mounts or PATH prepends needed — `UseNode@1` installs to `/opt/hostedtoolcache` (auto-mounted by AWF) and publishes PATH entries that AWF merges via `$GITHUB_PATH`
- Note: AWF overlays `~/.npmrc` with `/dev/null` for credential security — the `NPM_CONFIG_REGISTRY` env var approach avoids conflicting with this overlay

### .NET (`dotnet:`)
.NET runtime. Auto-installs the .NET SDK via `UseDotNet@2`, emits `NuGetAuthenticate@1` for internal feed access, adds .NET ecosystem domains to the AWF network allowlist, and extends the bash command allow-list with `dotnet`.

```yaml
# Simple enablement (installs default .NET SDK, currently 8.0.x)
runtimes:
  dotnet: true

# With options (pin version, configure internal feed)
runtimes:
  dotnet:
    version: "8.0.x"
    feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json"

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
| `feed-url` | string | Internal NuGet feed URL (typically the v3 `index.json` of an Azure Artifacts feed). When set, the compiler creates a minimal `nuget.config` if none exists and runs `NuGetAuthenticate@1`. |
| `config` | string | Path to a checked-in `nuget.config` in the repo. When set, the compiler runs `NuGetAuthenticate@1` (which auto-discovers `nuget.config` files in the workspace). Mutually exclusive with `feed-url`. |

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
- If `feed-url` is set, injects an ensure-`nuget.config` step (writes a minimal `nuget.config` referencing the feed only when one doesn't already exist) and `NuGetAuthenticate@1`
- If `config` is set (and `feed-url` is not), injects `NuGetAuthenticate@1` only — the user-checked-in `nuget.config` is assumed to be present in the workspace
- Auto-adds `dotnet` to the bash command allow-list
- Adds .NET ecosystem domains to the network allowlist (nuget.org, dotnet.microsoft.com, pkgs.dev.azure.com, etc.)
- Appends a prompt supplement informing the agent about .NET availability
- No AWF mounts or PATH prepends needed — `UseDotNet@2` installs to `/opt/hostedtoolcache` (auto-mounted by AWF) and publishes PATH entries that AWF merges via `$GITHUB_PATH`

**Differences from the Python and Node runtimes** (called out for clarity, since this runtime intentionally diverges):
- **No agent env var is injected for `feed-url`.** Unlike `pip` (`PIP_INDEX_URL`) and `npm` (`NPM_CONFIG_REGISTRY`), NuGet has no first-class environment-variable equivalent for selecting a package source. Feed configuration always goes through a `nuget.config` file.
- **`config:` is functional, not a deferred warning.** AWF only overlays files in `$HOME` (e.g., `~/.npmrc` → `/dev/null`); workspace files such as `nuget.config` are preserved inside the agent sandbox, so a checked-in `nuget.config` works today.
- **`NuGetAuthenticate@1` requires no `workingFile:` input.** It auto-discovers `nuget.config` files anywhere in the workspace, unlike `npmAuthenticate@0` which needs an explicit path.

### TLA+ (`tla:`)

TLA+ / TLC formal model-checking runtime. Uses `JavaToolInstaller@0` to select a pre-installed JDK and downloads `tla2tools.jar` from the TLA+ GitHub releases page, then creates convenience shims (`tlc`, `pluscal`, `sany`) and extends the bash command allow-list.

TLA+ is a natural complement to the `lean` runtime: **TLC discovers** liveness/safety gaps by exhaustively searching the state space and emitting counterexample traces, while **Lean proves** a fixed design. Agentic workflows that scan codebases for state-machine bugs — finding stuck states, deadlocks, and missing timeouts — are a primary use case.

```yaml
# Simple enablement (latest tla2tools.jar, JDK 21 LTS)
runtimes:
  tla: true

# With options (pin tla2tools.jar and JDK versions)
runtimes:
  tla:
    version: "1.8.0"   # tla2tools.jar version (omit for latest)
    jdk: "21"          # JDK major version (default: 21)
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | `tla2tools.jar` version to download from the TLA+ GitHub releases (e.g., `"1.8.0"`, `"1.7.3"`). When omitted, the latest release is downloaded via GitHub's `releases/latest/download/` redirect. |
| `jdk` | string | JDK major version to pass to `JavaToolInstaller@0` (pre-installed on the build agent), e.g., `"21"`, `"17"`. Defaults to `"21"` (current LTS). |

When enabled, the compiler:
- Contributes a `JavaToolInstaller@0` task step (pre-installed mode) that selects the JDK matching `jdk:` and sets `JAVA_HOME`
- Contributes a bash step to `Declarations::agent_prepare_steps` that:
  1. Downloads `tla2tools.jar` from [TLA+ GitHub releases](https://github.com/tlaplus/tlaplus/releases) into `$HOME/.tla/`
  2. Creates convenience shims (`tlc`, `pluscal`, `sany`) in `$HOME/.tla/bin/` that delegate to `java` from PATH
- Auto-adds `java`, `tlc`, `pluscal`, `sany` to the bash command allow-list
- GitHub is already in the built-in allowlist so no extra network host entry is needed for `tla2tools.jar`. The JDK is provided by `JavaToolInstaller@0` (pre-installed on the build agent) so no additional network access is needed for the JDK either.
- Mounts `$HOME/.tla` and `$(JAVA_HOME)` into the AWF container via `--mount` (read-only) so the toolchain is accessible inside the agent sandbox
- Prepends `$HOME/.tla/bin` and `$(JAVA_HOME)/bin` to `PATH` inside the AWF sandbox
- Sets `TLA_JAR` as a pipeline variable so downstream steps can reference the jar path
- Appends a prompt supplement informing the agent about TLA+ availability and invocation patterns

**Shims:**

| Command | Java main class | Description |
|---------|----------------|-------------|
| `tlc` | `tlc2.TLC` | Run the TLC model checker: `tlc -config M.cfg M.tla` |
| `pluscal` | `pcal.trans` | Translate PlusCal to TLA+: `pluscal -nocfg M.tla` |
| `sany` | `tla2sany.SANY` | Parse and syntax-check a spec: `sany M.tla` |

You can also invoke the JVM directly: `java -XX:+UseParallelGC -cp "$TLA_JAR" tlc2.TLC -workers auto -config M.cfg M.tla`

**Environment variables available inside the agent sandbox:**

| Variable | Value |
|----------|-------|
| `TLA_JAR` | Absolute path to `tla2tools.jar` |
| `JAVA_HOME` | JDK home directory (set by `JavaToolInstaller@0`) |

**Example: TLA+ model-checking workflow**

```yaml
---
name: tla-liveness-checker
description: |
  Scans the codebase for event-driven state machines, builds TLA+ models,
  runs TLC to find liveness gaps, and proposes findings via safe-outputs.
on:
  schedule: "weekly on monday"
runtimes:
  tla: true
tools:
  bash:
    - find
    - grep
    - tlc
    - pluscal
    - sany
    - java
safe-outputs:
  create-pull-request: {}
  create-work-item: {}
---

Scan the repository for state machine implementations (look for enums with
state names, switch/match blocks on state, or explicit state transition maps).

For each state machine found:
1. Build a TLA+ model in a temp file capturing the states and transitions.
2. Run `pluscal` to translate PlusCal if used, or write TLA+ directly.
3. Run `tlc -config <cfg> <spec>.tla` to check for deadlocks and liveness.
4. If TLC finds a counterexample trace, record the violation.

If violations are found, propose:
- A draft PR adding the TLA+ models under `formal/` in the repository.
- A work item per liveness gap with the TLC counterexample trace attached.
```

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
  tla:
    version: "1.8.0"
```

All runtime extensions are sorted into `ExtensionPhase::Runtime` and execute before tool extensions (`ExtensionPhase::Tool`), ensuring language toolchains are available before any tools that depend on them.
