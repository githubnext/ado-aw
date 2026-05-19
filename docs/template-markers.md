# Template Markers

_Part of the [ado-aw documentation](../AGENTS.md)._

## Output Format (Azure DevOps YAML)

The compiler transforms the input into valid Azure DevOps pipeline YAML based on the target platform:

- **Standalone**: Uses `src/data/base.yml`
- **1ES**: Uses `src/data/1es-base.yml`
- **Job template**: Uses `src/data/job-base.yml`
- **Stage template**: Uses `src/data/stage-base.yml`

Explicit markings are embedded in these templates that the compiler is allowed to replace e.g. `{{ engine_run }}` denotes the full engine invocation command. The compiler should not replace sections denoted by `${{ some content }}`. What follows is a mapping of markings to responsibilities (primarily for the standalone template).

## {{ parameters }}

Should be replaced with the top-level `parameters:` block generated from the `parameters` front matter field. If no parameters are defined (and no auto-injected parameters apply), this marker is replaced with an empty string.

When `tools.cache-memory` is configured, the compiler auto-injects a `clearMemory` boolean parameter (default: `false`) unless one is already user-defined.

Example output:
```yaml
parameters:
- name: clearMemory
  displayName: Clear agent memory
  type: boolean
  default: false
- name: verbose
  displayName: Verbose output
  type: boolean
  default: false
```

## {{ repositories }}
For each additional repository specified in the front matter append:

```yaml
- repository: reponame
  type: git
  name: reponame
  ref: refs/heads/main
```

## {{ schedule }}

This marker should be replaced with a cron-style schedule block generated from the fuzzy schedule syntax. The compiler parses the human-friendly schedule expression and generates a deterministic cron expression based on the agent name hash.

By default, when no branches are explicitly configured, the schedule defaults to `main` branch only. When the object form is used with a `branches` list, a `branches.include` block is generated with the specified branches.

```yaml
# Default (string form) — defaults to main branch
schedules:
  - cron: "43 14 * * *"    # Generated from "daily around 14:00"
    displayName: "Scheduled run"
    branches:
      include:
        - main
    always: true

# With custom branches (object form)
schedules:
  - cron: "43 14 * * *"
    displayName: "Scheduled run"
    branches:
      include:
        - main
        - release/*
    always: true
```

Examples of fuzzy schedule → cron conversion:
- `daily` → scattered across 24 hours (e.g., `"43 5 * * *"`)
- `daily around 14:00` → within 13:00-15:00 (e.g., `"13 14 * * *"`)
- `hourly` → every hour at scattered minute (e.g., `"43 * * * *"`)
- `weekly on monday` → Monday at scattered time (e.g., `"43 5 * * 1"`)
- `every 2h` → every 2 hours at scattered minute (e.g., `"53 */2 * * *"`)
- `bi-weekly` → every 14 days (e.g., `"43 5 */14 * *"`)

## {{ checkout_self }}

Should be replaced with the `checkout: self` step. This generates a simple checkout of the triggering branch.

All checkout steps across all jobs (Agent, Detection, SafeOutputs, Setup, Teardown) use this marker.

