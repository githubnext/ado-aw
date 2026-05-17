# CLI Commands

_Part of the [ado-aw documentation](../AGENTS.md)._

## CLI Commands

Global flags (apply to all subcommands): `--verbose, -v` (enable info-level logging), `--debug, -d` (enable debug-level logging, implies verbose), `--log-output-dir <path>` (write ado-aw logs to a specific directory; overrides `ADO_AW_LOG_DIR`)

- `init` - Initialize a repository for AI-first agentic pipeline authoring
  - `--path <path>` - Target directory (defaults to current directory)
  - `--force` - Bypass the GitHub-remote guard (use when running inside a GitHub-hosted repository like `githubnext/ado-aw` itself)
  - Creates `.github/agents/ado-aw.agent.md` — a Copilot dispatcher agent that routes to specialized prompts for creating, updating, and debugging agentic pipelines
  - The agent auto-downloads the ado-aw compiler and handles the full lifecycle (create → compile → check)
- `compile [<path>]` - Compile a markdown file to Azure DevOps pipeline YAML. If no path is given, auto-discovers and recompiles all detected agentic pipelines in the current directory.
  - `--output, -o <path>` - Optional output path for the generated YAML (only valid when a path is provided). If the path is an existing directory, the compiled YAML is written inside that directory using the default filename derived from the markdown source (e.g. `foo.md` → `<dir>/foo.lock.yml`).
  - `--force` - Bypass the GitHub-remote guard (use when running inside a GitHub-hosted repository like `githubnext/ado-aw` itself)
  - `--skip-integrity` - *(debug builds only)* Omit the "Verify pipeline integrity" step from the generated pipeline. Useful during local development when the compiled output won't match a released compiler version. This flag is not available in release builds. OR-ed with `ado-aw-debug.skip-integrity:` in front matter — either is sufficient. See [`docs/ado-aw-debug.md`](ado-aw-debug.md).
  - `--debug-pipeline` - *(debug builds only)* Include MCPG debug diagnostics in the generated pipeline: `DEBUG=*` environment variable for verbose MCPG logging, stderr streaming to log files, and a "Verify MCP backends" step that probes each backend with MCP initialize + tools/list before the agent runs. This flag is not available in release builds.
  - For `target: job` and `target: stage`, the output is an ADO YAML template (not a complete pipeline). Job names are prefixed with the agent name for uniqueness. Triggers configured via `on:` are ignored with a warning.
- `check <pipeline>` - Verify that a compiled pipeline matches its source markdown
  - `<pipeline>` - Path to the pipeline YAML file to verify
  - The source markdown path is auto-detected from the `@ado-aw` header in the pipeline file
  - Useful for CI checks to ensure pipelines are regenerated after source changes
- `mcp <output_directory> <bounding_directory>` - Run SafeOutputs as a stdio MCP server
  - `--enabled-tools <name>` - Restrict available tools to those named (repeatable)
- `mcp-http <output_directory> <bounding_directory>` - Run SafeOutputs as an HTTP MCP server (for MCPG integration)
  - `--port <port>` - Port to listen on (default: 8100)
  - `--api-key <key>` - API key for authentication (auto-generated if not provided)
  - `--enabled-tools <name>` - Restrict available tools to those named (repeatable)
- `execute` - Execute safe outputs from Stage 1 (Stage 3 of pipeline)
  - `--source, -s <path>` - Path to source markdown file
  - `--safe-output-dir <path>` - Directory containing safe output NDJSON (default: current directory)
  - `--output-dir <path>` - Output directory for processed artifacts (e.g., agent memory)
  - `--ado-org-url <url>` - Azure DevOps organization URL override
  - `--ado-project <name>` - Azure DevOps project name override
  - `--dry-run` - Validate inputs but skip ADO API calls (useful for local testing and QA review)

- `configure` *(deprecated; hidden in --help)* - Alias forwarding to `secrets set GITHUB_TOKEN`. Existing scripts keep working but get a stderr warning. The alias will be removed in the next minor release.

- `secrets set <name> [<value>] [PATH]` - Set a pipeline variable (with `isSecret=true`) on every matched ADO definition. Value resolution: positional `<value>` → `--value-stdin` (one line) → interactive tty prompt with echo off.
  - `--allow-override` - Mark the variable as `allowOverride=true` (default: false).
  - `--value-stdin` - Read the value from a single line on stdin.
  - `--dry-run` - Print the planned set without calling the ADO API.
  - `--org / --project / --pat` - ADO context overrides (same semantics as the other lifecycle commands).
  - `--definition-ids <ids>` - Explicit pipeline definition IDs (comma-separated; skips local-fixture auto-detection).

- `secrets list [PATH]` - List variable names and their `isSecret` / `allowOverride` flags on every matched definition. **Never prints values.**
  - `--json` - Emit machine-readable JSON.
  - `--org / --project / --pat / --definition-ids` - As above.

- `secrets delete <name> [PATH]` - Delete the named variable from every matched definition. No-op when the variable is absent.
  - `--dry-run` - Print the planned deletion plan without calling the ADO API.
  - `--org / --project / --pat / --definition-ids` - As above.


- `enable [PATH]` - Register an ADO build definition for each compiled pipeline discovered under `PATH` (or the current directory) and ensure it is `enabled`. For each fixture, matches against the existing ADO definitions by `yamlFilename` first, then by sanitized display name; creates a new definition when neither matches, flips `queueStatus` to `enabled` when an existing definition is `disabled` / `paused`, and skips when it is already `enabled`. Fail-soft per fixture; exits non-zero if any fixture failed.
  - `--org <url>` - Override: Azure DevOps organization (URL or bare org name). Inferred from git remote by default.
  - `--project <name>` - Override: Azure DevOps project name (inferred from git remote by default).
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (Azure CLI fallback if omitted).
  - `--folder <ado-folder>` - ADO folder for newly-created definitions. Defaults to `\` (root). Only applied on create — existing definitions stay where they are.
  - `--default-branch <ref>` - Default branch for newly-created definitions. Defaults to `refs/heads/main`.
  - `--dry-run` - Print the planned actions (and the full POST body for creates) without calling the ADO API.
  - `--also-set-token` - After creating a new definition, set its `GITHUB_TOKEN` variable (as an ADO secret).
  - `--token <value>` - The token value for `--also-set-token`. Falls back to `$GITHUB_TOKEN`, then to an interactive prompt. Requires `--also-set-token`.

  **Source-repo scope (Phase 1):** `enable` requires the local git remote to be an Azure DevOps Git remote (the source repo is what gets registered as the definition's repository). GitHub-hosted source repos are gated on a follow-up.
