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
| `env` | map | *(none)* | Engine-specific environment variables merged into the sandbox step's `env:` block. Keys must be valid env var names. Values are literal-only and must not contain ADO expressions (`$(`, `${{`, `$[`) or pipeline command injection (`##vso[`), **except** the Copilot BYOM provider keys (`COPILOT_PROVIDER_BASE_URL`, `COPILOT_PROVIDER_API_KEY`, `COPILOT_PROVIDER_BEARER_TOKEN`, `COPILOT_PROVIDER_WIRE_API`), which may carry ADO macro (`$(...)`) / runtime (`$[...]`) expressions — see [Copilot BYOM / BYOK provider configuration](#copilot-byom--byok-provider-configuration). Compiler-controlled keys (`GITHUB_TOKEN`, `PATH`, `BASH_ENV`, etc.) are blocked. |
| `command` | string | *(none)* | Custom engine executable path (skips the default engine binary installation — NuGet for `target: 1es`, GitHub Releases for all other targets). The path must be accessible inside the AWF container (e.g., `/tmp/...` or workspace-mounted paths). |


### `timeout-minutes`

The `timeout-minutes` field sets a wall-clock limit (in minutes) for the entire agent job. It maps to the Azure DevOps `timeoutInMinutes` job property on `Agent`. This is useful for:

- **Budget enforcement** — hard-capping the total runtime of an agent to control compute costs.
- **Pipeline hygiene** — preventing agents from occupying a runner indefinitely if they stall or enter long retry loops.
- **SLA compliance** — ensuring scheduled agents complete within a known window.

When omitted, Azure DevOps uses its default job timeout (60 minutes). When set, the compiler emits `timeoutInMinutes: <value>` on the agentic job.

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
`COPILOT_PROVIDER_WIRE_API` may carry ADO **macro** (`$(...)`) or **runtime**
(`$[...]`) expressions, so credentials can be sourced from a Setup-job output or
a pipeline variable rather than hard-coded. They may **not** carry ADO
**template** expressions (`${{ }}`, evaluated at compile time) or pipeline
command injection (`##vso[`). All non-provider `engine.env` values stay
literal-only.

#### Network allowlist

When `COPILOT_PROVIDER_BASE_URL` is a **literal** URL, the compiler automatically
adds its hostname to the AWF network allowlist. When the base URL is supplied via
an expression (so the concrete host is unknown at compile time), add the provider
hostname explicitly to `network.allowed`.

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
