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
| `model` | string | `claude-opus-4.7` | AI model to use. Options include `claude-sonnet-4.5`, `gpt-5.2-codex`, `gemini-3-pro-preview`, etc. |
| `timeout-minutes` | integer | *(none)* | Maximum time in minutes the agent job is allowed to run. Sets `timeoutInMinutes` on the `Agent` job in the generated pipeline. |
| `version` | string | *(none)* | Engine CLI version to install (e.g., `"0.0.422"`, `"latest"`). Overrides the pinned `COPILOT_CLI_VERSION`. Set to `"latest"` to use the newest available version. |
| `agent` | string | *(none)* | Custom agent file identifier (Copilot only). Adds `--agent <name>` to the CLI invocation, selecting a custom agent from `.github/agents/`. |
| `api-target` | string | *(none)* | Custom API endpoint hostname for GHES/GHEC (e.g., `"api.acme.ghe.com"`). Adds `--api-target <hostname>` to the CLI invocation and adds the hostname to the AWF network allowlist. |
| `args` | list | `[]` | Custom CLI arguments appended after compiler-generated args. Subject to shell-safety validation and blocked from overriding compiler-controlled flags (`--prompt`, `--allow-tool`, `--disable-builtin-mcps`, etc.). |
| `env` | map | *(none)* | Engine-specific environment variables merged into the sandbox step's `env:` block. Keys must be valid env var names; values must not contain ADO expressions (`$(`, `${{`) or pipeline command injection (`##vso[`). Compiler-controlled keys (`GITHUB_TOKEN`, `PATH`, `BASH_ENV`, etc.) are blocked. |
| `command` | string | *(none)* | Custom engine executable path (skips default NuGet installation). The path must be accessible inside the AWF container (e.g., `/tmp/...` or workspace-mounted paths). |


### `timeout-minutes`

The `timeout-minutes` field sets a wall-clock limit (in minutes) for the entire agent job. It maps to the Azure DevOps `timeoutInMinutes` job property on `Agent`. This is useful for:

- **Budget enforcement** — hard-capping the total runtime of an agent to control compute costs.
- **Pipeline hygiene** — preventing agents from occupying a runner indefinitely if they stall or enter long retry loops.
- **SLA compliance** — ensuring scheduled agents complete within a known window.

When omitted, Azure DevOps uses its default job timeout (60 minutes). When set, the compiler emits `timeoutInMinutes: <value>` on the agentic job.
