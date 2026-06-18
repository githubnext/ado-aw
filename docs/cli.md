# CLI Commands

_Part of the [ado-aw documentation](../AGENTS.md)._

## CLI Commands

Global flags (apply to all subcommands): `--verbose, -v` (enable info-level logging), `--debug, -d` (enable debug-level logging, implies verbose), `--log-output-dir <path>` (write ado-aw logs to a specific directory; overrides `ADO_AW_LOG_DIR`)

- `init` - Initialize a repository for AI-first agentic pipeline authoring
  - `--path <path>` - Target directory (defaults to current directory)
  - `--force` - Bypass the GitHub-remote guard (use when running inside a GitHub-hosted repository like `githubnext/ado-aw` itself)
  - `--agency` - **Additive.** Also generate an Agency / [Claude Code plugin](https://code.claude.com/docs/en/plugins-reference) under the Claude Code default `.claude` directory (in addition to the standard Copilot agent file). Emits `.claude/.claude-plugin/plugin.json` (manifest) + `marketplace.json`, an `agents/ado-aw.md` dispatcher subagent, and `commands/{create,update,debug}-ado-agentic-workflow.md` slash commands.
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
- `mcp-author` - Run the author-facing stdio MCP server for IDE/Copilot Chat integrations. See [`mcp-author.md`](mcp-author.md) for the full tool surface and trust model.
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

- `configure` *(deprecated; hidden in --help)* - Alias forwarding to `secrets set GITHUB_TOKEN`. Existing scripts keep working but get a stderr warning.

- `secrets set <name> [<value>] [PATH]` - Set a pipeline variable (with `isSecret=true`) on every matched ADO definition. Value resolution: positional `<value>` → `--value-stdin` (one line) → interactive tty prompt with echo off.
  - `--allow-override` - Force `allowOverride=true` on the set variable. When omitted, `allowOverride` is **preserved** on existing variables (so secret rotation does not silently downgrade an existing `allowOverride=true`) and defaults to `false` for new variables.
  - When ado-aw round-trips a definition through the ADO GET→PUT API, unchanged secret siblings returned by ADO as masked `***` are normalized to `null` before the PUT so their stored values are preserved instead of being overwritten by the literal mask.
  - `--value-stdin` - Read the value from a single line on stdin.
  - `--dry-run` - Print the planned set without calling the ADO API.
  - `--org / --project / --pat` - ADO context overrides (same semantics as the other lifecycle commands).
  - `--definition-ids <ids>` - Explicit pipeline definition IDs (comma-separated; skips local-fixture auto-detection).
  - `--all-repos` - **Project-scope mode.** Activates Preview-driven discovery and operates on every ado-aw pipeline ADO knows about in the project — direct ado-aw definitions *and* consumer pipelines that include ado-aw templates — regardless of which repo their root YAML lives in. Mutually exclusive with `--definition-ids`. Ignores local lock files for matching (uses ADO Pipeline Preview to find marker steps).
  - `--source <path>` - **Filter by template.** Restricts to definitions whose `# ado-aw-metadata` marker references the given source path (e.g. `agents/security-scan.md`). Activates the discovery code path; pairs with `--all-repos` to scope across the whole project. Mutually exclusive with `--definition-ids`.
  - `--include-disabled` - **Include inactive pipelines in project-wide discovery.** By default, definitions whose `queueStatus` is `disabled` or `paused` are skipped *before* the per-definition Preview call (the expensive part), so large projects with many inactive pipelines resolve faster. Pass this flag to also Preview those inactive definitions. Only takes effect on the discovery code path (`--all-repos` or `--source`). Definitions with no reported `queueStatus` are always treated as active and kept.

- `secrets list [PATH]` - List variable names and their `isSecret` / `allowOverride` flags on every matched definition. **Never prints values.**
  - `--json` - Emit machine-readable JSON.
  - `--org / --project / --pat / --definition-ids` - As above.
  - `--all-repos / --source <path> / --include-disabled` - As for `secrets set` (project-scope discovery).

- `secrets delete <name> [PATH]` - Delete the named variable from every matched definition. No-op when the variable is absent.
  - `--dry-run` - Print the planned deletion plan without calling the ADO API.
  - `--org / --project / --pat / --definition-ids` - As above.
  - `--all-repos / --source <path> / --include-disabled` - As for `secrets set` (project-scope discovery).

### Project-scope discovery (`--all-repos` / `--source`)

`secrets set / list / delete` accept opt-in flags that activate **Preview-driven discovery** instead of the default lexical local-fixture matching. They are the surface that solves token management for templates that get included by other pipelines.

- **`--all-repos`** — search every definition in the ADO project. With it, you can `secrets set GITHUB_TOKEN --all-repos` from anywhere; no local checkout of the consumer pipelines is needed.
- **`--source <path>`** — filter results to definitions whose `# ado-aw-metadata` marker references that template. Useful for fan-out: `secrets set GITHUB_TOKEN --source agents/security-scan.md` rotates the token on every consumer pipeline that includes that template.
- **`--include-disabled`** — also Preview `disabled` / `paused` definitions. By default these are pruned before the Preview fan-out: because discovery issues one Pipeline Preview call per in-scope definition, dropping inactive pipelines up-front is where the time is saved on large projects. Pass this flag (e.g. `secrets set GITHUB_TOKEN --all-repos --include-disabled`) when you need to operate on inactive pipelines too. Definitions without a reported `queueStatus` are always treated as active so nothing the API failed to annotate is silently skipped.

Both `--all-repos` and `--source` route through `ado-aw`'s `discover_ado_aw_pipelines` machinery, which calls ADO's Pipeline Preview API per definition and scans the expanded YAML for an `ado-aw-marker` step that every compiled pipeline now carries. `--definition-ids` remains the explicit-ID escape hatch and is mutually exclusive with these flags. `enable`, `disable`, and `remove` are **not** changed — they retain their source-scoped safety semantics.


- `enable [PATH]` - Register an ADO build definition for each compiled pipeline discovered under `PATH` (or the current directory) and ensure it is `enabled`. For each fixture, matches against the existing ADO definitions by `yamlFilename` first, then by sanitized display name; creates a new definition when neither matches, flips `queueStatus` to `enabled` when an existing definition is `disabled` / `paused`, and skips when it is already `enabled`. Fail-soft per fixture; exits non-zero if any fixture failed.
  - `--org <url>` - Override: Azure DevOps organization (URL or bare org name). Inferred from git remote by default.
  - `--project <name>` - Override: Azure DevOps project name (inferred from git remote by default).
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (Azure CLI fallback if omitted).
  - `--folder <ado-folder>` - ADO folder for newly-created definitions. Defaults to `\` (root). Only applied on create — existing definitions stay where they are.
  - `--default-branch <ref>` - Default branch for newly-created definitions. Defaults to `refs/heads/main`.
  - `--dry-run` - Print the planned actions (and the full POST body for creates) without calling the ADO API.
  - `--also-set-token` - After creating a new definition, set its `GITHUB_TOKEN` variable (as an ADO secret).
  - `--token <value>` - The token value for `--also-set-token`. Falls back to `$GITHUB_TOKEN`, then to an interactive prompt. Requires `--also-set-token`.
  - `--service-connection <name-or-guid>` - GitHub service-connection name (e.g. `ado-aw-github`) or GUID. **Required when the source repository is on GitHub.** Rejected (with a clear error) when the source is Azure DevOps Git.
  - `--repository-name <owner/repo>` - Source repository override (GitHub source only). Auto-detected from the git remote when omitted; useful when the local checkout's remote points at a fork rather than the canonical source repo the deployment should reference.

  **Source-repo autodetect:** `enable` parses `git remote get-url origin` and routes to either the TfsGit or GitHub create-definition body shape automatically. ADO Git remotes (`dev.azure.com/...`, legacy `*.visualstudio.com`) emit a `TfsGit` body. `github.com` remotes (HTTPS or SSH) emit a `GitHub` body whose `repository.properties.connectedServiceId` references the project-level GitHub service connection resolved from `--service-connection`. If the local remote can't be parsed at all, pass both `--repository-name owner/repo` and `--service-connection` explicitly. GitHub Enterprise (`github.example.com`) is not supported in v1.

  **GitHub-source pre-requisites:** create a GitHub service connection in the target ADO project (Project settings → Service connections → GitHub) before running `enable --service-connection ...`. ADO has no REST API for creating service connections — this is a one-time portal action. Then either install the Azure Pipelines GitHub App on the source repo or paste a fine-grained PAT.

  **Example (smoke fixtures registered in `msazuresphere/AgentPlayground` from a `githubnext/ado-aw` checkout):**
  ```powershell
  ado-aw enable `
    --org msazuresphere --project AgentPlayground `
    --service-connection ado-aw-github `
    --folder '\smoke' `
    tests/safe-outputs/
  ```
  Re-running is idempotent. `disable`, `remove`, `list`, `status`, and `run` work against the same definitions from the same GitHub checkout — they match by YAML path (provider-agnostic) and require explicit `--org`/`--project` because the git remote can't infer the deployment target for GitHub-source pipelines.

- `disable [PATH]`- Set `queueStatus` to `disabled` (default) or `paused` on every ADO build definition that matches a local fixture under `PATH`. Refuses to touch any ADO definition that is not the target of a local fixture match — that safety property falls naturally out of the same yaml-path + name match used by `configure`. Skips definitions that are already at the requested status; fail-soft per fixture; exits non-zero if any patch failed or if zero local fixtures matched ADO definitions.
  - `--org <url>` - Override: Azure DevOps organization (URL or bare org name). Inferred from git remote by default.
  - `--project <name>` - Override: Azure DevOps project name (inferred from git remote by default).
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (Azure CLI fallback if omitted).
  - `--paused` - Use `queueStatus: paused` instead of `disabled`. Paused definitions still queue scheduled runs but the queue is held; disabled definitions reject all queue requests.
  - `--dry-run` - Print the planned `from → to` transitions without calling the ADO API.

- `remove [PATH]` - **Destructive.** Delete every ADO build definition that matches a local fixture under `PATH`. The same `match_definitions` safety property as `disable` applies: definitions without a local fixture are never in scope. Bulk deletes (`>1` match) require `--yes`; a single match on a tty prompts interactively (`y/N`); non-tty contexts always require `--yes`. Fail-soft per fixture; exits non-zero if any deletion failed or if zero local fixtures matched ADO definitions.
  - `--org <url>` - Override: Azure DevOps organization (URL or bare org name). Inferred from git remote by default.
  - `--project <name>` - Override: Azure DevOps project name (inferred from git remote by default).
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (Azure CLI fallback if omitted).
  - `--yes` - Required for bulk deletes (>1 match) and for any delete in a non-tty context. A single match on a tty otherwise prompts interactively.
  - `--dry-run` - Print the planned deletions without calling the ADO API.

- `list [PATH]` - Render every ADO build definition that matches a local fixture (under `PATH`) along with its `queueStatus`, ADO folder, and latest-run summary. Pass `--all` to also include definitions with no matching local fixture. Output defaults to a human-readable table; `--json` emits a stable JSON array suitable for scripting.
  - `--org <url>` - Override: Azure DevOps organization (URL or bare org name). Inferred from git remote by default.
  - `--project <name>` - Override: Azure DevOps project name (inferred from git remote by default).
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (Azure CLI fallback if omitted).
  - `--all` - Include ADO definitions that do not match any local fixture.
  - `--json` - Emit machine-readable JSON.

- `status [PATH]` - Per-pipeline status: name, id, folder, `queueStatus`, latest-run summary, and a deep link — one block per matched definition. Read-only. `--json` emits the same shape as `list --json` so scripts can use either.
  - `--org <url>` - Override: Azure DevOps organization (URL or bare org name). Inferred from git remote by default.
  - `--project <name>` - Override: Azure DevOps project name (inferred from git remote by default).
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (Azure CLI fallback if omitted).
  - `--json` - Emit machine-readable JSON (same shape as `list --json`).

- `run [PATH]` - Queue an ADO build for every ADO definition that matches a local fixture (under `PATH`). With `--wait`, poll each queued build until completion and exit with an aggregate result — 0 only if every queued build succeeded.
  - `--org <url>` - Override: Azure DevOps organization (URL or bare org name). Inferred from git remote by default.
  - `--project <name>` - Override: Azure DevOps project name (inferred from git remote by default).
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (Azure CLI fallback if omitted).
  - `--branch <ref>` - Source branch to queue. Defaults to the definition's `defaultBranch`.
  - `--parameters <k=v[,k=v...]>` - ADO `templateParameters`. Repeatable and/or comma-separated. All values are strings (ADO coerces as the definition requires). Rejects malformed pairs (missing `=`). **Values must not contain commas** — each raw argument is split on `,` before the `=` split, so `key=https://a,b` is rejected. Use one `--parameters` flag per pair when values contain commas.
  - `--wait` - Poll each queued build to completion before exiting.
  - `--poll-interval <secs>` - Polling period when `--wait` is set (default 10).
  - `--timeout <secs>` - Hard cap on the polling loop when `--wait` is set (default 1800).
  - `--dry-run` - Print the planned `templateParameters` body without calling the ADO API.

- `audit <build-id-or-url>` - Audit a single Azure DevOps build: download the known stage artifacts, run the audit analyzers, and render a structured console report or `AuditData` JSON.
  - `-o, --output <dir>` - Output directory for downloaded artifacts and reports. Defaults to `./logs`; the run is stored under `<dir>/build-<id>/`. Non-CLI entry points (`ado-aw trace` and the mcp-author tools) instead anchor under `${TEMP}/ado-aw/audit` so they do not scatter directories under arbitrary working directories.
  - `--json` - Emit machine-readable JSON (`AuditData`) instead of the console report. Suppresses the trailing `Audit complete` stderr line.
  - `--org <url>` - Override: Azure DevOps organization (used when the input is a bare build ID). Full build URLs provide the host / org directly.
  - `--project <name>` - Override: Azure DevOps project name (used when the input is a bare build ID). Full build URLs provide the project directly.
  - `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` env var - PAT for ADO API authentication (Azure CLI fallback if omitted).
  - `--artifacts <set[,set...]>` - Restrict download + analysis to the named sets: `agent`, `detection`, `safe-outputs` (`safe_outputs` is also accepted). Defaults to all three.
  - `--no-cache` - Ignore `<output>/build-<id>/run-summary.json` and re-process the build.
  - See [`audit.md`](audit.md) for accepted build-reference formats, output layout, cache semantics, and the `AuditData` report shape.

- `trace <build-id-or-url> [--step <id>] [--json]` - Query audit telemetry plus local typed-IR graph correlation to explain failed-job chains and downstream skip classifications. Downloads / caches under `${TEMP}/ado-aw/audit/build-<id>/` (separate from `ado-aw audit`'s `./logs` default so the MCP server and IDE-driven traces do not scatter `./logs/` directories under arbitrary working dirs), and degrades to runtime-only output when the source markdown is not local.
  - `--step <id>` - Focus the report on a named IR step and show the containing job's runtime status plus upstream/downstream job classifications.
  - `--json` - Emit a structured `TraceReport`.
  - `--org <url>`, `--project <name>`, `--pat <pat>` / `AZURE_DEVOPS_EXT_PAT` - Same ADO context/auth passthroughs as `audit`.

- `inspect <source> [--json]` - Build the typed IR for an agent source file and emit a terse summary (jobs, stages, steps, output decls, derived `dependsOn` edges, isOutput-promoted outputs).
  - `<source>` - Path to the agent markdown file.
  - `--json` - Emit the full [`PipelineSummary`](ir.md#public-json-summary-irsummary) as pretty-printed JSON instead of the human view.
  - No YAML is written; this is a read-only query over the same IR the compiler builds.

- `graph dump <source> [--format text|json|dot]` - Print the resolved dependency graph (job edges, stage edges, step locations, outputs needing `isOutput=true`). The graph dump now uses an explicit `dump` subcommand so `graph deps` and `graph outputs` can share the namespace.
  - `<source>` - Path to the agent markdown file.
  - `--format text` - Default. Human-scannable plain text.
  - `--format json` - Emit the [`GraphSummary`](ir.md#public-json-summary-irsummary) JSON.
  - `--format dot` - Emit Graphviz DOT. Pipe to `dot -Tsvg -o pipeline.svg` to visualize.

- `graph deps <source> <step-id> [--direction upstream|downstream] [--json]` - Traverse transitive job and step-output dependencies for one step. If `<step-id>` names a job with no matching step, the command falls back to job-level traversal.

- `graph outputs <source> [--producer <step-id>] [--consumer <step-id>] [--json]` - Print declared step outputs and the steps that read them from `env` or `condition`.

- `whatif <source> --fail <step-id-or-job-id> [--json]` - Statically classify downstream jobs that would be skipped, or would run anyway, if a step or job failed.

- `lint <source> [--json]` - Run structural lint checks over an agent source file without writing YAML.
  - `<source>` - Path to the agent markdown file.
  - `--json` - Emit lint findings as structured JSON instead of the human-readable report.

- `catalog [--kind <safe-outputs|runtimes|tools|engines|models>] [--json]` - List the compiler's in-tree registries for scripting or discovery.
  - `--kind <...>` - Restrict output to one category. When omitted, emits every category.
  - `--json` - Emit the catalog as structured JSON instead of the human-readable report.

### Hidden Build-Time Tools

These commands are not shown in `--help` but are available for contributors working on the ado-aw compiler itself:

- `export-gate-schema` - Export the gate spec JSON Schema used by the `scripts/ado-script` TypeScript workspace for type codegen. Outputs JSON to stdout or to a file.
  - `--output, -o <path>` - Write the schema to a file instead of stdout. Parent directories are created automatically.
  - See [`docs/ado-script.md`](ado-script.md) for how this command fits into the ado-script build workflow (`cargo run -- export-gate-schema --output schema/gate-spec.schema.json`).

## Pipeline IR Reference

The compiler builds typed Azure DevOps pipeline IR and lowers it through one YAML emitter. The canonical Setup → Agent → Detection → SafeOutputs → Teardown shape lives in `agentic_pipeline.rs` (shared by every target); target-specific builders (`standalone_ir.rs`, `onees_ir.rs`, `job_ir.rs`, and `stage_ir.rs`) own only the per-target envelope (pipeline shape, template parameters, 1ES wrapping).

See [`docs/ir.md`](ir.md) for the complete IR reference.
