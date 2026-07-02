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
| `model` | string | `claude-opus-4.7` | AI model to use (e.g., `claude-sonnet-4.5`). The compiler passes the value directly to the Copilot CLI `--model` flag — any model identifier the Copilot CLI accepts is valid. |
| `timeout-minutes` | integer | *(none)* | Maximum time in minutes the agent job is allowed to run. Sets `timeoutInMinutes` on the `Agent` job in the generated pipeline. |
| `version` | string | *(none)* | Engine CLI version to install (e.g., `"1.0.64"`, `"latest"`). Overrides the pinned `COPILOT_CLI_VERSION`. Set to `"latest"` to use the newest available version. |
| `agent` | string | *(none)* | Custom agent file identifier (Copilot only). Adds `--agent <name>` to the CLI invocation, selecting a custom agent from `.github/agents/`. |
| `api-target` | string | *(none)* | Custom API endpoint hostname for GHES/GHEC (e.g., `"api.acme.ghe.com"`). Adds `--api-target <hostname>` to the CLI invocation and adds the hostname to the AWF network allowlist. |
| `args` | list | `[]` | Custom CLI arguments appended after compiler-generated args. Subject to shell-safety validation and blocked from overriding compiler-controlled flags (`--prompt`, `--additional-mcp-config`, `--allow-tool`, `--allow-all-tools`, `--allow-all-paths`, `--disable-builtin-mcps`, `--no-ask-user`, `--ask-user`). |
| `env` | map | *(none)* | Engine-specific environment variables merged into the sandbox step's `env:` block. Keys must be valid env var names. Values are literal-only and must not contain ADO expressions (`$(`, `${{`, `$[`) or pipeline command injection (`##vso[`), **except** the Copilot BYOM provider keys (`COPILOT_PROVIDER_BASE_URL`, `COPILOT_PROVIDER_API_KEY`, `COPILOT_PROVIDER_BEARER_TOKEN`, `COPILOT_PROVIDER_WIRE_API`), which may carry an ADO macro (`$(...)`) expression — see [Copilot BYOM / BYOK provider configuration](#copilot-byom--byok-provider-configuration). Compiler-controlled keys (`GITHUB_TOKEN`, `PATH`, `BASH_ENV`, etc.) are blocked. |
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
    app-id: 1234567            # literal App ID (or an ADO variable name)
    private-key: GH_APP_KEY    # ADO *secret* variable name holding the PEM
    owner: octo-org            # installation owner (org or user login)
    repositories: [octo-repo]  # optional; scopes the token to these repos
    # api-url: https://ghe.example.com/api/v3   # optional; GHES base URL
    # skip-token-revocation: false              # optional; revoke by default
```

| Field | Required | Description |
|-------|----------|-------------|
| `app-id` | yes | The GitHub App ID. Accepts a **literal** numeric App ID (e.g. `1234567`, quoted or unquoted) rendered verbatim, **or** the name of an ADO pipeline variable holding it (e.g. `GH_APP_ID`) rendered as a `$(NAME)` macro. The App ID is not secret. |
| `private-key` | yes | Name of the ADO **secret** pipeline variable holding the GitHub App private key (PEM). Rendered as a `$(NAME)` macro; the key material is never inlined. |
| `owner` | yes | GitHub installation owner (organization or user login). |
| `repositories` | no | Repository names (owner-relative) to scope the installation token to. Omit to span every repository the installation grants. |
| `api-url` | no | GitHub API base URL. Defaults to `https://api.github.com` (GHEC). For GitHub Enterprise Server, set the `/api/v3` base URL (e.g. `https://ghe.example.com/api/v3`). Must be an `https://` URL. |
| `skip-token-revocation` | no | When `true`, do not revoke the minted token after the Copilot run. Defaults to `false` (the token is revoked — see below). |

#### Setup

1. Create the GitHub App and install it on the owning org/user, ensuring the
   installation is suitable for Copilot organization-backed
   authentication/billing in your tenant. Copilot capability is granted on the
   **App/org side** — the installation token inherits it automatically; you do
   not (and cannot) configure it here.
2. Store the private key as an ADO **secret** pipeline variable (the App ID is
   not secret — you may inline it or, for parity, store it as a variable too):

   ```bash
   ado-aw secrets set GH_APP_KEY "$(cat app-private-key.pem)"
   # optional, if you prefer a variable over an inline app-id:
   ado-aw secrets set GH_APP_ID "1234567"
   ```
3. Set `engine.github-app-token` (inline `app-id` or reference the variable
   name) and compile.

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

- **GHEC by default; GHES via `api-url`.** The mint/revoke steps target
  `https://api.github.com` unless you set `api-url` to your GitHub Enterprise
  Server `/api/v3` base URL. This is independent of `engine.api-target`, which
  configures the Copilot API host, not the GitHub App API host.
- **`openssl` is not required** — the token is minted with Node's built-in
  crypto. The build agent needs Node (installed automatically) and network
  access to the GitHub API host.
- You may still need to pin `engine.version` until the relevant Copilot CLI auth
  behavior is broadly available.

### Copilot BYOM / BYOK provider configuration

The Copilot engine can route requests to an external LLM provider — for example a
private **Azure Copilot Foundry** instance — instead of GitHub's default routing.
This **Bring Your Own Model / Key (BYOM/BYOK)** mode is activated by setting
`COPILOT_PROVIDER_BASE_URL` in `engine.env`. The Copilot CLI reads the
`COPILOT_PROVIDER_*` environment variables to direct requests to the provider.

