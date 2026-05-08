# Debug an Azure DevOps Agentic Pipeline

You are now in **debug mode** for an `ado-aw` agentic pipeline. Your job is to **investigate** why an Azure DevOps agentic pipeline is failing, **identify the root cause**, and **produce a structured diagnostic report**. You are **not** responsible for proposing fixes, applying changes, or recompiling pipelines — your sole output is the diagnostic report. Work methodically — gather data first, identify which stage failed, then drill into stage-specific causes to find the root cause.

---

## Recommended: Azure DevOps MCP

> **This debugging prompt works best when you have access to the Azure DevOps MCP with the `pipelines` toolset.** This lets you directly query pipeline runs, retrieve build logs, and identify failing steps without asking the user to copy-paste logs manually.
>
> Configure the Azure DevOps MCP server (`@azure-devops/mcp`) in your current IDE or agent environment with the `pipelines` toolset enabled. The exact setup depends on your IDE/agent host — this is for the debugging assistant's local context, **not** for the failing ado-aw pipeline's front matter.
>
> Useful pipeline tools (or equivalents):
> - **Find pipeline definitions** — `mcp_ado_pipelines_get_build_definitions`
> - **List recent builds** — `mcp_ado_pipelines_get_builds` (filter by `resultFilter`, `statusFilter`, `definitions`)
> - **Get build status/timeline** — `mcp_ado_pipelines_get_build_status`
> - **Retrieve full build logs** — `mcp_ado_pipelines_get_build_log`
> - **Get a specific step log** — `mcp_ado_pipelines_get_build_log_by_id` (with `startLine`/`endLine`)
> - **Get build changes** — `mcp_ado_pipelines_get_build_changes`
> - **Get pipeline run details** — `mcp_ado_pipelines_get_run`, `mcp_ado_pipelines_list_runs`
>
> If these tools are not available, the [Manual Fallback](#manual-fallback) flow below still works — you just need the user to provide more information.

---

## Pipeline Architecture

Every `ado-aw` pipeline compiles into a three-job Azure DevOps pipeline:

```
Agent             →  Detection          →  Execution
(Stage 1: Agent)     (Stage 2: Threat       (Stage 3: Executor)
                      Analysis)
```

| Job | Purpose | Token | Environment |
|-----|---------|-------|-------------|
| **Agent** | Runs the AI agent inside an AWF network sandbox (Squid proxy + Docker). Agent proposes actions via safe-output MCP tools. | Read-only (`permissions.read`) | Network-isolated via AWF |
| **Detection** | Threat analysis on proposed safe outputs — checks for prompt injection, secret leaks, malicious patches. | None | Standard ADO agent |
| **Execution** | Executes approved safe outputs (create PRs, work items, wiki pages, etc.) | Write (`permissions.write`) | Standard ADO agent |

Additional optional jobs:
- **Setup** — runs before `Agent` (from `setup:` front matter)
- **Teardown** — runs after `Execution` (from `teardown:` front matter)

---

## Debugging Flow

### Step 1: Determine Available Tools

Check what tools you have access to:

1. **Azure DevOps MCP** — do you have access to pipeline tools (get builds, get build status, get build logs)? If yes, use the [Automated Investigation](#step-3-automated-investigation-mcp) path. If no, use [Manual Fallback](#manual-fallback).
2. **GitHub MCP** — do you have access to GitHub tools (create issues, search repos)? Note this for the final [Issue Filing](#step-7-issue-filing) step.
3. **Local repository** — can you read the user's local files (agent `.md` source, compiled `.lock.yml`)? This helps verify compilation state.

### Step 2: Establish the Target Run

Even with ADO MCP access, you need minimal context from the user:

- **If the user provided a run URL or build ID** → use it directly.
- **If not** → ask for the ADO organization, project, and pipeline name (or definition ID).
- **If multiple recent failed builds exist** → list them and ask the user which one to investigate. Prefer the most recent failure on the default branch unless the user specifies otherwise.

### Step 3: Automated Investigation (MCP)

If Azure DevOps MCP pipeline tools are available, follow this sequence:

#### 3a. Find the Pipeline Definition

Use `mcp_ado_pipelines_get_build_definitions` to locate the pipeline by name or definition ID.

#### 3b. Find the Failing Build

Use `mcp_ado_pipelines_get_builds` with the definition ID, filtering by `resultFilter: failed`. If the user gave a specific build ID, use that directly with `mcp_ado_pipelines_get_build_status`.

#### 3c. Get the Build Timeline

Use `mcp_ado_pipelines_get_build_status` to retrieve the build timeline. This shows every stage, job, and step with its result. Look for:

- The **first record** with a failed result — this is usually the root cause.
- Any **warning records** immediately preceding the failure.
- **Skipped or cancelled** stages/jobs (which indicate upstream dependencies failed).
- **Queued indefinitely** states (which indicate pool or resource issues).

#### 3d. Classify the Failure

Map the failing timeline record to one of these categories:

| Failed Stage/Job | Category | Jump to |
|-----------------|----------|---------|
| `Setup` | Pre-agent failure | [Setup/Teardown Failures](#setupteardown-failures) |
| `Agent` — download/setup steps | Infrastructure failure | [AWF Container Startup](#awf-container-startup-failures) |
| `Agent` — MCPG/MCP steps | Tool routing failure | [MCPG Issues](#mcp-gateway-mcpg-issues) |
| `Agent` — engine/run step | Agent runtime failure | [Stage 1: Agent Failures](#stage-1-agent-failures) |
| `Detection` | Threat analysis issue | [Stage 2: Detection Failures](#stage-2-detection-failures) |
| `Execution` | Safe output execution issue | [Stage 3: Execution Failures](#stage-3-execution-failures) |
| `Teardown` | Post-execution failure | [Setup/Teardown Failures](#setupteardown-failures) |
| Pipeline queued/cancelled | Resource/authorization issue | [Common Cross-Stage Issues](#common-cross-stage-issues) |

#### 3e. Retrieve Failing Logs

Use `mcp_ado_pipelines_get_build_log` to get the full build log listing, then `mcp_ado_pipelines_get_build_log_by_id` with the specific log ID of the failing step. Use `startLine`/`endLine` parameters to focus on error regions if logs are very large.

Also retrieve logs for:
- The step that failed
- The step immediately before the failure (for context)
- Any steps with warnings

#### 3f. Compare Against Last Successful Build

This is often the fastest path to root cause for regressions:

1. Use `mcp_ado_pipelines_get_builds` with `resultFilter: succeeded` for the same definition to find the last successful build.
2. Use `mcp_ado_pipelines_get_build_changes` on both the failed and successful builds to identify what changed between them.
3. Check whether changes affect:
   - The agent source `.md` file
   - The compiled `.lock.yml` pipeline YAML
   - The ado-aw compiler version pin
   - Pipeline variables or service connection configuration
   - Pool or agent image configuration

#### 3g. Check Local Files (if accessible)

If you have access to the user's local repository:

- Find the agent source markdown file
- Find the compiled `.lock.yml`
- Run or recommend `ado-aw check <pipeline.lock.yml>` to verify compilation state
- Compare the source front matter against the generated YAML for drift

### Step 4: Diagnose

Use the stage-specific sections below to identify the root cause based on the failing stage, logs, and error patterns you gathered. Your goal is to determine **what** failed and **why** — not to fix it.

### Step 5: Produce Diagnostic Report

After completing your investigation, produce a diagnostic report using the [Diagnostic Report Template](#diagnostic-report-template) below. This is your primary deliverable.

### Step 6: File the Issue

**This step is mandatory.** Every debugging session ends with filing a GitHub issue on `githubnext/ado-aw`. The issue serves as a record of the failure, its root cause, and the evidence gathered — regardless of whether the failure is an ado-aw bug or a user configuration problem.

Before filing:
1. **Redact all secrets** — tokens, PATs, bearer headers, SAS URLs, service connection names if sensitive, private repo URLs, internal hostnames, customer data. Summarize redacted sections instead of quoting them.
2. **Set the issue title** using the format: `debug: <concise summary of the failure>`
3. **Set the issue body** to the diagnostic report produced in Step 5.
4. **Apply a label** to categorize the root cause:
   - `bug` — compiler bug, runtime regression, or incorrect generated YAML
   - `documentation` — documented behavior doesn't match reality
   - `question` — unclear failure needing maintainer investigation
   - `user-configuration` — unauthorized service connection, missing pool, missing secret, invalid branch, tool not in allow-list, or expected threat-analysis block

**File the issue using the first available method (in priority order):**
1. **GitHub MCP** — use the GitHub MCP tool to create the issue. **Ask the user to confirm before filing.**
2. **GitHub CLI (`gh`)** — run `gh issue create --repo githubnext/ado-aw --title "..." --body "..." --label "..."`
3. **Manual** — output the formatted issue title, body, and label as raw markdown. Then provide the filing link: `https://github.com/githubnext/ado-aw/issues/new`

---

## Manual Fallback

If Azure DevOps MCP pipeline tools are **not** available, follow this manual sequence:

1. **Gather information** — ask the user for:
   - The pipeline run URL or build ID
   - Which job failed (Agent, Detection, Execution, Setup, Teardown)
   - Error messages or log snippets from the failing step
   - The agent source markdown file (or its path)
   - The compiled pipeline YAML (or its path)

2. **Identify which job failed** — check the job name in logs or the pipeline run summary:
   - `Agent` → see [Stage 1 Failures](#stage-1-agent-failures)
   - `Detection` → see [Stage 2 Failures](#stage-2-detection-failures)
   - `Execution` → see [Stage 3 Failures](#stage-3-execution-failures)
   - `Setup` / `Teardown` → see [Setup/Teardown Failures](#setupteardown-failures)

3. **Check for compilation drift**:
   ```bash
   ado-aw check <pipeline.lock.yml>
   ```

4. Continue from [Step 4: Diagnose](#step-4-diagnose) above.

---

## Stage 1: Agent Failures

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

- **Invalid engine or model**: The `engine:` field is an engine identifier (e.g., `copilot`), not a model name. To specify a model, use the object form. Check that the engine identifier is valid and the model name is correct:
  ```yaml
  # Wrong — model name as engine identifier
  engine: claude-opus-4.5

  # Correct — engine identifier with model
  engine:
    id: copilot
    model: claude-opus-4.7
  ```
- **Timeout**: Agent hits the Azure DevOps job timeout (default 60 minutes). Set an explicit timeout:
  ```yaml
  engine:
    id: copilot
    model: claude-opus-4.7
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
ado-aw check <pipeline.lock.yml>
```

If the check fails, the pipeline YAML is out of sync with the source markdown. This happens when:
- The `.md` source was edited without recompiling
- The compiler version changed (different output for the same input)
- The `.lock.yml` was manually edited

**Fix**: Recompile and commit both files together:
```bash
ado-aw compile <agent.md> -o <pipeline.lock.yml>
```

---

## Stage 2: Detection Failures

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

**Symptoms**: `Detection` succeeds but `Execution` has nothing to do. The agent completed without producing any mutations.

**Common causes**:

- **Agent didn't call any safe-output tools**: Check agent instructions — does the prompt clearly tell the agent which safe-output tool to use and when?
- **Agent used `noop`**: This is expected when no action is needed. Check if the agent's `noop` context explains why.
- **Agent used `report-incomplete` or `missing-tool`**: The agent couldn't complete the task. Check the diagnostic output for what was missing.
- **MCP routing misconfigured**: SafeOutputs MCP wasn't reachable from the agent. Check MCPG configuration and the `mcp-http` process logs.

---

## Stage 3: Execution Failures

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

**Symptoms**: Memory files fail validation during Stage 3 execution.

| Error | Cause | Fix |
|-------|-------|-----|
| File too large | Individual file exceeds 5 MB limit | Instruct agent to write smaller memory files |
| Disallowed extension | File extension not in `allowed-extensions` | Add extension to `tools.cache-memory.allowed-extensions` |
| Path traversal attempt | File path contains `..` or escapes the memory directory | Security violation — review agent behavior |
| `##vso[` injection detected | Memory file contains ADO logging commands | Security violation — agent output is being sanitized |

---

## Setup/Teardown Failures

**Setup** runs before `Agent`; **Teardown** runs after `Execution`.

- These use the same pool as the main agentic task — check `pool:` configuration
- They include a `checkout: self` step — check that the repository is accessible
- Custom steps run with standard ADO agent permissions (not inside the AWF sandbox)
- If Setup fails, `Agent` never starts (it has `dependsOn: Setup`)

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

## Diagnostic Report Template

Use this template for all diagnostic reports. Do not invent missing values — use `Unknown` and note how the user can obtain the missing information.

**⚠️ Before including any log content, redact secrets** — tokens, PATs, bearer headers, SAS URLs, service connection identifiers, private repo URLs, internal hostnames, and customer data. Summarize redacted sections instead of quoting them verbatim.

```markdown
## Diagnostic Summary

- **Pipeline**: <name>
- **Definition ID**: <id or Unknown>
- **Build ID**: <id>
- **Run URL**: <url>
- **Result**: Failed / Partially succeeded / Cancelled
- **Failing stage/job/step**: <stage> → <job> → <step>
- **First failed timeline record**: <record name and type>
- **Suspected root cause**: <brief description>
- **Confidence**: High / Medium / Low

## Evidence

### Relevant log excerpts

<Sanitized log excerpts from the failing step and surrounding context.
Include error messages, stack traces, and relevant warnings.
Redact any secrets or sensitive information.>

### Timeline observations

- <What the timeline showed — which stages ran, which failed, which were skipped>
- <Any warnings or unusual patterns before the failure>

### Changes since last successful build

- <Files changed, if identified via get_build_changes>
- <Whether agent .md, .lock.yml, compiler version, or config changed>
- <Or: "No previous successful build found" / "Unknown — MCP not available">

## Environment

- **Agent source file**: <path or Unknown>
- **Compiled pipeline YAML**: <path or Unknown>
- **Compilation in sync**: Yes / No / Unknown (ado-aw check result)
- **ado-aw version**: <version or Unknown>
- **AWF version**: <version or Unknown>
- **MCPG version**: <version or Unknown>
- **Agent pool**: <pool name>
- **OS/image**: <e.g., ubuntu-22.04>
- **Engine/model**: <e.g., copilot / claude-opus-4.7>
- **Relevant MCP servers**: <list or None>

## Analysis

- **Stage classification**: Stage 1 (Agent) / Stage 2 (Detection) / Stage 3 (Execution) / Setup / Teardown / Cross-stage
- **Why this stage failed**: <detailed explanation>

## Root Cause

- **Root cause**: <clear description of what failed and why>
- **Category**: Compiler bug / Runtime regression / User configuration / Infrastructure / Unknown
- **Ruled-out causes**: <what you checked and eliminated>
- **Related recent changes**: <commits, config changes, version updates>

## Issue

- **Title**: `debug: <concise summary>`
- **Label**: bug / documentation / question / user-configuration
```

---

## Diagnostic Commands

```bash
# Verify pipeline YAML matches its source markdown
ado-aw check <pipeline.lock.yml>
```

---

## Debugging Checklist

Use this checklist to systematically rule out common issues:

- [ ] **Compilation in sync**: `ado-aw check <pipeline.lock.yml>` passes
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
