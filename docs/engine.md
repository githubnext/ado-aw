# Engine Configuration

_Part of the [ado-aw documentation](../AGENTS.md)._

## Engine Configuration

The `engine` field specifies which engine to use for the agentic task. The string form is an engine identifier (currently only `copilot` is supported). The object form uses `id` for the engine identifier plus additional options like model selection and timeout.

```yaml
# Simple string format (engine identifier, defaults to copilot)
engine: copilot

# Object format with additional options
engine:
  id: copilot
  model: claude-opus-4.7
  timeout-minutes: 30
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | `copilot` | Engine identifier. Currently only `copilot` (GitHub Copilot CLI) is supported. |
| `model` | string | `claude-opus-4.7` | AI model to use (e.g., `claude-sonnet-4.6`). The compiler passes the value directly to the Copilot CLI `--model` flag — any model identifier the Copilot CLI accepts is valid. |
| `timeout-minutes` | integer | *(none)* | Maximum time in minutes the agent job is allowed to run. Sets `timeoutInMinutes` on the `Agent` job in the generated pipeline. |
| `version` | string | *(none)* | Engine CLI version to install (e.g., `"1.0.64"`, `"latest"`). Overrides the pinned `COPILOT_CLI_VERSION`. Set to `"latest"` to use the newest available version. |
| `agent` | string | *(none)* | Custom agent file identifier (Copilot only). Adds `--agent <name>` to the CLI invocation, selecting a custom agent from `.github/agents/`. |
| `api-target` | string | *(none)* | Custom API endpoint hostname for GHES/GHEC (e.g., `"api.acme.ghe.com"`). Adds `--api-target <hostname>` to the CLI invocation and adds the hostname to the AWF network allowlist. |
| `args` | list | `[]` | Custom CLI arguments appended after compiler-generated args. Subject to shell-safety validation and blocked from overriding compiler-controlled flags (`--prompt`, `--additional-mcp-config`, `--allow-tool`, `--allow-all-tools`, `--allow-all-paths`, `--disable-builtin-mcps`, `--no-ask-user`, `--ask-user`). |
| `env` | map | *(none)* | Engine-specific environment variables merged into the sandbox step's `env:` block. Keys must be valid env var names. Values are literal-only and must not contain ADO expressions (`$(`, `${{`, `$[`) or pipeline command injection (`##vso[`), **except** the Copilot provider keys (`COPILOT_PROVIDER_BASE_URL`, `COPILOT_PROVIDER_API_KEY`, `COPILOT_PROVIDER_BEARER_TOKEN`, `COPILOT_PROVIDER_WIRE_API`), which may carry an ADO macro (`$(...)`) expression. Prefer the typed [`provider`](#copilot-model-provider-byok-configuration) block over raw provider env keys. Compiler-controlled keys (`GITHUB_TOKEN`, `PATH`, `BASH_ENV`, etc.) are blocked. |
| `provider` | map | *(none)* | Copilot external model-provider (BYOK) configuration: `base-url`, `type`, `wire-api`, `token` (compiler-minted bearer via a service connection), `api-key`. Maps to the `COPILOT_PROVIDER_*` env vars. See [Copilot model provider (BYOK) configuration](#copilot-model-provider-byok-configuration). |
| `command` | string | *(none)* | Custom engine executable path (skips the default engine binary installation — NuGet for `target: 1es`, GitHub Releases for all other targets). The path must be accessible inside the AWF container (e.g., `/tmp/...` or workspace-mounted paths). |
| `github-app-token` | map | *(none)* | GitHub App-backed Copilot engine authentication. When set, the compiler mints (and, by default, revokes) a GitHub App installation token in the Agent and Detection jobs and sources `GITHUB_TOKEN` from it (for Copilot only). See [GitHub App-backed Copilot engine auth](#github-app-backed-copilot-engine-auth). |


### `timeout-minutes`

The `timeout-minutes` field sets a wall-clock limit (in minutes) for the entire agent job. It maps to the Azure DevOps `timeoutInMinutes` job property on `Agent`. This is useful for:

- **Budget enforcement** — hard-capping the total runtime of an agent to control compute costs.
- **Pipeline hygiene** — preventing agents from occupying a runner indefinitely if they stall or enter long retry loops.
- **SLA compliance** — ensuring scheduled agents complete within a known window.

When omitted, Azure DevOps uses its default job timeout (60 minutes). When set, the compiler emits `timeoutInMinutes: <value>` on the agentic job.

### GitHub App-backed Copilot engine auth

By default the Copilot engine authenticates with the `GITHUB_TOKEN` pipeline
variable you provide (`ado-aw secrets set GITHUB_TOKEN <pat>`). For
organization-backed Copilot authentication you can instead have the compiler
mint a **GitHub App installation access token** at runtime — mirroring
[gh-aw's](https://github.com/githubnext/gh-aw) `create-github-app-token` model,
adapted to Azure DevOps.

```yaml
engine:
  id: copilot
  github-app-token:
    app-id: 1234567            # literal App ID or client ID (required)
    owner: octo-org            # installation owner (org or user login)
    repositories: [octo-repo]  # optional; scopes the token to these repos
    # api-url: https://ghe.example.com/api/v3   # optional; GHES base URL
    # skip-token-revocation: false              # optional; revoke by default
    # private-key: MY_SECRET_VAR                # optional override; see below
```

Store the private key once and you're done:

```bash
ado-aw secrets set GITHUB_APP_PRIVATE_KEY "$(cat app-private-key.pem)"
```

| Field | Required | Description |
|-------|----------|-------------|
| `app-id` | yes | The GitHub App ID — a **literal** value: either a numeric App ID (e.g. `1234567`, quoted or unquoted) or an alphanumeric client ID (e.g. `Iv23liABC…`). The App ID is not secret (it is plain per-app config, like `owner`), so it is written verbatim, not indirected through a variable. |
| `owner` | yes | GitHub installation owner (organization or user login). |
| `repositories` | no | Repository names (owner-relative) to scope the installation token to. Omit to span every repository the installation grants. |
| `api-url` | no | GitHub API base URL. Defaults to `https://api.github.com` (GHEC). For GitHub Enterprise Server, set the `/api/v3` base URL (e.g. `https://ghe.example.com/api/v3`). Must be an `https://` URL. |
| `skip-token-revocation` | no | When `true`, do not revoke the minted token after the Copilot run. Defaults to `false` (the token is revoked — see below). |
| `private-key` | no | Name of the ADO **secret** pipeline variable holding the private key (PEM). **Defaults to `GITHUB_APP_PRIVATE_KEY`** — the compiler owns the name, exactly like `GITHUB_TOKEN`, so the common case sets no field and just runs `ado-aw secrets set GITHUB_APP_PRIVATE_KEY …`. The mint bundle normalizes common ADO PEM representations (raw multiline PEM, escaped-newline text like `\\n`/`\\r\\n`, and whitespace-collapsed PEM bodies). Set this only to point at a differently-named secret (e.g. reusing an existing variable). |

#### Setup

1. Create the GitHub App and install it on the owning org/user, ensuring the
   installation is suitable for Copilot organization-backed
   authentication/billing in your tenant. Copilot capability is granted on the
   **App/org side** — the installation token inherits it automatically; you do
   not (and cannot) configure it here.
2. Store the private key as an ADO **secret** pipeline variable — the default
   name `GITHUB_APP_PRIVATE_KEY` unless you set a `private-key` override:

   ```bash
   ado-aw secrets set GITHUB_APP_PRIVATE_KEY "$(cat app-private-key.pem)"
   ```
3. Set `engine.github-app-token` (`app-id` + `owner` at minimum) and compile.
   Each compile prints a non-blocking advisory reminding you to store the
   private-key variable as a secret — this is expected (the compiler cannot
   verify secrecy itself).

#### What the compiler generates

- A **token-mint step** (the `github-app-token` ado-script bundle) runs
  immediately before the Copilot invocation in **both** the Agent and Detection
  jobs. It builds a short-lived RS256 JWT from the App ID + private key,
  resolves the installation for `owner`, exchanges it for an installation
  access token (optionally scoped to `repositories`), and exposes it as a
  **masked, same-job** `GITHUB_APP_TOKEN` variable.
- The Copilot engine's `GITHUB_TOKEN` is then sourced from `$(GITHUB_APP_TOKEN)`
  instead of the operator-provided `$(GITHUB_TOKEN)` variable.
- A **token-revocation step** runs after the Copilot invocation in both jobs
  (unless `skip-token-revocation: true`). It deletes the minted token
  (`DELETE /installation/token`) so it does not remain valid for its full ~1h
  lifetime — matching `actions/create-github-app-token`'s default. Revocation is
  best-effort (`always()` + `continueOnError`) and never fails the build.
- The token is **never** provided to SafeOutputs, user-authored `steps:`,
  ManualReview, Teardown, or Conclusion.

The mint/revoke steps run **outside** the AWF network sandbox (like the
ADO-token and execution-context steps), reaching the GitHub API over the build
agent pool's normal network — no AWF `network.allowed` entry is required.

#### Scope and boundaries — what this token is *not*

- **ADO API permissions** (`permissions.read` / `permissions.write`) are
  entirely separate: they describe Azure DevOps OAuth/service-connection tokens
  used by the pipeline and Stage 3 executor. The GitHub App token has no effect
  on them.
- The GitHub App token is for **Copilot engine authentication only**. It does
  **not** authenticate the `gh` CLI, grant GitHub MCP permissions, or give the
  agent sandbox GitHub write access.
- There is **no `permissions:` sub-field** to scope the token. Copilot access
  rides on the App's own granted capability (configured App/org-side); narrowing
  the installation token's permissions at mint time cannot grant Copilot access
  and risks stripping the capability it needs.

#### Notes and limitations

- **Copilot engine only.** `github-app-token` is rejected at compile time for
  any other `engine.id` (the minted token is wired into `GITHUB_TOKEN` only on
  the Copilot path), so a misconfiguration fails fast rather than silently
  no-opping.
- **Secrecy is your responsibility.** The compiler validates the private-key
  *variable name* but cannot verify the ADO variable is actually marked secret,
  so every compile of a `github-app-token` workflow emits an advisory:
  `Warning: engine.github-app-token uses pipeline variable '<name>' … Ensure
  '<name>' is stored as a SECRET …`. It is non-blocking — heed it by setting the
  value with `ado-aw secrets set` (which stores it as a secret).
- **Project-level keys via variable groups.** To manage the private key once
  at the project level instead of setting it on every pipeline definition,
  store it in an Azure DevOps **variable group** and import that group with the
  top-level [`variable-groups:`](front-matter.md#variable-groups-variable-groups)
  front-matter field. Point `private-key` at the variable the group supplies
  (e.g. `private-key: AGENTIC_WORKFLOWS_GITHUB_APP_PRIVATE_KEY`). Remember that
  in ADO the group must be **both** authorized on the pipeline definition **and**
  imported in YAML — `variable-groups:` provides the import.
- **GHEC by default; GHES via `api-url`.** The mint/revoke steps target
  `https://api.github.com` unless you set `api-url` to your GitHub Enterprise
  Server `/api/v3` base URL. This is independent of `engine.api-target`, which
  configures the Copilot API host, not the GitHub App API host.
- **`openssl` is not required** — the token is minted with Node's built-in
  crypto. The build agent needs Node (installed automatically) and network
  access to the GitHub API host.
- You may still need to pin `engine.version` until the relevant Copilot CLI auth
  behavior is broadly available.

### Copilot model provider (BYOK) configuration

The Copilot engine can route requests to an external LLM provider — for example a
private **Azure Copilot Foundry** instance — instead of GitHub's default routing.
This **Bring Your Own Key (BYOK)** mode is configured with the dedicated
**`engine.provider`** block, which the compiler maps to the `COPILOT_PROVIDER_*`
environment variables the Copilot CLI reads to reach the provider.

Prefer `engine.provider` over hand-writing `COPILOT_PROVIDER_*` keys in
`engine.env`: it is typed and validated, and — critically — its `token`
sub-block lets the compiler **acquire the provider bearer token for you**,
minted **in the same job** as each engine run so it resolves correctly at
runtime (raw `engine.env` cross-job macros like `$(Setup.FOUNDRY_TOKEN)` do
**not** resolve — see the note at the end of this section).

#### `engine.provider` fields

| Field | Required | Maps to | Description |
|-------|----------|---------|-------------|
| `base-url` | yes | `COPILOT_PROVIDER_BASE_URL` | Base URL of the external provider (e.g. `https://RESOURCE.cognitiveservices.azure.com/openai/v1`). A literal host is auto-added to the AWF network allowlist. |
| `type` | optional | `COPILOT_PROVIDER_TYPE` | Provider format: `openai` (default), `azure`, or `anthropic`. |
| `wire-api` | optional | `COPILOT_PROVIDER_WIRE_API` | Wire API variant: `completions` (default) or `responses`. |
| `token` | optional | `COPILOT_PROVIDER_API_KEY` | Compiler-minted credential via Azure CLI (see below). Mutually exclusive with `api-key`. |
| `api-key` | optional | `COPILOT_PROVIDER_API_KEY` | Static API key, typically a `$(VAR)` secret pipeline variable. Mutually exclusive with `token`. |

The model itself is set via `engine.model` (or a `COPILOT_MODEL` env var).

#### Compiler-owned token acquisition (`provider.token`)

Set `provider.token` to have the compiler mint the provider bearer token in-job
via Azure CLI + an ARM service connection:

| `token` field | Required | Description |
|---------------|----------|-------------|
| `service-connection` | yes | ARM service connection used to authenticate `az` before minting the token. |
| `resource` | optional | Azure resource (audience) passed to `az account get-access-token --resource`. Defaults to `https://cognitiveservices.azure.com`. |

The compiler emits an `AzureCLI@2` step immediately before the Copilot
invocation in **both** the Agent and Detection jobs. That step runs
`az account get-access-token` and publishes the result as a **same-job secret**
pipeline variable (`AW_PROVIDER_BEARER_TOKEN`), which is wired into
`COPILOT_PROVIDER_API_KEY` — the credential env var the AWF api-proxy sidecar
reads and forwards as `Authorization: Bearer <value>` (the sidecar has no
`COPILOT_PROVIDER_BEARER_TOKEN` concept). Because the token is minted in the same
job as the engine run, it resolves via a plain `$(...)` macro — no cross-job
output plumbing, no `dependsOn`.

The minted token is a short-lived AAD access token (typically valid ~1 hour). The
mint step is emitted as the **last step before** the engine invocation to keep it
fresh; a pool that queues or idles for the full token lifetime between the mint
step and the run could observe an expired token.

#### Credential isolation (api-proxy sidecar)

When a provider credential is configured (`base-url` + `token`/`api-key`), the
compiler automatically enables the AWF **api-proxy sidecar**
(`--enable-api-proxy`) on the agent step and pre-pulls its container image. With
the sidecar active:

- The **real** credential is read by the AWF host process and held inside the
  proxy container; the agent container receives only a placeholder value and a
  proxy URL. The proxy strips the client's auth header and injects the real
  credential on the outbound request, so the secret never reaches the Copilot
  CLI process or the agent sandbox.
- The credential keys are additionally passed as AWF `--exclude-env` flags so the
  raw value is never copied into the agent via `--env-all` (defense-in-depth).

This isolation applies to **both** the Agent stage and the Detection
(threat-analysis) stage: the detection Copilot run inherits the same
`COPILOT_PROVIDER_*` routing and api-proxy credential isolation, so it reaches
the same external provider without exposing the credential (matching gh-aw,
whose detection engine config inherits the main engine's `env`).

#### Network allowlist

When `base-url` is a **literal** URL, the compiler automatically adds its
hostname to the AWF network allowlist. If a literal value cannot be resolved to
a DNS-safe host — for example a scheme-less string like `my-foundry/openai/v1`,
or an IPv6 literal — the compiler emits a non-fatal warning telling you to add
the provider hostname manually via `network.allowed`.

#### Example — Azure Copilot Foundry with a compiler-minted bearer token

```yaml
engine:
  id: copilot
  model: gpt-4o
  provider:
    base-url: https://my-foundry.cognitiveservices.azure.com/openai/v1
    type: azure
    token:
      service-connection: my-arm-connection
      # resource: https://cognitiveservices.azure.com   # optional; this is the default
```

The compiler mints the token in-job (Agent + Detection) and adds
`my-foundry.cognitiveservices.azure.com` to the AWF allowlist automatically. No
`setup:` step, `##vso[task.setvariable]`, or `COPILOT_PROVIDER_*` env keys are
needed.

To use a static key instead, drop `token` and set `api-key` to a secret
variable:

```yaml
engine:
  id: copilot
  provider:
    base-url: https://api.openai.com/v1
    type: openai
    api-key: $(OPENAI_API_KEY)
```

#### Raw `engine.env COPILOT_PROVIDER_*` (legacy, discouraged)

For back-compat you may still set `COPILOT_PROVIDER_*` keys directly in
`engine.env` (they may carry an ADO **macro** `$(...)` — but not `${{ }}`,
`$[...]`, or `##vso[`). `engine.provider` and raw `COPILOT_PROVIDER_*` keys are
mutually exclusive: setting both is a compile error. Note that a **cross-job**
macro such as `$(Setup.FOUNDRY_TOKEN)` (sourcing a value set with `isOutput=true`
in a separate `setup:` job) does **not** resolve inside a step `env:` block and
yields an empty token at runtime — use `provider.token` (or a same-job / pipeline
/ variable-group `$(VAR)` secret) instead.