This mirrors the model used by
[GitHub Agentic Workflows (gh-aw)](https://github.com/githubnext/gh-aw): only a
fixed allowlist of provider env keys may carry expressions; every other
`engine.env` value remains literal-only.

#### Provider variables

| Variable | Required | Description |
|----------|----------|-------------|
| `COPILOT_PROVIDER_BASE_URL` | ✅ for BYOM | Base URL of the external provider (e.g. `https://RESOURCE.cognitiveservices.azure.com/openai/v1`). Setting this activates BYOM mode. |
| `COPILOT_MODEL` | Often required | Model to use (most providers require it). Set via `engine.model` or this env var. |
| `COPILOT_PROVIDER_API_KEY` | Optional | API key for cloud providers. Not needed for local providers. |
| `COPILOT_PROVIDER_BEARER_TOKEN` | Optional | Bearer token alternative to `COPILOT_PROVIDER_API_KEY`; takes precedence when set. |
| `COPILOT_PROVIDER_TYPE` | Optional | Provider format: `openai` (default), `azure`, or `anthropic`. |
| `COPILOT_PROVIDER_WIRE_API` | Optional | Wire API variant: `completions` (default) or `responses`. |

#### Allowed expressions

The credential/provider keys `COPILOT_PROVIDER_BASE_URL`,
`COPILOT_PROVIDER_API_KEY`, `COPILOT_PROVIDER_BEARER_TOKEN`, and
`COPILOT_PROVIDER_WIRE_API` may carry an ADO **macro** (`$(...)`) expression, so
credentials can be sourced from a Setup-job output or a pipeline variable rather
than hard-coded. Macros are the only expression form ADO evaluates inside a step
`env:` block. These keys may **not** carry ADO **template** expressions (`${{ }}`,
evaluated at compile time) or **runtime** expressions (`$[ ... ]`, which ADO does
not evaluate in step env — the literal string would be passed verbatim), nor
pipeline command injection (`##vso[`). All non-provider `engine.env` values stay
literal-only.

#### Credential isolation (api-proxy sidecar)

When a BYOM credential key (`COPILOT_PROVIDER_BASE_URL`,
`COPILOT_PROVIDER_API_KEY`, or `COPILOT_PROVIDER_BEARER_TOKEN`) is present in
`engine.env`, the compiler automatically enables the AWF **api-proxy sidecar**
(`--enable-api-proxy`) on the agent step and pre-pulls its container image. With
the sidecar active:

- The **real** credential is read by the AWF host process and held inside the
  proxy container; the agent container receives only a placeholder value and a
  proxy URL. The proxy strips the client's auth header and injects the real
  credential on the outbound request, so the secret never reaches the Copilot
  CLI process or the agent sandbox.
- The credential keys are additionally passed as AWF `--exclude-env` flags so the
  raw value is never copied into the agent via `--env-all` (defense-in-depth; AWF
  also overrides them with placeholders).

This isolation applies to **both** the Agent stage and the Detection
(threat-analysis) stage: the detection Copilot run inherits the same
`COPILOT_PROVIDER_*` routing and api-proxy credential isolation, so it reaches
the same external provider without exposing the credential (matching gh-aw,
whose detection engine config inherits the main engine's `env`).

#### Network allowlist

When `COPILOT_PROVIDER_BASE_URL` is a **literal** URL, the compiler automatically
adds its hostname to the AWF network allowlist. When the base URL is supplied via
an expression (so the concrete host is unknown at compile time), add the provider
hostname explicitly to `network.allowed`.

If a **literal** value cannot be resolved to a DNS-safe host — for example a
scheme-less string like `my-foundry/openai/v1`, or an IPv6 literal — the compiler
cannot add it automatically. Rather than silently dropping it (which would fail at
runtime with a firewall block), the compiler emits a non-fatal warning:

```
warning: COPILOT_PROVIDER_BASE_URL: 'my-foundry/openai/v1' is not a parseable
absolute URL (or its host is not DNS-safe); the host was not added to the AWF
allowlist — add the provider hostname manually via network.allowed.
```

Fix it by using a full absolute URL (including `https://`) and/or adding the host
to `network.allowed`.

#### Example — Azure Copilot Foundry with a Setup-acquired bearer token

```yaml
setup:
  - task: AzureCLI@2
    inputs:
      azureSubscription: my-service-connection
      scriptType: bash
      inlineScript: |
        TOKEN=$(az account get-access-token --resource https://cognitiveservices.azure.com/ --query accessToken -o tsv)
        echo "##vso[task.setvariable variable=FOUNDRY_TOKEN;isOutput=true]$TOKEN"
    displayName: Acquire Foundry bearer token

engine:
  id: copilot
  model: gpt-4o
  env:
    COPILOT_PROVIDER_TYPE: azure
    COPILOT_PROVIDER_BASE_URL: https://my-foundry.cognitiveservices.azure.com/openai/v1
    COPILOT_PROVIDER_BEARER_TOKEN: $(Setup.FOUNDRY_TOKEN)
```

Because `COPILOT_PROVIDER_BASE_URL` is a literal URL above,
`my-foundry.cognitiveservices.azure.com` is added to the AWF allowlist
automatically. If you instead source the base URL from an expression
(`COPILOT_PROVIDER_BASE_URL: $(Setup.BASE_URL)`), add the host to
`network.allowed` yourself.
