# Debug an Azure DevOps Agentic Pipeline

You are now in **debug mode** for an `ado-aw` agentic pipeline. Your job is to help the user diagnose why their Azure DevOps agentic pipeline is failing, identify the root cause, and suggest targeted fixes. Work methodically — identify which stage failed first, then drill into stage-specific causes.

---

## Pipeline Architecture

Every `ado-aw` pipeline compiles into a three-job Azure DevOps pipeline:

```
PerformAgenticTask  →  AnalyzeSafeOutputs  →  ProcessSafeOutputs
(Stage 1: Agent)       (Threat Analysis)       (Stage 2: Executor)
```

| Job | Purpose | Token | Environment |
|-----|---------|-------|-------------|
| **PerformAgenticTask** | Runs the AI agent inside an AWF network sandbox (Squid proxy + Docker). Agent proposes actions via safe-output MCP tools. | Read-only (`permissions.read`) | Network-isolated via AWF |
| **AnalyzeSafeOutputs** | Threat analysis on proposed safe outputs — checks for prompt injection, secret leaks, malicious patches. | None | Standard ADO agent |
| **ProcessSafeOutputs** | Executes approved safe outputs (create PRs, work items, wiki pages, etc.) | Write (`permissions.write`) | Standard ADO agent |

Additional optional jobs:
- **SetupJob** — runs before `PerformAgenticTask` (from `setup:` front matter)
- **TeardownJob** — runs after `ProcessSafeOutputs` (from `teardown:` front matter)

---

## Debugging Flow

Follow this sequence for every debugging session:

1. **Gather information** — ask the user for:
   - The pipeline run URL or build ID
   - Error messages or log snippets
   - The agent source markdown file
   - The compiled pipeline YAML