## {{ checkout_repositories }}
Should be replaced with checkout steps for additional repositories the agent will work with. The behavior depends on the `repos:` front-matter field (each entry's `checkout:` flag, which defaults to `true`):

- **If `repos:` is omitted or all entries have `checkout: false`**: No additional repositories are checked out. Only `self` is checked out (from the template).
- **If `repos:` has entries with `checkout: true`**: Those repository aliases are checked out in addition to `self`.

This distinction allows resources (like templates) to be available as pipeline resources without being checked out into the workspace for the agent to analyze.

```yaml
- checkout: reponame
```

## {{ agent_name }}

Should be replaced with the human-readable name from the front matter
(e.g., `Daily Code Review`). The value is substituted **as-is**, with
no quoting or escaping — front-matter `name` values are free-form and
have not been validated against YAML scalar rules.

> ⚠️ This marker is only safe inside a position that is **not parsed as
> YAML** (currently only `src/data/threat-analysis.md`, which is a
> markdown body). YAML positions inside the generated pipelines use
> [`{{ pipeline_agent_name }}`](#-pipeline_agent_name-) (top-level `name:` line)
> or [`{{ agent_display_name }}`](#-agent_display_name-)
> (`displayName:` positions). Both emit a fully-quoted-and-escaped
> double-quoted YAML scalar, so colons, embedded `"`, and other
> plain-scalar-unsafe characters in the agent name cannot break parsing.

## {{ agent_display_name }}

Should be replaced with the front-matter agent name, emitted as a
**YAML double-quoted scalar** with proper escaping for `\`, `"`,
`\n`, `\r`, `\t`, and other ASCII control characters. Used for
`displayName:` positions inside the generated YAML where the templates
previously hand-wrapped `{{ agent_name }}` in double quotes (which
silently corrupted any agent name containing an embedded `"`).

For an agent named `My "special": agent`, this expands to:

```yaml
  displayName: "My \"special\": agent"
```

Used in `src/data/1es-base.yml` (1ES stage display name) and
`src/data/stage-base.yml` (stage-target stage display name). The marker
deliberately does **not** include the `-$(BuildID)` suffix that
[`{{ pipeline_agent_name }}`](#-pipeline_agent_name-) carries — stage labels are
static and don't need per-run uniqueness.

## {{ pipeline_agent_name }}

Should be replaced with a sanitized front-matter agent name plus the
`-$(BuildID)` suffix, emitted as a **YAML double-quoted scalar**. Used
only for the top-level pipeline `name:` line, which in Azure DevOps is
the build-number format string. The marker strips build-number-invalid
characters (`"`, `/`, `:`, `<`, `>`, `\`, `|`, `?`, `@`, `*`), trims
trailing `.` from the name fragment, and enforces the 255-character
build-number limit when combined with the `-$(BuildID)` suffix. The
suffix is the
[varying token ADO requires](https://learn.microsoft.com/azure/devops/pipelines/process/run-number)
to give each run a unique display name in the runs view; without it,
every run shows the same name.

For an agent named `Daily safe-output smoke: noop`, this expands to:

```yaml
name: "Daily safe-output smoke noop-$(BuildID)"
```

`$(BuildID)` is an ADO macro and is expanded at queue time after YAML
parsing; `$` has no special meaning inside a YAML double-quoted scalar
so the macro passes through untouched.

Used in `src/data/base.yml` and `src/data/1es-base.yml` only. The
job- and stage-level templates don't emit a top-level pipeline name.

## {{ engine_install_steps }}

Should be replaced with engine-specific pipeline steps to install the engine binary. Generated by `Engine::install_steps()`. The install strategy is **target-aware**:

**For `target: 1es`** — authenticates with the Azure Artifacts NuGet feed for the user's ADO organization and installs the package:
- Optional bash step to resolve the ADO org at runtime (emitted only when the org cannot be inferred at compile time from the git remote): extracts the organization name from `$(System.CollectionUri)` and stores it in the `AW_ADO_ORG` pipeline variable.
- `NuGetAuthenticate@1` task
- `NuGetCommand@2` task to install `Microsoft.Copilot.CLI.linux-x64` from `pkgs.dev.azure.com/{org}/_packaging/Guardian1ESPTUpstreamOrgFeed`, where `{org}` is the ADO organization inferred at compile time (e.g. `contoso`) or the runtime variable `$(AW_ADO_ORG)` when compile-time inference is unavailable. Uses `engine.version` if set, otherwise `COPILOT_CLI_VERSION` constant; omits `-Version` flag when `"latest"`.
- Bash step to copy binary to `/tmp/awf-tools/copilot`
- Bash step to verify installation

**For all other targets (standalone, job, stage)** — downloads from GitHub Releases with SHA256 checksum verification:
- Bash step that: resolves `SHA256SUMS.txt` and the tarball from the GitHub Releases URL for the configured version, verifies the SHA256 checksum, extracts the binary, copies it to `/tmp/awf-tools/copilot`
- Bash step to verify installation

Both paths stage the binary at `/tmp/awf-tools/copilot`.

Returns empty when `engine.command` is set (user provides own binary).

## {{ engine_run }}

Should be replaced with the full AWF `--` command string for the Agent job. Generated by `Engine::invocation()`. For Copilot, this produces:
```
<command_path> --prompt "$(cat /tmp/awf-tools/agent-prompt.md)" --additional-mcp-config @/tmp/awf-tools/mcp-config.json <engine args> <user args>
```

The binary path defaults to `/tmp/awf-tools/copilot` but can be overridden via `engine.command`. The engine controls how the prompt is delivered (`--prompt "$(cat ...)"`), and how MCP config is referenced (`--additional-mcp-config @...`).

Engine args include:
- `--model <model>` - AI model from `engine` front matter field (default: claude-opus-4.7)
- `--agent <name>` - Custom agent file from `engine.agent` (selects from `.github/agents/`)
- `--api-target <hostname>` - Custom API endpoint from `engine.api-target` (GHES/GHEC)
- `--no-ask-user` - Prevents interactive prompts
- `--disable-builtin-mcps` - Disables all built-in Copilot CLI MCPs (single flag, no argument)
- `--allow-all-tools` - When bash is omitted (default) or has a wildcard (`":*"` or `"*"`), allows all tools instead of individual `--allow-tool` flags
- `--allow-tool <tool>` - When bash is NOT wildcard, explicitly allows configured tools (github, safeoutputs, write, and shell commands from the `bash:` field plus any runtime-required commands)
- `--allow-all-paths` - When `edit` tool is enabled (default), allows the agent to write to any file path
- Custom args from `engine.args` — appended after compiler-generated args (subject to shell-safety validation and blocked flag checks)

MCP servers are handled entirely by the MCP Gateway (MCPG) and are not passed as copilot CLI params.

## {{ engine_run_detection }}

Same as `{{ engine_run }}` but for the Detection (threat analysis) job. Uses a different prompt path (`/tmp/awf-tools/threat-analysis-prompt.md`) and no MCP config.

## {{ engine_env }}

Generates engine-specific environment variable entries for the AWF sandbox step via `Engine::env()`. For the Copilot engine, this produces:

- `GITHUB_TOKEN: $(GITHUB_TOKEN)` — GitHub authentication
- `GITHUB_READ_ONLY: 1` — Restricts GitHub API to read-only access
- `COPILOT_OTEL_ENABLED`, `COPILOT_OTEL_EXPORTER_TYPE`, `COPILOT_OTEL_FILE_EXPORTER_PATH` — OpenTelemetry file-based tracing for agent statistics
- Custom env vars from `engine.env` — merged after compiler-controlled vars (YAML-quoted, validated for safety)

ADO access tokens (`AZURE_DEVOPS_EXT_PAT`, `SYSTEM_ACCESSTOKEN`) are not part of this marker — they are injected separately by `{{ acquire_ado_token }}` and extension pipeline variable mappings when `permissions.read` is configured.

## {{ engine_log_dir }}

Should be replaced with the engine's log directory path, generated by `Engine::log_dir()`. For Copilot: `$HOME/.copilot/logs`. Used by log collection steps to copy engine logs to pipeline artifacts.

> **Note:** `$HOME` is used instead of `~` because tilde does not expand inside double-quoted strings in bash. Using `~` would cause the directory check (`[ -d "~/.copilot/logs" ]`) to always fail, silently preventing log collection.

## {{ pool }}

Used by all templates under a `pool:` block and expands to:
- non-1ES targets: one line (`vmImage: <image>` or `name: <pool>`)
- 1ES target: two lines (`name: <pool>` and `os: <linux|windows>`)

Defaults:
- non-1ES: `vmImage: ubuntu-22.04`
- 1ES: `name: AZS-1ES-L-MMS-ubuntu-22.04` + `os: linux`

## {{ setup_job }}

Generates a separate setup job YAML if `setup` contains steps. The job:
- Runs before `Agent`
- Uses the same pool as the main agentic task
- Includes a checkout of self
- Display name: `Setup`

If `setup` is empty, this is replaced with an empty string.

## {{ teardown_job }}

Generates a separate teardown job YAML if `teardown` contains steps. The job:
- Runs after `SafeOutputs` (depends on it)
- Uses the same pool as the main agentic task
- Includes a checkout of self
- Display name: `Teardown`

If `teardown` is empty, this is replaced with an empty string.

## {{ prepare_steps }}

Generates inline steps that run inside the `Agent` job, **before** the agent runs. These steps can generate context files, fetch secrets, or prepare the workspace for the agent.

Steps are inserted after the agent prompt is prepared but before AWF network isolation starts.

If `steps` is empty, this is replaced with an empty string.

## {{ finalize_steps }}

Generates inline steps that run inside the `Agent` job, **after** the agent completes. These steps can validate outputs, process workspace artifacts, or perform cleanup.

Steps are inserted after the AWF-isolated agent completes but before logs are collected.

If `post-steps` is empty, this is replaced with an empty string.

## {{ agentic_depends_on }}

Generates job dependency and condition configuration for the `Agent` job. This marker is populated whenever any of the following are true:

- A **setup job** is configured (`setup:` steps are present)
- **PR runtime filters** are configured (`on.pr.filters`)
- **Pipeline runtime filters** are configured (`on.pipeline.filters`)

When a setup job or gate step is needed, this emits `dependsOn: Setup`. When PR or pipeline filter conditions are also present, it additionally emits a `condition:` expression that gates the Agent job on the gate evaluator's output from the Setup job (e.g. `dependencies.Setup.outputs['prGate.SHOULD_RUN'] == 'true'`).

If none of these are configured, this is replaced with an empty string.

## {{ job_timeout }}

Generates a `timeoutInMinutes: <value>` job property for `Agent` when `engine.timeout-minutes` is configured. This sets the Azure DevOps job-level timeout for the agentic task.

If `timeout-minutes` is not configured, this is replaced with an empty string.

## {{ working_directory }}

Should be replaced with the appropriate working directory based on the effective workspace setting.

**Workspace Resolution Logic:**
1. If `workspace` is explicitly set in front matter, that value is used (after validation)
2. If `workspace` is not set and `repos:` has entries with `checkout: true` (the default), defaults to `repo`
3. If `workspace` is not set and only `self` is checked out, defaults to `root`

**Warning:** If `workspace: repo` (or `self`) is explicitly set but no additional repositories are configured with `checkout: true` in `repos:`, a warning is emitted because when only `self` is checked out, `$(Build.SourcesDirectory)` already contains the repository content directly.

**Accepted values:**
- `root` → `$(Build.SourcesDirectory)` — the checkout root directory
- `repo` (or `self`) → `$(Build.SourcesDirectory)/$(Build.Repository.Name)` — the trigger repository's subfolder
- `<alias>` → `$(Build.SourcesDirectory)/<alias>` — a specific checked-out repository's subfolder. The alias must be the alias of a `repos:` entry with `checkout: true` (the default). This form is only valid when at least one additional repository is checked out; otherwise compilation fails.

**Example — pointing the agent's workspace at a checked-out repository:**
```yaml
repos:
  - name: msazuresphere/exp23-a7-nw
    alias: exp23-a7-nw
workspace: exp23-a7-nw    # Resolves to $(Build.SourcesDirectory)/exp23-a7-nw
```

This is used for the `workingDirectory` property of the copilot task.

## {{ source_path }}

Should be replaced with the path to the agent markdown source file for Stage 3 execution. The path is anchored at the **trigger ("self") repository** via `{{ trigger_repo_directory }}` (see below), independent of the user's `workspace:` setting, and mirrors the relative path used at compile time:
- No additional checkouts: `$(Build.SourcesDirectory)/<relative-path>.md`
- Additional checkouts present: `$(Build.SourcesDirectory)/$(Build.Repository.Name)/<relative-path>.md`

For example, compiling `agents/my-agent.md` produces a runtime path of `$(Build.SourcesDirectory)/agents/my-agent.md` (or the equivalent under `$(Build.Repository.Name)` when additional repositories are checked out).

Used by the execute command's --source parameter. The agent markdown only ever lives in the trigger repo, so this is intentionally not affected by `workspace:` pointing at a non-self alias.

## {{ pipeline_path }}

Should be replaced with the path to the compiled pipeline YAML file for runtime integrity checking. The path is **relative** to the trigger repository root (e.g. `agents/ctf.yml`, `pipelines/production/review.lock.yml`). The integrity check step itself sets `workingDirectory: {{ trigger_repo_directory }}` so the relative path resolves correctly regardless of whether additional repositories are checked out, and so that `ado-aw check`'s recompile step has access to the trigger repo's `.git` directory (required to infer the ADO org for `tools.azure-devops`).

Used by the pipeline's integrity check step to verify the pipeline hasn't been modified outside the compilation process.

## {{ trigger_repo_directory }}

Should be replaced with the directory where the trigger ("self") repository is checked out. This is independent of the `workspace:` setting and depends only on whether any additional repositories are configured with `checkout: true` (the default) in `repos:`:
- No additional checkouts → `$(Build.SourcesDirectory)` (ADO checks `self` into the root)
- One or more additional checkouts → `$(Build.SourcesDirectory)/$(Build.Repository.Name)` (ADO puts each checked-out repo, including `self`, into a subfolder named after the repository)

Use this marker (rather than `{{ working_directory }}` / `{{ workspace }}`) for any path that refers to a file shipped in the trigger repo (e.g. the agent markdown source) or as a `workingDirectory:` for steps that need access to the trigger repo's `.git` (e.g. the integrity check step).

## {{ integrity_check }}

Generates the "Verify pipeline integrity" pipeline step that downloads the released ado-aw compiler and runs `ado-aw check` against the compiled pipeline YAML. This step ensures the pipeline file hasn't been modified outside the compilation process.

The step sets `workingDirectory: {{ trigger_repo_directory }}` so that the relative `{{ pipeline_path }}` argument resolves correctly when `repos:` produces a multi-repo `$(Build.SourcesDirectory)` layout, and so `ado-aw check`'s internal recompile can infer the ADO org from the trigger repo's git remote.

When the compiler is built with `--skip-integrity` (debug builds only) **OR** when the agent's front matter sets `ado-aw-debug.skip-integrity: true`, this placeholder is replaced with an empty string and the integrity step is omitted from the generated pipeline. The two flags are OR-ed — either is sufficient. See [`docs/ado-aw-debug.md`](ado-aw-debug.md).

## {{ mcpg_debug_flags }}

Generates MCPG debug environment flags for the Docker run command. When `--debug-pipeline` is passed (debug builds only), this inserts `-e DEBUG="*"` to enable verbose MCPG logging.

When `--debug-pipeline` is not passed, this placeholder is replaced with a bare `\` to maintain bash line continuation.

## {{ verify_mcp_backends }}

Generates a pipeline step that probes each configured MCPG backend with an MCP initialize + tools/list handshake. This forces MCPG's lazy initialization and catches failures (e.g., container timeout, network blocked) before the agent runs, surfacing them as ADO pipeline warnings.

When `--debug-pipeline` is not passed (the default), this placeholder is replaced with an empty string.

## {{ pr_trigger }}

Generates PR trigger configuration. When a schedule or pipeline trigger is configured, this generates `pr: none` to disable PR triggers. Otherwise, it generates an empty string, allowing the default PR trigger behavior.

## {{ ci_trigger }}

Generates CI trigger configuration. When a schedule or pipeline trigger is configured, this generates `trigger: none` to disable CI triggers. Otherwise, it generates an empty string, allowing the default CI trigger behavior.

## {{ pipeline_resources }}

Generates pipeline resource YAML when `on.pipeline` is configured in the front matter. Creates a pipeline resource with appropriate trigger configuration based on the specified branches. If no branches are specified, the pipeline triggers on any branch.

Example output when `on.pipeline` is configured:
```yaml
resources:
  pipelines:
    - pipeline: source_pipeline
      source: Build Pipeline
      project: OtherProject
      trigger:
        branches:
          include:
            - main
            - release/*
```

## {{ agent_content }}

Should be replaced with the markdown body (agent instructions) extracted from the source markdown file, excluding the YAML front matter. This content provides the agent with its task description and guidelines.

When `inlined-imports: false` (the default), the compiler emits a top-level `{{#runtime-import ...}}` marker here so the prompt body is reloaded from the source markdown at pipeline runtime. When `inlined-imports: true`, any `{{#runtime-import ...}}` markers in the markdown body are resolved at compile time and the emitted YAML contains the expanded content directly.

## {{ mcpg_config }}

Should be replaced with the MCP Gateway (MCPG) configuration JSON generated from the `mcp-servers:` front matter. This configuration defines the MCPG server entries and gateway settings.

The generated JSON has two top-level sections:
- `mcpServers`: Maps server names to their configuration (type, container/url, tools, etc.)
- `gateway`: Gateway settings (port, domain, apiKey, payloadDir)

SafeOutputs is always included as an HTTP backend (`type: "http"`) pointing to `localhost` (MCPG runs with `--network host`, so `localhost` is the host loopback). Containerized MCPs with `container:` are included as stdio servers (`type: "stdio"` with `container`, `entrypoint`, `entrypointArgs`). HTTP MCPs with `url:` are included as HTTP servers. MCPs without a container or url are skipped.

Runtime placeholders (`${SAFE_OUTPUTS_PORT}`, `${SAFE_OUTPUTS_API_KEY}`, `${MCP_GATEWAY_API_KEY}`) are substituted by the pipeline at runtime before passing the config to MCPG.

## {{ mcpg_docker_env }}

Should be replaced with additional `-e` flags for the MCPG Docker run command, enabling environment variable passthrough from the pipeline to MCP containers.

When `permissions.read` is configured, the compiler automatically adds `-e AZURE_DEVOPS_EXT_PAT="$(SC_READ_TOKEN)"` to forward the ADO access token to MCP containers that need it (e.g., Azure DevOps MCP).

Additionally, any env vars in MCP configs with empty string values (`""`) are collected and forwarded as `-e VAR_NAME` flags, enabling passthrough from the pipeline environment through MCPG to MCP child containers.

Environment variable names are validated against `[A-Za-z_][A-Za-z0-9_]*` to prevent Docker flag injection.

If no passthrough env vars are needed, this marker is replaced with an empty string.

## {{ mcpg_step_env }}

Generates an `env:` block for the "Start MCP Gateway (MCPG)" pipeline step, forwarding pipeline variables required by enabled extensions (e.g., `AZURE_DEVOPS_EXT_PAT` when the Azure DevOps MCP tool is configured). The compiler iterates through all active `CompilerExtension` instances, collects their `required_pipeline_vars()` mappings, de-duplicates by variable name, and emits each as `VAR_NAME: $(VAR_NAME)` in ADO variable-reference syntax.

When no extensions require pipeline variables, this marker is replaced with an empty string and the MCPG step has no `env:` block.

## {{ mcp_client_config }}

**Removed.** The Copilot CLI `mcp-config.json` is no longer generated at compile time. Instead, it is derived at **pipeline runtime** from MCPG's actual gateway output, matching gh-aw's `convert_gateway_config_copilot.cjs` pattern.

The "Start MCP Gateway (MCPG)" pipeline step:
1. Redirects MCPG's stdout to `gateway-output.json`
2. Waits for the health check and for valid JSON output
3. Transforms the output with a Python script that:
   - Rewrites URLs from `127.0.0.1` → `host.docker.internal` (AWF container loopback vs host)
   - Ensures `tools: ["*"]` on each server entry (Copilot CLI requirement)
   - Preserves all other fields (headers, type, etc.)
4. Writes the result to `/tmp/awf-tools/mcp-config.json` and `$HOME/.copilot/mcp-config.json`

This ensures the Copilot CLI config reflects MCPG's actual runtime state rather than a compile-time prediction.

## {{ allowed_domains }}

Should be replaced with the comma-separated domain list for AWF's `--allow-domains` flag. The list includes:
1. Core Azure DevOps/GitHub endpoints (from `allowed_hosts.rs`)
2. MCP-specific endpoints for each enabled MCP
3. Engine-required hosts (e.g., `engine.api-target` hostname for GHES/GHEC)
4. Ecosystem identifier expansions from `network.allowed:` (e.g., `python` → PyPI/pip domains)
5. User-specified additional hosts from `network.allowed:` front matter

The output is formatted as a comma-separated string (e.g., `github.com,*.dev.azure.com,api.github.com`).

## {{ awf_mounts }}

Replaced with `--mount` flags for the **agent job** AWF invocation only (not the detection job), collected from `CompilerExtension::required_awf_mounts()`. Each extension can declare volume mounts needed inside the AWF chroot as [`AwfMount`][AwfMount] values (e.g., the Lean runtime mounts `$HOME/.elan` so the elan toolchain is accessible).

When no extensions declare mounts, this is replaced with `\` (a bare bash continuation marker) so the surrounding `\`-continuation chain is preserved. When mounts are present, each is formatted as `--mount "spec" \` on its own line; indentation is handled by `replace_with_indent` at the call site.

AWF replaces `$HOME` with an empty directory overlay for security; only explicitly mounted subdirectories are accessible inside the chroot. Shell variables like `$HOME` are expanded at runtime by bash.

## {{ awf_path_step }}

Replaced with a dedicated pipeline step that generates a `GITHUB_PATH` file for AWF chroot PATH discovery. The step is collected from `CompilerExtension::awf_path_prepends()` — each extension can declare directories that should be on PATH inside the AWF chroot (e.g., the Lean runtime declares `$HOME/.elan/bin`).

AWF reads the `$GITHUB_PATH` environment variable (a path to a file) at startup, reads path entries from it (one per line), and merges them into `AWF_HOST_PATH` which becomes the chroot PATH. This bypasses the `sudo` `secure_path` reset that strips custom PATH entries.

When no extensions declare path prepends, this is replaced with an empty string and the step is omitted.

Example generated step (with Lean enabled):

```yaml
- bash: |
    AWF_PATH_FILE="/tmp/awf-tools/ado-path-entries"
    cat > "$AWF_PATH_FILE" << AWF_PATH_EOF
    $HOME/.elan/bin
    AWF_PATH_EOF
    echo "##vso[task.setvariable variable=GITHUB_PATH]$AWF_PATH_FILE"
  displayName: "Generate GITHUB_PATH file"
```

The heredoc uses an unquoted delimiter so shell variables like `$HOME` are expanded by bash at write time — AWF reads the file as literal resolved paths and does not perform shell expansion itself.

The `GITHUB_PATH` pipeline variable is also explicitly passed through the AWF step's `env:` block (appended to `{{ engine_env }}`) as `GITHUB_PATH: $(GITHUB_PATH)` for robust environment passthrough.

## {{ enabled_tools_args }}

Should be replaced with `--enabled-tools <name>` CLI arguments for the SafeOutputs MCP HTTP server. The tool list is derived from `safe-outputs:` front matter keys plus always-on diagnostic tools (`noop`, `missing-data`, `missing-tool`, `report-incomplete`).

When `safe-outputs:` is empty (or omitted), this is replaced with an empty string and all tools remain available (backward compatibility). When non-empty, the replacement includes a trailing space to prevent concatenation with the next positional argument in the shell command.

Tool names are validated at compile time:
- Names must contain only ASCII alphanumerics and hyphens (shell injection prevention)
- Unrecognized names (not in `ALL_KNOWN_SAFE_OUTPUTS`) emit a warning to catch typos

## {{ threat_analysis_prompt }}

Should be replaced with the embedded threat detection analysis prompt from `src/data/threat-analysis.md`. This prompt template includes markers for `{{ source_path }}`, `{{ agent_name }}`, `{{ agent_description }}`, and `{{ working_directory }}` which are replaced during compilation.

When `inlined-imports: false`, the compiler emits a top-level `{{#runtime-import ...}}` marker pointing at the agent's source `.md` file so the agent body is reloaded from the trigger-repo checkout at pipeline runtime. The threat-analysis prompt itself is **always** inlined at compile time via `include_str!` regardless of `inlined-imports`, because it is tooling-shipped (compiled into the `ado-aw` binary) rather than authored alongside agents. See the comment block at step 11 of `compile_shared` in `src/compile/common.rs` for the rationale; this mirrors gh-aw's model.

The threat analysis prompt instructs the security analysis agent to check for:
- Prompt injection attempts
- Secret leaks
- Malicious patches (suspicious web calls, backdoors, encoded strings, suspicious dependencies)

## {{ agent_description }}

Should be replaced with the description field from the front matter. This is used in display contexts and the threat analysis prompt template.

## {{ acquire_ado_token }}

Generates an `AzureCLI@2` step that acquires a read-only ADO-scoped access token from the ARM service connection specified in `permissions.read`. This token is used by the agent in Stage 1 (inside the AWF sandbox).

The step:
- Uses the ARM service connection from `permissions.read`
- Calls `az account get-access-token` with the ADO resource ID
- Stores the token in a secret pipeline variable `SC_READ_TOKEN`

If `permissions.read` is not configured, this marker is replaced with an empty string.

## {{ acquire_write_token }}

Generates an `AzureCLI@2` step that acquires a write-capable ADO-scoped access token from the ARM service connection specified in `permissions.write`. This token is used only by the executor in Stage 3 (`SafeOutputs` job) and is never exposed to the agent.

The step:
- Uses the ARM service connection from `permissions.write`
- Calls `az account get-access-token` with the ADO resource ID
- Stores the token in a secret pipeline variable `SC_WRITE_TOKEN`

If `permissions.write` is not configured, this marker is replaced with an empty string.

## {{ executor_ado_env }}

Generates the complete `env:` block (including the `env:` key) for the Stage 3 executor step. The block contains zero, one, or two lines depending on which features are configured:

* `SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)` — emitted when `permissions.write` is configured. Provides the write-capable ADO token to the executor.
* `ADO_AW_DEBUG_GITHUB_TOKEN: $(ADO_AW_DEBUG_GITHUB_TOKEN)` — emitted when `ado-aw-debug.create-issue` is configured. Provides the GitHub PAT used by the debug-only `create-issue` safe output. See [`docs/ado-aw-debug.md`](ado-aw-debug.md).

If neither feature is configured, this marker is replaced with an empty string so that no `env:` block is emitted at all. Note: `System.AccessToken` is never used directly — all ADO tokens come from explicitly configured service connections, and the GitHub PAT is sourced from a dedicated pipeline variable separate from the read-only `GITHUB_TOKEN` the agent sees in Stage 1.

## {{ compiler_version }}

Should be replaced with the version of the `ado-aw` compiler that generated the pipeline (derived from `CARGO_PKG_VERSION` at compile time). This version is used to construct the GitHub Releases download URL for the `ado-aw` binary.

The generated pipelines download the compiler binary from:
```
https://github.com/githubnext/ado-aw/releases/download/v{VERSION}/ado-aw-linux-x64
```

A `checksums.txt` file is also downloaded and verified via `sha256sum -c checksums.txt --ignore-missing` to ensure binary integrity.

## {{ firewall_version }}

Should be replaced with the pinned version of the AWF (Agentic Workflow Firewall) binary (defined as `AWF_VERSION` constant in `src/compile/common.rs`). This version is used to construct the GitHub Releases download URL for the AWF binary.

The generated pipelines download the AWF binary from:
```
https://github.com/github/gh-aw-firewall/releases/download/v{VERSION}/awf-linux-x64
```

A `checksums.txt` file is also downloaded and verified via `sha256sum -c checksums.txt --ignore-missing` to ensure binary integrity.

## {{ mcpg_version }}

Should be replaced with the pinned version of the MCP Gateway (defined as `MCPG_VERSION` constant in `src/compile/common.rs`). Used to tag the MCPG Docker image in the pipeline.

## {{ mcpg_image }}

Should be replaced with the MCPG Docker image name (defined as `MCPG_IMAGE` constant in `src/compile/common.rs`). Currently `ghcr.io/github/gh-aw-mcpg`.

## {{ mcpg_port }}

Should be replaced with the MCPG listening port (defined as `MCPG_PORT` constant in `src/compile/common.rs`, currently `80`). Used in the pipeline to set the `MCP_GATEWAY_PORT` ADO variable and in the MCPG health-check URL.

## {{ mcpg_domain }}

Should be replaced with the domain the AWF-sandboxed agent uses to reach MCPG on the host (defined as `MCPG_DOMAIN` constant in `src/compile/common.rs`, currently `host.docker.internal`). Used in the pipeline to set the `MCP_GATEWAY_DOMAIN` ADO variable. Docker's `host.docker.internal` resolves to the host loopback from inside containers.

## {{ copilot_version }}

**Removed.** This marker has been absorbed into `{{ engine_install_steps }}`. The `COPILOT_CLI_VERSION` constant now lives in `src/engine.rs` and is used internally by `Engine::install_steps()`. The version can be overridden per-agent via `engine: { id: copilot, version: "..." }` in front matter.

## 1ES-Specific Template Markers

The 1ES target uses the same template markers as standalone, plus the 1ES-specific `extends:` / `stages:` / `templateContext` wrapping. The 1ES template includes `templateContext.type: buildJob` for all jobs, and the pool is specified at the top-level `parameters.pool` rather than per-job.

Both targets share the same execution model (Copilot CLI + AWF + MCPG) and the same set of template markers.

## Job/Stage Template Markers

The `target: job` and `target: stage` targets use `job-base.yml` and `stage-base.yml`
respectively. Both include the AWF/MCPG execution and agent-lifecycle markers above, but
omit the top-level pipeline structure markers that do not apply to reusable templates:
`{{ schedule }}`, `{{ pr_trigger }}`, `{{ ci_trigger }}`, `{{ pipeline_resources }}`,
`{{ repositories }}`, `{{ parameters }}`, and `{{ pipeline_agent_name }}`. These are
owned by the parent pipeline that includes the template. Additionally, job/stage templates
replace `{{ parameters }}` with `{{ template_parameters }}` (a `parameters:` block for
callers to pass values in). The two template-specific markers below are added.

### {{ stage_prefix }}

Replaced with a PascalCase ADO-safe identifier derived from the agent `name:` front
matter field. Used to prefix the three job names so that including multiple templates
in the same pipeline produces unique job identifiers.

Derivation rules:

- Non-ASCII-alphanumeric characters are treated as word separators (they are not
  included in the output).
- Each word is capitalised and the words are concatenated: `"daily code review"` →
  `"DailyCodeReview"`.
- An empty result (all characters stripped) falls back to `"Agent"`.
- A result starting with a digit is prefixed with `_`: `"123start"` → `"_123start"`.
- Names containing non-ASCII alphanumeric characters (e.g. `"über-agent"`) produce a
  compiler warning because those characters are silently dropped.

Example job names produced for `name: Daily Code Review`:

```yaml
jobs:
  - job: DailyCodeReview_Agent
  - job: DailyCodeReview_Detection
    dependsOn: DailyCodeReview_Agent
  - job: DailyCodeReview_SafeOutputs
    dependsOn: [DailyCodeReview_Agent, DailyCodeReview_Detection]
```

### {{ template_parameters }}

Replaced with the `parameters:` block that callers pass when including the template.
Contains `clearMemory` (auto-injected when `tools.cache-memory` is configured) and any
user-defined `parameters:` from front matter. Replaced with an empty string when no
parameters are needed.

Example output when `tools.cache-memory` is configured:

```yaml
parameters:
- name: clearMemory
  displayName: Clear agent memory
  type: boolean
  default: false
```
