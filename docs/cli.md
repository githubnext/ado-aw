# CLI Commands

_Part of the [ado-aw documentation](../AGENTS.md)._

## CLI Commands

Global flags (apply to all subcommands): `--verbose, -v` (enable info-level logging), `--debug, -d` (enable debug-level logging, implies verbose)

- `init` - Initialize a repository for AI-first agentic pipeline authoring
  - `--path <path>` - Target directory (defaults to current directory)
  - `--force` - Overwrite existing agent file
  - Creates `.github/agents/ado-aw.agent.md` — a Copilot dispatcher agent that routes to specialized prompts for creating, updating, and debugging agentic pipelines
  - The agent auto-downloads the ado-aw compiler and handles the full lifecycle (create → compile → check)
- `compile [<path>]` - Compile a markdown file to Azure DevOps pipeline YAML. If no path is given, auto-discovers and recompiles all detected agentic pipelines in the current directory.
  - `--output, -o <path>` - Optional output path for the generated YAML (only valid when a path is provided). If the path is an existing directory, the compiled YAML is written inside that directory using the default filename derived from the markdown source (e.g. `foo.md` → `<dir>/foo.lock.yml`).
  - `--skip-integrity` - *(debug builds only)* Omit the "Verify pipeline integrity" step from the generated pipeline. Useful during local development when the compiled output won't match a released compiler version. This flag is not available in release builds.
  - `--debug-pipeline` - *(debug builds only)* Include MCPG debug diagnostics in the generated pipeline: `DEBUG=*` environment variable for verbose MCPG logging, stderr streaming to log files, and a "Verify MCP backends" step that probes each backend with MCP initialize + tools/list before the agent runs. This flag is not available in release builds.
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

- `configure` - Detect agentic pipelines in a local repository and update the `GITHUB_TOKEN` pipeline variable on their Azure DevOps build definitions
  - `--token <token>` / `GITHUB_TOKEN` env var - The new GITHUB_TOKEN value (prompted if omitted)
  - `--org <url>` - Override: Azure DevOps organization URL (inferred from git remote by default)
  - `--project <name>` - Override: Azure DevOps project name (inferred from git remote by default)
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (prompted if omitted)
  - `--path <path>` - Path to the repository root (defaults to current directory)
  - `--dry-run` - Preview changes without applying them
  - `--definition-ids <ids>` - Explicit pipeline definition IDs to update (comma-separated, skips auto-detection)