2. **Identify which job failed** — check the job name in logs or the pipeline run summary:
   - `PerformAgenticTask` → see [Stage 1 Failures](#stage-1-performagentictask-failures)
   - `AnalyzeSafeOutputs` → see [Stage 2 Failures](#stage-2-analyzesafeoutputs-failures)
   - `ProcessSafeOutputs` → see [Stage 3 Failures](#stage-3-processsafeoutputs-failures)
   - `SetupJob` / `TeardownJob` → see [Setup/Teardown Failures](#setupteardown-failures)

3. **Check for compilation drift** — before deep-diving into runtime errors, verify the pipeline YAML is in sync with its source markdown:
   ```bash
   ado-aw check <pipeline.yml>
   ```

4. **Apply the fix** — make the targeted change to the agent `.md` source file, then recompile:
   ```bash
   ado-aw compile <agent.md>
   ```

5. **Verify** — confirm the fix with `ado-aw check` and review the generated YAML diff.

---

## Stage 1: PerformAgenticTask Failures

This is the most complex stage — it involves downloading binaries, starting Docker containers, configuring the network sandbox, launching the MCP Gateway, and running the AI agent.

### AWF / Network Issues

**Symptoms**: Agent logs show HTTP 403, connection refused, proxy errors, or `CONNECT` failures. The agent cannot reach APIs or download packages.

**Common causes and fixes**:

| Error Pattern | Likely Cause | Fix |
|---------------|-------------|-----|
| `503 Service Unavailable` from Squid | Domain not in allowlist | Add domain to `network.allowed` in front matter |
| `CONNECT tunnel failed` | Wildcard pattern mismatch | Check pattern format — use `*.example.com` not `example.com/*` |
| Agent can't reach Azure DevOps APIs | Missing core domains | These are included by default — check if `network.blocked` accidentally blocks them |
| Agent can't reach custom MCP endpoints | MCP-specific domains not added | Add the MCP server's hostname to `network.allowed` |

**Checking the allowlist**: The compiler merges three domain sources:
1. Built-in core domains (Azure DevOps, GitHub, Microsoft auth, Azure services)
2. MCP-specific domains (auto-added per enabled MCP)
3. User-specified domains from `network.allowed`

If the agent needs to reach `api.myservice.com`, add it:
```yaml
network:
  allowed:
    - "api.myservice.com"
    - "*.myservice.com"   # if subdomains are also needed
```

### AWF Container Startup Failures

**Symptoms**: Pipeline fails before the agent runs. Errors mention Docker, AWF binary, or container startup.

**Common causes**:

- **Docker not available**: The `DockerInstaller@0` task failed or was skipped. Check that the agent pool supports Docker.
- **AWF binary download failure**: The pipeline downloads AWF from `https://github.com/github/gh-aw-firewall/releases/`. If this fails:
  - Check network connectivity from the ADO agent
  - Verify `github.com` and `*.githubusercontent.com` are reachable (they're in the default allowlist but the download happens *before* AWF starts)
  - Check if the pinned AWF version exists in releases
- **SHA256 checksum mismatch**: The `checksums.txt` verification failed — the binary may be corrupted or the version mismatch between binary and checksums file

### MCP Gateway (MCPG) Issues

**Symptoms**: Agent starts but can't call any tools. Errors mention MCP connection failures, tool not found, or MCPG container crash.

**Common causes and fixes**:

- **MCPG container won't start**: Check the MCPG Docker image tag. The pipeline pulls `ghcr.io/github/gh-aw-mcpg:<version>`. Verify the image is accessible from the agent pool.
- **Tool not in `allowed:` list**: The agent tries to call a tool that isn't in the MCP's `allowed:` array. Add it:
  ```yaml
  mcp-servers:
    my-tool:
      container: "node:20-slim"
      entrypoint: "node"
      entrypoint-args: ["server.js"]
      allowed:
        - missing_tool_name   # ← add the tool here
  ```
- **SafeOutputs HTTP server not responding**: The `ado-aw mcp-http` process crashed or didn't start. Check for port conflicts on 8100.
- **Environment variable passthrough**: MCP container needs a secret but it's not reaching it. Verify `env:` mapping:
  ```yaml
  env:
    MY_SECRET: ""   # empty string = passthrough from pipeline environment
  ```
- **Custom MCP container crash**: The container image or entrypoint is wrong. Test the container locally:
  ```bash
  docker run --rm <container> <entrypoint> <entrypoint-args...>
  ```

### Model / Engine Failures

**Symptoms**: The Copilot CLI starts but the agent fails immediately with model errors.

**Common causes**:

- **Invalid model name**: Check the `engine:` field matches a supported model (`claude-opus-4.5`, `claude-sonnet-4.5`, `gpt-5.2-codex`, `gemini-3-pro-preview`, etc.)
- **Timeout**: Agent hits the Azure DevOps job timeout (default 60 minutes). Set an explicit timeout:
  ```yaml
  engine:
    model: claude-opus-4.5
    timeout-minutes: 120
  ```
- **API rate limiting**: The model provider is rate-limiting requests. Check Copilot CLI logs for 429 responses.

### Agent Tool Errors

**Symptoms**: Agent runs but fails when trying to use bash commands or edit files.

**Common causes**:

- **Bash command not in allow-list**: The default allow-list is: `cat, date, echo, grep, head, ls, pwd, sort, tail, uniq, wc, yq`. If the agent needs additional commands:
  ```yaml
  tools:
    bash: ["cat", "ls", "grep", "find", "jq"]   # explicit list
    # or
    bash: [":*"]   # unrestricted (use with caution)
  ```
- **Edit tool disabled**: File editing is enabled by default. If it's been explicitly disabled:
  ```yaml
  tools:
    edit: true   # re-enable
  ```
- **Cache memory errors**: Agent can't read/write memory files. Check `tools.cache-memory` configuration and `allowed-extensions`.

### Compilation Drift

**Symptoms**: Pipeline behavior doesn't match what the source markdown describes. Features seem missing or misconfigured.

**Diagnosis**:
```bash
ado-aw check <pipeline.yml>
```

If the check fails, the pipeline YAML is out of sync with the source markdown. This happens when:
- The `.md` source was edited without recompiling
- The compiler version changed (different output for the same input)
- The `.yml` was manually edited

**Fix**: Recompile and commit both files together:
```bash
ado-aw compile <agent.md> -o <pipeline.yml>
```

---

## Stage 2: AnalyzeSafeOutputs Failures

This job runs threat analysis on the agent's proposed safe outputs.

### Threat Analysis False Positives

**Symptoms**: The threat analysis flags legitimate agent output as malicious. Pipeline stops before executing safe outputs.

**Common causes**:

- **Agent output contains URLs or encoded strings**: The threat analysis prompt checks for suspicious web calls, encoded data, and backdoor patterns. If the agent legitimately produces such content, review the threat analysis logs for the specific flag.
- **Prompt injection detection**: The agent's output text matches prompt injection patterns. This is usually a sign that the agent's input (repository content, work items, PRs) contains adversarial content — which is exactly what the analysis is designed to catch.

**What to do**:
- Review the threat analysis output carefully — false positives are rare by design
- If genuinely false, adjust the agent's instructions to produce output that doesn't trigger detection
- Do NOT bypass the threat analysis — it exists for security

### No Safe Outputs Produced

**Symptoms**: `AnalyzeSafeOutputs` succeeds but `ProcessSafeOutputs` has nothing to do. The agent completed without producing any mutations.

**Common causes**:

- **Agent didn't call any safe-output tools**: Check agent instructions — does the prompt clearly tell the agent which safe-output tool to use and when?
- **Agent used `noop`**: This is expected when no action is needed. Check if the agent's `noop` context explains why.
- **Agent used `report-incomplete` or `missing-tool`**: The agent couldn't complete the task. Check the diagnostic output for what was missing.
- **MCP routing misconfigured**: SafeOutputs MCP wasn't reachable from the agent. Check MCPG configuration and the `mcp-http` process logs.

---

## Stage 3: ProcessSafeOutputs Failures

This job executes the approved safe outputs using the write token. Failures here are usually ADO API errors or validation issues.

### Write Token Issues

**Symptoms**: API calls return 401/403. The executor can't authenticate to Azure DevOps.

**Common causes**:

- **`permissions.write` not set**: The front matter is missing the write ARM service connection:
  ```yaml
  permissions:
    write: my-write-arm-connection
  ```
- **ARM service connection not authorized**: The pipeline needs explicit authorization for the service connection. Go to the pipeline's settings in ADO and authorize the service connection.
- **Token scope insufficient**: The ARM service connection may not have the required permissions on the ADO project. Verify the connection's role assignments.
- **Compile-time validation**: The compiler should catch missing `permissions.write` when write-requiring safe outputs are configured. If you're seeing this at runtime, the front matter may have been edited without recompiling.

### PR Creation Failures

**Symptoms**: `create-pull-request` safe output fails during execution.

| Error | Cause | Fix |
|-------|-------|-----|
| Patch doesn't apply | Merge conflicts — target branch diverged since the agent ran | Rerun the pipeline; consider more frequent schedules |
| Target branch not found | Branch name doesn't exist in the repository | Check `safe-outputs.create-pull-request.target-branch` |
| Repository not in allowed list | Agent tried to create PR in a repo not in `checkout:` | Add the repository to both `repositories:` and `checkout:` |
| Patch too large | Patch file exceeds 5 MB limit | Reduce the scope of changes in agent instructions |
| Path validation failed | Patch contains `..`, `.git`, or absolute paths | This is a security violation — review what the agent generated |

### Work Item Failures

**Symptoms**: `create-work-item` or `update-work-item` safe output fails.

| Error | Cause | Fix |
|-------|-------|-----|
| Invalid area path | The configured `area-path` doesn't exist in the ADO project | Verify the path in ADO project settings |
| Missing required fields | ADO work item type requires fields not provided | Check `safe-outputs.create-work-item` config for required fields |
| Work item not found (update) | The work item ID doesn't exist | Check `safe-outputs.update-work-item.target` scoping |
| Title/tag prefix mismatch (update) | Work item doesn't match `title-prefix` or `tag-prefix` filter | Verify the target work item has the required prefix/tag |
| Max limit exceeded | More outputs than `max` allows | Increase `max` in the safe-output config or reduce agent output |

### Wiki Page Failures

**Symptoms**: `create-wiki-page` or `update-wiki-page` safe output fails.

| Error | Cause | Fix |
|-------|-------|-----|
| Page already exists | Using `create-wiki-page` for an existing page | Use `update-wiki-page` instead |
| Page not found | Using `update-wiki-page` for a non-existent page | Use `create-wiki-page` instead |
| Wiki name not found | `wiki-name` doesn't match any wiki in the project | Verify the wiki name in ADO project settings |
| Wiki name not set | `wiki-name` is missing from the configuration | Add `wiki-name` to the safe-output config (it's required) |
| Path traversal blocked | Page path contains `..` | Fix the agent instructions to produce valid paths |

### Agent Memory Failures

**Symptoms**: Memory files fail validation during Stage 2 execution.

| Error | Cause | Fix |
|-------|-------|-----|
| File too large | Individual file exceeds 5 MB limit | Instruct agent to write smaller memory files |
| Disallowed extension | File extension not in `allowed-extensions` | Add extension to `tools.cache-memory.allowed-extensions` |
| Path traversal attempt | File path contains `..` or escapes the memory directory | Security violation — review agent behavior |
| `##vso[` injection detected | Memory file contains ADO logging commands | Security violation — agent output is being sanitized |

---

## Setup/Teardown Failures

**SetupJob** runs before `PerformAgenticTask`; **TeardownJob** runs after `ProcessSafeOutputs`.

- These use the same pool as the main agentic task — check `pool:` configuration
- They include a `checkout: self` step — check that the repository is accessible
- Custom steps run with standard ADO agent permissions (not inside the AWF sandbox)
- If SetupJob fails, `PerformAgenticTask` never starts (it has `dependsOn: SetupJob`)

---

## Common Cross-Stage Issues

### Permission Mismatches

The compiler validates that write-requiring safe outputs have `permissions.write` at compile time. If you're hitting permission errors at runtime:

- The front matter was edited without recompiling → run `ado-aw compile`
- The service connection exists but isn't authorized for this pipeline → authorize it in ADO pipeline settings
- The service connection's managed identity lacks the required ADO permissions

### Service Connection Authorization

On the first run of a new pipeline (or after adding a new service connection), Azure DevOps requires explicit authorization:

1. The pipeline run will fail with a "needs permission" banner
2. Click "Permit" in the ADO UI to authorize the service connection
3. Rerun the pipeline

This is a one-time step per service connection per pipeline.

### Pool Availability

**Symptoms**: Pipeline is queued indefinitely or fails with "no agent available."

- Verify the `pool:` name matches an existing agent pool in the ADO organization
- Default pool: `AZS-1ES-L-MMS-ubuntu-22.04`
- Check that the pool has available agents (not all busy or offline)
- For 1ES target, ensure the pool supports the specified `os:` (linux/windows)

### Copilot CLI / ado-aw Binary Issues

The pipeline downloads both binaries from GitHub Releases:

- **ado-aw**: `https://github.com/githubnext/ado-aw/releases/download/v{VERSION}/ado-aw-linux-x64`
- **AWF**: `https://github.com/github/gh-aw-firewall/releases/download/v{VERSION}/awf-linux-x64`

If downloads fail:
- Check that `github.com` and `*.githubusercontent.com` are reachable from the agent (these downloads happen before AWF starts)
- Verify the version exists in the release page
- Check SHA256 checksum verification isn't failing (indicates corruption or version mismatch)

---

## Diagnostic Commands

```bash
# Verify pipeline YAML matches its source markdown
ado-aw check <pipeline.yml>

# Recompile a single agent
ado-aw compile <path/to/agent.md>

# Recompile all detected agentic pipelines in the current directory
ado-aw compile

# Update GITHUB_TOKEN pipeline variable on ADO build definitions
ado-aw configure

# Dry-run configure to preview changes
ado-aw configure --dry-run
```

---

## Debugging Checklist

Use this checklist to systematically rule out common issues:

- [ ] **Compilation in sync**: `ado-aw check <pipeline.yml>` passes
- [ ] **Correct stage identified**: Know which of the 3 jobs failed
- [ ] **Network allowlist**: All required domains are in `network.allowed` or built-in
- [ ] **MCP tools allowed**: Every tool the agent needs is in an `allowed:` list
- [ ] **Permissions set**: `permissions.write` is present if write safe-outputs are configured
- [ ] **Service connections authorized**: ARM connections are permitted for this pipeline
- [ ] **Pool available**: Agent pool exists and has capacity
- [ ] **Engine valid**: Model name matches a supported model
- [ ] **Bash allow-list**: All needed shell commands are listed in `tools.bash`
- [ ] **Binary versions**: ado-aw and AWF version pins match available releases

---

## Reference

For full project documentation, front matter schema, and architecture details:

- **AGENTS.md**: <https://raw.githubusercontent.com/githubnext/ado-aw/main/AGENTS.md>
- **README.md**: <https://github.com/githubnext/ado-aw/blob/main/README.md>
- **AWF (Agentic Workflow Firewall)**: <https://github.com/github/gh-aw-firewall>
- **MCP Gateway (MCPG)**: <https://github.com/github/gh-aw-mcpg>
